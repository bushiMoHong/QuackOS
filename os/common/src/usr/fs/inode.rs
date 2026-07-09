//! Inode operations trait — the core VFS abstraction that every filesystem
//! implements.  Mirrors Linux's `struct inode_operations`.

use alloc::sync::Arc;
use core::any::Any;

use super::dentry::Dentry;
use super::types::{Kstat, SyscallRet};

/// The central VFS trait.  Every inode (file, directory, device, pipe, etc.)
/// must implement this trait.
pub trait InodeOp: Send + Sync {
    /// Downcast to concrete type.
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

    /// Create a new symbolic link inside this directory.
    fn symlink(&self, dentry: Arc<Dentry>, target: &str);

    /// Create a device node (chr/blk) inside this directory.
    fn mknod(&self, dentry: Arc<Dentry>, mode: u16, rdev: u64);

    /// Remove a directory entry from this directory.
    fn unlink(&self, dentry: Arc<Dentry>) -> SyscallRet;

    /// Truncate the file to `size` bytes.
    fn truncate(&self, size: usize) -> SyscallRet;

    /// Flush any pending metadata/data to storage.
    fn fsync(&self) -> SyscallRet;

    /// Return file metadata.
    fn get_stat(&self) -> Kstat;

    /// Return the file size in bytes.
    fn get_size(&self) -> usize;

    /// Read directory entries into a buffer.  Returns (bytes_written, file_offset).
    fn getdents(&self, buf: &mut [u8]) -> (usize, usize);
}
