//! File System server — user-space IPC server that handles all filesystem
//! requests.  Single-threaded event loop + worker thread pool for block I/O.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use spin::RwLock;

use super::dentry::{Dentry, DentryFlags};
use super::file::{FdTable, File};
use super::inode::InodeOp;
use super::types::{Errno, Kstat, OpenFlags, SeekWhence};

// ---------------------------------------------------------------------------
// FsServer
// ---------------------------------------------------------------------------

pub struct FsServer {
    /// Global root dentry.
    pub root: Arc<Dentry>,
    /// Per-process file descriptor tables.
    pub fd_tables: RwLock<BTreeMap<u32, FdTable>>,
}

impl FsServer {
    pub fn new(root_inode: Arc<dyn InodeOp>) -> Self {
        let root = Dentry::new("/".into(), None, DentryFlags::DIRECTORY, Some(root_inode));
        Self {
            root,
            fd_tables: RwLock::new(BTreeMap::new()),
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
            let next = if let Some(child) = current.get_child(name) {
                child
            } else {
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

    /// Open a file for a process, returning the fd.
    pub fn open(
        &self,
        pid: u32,
        path: &str,
        flags: OpenFlags,
        _mode: u16,
    ) -> Result<usize, Errno> {
        let (dentry, _parent) = Self::path_walk(&self.root, path)?;
        if dentry.is_negative() && !flags.contains(OpenFlags::O_CREAT) {
            return Err(Errno::ENOENT);
        }
        let file = Arc::new(File::new(dentry, flags));
        let mut tables = self.fd_tables.write();
        let table = tables.entry(pid).or_insert_with(FdTable::new);
        table
            .alloc_fd(file)
            .ok_or(Errno::ENOMEM)
    }

    /// Read from a file descriptor.
    pub fn read(&self, pid: u32, fd: usize, count: usize) -> Result<Vec<u8>, Errno> {
        let tables = self.fd_tables.read();
        let table = tables.get(&pid).ok_or(Errno::EBADF)?;
        let file = table.get_file(fd).ok_or(Errno::EBADF)?;
        let mut buf = vec![0u8; count];
        let n = file.read(&mut buf);
        buf.truncate(n);
        Ok(buf)
    }

    /// Write to a file descriptor.
    pub fn write(&self, pid: u32, fd: usize, data: &[u8]) -> Result<usize, Errno> {
        let tables = self.fd_tables.read();
        let table = tables.get(&pid).ok_or(Errno::EBADF)?;
        let file = table.get_file(fd).ok_or(Errno::EBADF)?;
        Ok(file.write(data))
    }

    /// Close a file descriptor.
    pub fn close(&self, pid: u32, fd: usize) -> Result<(), Errno> {
        let mut tables = self.fd_tables.write();
        let table = tables.get_mut(&pid).ok_or(Errno::EBADF)?;
        if table.close(fd) {
            Ok(())
        } else {
            Err(Errno::EBADF)
        }
    }

    /// Seek within a file.
    pub fn lseek(&self, pid: u32, fd: usize, offset: isize, whence: SeekWhence) -> Result<usize, Errno> {
        let tables = self.fd_tables.read();
        let table = tables.get(&pid).ok_or(Errno::EBADF)?;
        let file = table.get_file(fd).ok_or(Errno::EBADF)?;
        Ok(file.seek(offset, whence))
    }

    /// Get file metadata by path.
    pub fn stat(&self, _pid: u32, path: &str) -> Result<Kstat, Errno> {
        let (dentry, _) = Self::path_walk(&self.root, path)?;
        let inode_opt = dentry.inode.read().clone();
        if let Some(ref inode) = inode_opt {
            Ok(inode.get_stat())
        } else {
            Err(Errno::ENOENT)
        }
    }

    /// Get file metadata by fd.
    pub fn fstat(&self, pid: u32, fd: usize) -> Result<Kstat, Errno> {
        let tables = self.fd_tables.read();
        let table = tables.get(&pid).ok_or(Errno::EBADF)?;
        let file = table.get_file(fd).ok_or(Errno::EBADF)?;
        let inode_opt = file.dentry.inode.read().clone();
        if let Some(ref inode) = inode_opt {
            Ok(inode.get_stat())
        } else {
            Err(Errno::EBADF)
        }
    }

    /// Read directory entries.
    pub fn getdents(&self, pid: u32, fd: usize, count: usize) -> Result<Vec<u8>, Errno> {
        let tables = self.fd_tables.read();
        let table = tables.get(&pid).ok_or(Errno::EBADF)?;
        let file = table.get_file(fd).ok_or(Errno::EBADF)?;
        if file.dentry.flags.read().contains(DentryFlags::DIRECTORY) {
            let mut buf = vec![0u8; count];
            let (n, _) = file.getdents(&mut buf);
            buf.truncate(n);
            Ok(buf)
        } else {
            Err(Errno::ENOTDIR)
        }
    }

    /// Create a new file.
    pub fn create(&self, _pid: u32, path: &str, mode: u16) -> Result<(), Errno> {
        let (dentry, parent_dentry) = Self::path_walk(&self.root, path)?;
        if !dentry.is_negative() {
            return Err(Errno::EEXIST);
        }
        let pinode_opt = parent_dentry.inode.read().clone();
        if let Some(ref inode) = pinode_opt {
            inode.create(dentry.clone(), mode);
            Ok(())
        } else {
            Err(Errno::ENOENT)
        }
    }

    /// Create a new directory.
    pub fn mkdir(&self, _pid: u32, path: &str, mode: u16) -> Result<(), Errno> {
        let (dentry, parent_dentry) = Self::path_walk(&self.root, path)?;
        if !dentry.is_negative() {
            return Err(Errno::EEXIST);
        }
        let pinode_opt = parent_dentry.inode.read().clone();
        if let Some(ref inode) = pinode_opt {
            inode.mkdir(dentry.clone(), mode);
            Ok(())
        } else {
            Err(Errno::ENOENT)
        }
    }
}
