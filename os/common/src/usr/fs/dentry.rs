//! Directory entry cache (dcache).  Maps (parent, name) → child Dentry,
//! caching resolved lookups so we don't re-read the directory on every access.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use spin::RwLock;

use super::inode::InodeOp;

// ---------------------------------------------------------------------------
// Dentry flags — mimics Linux `DCACHE_*`
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DentryFlags(u32);

impl DentryFlags {
    pub const NEGATIVE: Self = Self(0x0001);
    pub const REGULAR: Self = Self(0x0002);
    pub const DIRECTORY: Self = Self(0x0004);
    pub const SYMLINK: Self = Self(0x0008);
    pub const SPECIAL: Self = Self(0x0010);
    pub const MOUNT_POINT: Self = Self(0x0020);
    pub const MISS: Self = Self(0x0040);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub fn contains(&self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }

    /// Update type flags from a negative dentry (clears NEGATIVE, adds type).
    pub fn update_type_from_negative(&mut self, ty: DentryFlags) {
        self.remove(DentryFlags::NEGATIVE);
        self.insert(ty);
    }
}

// ---------------------------------------------------------------------------
// Dentry — a node in the directory-cache tree
// ---------------------------------------------------------------------------

pub struct Dentry {
    /// Full absolute path (e.g. "/usr/bin/bash").
    /// NOTE: 只在创建/首次解析时写入，不做 rename 缓存，rename 时必须同步重建。
    pub absolute_path: String,
    /// Parent dentry.  **子持父强引用**（Linux 语义）：只要子还活着，父就不会被回收，
    /// 保证 `..` / 路径回溯 / 父 dcache 始终有效。父→子用 `Weak`，无循环。
    pub parent: RwLock<Option<Arc<Dentry>>>,
    /// Children keyed by name (weak: 父不延长子生命周期).
    pub children: RwLock<BTreeMap<String, Weak<Dentry>>>,
    /// Type flags for this dentry.
    pub flags: RwLock<DentryFlags>,
    /// The actual inode — None for negative dentries.
    pub inode: RwLock<Option<Arc<dyn InodeOp>>>,
}

impl Dentry {
    /// Create a new dentry.
    pub fn new(
        absolute_path: String,
        parent: Option<Arc<Dentry>>,
        flags: DentryFlags,
        inode: Option<Arc<dyn InodeOp>>,
    ) -> Arc<Self> {
        // 一致性校验：inode == None ⇔ NEGATIVE
        debug_assert!(
            inode.is_none() == flags.contains(DentryFlags::NEGATIVE),
            "dentry flags / inode mismatch: path={}",
            absolute_path
        );
        Arc::new(Self {
            absolute_path,
            parent: RwLock::new(parent),
            children: RwLock::new(BTreeMap::new()),
            flags: RwLock::new(flags),
            inode: RwLock::new(inode),
        })
    }

    /// Create a negative dentry (looked up but not found).
    pub fn negative(absolute_path: String, parent: Option<Arc<Dentry>>) -> Arc<Self> {
        Self::new(absolute_path, parent, DentryFlags::NEGATIVE, None)
    }

    /// Is this a negative dentry?
    pub fn is_negative(&self) -> bool {
        self.flags.read().contains(DentryFlags::NEGATIVE)
    }

    pub fn is_dir(&self) -> bool {
        self.flags.read().contains(DentryFlags::DIRECTORY)
    }

    /// Parent dentry. 强引用 Arc 保证父在子存活期间不被回收。
    pub fn get_parent(&self) -> Option<Arc<Dentry>> {
        self.parent.read().clone()
    }

    // -----------------------------------------------------------------------
    // dcache 子表操作
    // -----------------------------------------------------------------------

    /// 查子表。`None` = miss；`Some(dentry)` = hit。
    /// Weak 已失效的条目会被顺手清掉（与 Linux `__d_lookup` 一致）。
    pub fn lookup_child(&self, name: &str) -> Option<Arc<Dentry>> {
        let mut map = self.children.write();
        match map.get(name) {
            None => None,
            Some(w) => match w.upgrade() {
                Some(arc) => Some(arc),
                None => {
                    map.remove(name); // 失效清理
                    None
                }
            },
        }
    }

    /// 把 lookup 的结果挂进 dcache。**竞争安全**：如果别的线程已经插入了
    /// 一个有效条目，返回那个既有条目，丢弃 `child`（调用者必须用返回值！）。
    ///
    /// 路径解析的正确用法：
    /// ```ignore
    /// let child = match cur.lookup_child(name) {
    ///     Some(d) => d,
    ///     None => {
    ///         let d = cur.inode.read().as_ref().unwrap()
    ///             .lookup(name, cur.clone())?;   // 改成 Result 后
    ///         cur.insert_child(name.to_string(), d)  // ← 用返回值
    ///     }
    /// };
    /// ```
    pub fn insert_child(&self, name: String, child: Arc<Dentry>) -> Arc<Dentry> {
        // 1. 路径一致性（仅在 path 尚未被改写时成立；调用者应保证 child 的
        //    absolute_path 由 (self.absolute_path, name) 拼出）
        debug_assert_eq!(
            child.get_parent().map(|p| Arc::as_ptr(&p)),
            Some(self as *const Dentry),
            "insert_child: child.parent != self"
        );

        let mut map = self.children.write();
        if let Some(w) = map.get(&name) {
            if let Some(existing) = w.upgrade() {
                return existing; // 竞争：别人先插入了，用他的
            }
        }
        map.insert(name, Arc::downgrade(&child));
        child
    }

    /// 从父 dcache 删除一个子条目（unlink / rename / 失败回滚用）。
    pub fn remove_child(&self, name: &str) {
        self.children.write().remove(name);
    }

    /// 便捷构造：创建子 dentry 并**自动**挂到当前目录的 dcache。
    /// `child_path` 必须等于 `self.absolute_path` + "/" + `name`（根目录除外）。
    pub fn new_child(
        self: &Arc<Self>,
        name: &str,
        flags: DentryFlags,
        inode: Option<Arc<dyn InodeOp>>,
    ) -> Arc<Dentry> {
        let path = if self.absolute_path == "/" {
            alloc::format!("/{}", name)
        } else {
            alloc::format!("{}/{}", self.absolute_path, name)
        };
        let child = Self::new(path, Some(self.clone()), flags, inode);
        self.insert_child(name.to_string(), child)
    }

    /// Get the last component of the path (e.g. "bash" from "/usr/bin/bash").
    pub fn get_last_name(&self) -> String {
        if let Some(pos) = self.absolute_path.rfind('/') {
            self.absolute_path[pos + 1..].to_string()
        } else {
            self.absolute_path.clone()
        }
    }
}