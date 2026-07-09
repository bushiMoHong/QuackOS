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
    pub absolute_path: String,
    /// Parent dentry (Weak to avoid cycles).
    pub parent: RwLock<Option<Weak<Dentry>>>,
    /// Children keyed by name.
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
        parent: Option<Weak<Dentry>>,
        flags: DentryFlags,
        inode: Option<Arc<dyn InodeOp>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            absolute_path,
            parent: RwLock::new(parent),
            children: RwLock::new(BTreeMap::new()),
            flags: RwLock::new(flags),
            inode: RwLock::new(inode),
        })
    }

    /// Create a negative dentry (looked up but not found).
    pub fn negative(absolute_path: String, parent: Option<Weak<Dentry>>) -> Arc<Self> {
        Self::new(absolute_path, parent, DentryFlags::NEGATIVE, None)
    }

    /// Is this a negative dentry?
    pub fn is_negative(&self) -> bool {
        self.flags.read().contains(DentryFlags::NEGATIVE)
    }

    /// Get a child by name from the cache.
    pub fn get_child(&self, name: &str) -> Option<Arc<Dentry>> {
        self.children
            .read()
            .get(name)
            .and_then(|w| w.upgrade())
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
