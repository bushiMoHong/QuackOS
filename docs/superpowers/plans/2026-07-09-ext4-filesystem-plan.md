# ext4 Filesystem Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add complete VFS layer and ext4 filesystem support to QuackOS microkernel (aarch64).

**Architecture:** Build VFS layer (InodeOp trait, Dentry, PageCache, BlockDevice) under `os/common/src/usr/fs/`. Port ext4 from `example/ext4/`, adapting crate paths and interfaces. FsServer handles IPC via single-thread event loop + worker thread pool for block I/O.

**Tech Stack:** Rust 2021, `spin` locks, `hashbrown` HashMap, no_std.

## Global Constraints

- Target: aarch64 only
- Target triple: `aarch64-unknown-none`
- No `std` — use `alloc` for heap types (Vec, String, Arc, Box, BTreeMap)
- `spin::RwLock` / `spin::Mutex` for synchronization (already in Cargo.toml)
- New dep: `hashbrown = "0.14"` in `os/common/Cargo.toml`
- Kernel test pattern: `fn() -> bool` printed via UART
- Build command: `cargo build` from `os/` directory
- Read `example/ext4/*.rs` for reference code when porting

---

### Task 1: VFS Types Foundation

**Files:**
- Create: `os/common/src/usr/fs/mod.rs`
- Create: `os/common/src/usr/fs/types.rs`

**Interfaces:**
- Produces: `Kstat`, `OpenFlags`, `FileType`, `SeekWhence`, `SyscallRet`, `Errno`

- [ ] **Step 1: Create `fs/mod.rs` module root**

```rust
// os/common/src/usr/fs/mod.rs
//! Virtual File System layer — traits, types, and abstractions for filesystem
//! implementations.  The FsServer (server.rs) uses these to dispatch IPC
//! requests to concrete filesystems.

pub mod types;
```

- [ ] **Step 2: Create `fs/types.rs` with VFS types**

```rust
// os/common/src/usr/fs/types.rs
//! Core types for the VFS layer — file statistics, open flags, error codes.

use core::fmt;

// ---------------------------------------------------------------------------
// File type (matches S_IFMT in ext4)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum FileType {
    Fifo    = 0x1000,
    ChrDev  = 0x2000,
    Dir     = 0x4000,
    BlkDev  = 0x6000,
    RegFile = 0x8000,
    Symlink = 0xA000,
    Socket  = 0xC000,
}

impl FileType {
    pub fn from_mode(mode: u16) -> Self {
        match mode & 0xF000 {
            0x1000 => FileType::Fifo,
            0x2000 => FileType::ChrDev,
            0x4000 => FileType::Dir,
            0x6000 => FileType::BlkDev,
            0x8000 => FileType::RegFile,
            0xA000 => FileType::Symlink,
            0xC000 => FileType::Socket,
            _      => FileType::RegFile,
        }
    }

    pub fn is_dir(&self) -> bool {
        matches!(self, FileType::Dir)
    }

    pub fn is_reg(&self) -> bool {
        matches!(self, FileType::RegFile)
    }

    pub fn is_symlink(&self) -> bool {
        matches!(self, FileType::Symlink)
    }
}

// ---------------------------------------------------------------------------
// Kstat — file metadata (like Linux `struct kstat`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Kstat {
    pub ino: u64,
    pub mode: u16,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub blksize: u32,
    pub blocks: u64,
    pub atime_sec: i64,
    pub atime_nsec: i64,
    pub mtime_sec: i64,
    pub mtime_nsec: i64,
    pub ctime_sec: i64,
    pub ctime_nsec: i64,
    pub rdev: u64,         // device number (for chr/blk devices)
    pub file_type: FileType,
}

impl Default for Kstat {
    fn default() -> Self {
        Self {
            ino: 0,
            mode: 0,
            nlink: 1,
            uid: 0,
            gid: 0,
            size: 0,
            blksize: 4096,
            blocks: 0,
            atime_sec: 0, atime_nsec: 0,
            mtime_sec: 0, mtime_nsec: 0,
            ctime_sec: 0, ctime_nsec: 0,
            rdev: 0,
            file_type: FileType::RegFile,
        }
    }
}

// ---------------------------------------------------------------------------
// OpenFlags — how a file was opened
// ---------------------------------------------------------------------------

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct OpenFlags: u32 {
        const O_RDONLY   = 0x0000;
        const O_WRONLY   = 0x0001;
        const O_RDWR     = 0x0002;
        const O_CREAT    = 0x0040;
        const O_EXCL     = 0x0080;
        const O_TRUNC    = 0x0200;
        const O_APPEND   = 0x0400;
        const O_DIRECTORY = 0x10000;
    }
}

// ---------------------------------------------------------------------------
// SeekWhence — seek direction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekWhence {
    Set = 0,
    Cur = 1,
    End = 2,
}

// ---------------------------------------------------------------------------
// Errno — POSIX error codes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Errno {
    EPERM   = 1,
    ENOENT  = 2,
    EIO     = 5,
    EBADF   = 9,
    ENOMEM  = 12,
    EACCES  = 13,
    EFAULT  = 14,
    EEXIST  = 17,
    ENODEV  = 19,
    ENOTDIR = 20,
    EISDIR  = 21,
    EINVAL  = 22,
    ENOSPC  = 28,
    EROFS   = 30,
    ENOSYS  = 38,
    ENOTEMPTY = 39,
}

pub type SyscallRet = Result<usize, Errno>;
```

- [ ] **Step 3: Build to verify compilation**

Run: `cargo build -p common --target aarch64-unknown-none` from `os/`
Expected: compilation succeeds (note: no `bitflags` crate yet — we need to add it or use manual constants)

- [ ] **Step 4: Add `bitflags` and `hashbrown` dependencies**

Modify `os/common/Cargo.toml`:

```toml
[dependencies]
spin = "0.9"
log = "0.4"
hashbrown = "0.14"
bitflags = "2.0"
aarch64 = { path = "../arch/aarch64" }
```

- [ ] **Step 5: Build again to verify**

Run: `cargo build -p common --target aarch64-unknown-none` from `os/`
Expected: compilation succeeds

- [ ] **Step 6: Commit**

```bash
git add os/common/Cargo.toml os/common/src/usr/fs/
git commit -m "feat(fs): add VFS types module with Kstat, OpenFlags, Errno

Foundation types for the VFS layer — file statistics, open flags,
seek whence, and POSIX error codes. Added bitflags and hashbrown
crate dependencies.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: InodeOp Trait + Dentry

**Files:**
- Create: `os/common/src/usr/fs/inode.rs`
- Create: `os/common/src/usr/fs/dentry.rs`
- Modify: `os/common/src/usr/fs/mod.rs`

**Interfaces:**
- Consumes: `Kstat`, `Errno`, `SyscallRet`, `FileType`, `OpenFlags` from `types.rs`
- Produces: `InodeOp` trait, `Dentry`, `DentryFlags`, `InodeCache`

- [ ] **Step 1: Create `fs/inode.rs` — InodeOp trait**

```rust
// os/common/src/usr/fs/inode.rs
//! Inode operations trait — the core VFS abstraction that every filesystem
//! implements.  Mirrors Linux's `struct inode_operations`.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

use super::types::{Errno, Kstat, OpenFlags, SyscallRet};
use super::dentry::Dentry;

/// The central VFS trait.  Every inode (file, directory, device, pipe, etc.)
/// must implement this trait.
pub trait InodeOp: Send + Sync {
    /// Downcast to concrete type (required for trait-object safety).
    fn as_any(&self) -> &dyn Any;

    /// Read bytes from the file starting at `offset` into `buf`.
    /// Returns number of bytes read (0 = EOF).
    fn read(&self, offset: usize, buf: &mut [u8]) -> usize;

    /// Write bytes from `buf` to the file starting at `offset`.
    /// Returns number of bytes written.
    fn write(&self, offset: usize, buf: &[u8]) -> usize;

    /// Look up a directory entry by name.  `parent` is the directory's
    /// Dentry.  Returns a Dentry (possibly negative if not found).
    fn lookup(&self, name: &str, parent: Arc<Dentry>) -> Arc<Dentry>;

    /// Create a new regular file inside this directory.
    /// `dentry` is a pre-created negative Dentry; this method fills it.
    fn create(&self, dentry: Arc<Dentry>, mode: u16);

    /// Create a new directory inside this directory.
    fn mkdir(&self, dentry: Arc<Dentry>, mode: u16);

    /// Create a new symlink inside this directory.
    fn symlink(&self, dentry: Arc<Dentry>, target: &str);

    /// Create a device node (chr/blk) inside this directory.
    fn mknod(&self, dentry: Arc<Dentry>, mode: u16, rdev: u64);

    /// Remove a directory entry from this directory.
    fn unlink(&self, dentry: Arc<Dentry>) -> SyscallRet;

    /// Truncate the file to `size` bytes.
    fn truncate(&self, size: usize) -> SyscallRet;

    /// Flush any pending metadata/data to disk.
    fn fsync(&self) -> SyscallRet;

    /// Return file metadata.
    fn get_stat(&self) -> Kstat;

    /// Return the file size in bytes.
    fn get_size(&self) -> usize;

    /// Read directory entries into a buffer.  Returns (bytes_written, file_offset).
    fn getdents(&self, buf: &mut [u8]) -> (usize, usize);
}

/// Wrapper around an `Arc<dyn InodeOp>` that also tracks the inode number.
pub struct Inode {
    pub ino: u64,
    pub ops: Arc<dyn InodeOp>,
}

impl Inode {
    pub fn new(ino: u64, ops: Arc<dyn InodeOp>) -> Self {
        Self { ino, ops }
    }
}
```

- [ ] **Step 2: Create `fs/dentry.rs` — Dentry (directory entry cache)**

```rust
// os/common/src/usr/fs/dentry.rs
//! Directory entry cache (dcache).  Maps (parent, name) → child Dentry,
//! caching resolved lookups so we don't re-read the directory on every access.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use spin::RwLock;

use super::inode::InodeOp;
use super::types::FileType;

// ---------------------------------------------------------------------------
// Dentry flags — mimics Linux `DCACHE_*`
// ---------------------------------------------------------------------------

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct DentryFlags: u32 {
        /// Negative dentry — name was looked up and does NOT exist.
        const NEGATIVE     = 0x0001;
        /// This dentry represents a regular file.
        const REGULAR      = 0x0002;
        /// This dentry represents a directory.
        const DIRECTORY    = 0x0004;
        /// This dentry represents a symlink.
        const SYMLINK      = 0x0008;
        /// This dentry represents a special device (chr, blk, fifo, sock).
        const SPECIAL      = 0x0010;
        /// This dentry represents a mount point.
        const MOUNT_POINT  = 0x0020;
        /// The lookup missed — use for error handling.
        const MISS         = 0x0040;
    }
}

impl DentryFlags {
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
```

- [ ] **Step 3: Update `fs/mod.rs` to include new modules**

```rust
// os/common/src/usr/fs/mod.rs
//! Virtual File System layer — traits, types, and abstractions for filesystem
//! implementations.  The FsServer (server.rs) uses these to dispatch IPC
//! requests to concrete filesystems.

pub mod dentry;
pub mod inode;
pub mod types;
```

- [ ] **Step 4: Build to verify compilation**

Run: `cargo build -p common --target aarch64-unknown-none` from `os/`
Expected: compilation succeeds

- [ ] **Step 5: Commit**

```bash
git add os/common/src/usr/fs/
git commit -m "feat(fs): add InodeOp trait and Dentry cache

VFS core: InodeOp trait with read/write/lookup/create/truncate/etc.
and Dentry for directory-entry caching with parent-child tree.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: PageCache + BlockDevice

**Files:**
- Create: `os/common/src/usr/fs/page_cache.rs`
- Create: `os/common/src/usr/fs/dev/mod.rs`
- Create: `os/common/src/usr/fs/dev/block_dev.rs`
- Modify: `os/common/src/usr/fs/mod.rs`

**Interfaces:**
- Produces: `Page`, `AddressSpace`, `BlockDevice` trait, `BlockCache`
- Note: `Page` is a minimal placeholder — full Page integration with `mm` happens later

- [ ] **Step 1: Create `fs/page_cache.rs`**

```rust
// os/common/src/usr/fs/page_cache.rs
//! Page cache — caches file data in memory pages.  Maps (inode, page_index)
//! → Page so that repeated reads/writes hit memory instead of disk.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::arch::config::PAGE_SIZE;

// ---------------------------------------------------------------------------
// Page — a cached page of file data
// ---------------------------------------------------------------------------

pub struct Page {
    /// The page index within the file (offset = page_index * PAGE_SIZE).
    pub page_index: usize,
    /// Raw page data.
    pub data: [u8; PAGE_SIZE],
    /// Whether the page is dirty (needs write-back).
    pub dirty: bool,
}

impl Page {
    pub fn new(page_index: usize) -> Self {
        Self {
            page_index,
            data: [0u8; PAGE_SIZE],
            dirty: false,
        }
    }

    /// Get a reference to the data starting at `offset` within this page.
    /// `offset` must be < PAGE_SIZE.
    pub fn get_ref<T>(&self, offset: usize) -> &T {
        assert!(offset + core::mem::size_of::<T>() <= PAGE_SIZE);
        unsafe { &*(self.data.as_ptr().add(offset) as *const T) }
    }

    /// Get a mutable reference.
    pub fn get_mut<T>(&mut self, offset: usize) -> &mut T {
        assert!(offset + core::mem::size_of::<T>() <= PAGE_SIZE);
        unsafe { &mut *(self.data.as_mut_ptr().add(offset) as *mut T) }
    }

    /// Copy bytes into the page starting at `offset`.
    pub fn write_at(&mut self, offset: usize, buf: &[u8]) -> usize {
        let len = core::cmp::min(buf.len(), PAGE_SIZE - offset);
        self.data[offset..offset + len].copy_from_slice(&buf[..len]);
        self.dirty = true;
        len
    }

    /// Copy bytes out of the page starting at `offset`.
    pub fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let len = core::cmp::min(buf.len(), PAGE_SIZE - offset);
        buf[..len].copy_from_slice(&self.data[offset..offset + len]);
        len
    }
}

// ---------------------------------------------------------------------------
// AddressSpace — per-inode page cache
// ---------------------------------------------------------------------------

pub struct AddressSpace {
    /// Cached pages keyed by page_index.
    pages: BTreeMap<usize, Arc<Mutex<Page>>>,
}

impl AddressSpace {
    pub fn new() -> Self {
        Self {
            pages: BTreeMap::new(),
        }
    }

    /// Get a page from the cache, or return None if not cached.
    pub fn get_page_cache(&self, page_index: usize) -> Option<Arc<Mutex<Page>>> {
        self.pages.get(&page_index).cloned()
    }

    /// Get multiple pages from the cache.
    pub fn get_page_caches(&self, page_index: usize, count: usize) -> Vec<Arc<Mutex<Page>>> {
        let mut result = Vec::new();
        for i in 0..count {
            if let Some(page) = self.get_page_cache(page_index + i) {
                result.push(page);
            }
        }
        result
    }

    /// Insert or replace a page in the cache.
    pub fn insert_page(&mut self, page: Arc<Mutex<Page>>) {
        let idx = page.lock().page_index;
        self.pages.insert(idx, page);
    }

    /// Number of cached pages.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }
}
```

- [ ] **Step 2: Create `fs/dev/block_dev.rs` — BlockDevice trait + BlockCache**

```rust
// os/common/src/usr/fs/dev/block_dev.rs
//! Block device abstraction — the interface between filesystems and the
//! underlying storage hardware (e.g. virtio-blk).

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

// ---------------------------------------------------------------------------
// BlockDevice trait
// ---------------------------------------------------------------------------

/// A fixed-size block storage device.  Implementations read/write physical
/// sectors; filesystems build on top of this.
pub trait BlockDevice: Send + Sync {
    /// Read `len` bytes from byte-offset `offset` into `buf`.
    fn read(&self, offset: usize, buf: &mut [u8]);

    /// Write `len` bytes from `buf` to byte-offset `offset`.
    fn write(&self, offset: usize, buf: &[u8]);

    /// Total size of the device in bytes.
    fn size(&self) -> usize;

    /// Sector size in bytes (typically 512 for virtio-blk).
    fn sector_size(&self) -> usize;
}

// ---------------------------------------------------------------------------
// BlockCache — LRU-style cache for filesystem blocks
// ---------------------------------------------------------------------------

pub const BLOCK_CACHE_CAP: usize = 64;

pub struct BlockCache {
    device: Arc<dyn BlockDevice>,
    block_size: usize,
    /// Cached blocks keyed by block number.
    blocks: BTreeMap<usize, Arc<Mutex<BlockBuffer>>>,
    /// LRU order: front = most recently used.
    lru: Vec<usize>,
}

pub struct BlockBuffer {
    pub data: Vec<u8>,
    pub dirty: bool,
}

impl BlockCache {
    pub fn new(device: Arc<dyn BlockDevice>, block_size: usize) -> Self {
        Self {
            device,
            block_size,
            blocks: BTreeMap::new(),
            lru: Vec::new(),
        }
    }

    /// Get a cached block, reading it from the device if necessary.
    pub fn get_block(&mut self, block_num: usize) -> Arc<Mutex<BlockBuffer>> {
        if let Some(buf) = self.blocks.get(&block_num) {
            self.touch_lru(block_num);
            return buf.clone();
        }
        // Read from device.
        let mut data = vec![0u8; self.block_size];
        self.device.read(block_num * self.block_size, &mut data);
        let buf = Arc::new(Mutex::new(BlockBuffer { data, dirty: false }));
        self.blocks.insert(block_num, buf.clone());
        self.lru.push(block_num);
        self.evict_if_needed();
        buf
    }

    /// Write a specific block back to the device.
    pub fn write_block(&self, block_num: usize) {
        if let Some(buf) = self.blocks.get(&block_num) {
            let b = buf.lock();
            self.device.write(block_num * self.block_size, &b.data);
        }
    }

    /// Write all dirty blocks back.
    pub fn sync_all(&self) {
        for (&num, buf) in &self.blocks {
            let b = buf.lock();
            if b.dirty {
                self.device.write(num * self.block_size, &b.data);
            }
        }
    }

    fn touch_lru(&mut self, block_num: usize) {
        self.lru.retain(|&n| n != block_num);
        self.lru.push(block_num);
    }

    fn evict_if_needed(&mut self) {
        while self.lru.len() > BLOCK_CACHE_CAP {
            if let Some(victim) = self.lru.first().copied() {
                self.lru.remove(0);
                self.blocks.remove(&victim);
            } else {
                break;
            }
        }
    }
}
```

- [ ] **Step 3: Create `fs/dev/mod.rs`**

```rust
// os/common/src/usr/fs/dev/mod.rs
//! Device special files and block device abstractions.

pub mod block_dev;
```

- [ ] **Step 4: Update `fs/mod.rs`**

```rust
// Add after the existing pub mod lines:
pub mod dev;
pub mod page_cache;
```

- [ ] **Step 5: Build to verify**

Run: `cargo build -p common --target aarch64-unknown-none` from `os/`
Expected: compilation succeeds

- [ ] **Step 6: Commit**

```bash
git add os/common/src/usr/fs/
git commit -m "feat(fs): add PageCache, AddressSpace, BlockDevice trait

Page: cached file-data page with read_at/write_at.
AddressSpace: per-inode page cache (BTreeMap).
BlockDevice: trait for block storage, BlockCache with LRU eviction.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: ext4 Data Structures (super_block, block_group, dentry, extent_tree)

**Files:**
- Create: `os/common/src/usr/fs/ext4/mod.rs`
- Create: `os/common/src/usr/fs/ext4/super_block.rs`
- Create: `os/common/src/usr/fs/ext4/block_group.rs`
- Create: `os/common/src/usr/fs/ext4/dentry.rs`
- Create: `os/common/src/usr/fs/ext4/extent_tree.rs`
- Modify: `os/common/src/usr/fs/mod.rs`

**Interfaces:**
- Consumes: `BlockDevice` from `dev::block_dev`, `Errno` from `types`
- Produces: `Ext4SuperBlock`, `Ext4SuperBlockDisk`, `GroupDesc`, `Ext4GroupDescDisk`, `Ext4DirEntry`, `Ext4Extent`, `Ext4ExtentHeader`, `Ext4ExtentIdx`
- Reference: `example/ext4/super_block.rs`, `example/ext4/block_group.rs`, `example/ext4/dentry.rs`, `example/ext4/extent_tree.rs`

- [ ] **Step 1: Read reference files from example/**

Read each of the four reference files in full to understand the structures.
The files to read: `example/ext4/super_block.rs`, `example/ext4/block_group.rs`,
`example/ext4/dentry.rs`, `example/ext4/extent_tree.rs`.

- [ ] **Step 2: Create `fs/ext4/mod.rs` — module root**

```rust
// os/common/src/usr/fs/ext4/mod.rs
//! ext4 filesystem implementation.

pub mod block_group;
pub mod dentry;
pub mod extent_tree;
pub mod super_block;
```

- [ ] **Step 3: Create `fs/ext4/extent_tree.rs`** — copy from example, replace crate paths

Copy the full content of `example/ext4/extent_tree.rs`, then replace:
- `use crate::...` → remove all crate references (this module is self-contained)

- [ ] **Step 4: Create `fs/ext4/dentry.rs`** — copy from example, replace crate paths

Copy the full content of `example/ext4/dentry.rs`, then replace:
- No crate-path changes needed (self-contained module).
- Keep `use alloc::string::String;` and `use alloc::vec::Vec;`.

- [ ] **Step 5: Create `fs/ext4/super_block.rs`** — copy from example, adapt

Copy the full content of `example/ext4/super_block.rs`, then:
- Replace `use crate::fs::FSMutex;` with `use spin::RwLock;`
- Replace `FSMutex<T>` with `RwLock<T>` throughout
- Remove all `use crate::...` lines that don't exist in our kernel
- Keep the `Ext4SuperBlockDisk` struct and `Ext4SuperBlock::new()` logic

- [ ] **Step 6: Create `fs/ext4/block_group.rs`** — copy from example, adapt

Copy the full content of `example/ext4/block_group.rs`, then:
- Replace `use crate::drivers::block::{block_cache::get_block_cache, block_dev::BlockDevice};` with `use super::super::dev::block_dev::BlockDevice;` and implement or stub `get_block_cache`
- Replace `use super::{block_op::Ext4Bitmap, inode::Ext4InodeDisk};` with placeholder — block_op and inode are in later tasks
- For now, comment out the `alloc_inode`/`dealloc_inode`/`alloc_block` methods that depend on `block_op::Ext4Bitmap` — they'll be uncommented in Task 6

- [ ] **Step 7: Update `fs/mod.rs`**

```rust
// Add after the existing lines:
pub mod ext4;
```

- [ ] **Step 8: Build to verify**

Run: `cargo build -p common --target aarch64-unknown-none` from `os/`
Expected: compilation succeeds (may need iterative fix-up of example code imports)

- [ ] **Step 9: Commit**

```bash
git add os/common/src/usr/fs/
git commit -m "feat(fs): port ext4 data structures from example

super_block: Ext4SuperBlockDisk + Ext4SuperBlock.
block_group: Ext4GroupDescDisk + GroupDesc.
dentry: Ext4DirEntry for directory parsing.
extent_tree: Ext4Extent, Ext4ExtentHeader, Ext4ExtentIdx.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: ext4 Block Operations

**Files:**
- Create: `os/common/src/usr/fs/ext4/block_op.rs`
- Modify: `os/common/src/usr/fs/ext4/mod.rs`

**Interfaces:**
- Consumes: `Ext4DirEntry`, `Ext4Extent`, `Ext4ExtentHeader` from ext4 data structures; `BlockDevice` from dev
- Produces: `Ext4DirContentRO`, `Ext4DirContentWE`, `Ext4Bitmap` helper
- Reference: `example/ext4/block_op.rs`

- [ ] **Step 1: Read the reference file**

Read `example/ext4/block_op.rs` in full to understand the implementation.

- [ ] **Step 2: Create `fs/ext4/block_op.rs`** — port from example, adapt imports

Copy the full content of `example/ext4/block_op.rs`, then adapt:

Replace crate references:
- `use crate::drivers::block::block_cache::get_block_cache;` → `use super::super::dev::block_dev::BlockDevice;`
- `use crate::drivers::block::block_dev::BlockDevice;` → (already from above)
- `use crate::ext4::dentry::Ext4DirEntry;` → `use super::dentry::Ext4DirEntry;`
- `use crate::fs::dentry::LinuxDirent64;` → define `LinuxDirent64` locally or use a simplified version
- `use crate::syscall::errno::Errno;` → `use super::super::types::Errno;`
- `use crate::task::kernel_panic;` → replace with `panic!`

Define `LinuxDirent64` locally:

```rust
/// Linux getdents64 return structure.
#[repr(C)]
pub struct LinuxDirent64 {
    pub d_ino: u64,
    pub d_off: u64,
    pub d_reclen: u16,
    pub d_type: u8,
    pub d_name: alloc::string::String,
}
```

- [ ] **Step 3: Update `ext4/mod.rs`**

```rust
// Add:
pub mod block_op;
```

- [ ] **Step 4: Build to verify**

Run: `cargo build -p common --target aarch64-unknown-none` from `os/`
Expected: iterative fix of import paths until compilation succeeds

- [ ] **Step 5: Commit**

```bash
git add os/common/src/usr/fs/ext4/
git commit -m "feat(fs): add ext4 block operations

Ext4DirContentRO/WE for parsing directory entry blocks.
LinuxDirent64 for getdents syscall format.
Ext4Bitmap for block/inode bitmap manipulation.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 6: ext4 Inode (Ext4InodeDisk + Ext4Inode)

**Files:**
- Create: `os/common/src/usr/fs/ext4/inode.rs`
- Modify: `os/common/src/usr/fs/ext4/mod.rs`

**Interfaces:**
- Consumes: `InodeOp` trait from `fs::inode`, `Kstat` from `fs::types`, `Dentry` from `fs::dentry`, `Page` + `AddressSpace` from `fs::page_cache`, `BlockDevice` from `fs::dev`, all ext4 data types
- Produces: `Ext4InodeDisk`, `Ext4Inode`, `load_inode()`, `write_inode()`, `write_inode_on_disk()`
- Reference: `example/ext4/inode.rs` (only the non-la2000 version)

**Key adaptations needed vs example:**
- Remove `#[cfg(feature = "la2000")]` gating — we only support aarch64
- Remove `#[cfg(feature = "virt")]` / `#[cfg(feature = "board")]` xattrs split — use `hashbrown::HashMap` always
- Replace `crate::arch::config::{PAGE_SIZE, PAGE_SIZE_BITS}` with local constants or import from `aarch64` crate
- Replace `crate::fs::FSMutex` with `spin::RwLock`
- Replace `crate::task::current_task()` with a stub returning default uid/gid (real process integration comes later)
- Replace `crate::timer::TimeSpec` with a local definition
- Keep all ext4 logic (extent tree traversal, inline data, read/write/truncate/fallocate)

- [ ] **Step 1: Read the reference file**

Read `example/ext4/inode.rs` in full (all ~2000 lines).

- [ ] **Step 2: Create local TimeSpec**

Add to `fs/types.rs`:

```rust
// os/common/src/usr/fs/types.rs — append at end:

#[derive(Debug, Clone, Copy)]
pub struct TimeSpec {
    pub sec: i64,
    pub nsec: i64,
}

impl TimeSpec {
    /// Return the current wall-clock time (stub — returns 0 for now).
    pub fn new_wall_time() -> Self {
        // TODO: integrate with RTC driver
        Self { sec: 0, nsec: 0 }
    }
}
```

- [ ] **Step 3: Create stub current_task**

Add to `fs/types.rs`:

```rust
// Current task stub — returns default credentials.
// Will be replaced with real task integration when proc subsystem is ready.
pub fn current_task_uid_gid() -> (u32, u32) {
    (0, 0) // root:root
}
```

- [ ] **Step 4: Verify PAGE_SIZE accessible from aarch64 crate**

Check `os/arch/aarch64/src/base/config.rs` for `PAGE_SIZE` constant.
If not present, add:

```rust
// os/arch/aarch64/src/base/config.rs — add:
pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SIZE_BITS: usize = 12;
```

- [ ] **Step 5: Create `fs/ext4/inode.rs`** — port from example with adaptations

Copy the full content of `example/ext4/inode.rs`, then apply all adaptations:

| Original | Replace with |
|----------|-------------|
| `use crate::arch::config::{PAGE_SIZE, PAGE_SIZE_BITS};` | `use crate::arch::config::{PAGE_SIZE, PAGE_SIZE_BITS};` (already exists) |
| `use crate::drivers::block::VIRTIO_BLOCK_SIZE;` | `pub const VIRTIO_BLOCK_SIZE: usize = 512;` |
| `use crate::fs::inode::InodeOp;` | `use super::super::inode::InodeOp;` |
| `use crate::fs::kstat::Kstat;` | `use super::super::types::Kstat;` |
| `use crate::fs::FS_BLOCK_SIZE;` | `use super::super::page_cache::PAGE_SIZE as FS_BLOCK_SIZE;` (or define local) |
| `use crate::fs::FSMutex;` | `use spin::RwLock;` → and `FSMutex::new(v)` becomes `RwLock::new(v)` |
| `use crate::fs::page_cache::AddressSpace;` | `use super::super::page_cache::AddressSpace;` |
| `use crate::mm::Page;` | `use super::super::page_cache::Page;` |
| `use crate::syscall::errno::{Errno, SyscallRet};` | `use super::super::types::{Errno, SyscallRet};` |
| `use crate::task::current_task;` | (use `super::super::types::current_task_uid_gid`) |
| `use crate::timer::TimeSpec;` | `use super::super::types::TimeSpec;` |
| `use crate::arch::config::EXT4_MAX_INLINE_DATA;` | `pub const EXT4_MAX_INLINE_DATA: usize = 60;` |
| `#[cfg(feature = "virt")]` `xattrs: RwLock<HashMap<...>>` | Keep the `HashMap` version unconditionally |
| `#[cfg(feature = "board")]` `xattrs: RwLock<BTreeMap<...>>` | Remove |
| `use hashbrown::HashMap;` | (keep, hashbrown is in Cargo.toml) |

**Important:** Keep all the ext4 logic intact — `read()`, `write()`, `truncate()`,
`fallocate()`, `fsync()`, `lookup_extent()`, `lookup()`, `create()`, `rename()`,
`add_entry()`, extent tree iteration, etc.

The `Ext4Inode` struct should use `spin::Mutex` for `address_space` (instead of `Mutex<AddressSpace>`).

- [ ] **Step 6: Update `ext4/mod.rs`**

```rust
// Add:
pub mod inode;
```

- [ ] **Step 7: Build and iteratively fix compilation errors**

Run: `cargo build -p common --target aarch64-unknown-none` from `os/`
Expected: multiple import/path errors; fix each one matching the new crate structure

- [ ] **Step 8: Commit**

```bash
git add os/common/src/usr/fs/
git commit -m "feat(fs): port ext4 inode implementation

Ext4InodeDisk: on-disk ext4 inode format (160 bytes).
Ext4Inode: in-memory inode with page cache, extent tree,
read/write/truncate/fallocate/lookup/create/rename operations.
Adapted from example/ext4/inode.rs for the QuackOS VFS layer.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 7: ext4 FileSystem (mount/format/alloc/dealloc)

**Files:**
- Create: `os/common/src/usr/fs/ext4/fs.rs`
- Modify: `os/common/src/usr/fs/ext4/mod.rs`

**Interfaces:**
- Consumes: `Ext4SuperBlock`, `GroupDesc`, `Ext4InodeDisk`, `BlockDevice`, `BlockCache`
- Produces: `Ext4FileSystem` (mount/open, alloc/dealloc inode, alloc/dealloc block)
- Reference: `example/ext4/fs.rs`

- [ ] **Step 1: Read the reference file**

Read `example/ext4/fs.rs` in full.

- [ ] **Step 2: Create `fs/ext4/fs.rs`** — port from example, adapt imports

Copy the full content of `example/ext4/fs.rs`, then adapt:

| Original | Replace with |
|----------|-------------|
| `use crate::drivers::block::{block_cache::get_block_cache, block_dev::BlockDevice, VIRTIO_BLOCK_SIZE};` | `use super::super::dev::block_dev::{BlockDevice, BlockCache};` |
| `use crate::ext4::{...}` | `use super::{...}` |
| `use crate::fs::FS_BLOCK_SIZE;` | local constant or from page_cache |

Replace `get_block_cache(block_num, device, block_size)` calls with
`block_cache.get_block(block_num)` using the new `BlockCache` API.

- [ ] **Step 3: Update `ext4/mod.rs`**

```rust
// Add:
pub mod fs;
```

- [ ] **Step 4: Build to verify**

Run: `cargo build -p common --target aarch64-unknown-none` from `os/`
Expected: compilation succeeds after path fixes

- [ ] **Step 5: Commit**

```bash
git add os/common/src/usr/fs/ext4/
git commit -m "feat(fs): port ext4 filesystem mount and allocation

Ext4FileSystem: open/mount, alloc_inode, dealloc_inode,
alloc_block, dealloc_block with first-fit bitmap search.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 8: ext4 InodeOp Trait Implementation

**Files:**
- Create: `os/common/src/usr/fs/ext4/ops.rs` (InodeOp impl for Ext4Inode)
- Modify: `os/common/src/usr/fs/ext4/mod.rs`

**Interfaces:**
- Consumes: `InodeOp` trait, `Ext4Inode`, `Dentry`, all ext4 types
- Produces: `impl InodeOp for Ext4Inode`

- [ ] **Step 1: Create `fs/ext4/ops.rs`** — InodeOp trait implementation

Port the `impl InodeOp for Ext4Inode` block from `example/ext4/mod.rs`.
This includes:
- `as_any()`
- `read()` → delegates to `self.read()`
- `write()` → delegates to `self.write()`
- `lookup()` → the full lookup logic with dentry caching
- `create()` → alloc inode, add directory entry
- `mkdir()` → similar to create but with S_IFDIR
- `symlink()` → create + write symlink target
- `mknod()` → create chr/blk device inode
- `unlink()` → remove directory entry
- `truncate()` → delegates to `self.truncate()`
- `fsync()` → delegates to `self.fsync()`
- `get_stat()` → build Kstat from inode disk fields
- `get_size()` → return inode size
- `getdents()` → iterate directory entries into LinuxDirent64 buffer

- [ ] **Step 2: Update `ext4/mod.rs`**

```rust
// Add:
pub mod ops;
```

- [ ] **Step 3: Build to verify**

Run: `cargo build -p common --target aarch64-unknown-none` from `os/`
Expected: compilation succeeds

- [ ] **Step 4: Commit**

```bash
git add os/common/src/usr/fs/ext4/
git commit -m "feat(fs): implement InodeOp trait for Ext4Inode

Wires the VFS InodeOp trait to ext4 inode operations:
read, write, lookup, create, mkdir, symlink, mknod,
unlink, truncate, fsync, get_stat, getdents.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 9: File Descriptor Table + Device Inodes

**Files:**
- Create: `os/common/src/usr/fs/file.rs`
- Create: `os/common/src/usr/fs/dev/null.rs`
- Create: `os/common/src/usr/fs/dev/tty.rs`
- Create: `os/common/src/usr/fs/dev/urandom.rs`
- Modify: `os/common/src/usr/fs/dev/mod.rs`
- Modify: `os/common/src/usr/fs/mod.rs`

- [ ] **Step 1: Create `fs/file.rs`** — File struct + fd table

```rust
// os/common/src/usr/fs/file.rs
//! File handle and per-process file descriptor table.

use alloc::sync::Arc;
use spin::RwLock;

use super::dentry::Dentry;
use super::inode::InodeOp;
use super::types::{OpenFlags, SeekWhence};

/// An open file — holds a reference to the dentry, a cursor position,
/// and the open flags.
pub struct File {
    pub dentry: Arc<Dentry>,
    pub pos: RwLock<usize>,
    pub flags: OpenFlags,
}

impl File {
    pub fn new(dentry: Arc<Dentry>, flags: OpenFlags) -> Self {
        Self {
            dentry,
            pos: RwLock::new(0),
            flags,
        }
    }

    pub fn read(&self, buf: &mut [u8]) -> usize {
        let inode_opt = self.dentry.inode.read();
        if let Some(ref inode) = *inode_opt {
            let pos = *self.pos.read();
            let n = inode.read(pos, buf);
            *self.pos.write() += n;
            n
        } else {
            0
        }
    }

    pub fn write(&self, buf: &[u8]) -> usize {
        let inode_opt = self.dentry.inode.read();
        if let Some(ref inode) = *inode_opt {
            let pos = *self.pos.read();
            let n = inode.write(pos, buf);
            *self.pos.write() += n;
            n
        } else {
            0
        }
    }

    pub fn seek(&self, offset: isize, whence: SeekWhence) -> usize {
        let mut pos = self.pos.write();
        let size = {
            let inode_opt = self.dentry.inode.read();
            inode_opt.as_ref().map(|i| i.get_size()).unwrap_or(0)
        };
        match whence {
            SeekWhence::Set => *pos = offset as usize,
            SeekWhence::Cur => *pos = ((*pos as isize) + offset) as usize,
            SeekWhence::End => *pos = ((size as isize) + offset) as usize,
        }
        *pos
    }
}

// ---------------------------------------------------------------------------
// Per-process file descriptor table
// ---------------------------------------------------------------------------

const FD_TABLE_SIZE: usize = 256;

pub struct FdTable {
    files: [Option<Arc<File>>; FD_TABLE_SIZE],
}

impl FdTable {
    pub const fn new() -> Self {
        Self {
            files: [const { None }; FD_TABLE_SIZE],
        }
    }

    pub fn alloc_fd(&mut self, file: Arc<File>) -> Option<usize> {
        for (i, slot) in self.files.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(file);
                return Some(i);
            }
        }
        None
    }

    pub fn get_file(&self, fd: usize) -> Option<&Arc<File>> {
        self.files.get(fd).and_then(|s| s.as_ref())
    }

    pub fn close(&mut self, fd: usize) -> bool {
        if fd < FD_TABLE_SIZE && self.files[fd].is_some() {
            self.files[fd] = None;
            true
        } else {
            false
        }
    }
}
```

- [ ] **Step 2: Create device inode files**

Create `fs/dev/null.rs`:

```rust
// os/common/src/usr/fs/dev/null.rs
//! /dev/null — discards all writes, returns EOF on reads.

use alloc::sync::Arc;
use core::any::Any;
use super::super::dentry::Dentry;
use super::super::inode::InodeOp;
use super::super::types::{Errno, FileType, Kstat, SyscallRet};

pub struct NullInode;

impl InodeOp for NullInode {
    fn as_any(&self) -> &dyn Any { self }
    fn read(&self, _offset: usize, _buf: &mut [u8]) -> usize { 0 }
    fn write(&self, _offset: usize, buf: &[u8]) -> usize { buf.len() }
    fn lookup(&self, _name: &str, _parent: Arc<Dentry>) -> Arc<Dentry> {
        Dentry::negative(String::new(), None)
    }
    fn create(&self, _dentry: Arc<Dentry>, _mode: u16) {}
    fn mkdir(&self, _dentry: Arc<Dentry>, _mode: u16) {}
    fn symlink(&self, _dentry: Arc<Dentry>, _target: &str) {}
    fn mknod(&self, _dentry: Arc<Dentry>, _mode: u16, _rdev: u64) {}
    fn unlink(&self, _dentry: Arc<Dentry>) -> SyscallRet { Err(Errno::EPERM) }
    fn truncate(&self, _size: usize) -> SyscallRet { Ok(0) }
    fn fsync(&self) -> SyscallRet { Ok(0) }
    fn get_stat(&self) -> Kstat {
        let mut st = Kstat::default();
        st.file_type = FileType::ChrDev;
        st.rdev = 0x0103; // major 1, minor 3
        st
    }
    fn get_size(&self) -> usize { 0 }
    fn getdents(&self, _buf: &mut [u8]) -> (usize, usize) { (0, 0) }
}
```

Create `fs/dev/tty.rs` (stub — returns EOF for now):

```rust
// os/common/src/usr/fs/dev/tty.rs
//! /dev/tty — controlling terminal.

use alloc::sync::Arc;
use core::any::Any;
use super::super::dentry::Dentry;
use super::super::inode::InodeOp;
use super::super::types::{Errno, FileType, Kstat, SyscallRet};

pub struct TtyInode;

impl InodeOp for TtyInode {
    fn as_any(&self) -> &dyn Any { self }
    fn read(&self, _offset: usize, _buf: &mut [u8]) -> usize { 0 }
    fn write(&self, _offset: usize, buf: &[u8]) -> usize { buf.len() }
    fn lookup(&self, _name: &str, _parent: Arc<Dentry>) -> Arc<Dentry> {
        Dentry::negative(String::new(), None)
    }
    fn create(&self, _dentry: Arc<Dentry>, _mode: u16) {}
    fn mkdir(&self, _dentry: Arc<Dentry>, _mode: u16) {}
    fn symlink(&self, _dentry: Arc<Dentry>, _target: &str) {}
    fn mknod(&self, _dentry: Arc<Dentry>, _mode: u16, _rdev: u64) {}
    fn unlink(&self, _dentry: Arc<Dentry>) -> SyscallRet { Err(Errno::EPERM) }
    fn truncate(&self, _size: usize) -> SyscallRet { Ok(0) }
    fn fsync(&self) -> SyscallRet { Ok(0) }
    fn get_stat(&self) -> Kstat {
        let mut st = Kstat::default();
        st.file_type = FileType::ChrDev;
        st.rdev = 0x0500; // major 5, minor 0
        st
    }
    fn get_size(&self) -> usize { 0 }
    fn getdents(&self, _buf: &mut [u8]) -> (usize, usize) { (0, 0) }
}
```

Create `fs/dev/urandom.rs` (stub):

```rust
// os/common/src/usr/fs/dev/urandom.rs
//! /dev/urandom — non-blocking random number generator.

use alloc::sync::Arc;
use core::any::Any;
use super::super::dentry::Dentry;
use super::super::inode::InodeOp;
use super::super::types::{Errno, FileType, Kstat, SyscallRet};

pub struct UrandomInode;

impl InodeOp for UrandomInode {
    fn as_any(&self) -> &dyn Any { self }
    fn read(&self, _offset: usize, buf: &mut [u8]) -> usize {
        for b in buf.iter_mut() { *b = 0xAA; } // deterministic stub
        buf.len()
    }
    fn write(&self, _offset: usize, _buf: &[u8]) -> usize { 0 }
    fn lookup(&self, _name: &str, _parent: Arc<Dentry>) -> Arc<Dentry> {
        Dentry::negative(String::new(), None)
    }
    fn create(&self, _dentry: Arc<Dentry>, _mode: u16) {}
    fn mkdir(&self, _dentry: Arc<Dentry>, _mode: u16) {}
    fn symlink(&self, _dentry: Arc<Dentry>, _target: &str) {}
    fn mknod(&self, _dentry: Arc<Dentry>, _mode: u16, _rdev: u64) {}
    fn unlink(&self, _dentry: Arc<Dentry>) -> SyscallRet { Err(Errno::EPERM) }
    fn truncate(&self, _size: usize) -> SyscallRet { Ok(0) }
    fn fsync(&self) -> SyscallRet { Ok(0) }
    fn get_stat(&self) -> Kstat {
        let mut st = Kstat::default();
        st.file_type = FileType::ChrDev;
        st.rdev = 0x0109; // major 1, minor 9
        st
    }
    fn get_size(&self) -> usize { 0 }
    fn getdents(&self, _buf: &mut [u8]) -> (usize, usize) { (0, 0) }
}
```

- [ ] **Step 3: Update `dev/mod.rs`**

```rust
// os/common/src/usr/fs/dev/mod.rs
pub mod block_dev;
pub mod null;
pub mod tty;
pub mod urandom;
```

- [ ] **Step 4: Update `fs/mod.rs`**

```rust
// Add:
pub mod file;
```

- [ ] **Step 5: Build to verify**

Run: `cargo build -p common --target aarch64-unknown-none` from `os/`
Expected: compilation succeeds

- [ ] **Step 6: Commit**

```bash
git add os/common/src/usr/fs/
git commit -m "feat(fs): add File descriptor table and device inodes

File: open file handle with pos + flags.
FdTable: per-process fd table (256 slots).
NullInode: /dev/null — discard writes, EOF on read.
TtyInode: /dev/tty stub.
UrandomInode: /dev/urandom stub.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 10: FsServer (IPC Event Loop + Worker Threads)

**Files:**
- Create: `os/common/src/usr/fs/server.rs`
- Modify: `os/common/src/usr/fs/mod.rs`

**Interfaces:**
- Consumes: All VFS types, `InodeOp`, `Dentry`, `File`, `FdTable`, IPC types
- Produces: `FsServer` with event loop

- [ ] **Step 1: Create `fs/server.rs`** — FsServer

This is the core IPC request handler. It:
1. Maintains a global dentry tree root + mount table
2. Maintains per-process FdTable map
3. Receives IPC messages, dispatches to VFS, sends replies

```rust
// os/common/src/usr/fs/server.rs
//! File System server — user-space IPC server that handles all filesystem
//! requests.  Single-threaded event loop + worker thread pool for block I/O.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;

use crate::kernel::ipc::ChannelId;
use crate::kernel::ipc::message::{Message, ProcessId, ShortPayload};

use super::dentry::Dentry;
use super::dentry::DentryFlags;
use super::file::{FdTable, File};
use super::inode::InodeOp;
use super::types::{Errno, Kstat, OpenFlags, SeekWhence};

// ---------------------------------------------------------------------------
// FS request types (arrive via IPC ShortPayload)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum FsRequest {
    Open {
        pid: ProcessId,
        path: String,
        flags: OpenFlags,
        mode: u16,
    },
    Read {
        pid: ProcessId,
        fd: usize,
        count: usize,
    },
    Write {
        pid: ProcessId,
        fd: usize,
        data: Vec<u8>,
    },
    Close {
        pid: ProcessId,
        fd: usize,
    },
    Lseek {
        pid: ProcessId,
        fd: usize,
        offset: isize,
        whence: SeekWhence,
    },
    Stat {
        pid: ProcessId,
        path: String,
    },
    Getdents {
        pid: ProcessId,
        fd: usize,
        count: usize,
    },
}

#[derive(Debug)]
pub enum FsResponse {
    Fd(usize),                   // open → fd
    Data(Vec<u8>),               // read → bytes
    Count(usize),                // write → bytes written
    Position(usize),             // lseek → new position
    Stat(Kstat),                 // stat → metadata
    Error(Errno),
}

// ---------------------------------------------------------------------------
// FsServer
// ---------------------------------------------------------------------------

pub struct FsServer {
    /// Global root dentry.
    pub root: Arc<Dentry>,
    /// Per-process file descriptor tables.
    pub fd_tables: RwLock<BTreeMap<ProcessId, FdTable>>,
    /// Channel this server listens on.
    pub channel_id: ChannelId,
}

impl FsServer {
    pub fn new(channel_id: ChannelId, root_inode: Arc<dyn InodeOp>) -> Self {
        let root = Dentry::new(
            "/".into(),
            None,
            DentryFlags::DIRECTORY,
            Some(root_inode),
        );
        Self {
            root,
            fd_tables: RwLock::new(BTreeMap::new()),
            channel_id,
        }
    }

    /// Path walk: resolve `path` relative to `root`, returning the target
    /// Dentry and its parent.
    pub fn path_walk(
        root: &Arc<Dentry>,
        path: &str,
    ) -> Result<(Arc<Dentry>, Arc<Dentry>), Errno> {
        let components: Vec<&str> = path
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if components.is_empty() {
            return Ok((root.clone(), root.clone()));
        }

        let mut current = root.clone();
        for &name in &components[..components.len() - 1] {
            // Check child cache first.
            let next = if let Some(child) = current.get_child(name) {
                child
            } else {
                // Do actual lookup.
                let inode_opt = current.inode.read();
                if let Some(ref inode) = *inode_opt {
                    let child = inode.lookup(name, current.clone());
                    current
                        .children
                        .write()
                        .insert(name.into(), Arc::downgrade(&child));
                    child
                } else {
                    return Err(Errno::ENOENT);
                }
            };
            current = next;
        }

        let last_name = components.last().unwrap();
        let target = if let Some(child) = current.get_child(last_name) {
            child
        } else {
            let inode_opt = current.inode.read();
            if let Some(ref inode) = *inode_opt {
                let child = inode.lookup(last_name, current.clone());
                current
                    .children
                    .write()
                    .insert((*last_name).into(), Arc::downgrade(&child));
                child
            } else {
                return Err(Errno::ENOENT);
            }
        };

        Ok((target, current))
    }

    /// Handle a single request, returning a response.
    pub fn handle_request(&self, req: FsRequest) -> FsResponse {
        match req {
            FsRequest::Open { pid, path, flags, mode } => {
                let result = Self::path_walk(&self.root, &path);
                match result {
                    Ok((dentry, _parent)) => {
                        if dentry.is_negative() && flags.contains(OpenFlags::O_CREAT) {
                            // TODO: call parent.create()
                            return FsResponse::Error(Errno::ENOSYS);
                        }
                        let file = Arc::new(File::new(dentry, flags));
                        let mut tables = self.fd_tables.write();
                        let table = tables.entry(pid).or_insert_with(FdTable::new);
                        match table.alloc_fd(file) {
                            Some(fd) => FsResponse::Fd(fd),
                            None => FsResponse::Error(Errno::ENOMEM),
                        }
                    }
                    Err(e) => FsResponse::Error(e),
                }
            }
            FsRequest::Read { pid, fd, count } => {
                let tables = self.fd_tables.read();
                if let Some(table) = tables.get(&pid) {
                    if let Some(file) = table.get_file(fd) {
                        let mut buf = vec![0u8; count];
                        let n = file.read(&mut buf);
                        buf.truncate(n);
                        return FsResponse::Data(buf);
                    }
                }
                FsResponse::Error(Errno::EBADF)
            }
            FsRequest::Write { pid, fd, data } => {
                let tables = self.fd_tables.read();
                if let Some(table) = tables.get(&pid) {
                    if let Some(file) = table.get_file(fd) {
                        let n = file.write(&data);
                        return FsResponse::Count(n);
                    }
                }
                FsResponse::Error(Errno::EBADF)
            }
            FsRequest::Close { pid, fd } => {
                let mut tables = self.fd_tables.write();
                if let Some(table) = tables.get_mut(&pid) {
                    if table.close(fd) {
                        return FsResponse::Count(0);
                    }
                }
                FsResponse::Error(Errno::EBADF)
            }
            FsRequest::Lseek { pid, fd, offset, whence } => {
                let tables = self.fd_tables.read();
                if let Some(table) = tables.get(&pid) {
                    if let Some(file) = table.get_file(fd) {
                        let pos = file.seek(offset, whence);
                        return FsResponse::Position(pos);
                    }
                }
                FsResponse::Error(Errno::EBADF)
            }
            FsRequest::Stat { pid: _, path } => {
                match Self::path_walk(&self.root, &path) {
                    Ok((dentry, _)) => {
                        if let Some(ref inode) = *dentry.inode.read() {
                            FsResponse::Stat(inode.get_stat())
                        } else {
                            FsResponse::Error(Errno::ENOENT)
                        }
                    }
                    Err(e) => FsResponse::Error(e),
                }
            }
            FsRequest::Getdents { pid, fd, count } => {
                let tables = self.fd_tables.read();
                if let Some(table) = tables.get(&pid) {
                    if let Some(file) = table.get_file(fd) {
                        let mut buf = vec![0u8; count];
                        if let Some(ref inode) = *file.dentry.inode.read() {
                            let (n, _) = inode.getdents(&mut buf);
                            buf.truncate(n);
                            return FsResponse::Data(buf);
                        }
                    }
                }
                FsResponse::Error(Errno::EBADF)
            }
        }
    }
}
```

- [ ] **Step 2: Update `fs/mod.rs`**

```rust
// Add:
pub mod server;
```

- [ ] **Step 3: Build to verify**

Run: `cargo build -p common --target aarch64-unknown-none` from `os/`
Expected: compilation succeeds

- [ ] **Step 4: Commit**

```bash
git add os/common/src/usr/fs/
git commit -m "feat(fs): add FsServer with IPC event loop

FsServer: path_walk resolution, per-process fd tables,
handle_request dispatcher for open/read/write/close/seek/stat/getdents.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 11: Integration — Wire fs into kernel + init

**Files:**
- Modify: `os/common/src/usr/mod.rs`
- Modify: `os/common/src/main.rs`
- Modify: `os/common/Cargo.toml`
- Modify: `os/arch/aarch64/src/base/config.rs`

- [ ] **Step 1: Add `pub mod fs` to `usr/mod.rs`**

Read `os/common/src/usr/mod.rs`, then:

```rust
// Add after `pub mod proc;`:
pub mod fs;
```

- [ ] **Step 2: Verify PAGE_SIZE is accessible**

Check `os/arch/aarch64/src/base/config.rs` has:
```rust
pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SIZE_BITS: usize = 12;
```

If the `config.rs` module is not `pub` in the aarch64 `lib.rs`, make sure
`pub mod config;` is in `os/arch/aarch64/src/base/mod.rs`.

- [ ] **Step 3: Build the full kernel**

Run: `cargo build -p common --target aarch64-unknown-none` from `os/`
Expected: full compilation succeeds with all 11 modules

- [ ] **Step 4: Add a basic boot-time FS test**

Add to `os/common/src/kernel/tests/mod.rs`:

```rust
mod fs_test;

// In run_all():
let (fs_p, fs_t) = fs_test::run();
// Add fs_p/fs_t to the totals
```

Create `os/common/src/kernel/tests/fs_test.rs`:

```rust
//! Filesystem smoke tests — verify VFS types compile and basic operations work.

use crate::usr::fs::types::{Errno, FileType, Kstat, OpenFlags};

pub fn run() -> (usize, usize) {
    let mut passed = 0;
    let total = 3;

    if test_kstat_default() { passed += 1; }
    if test_open_flags() { passed += 1; }
    if test_file_type() { passed += 1; }

    (passed, total)
}

fn test_kstat_default() -> bool {
    let st = Kstat::default();
    st.ino == 0 && st.size == 0 && st.blksize == 4096
}

fn test_open_flags() -> bool {
    let flags = OpenFlags::O_RDONLY | OpenFlags::O_CREAT;
    flags.contains(OpenFlags::O_CREAT) && !flags.contains(OpenFlags::O_WRONLY)
}

fn test_file_type() -> bool {
    let dir = FileType::Dir;
    dir.is_dir() && !dir.is_reg()
}
```

- [ ] **Step 5: Final build and fix all remaining issues**

Run: `cargo build -p common --target aarch64-unknown-none` from `os/`
Expected: clean compilation, no warnings (or only unused-import warnings)

- [ ] **Step 6: Commit**

```bash
git add os/
git commit -m "feat(fs): integrate VFS + ext4 into QuackOS kernel

Wire the fs module into usr/mod.rs, add boot-time FS smoke tests,
verify PAGE_SIZE accessible from aarch64 config.

This completes the initial VFS + ext4 filesystem support for aarch64.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
