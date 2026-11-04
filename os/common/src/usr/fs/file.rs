//! File handle and per-process file descriptor table.

use alloc::sync::Arc;
use spin::RwLock;

use super::dentry::Dentry;
use super::types::{OpenFlags, SeekWhence, Errno};

use crate::print_uart_hex;
use crate::print_uart;

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

    pub fn open(dentry: Arc<Dentry>, flags: OpenFlags) -> Result<Self, Errno> {
        if dentry.is_negative() {
            return Err(Errno::ENOENT);
        }
        if dentry.inode.read().is_none() {
            return Err(Errno::EIO);
        }
        Ok(Self { dentry, pos: RwLock::new(0), flags })
    }

    pub fn read(&self, buf: &mut [u8]) -> Result<usize, Errno> {
        let inode_guard = self.dentry.inode.read();
        let inode = inode_guard.as_ref().ok_or(Errno::EIO)?;
        let pos = *self.pos.read();

        print_uart("[file.read] pos=");
        print_uart_hex(pos as u64);
        print_uart(" len=");
        print_uart_hex(buf.len() as u64);

        let n = inode.read(pos, buf);

        print_uart(" n=");
        print_uart_hex(n as u64);
        print_uart("\n");

        if n > 0 {
            *self.pos.write() += n;
        }
        Ok(n)
    }

    pub fn write(&self, buf: &[u8]) -> Result<usize, Errno> {
        let inode_guard = self.dentry.inode.read();
        let inode = inode_guard.as_ref().ok_or(Errno::EIO)?;
        let pos = *self.pos.read();
        let n = inode.write(pos, buf);
        if n > 0 {
            *self.pos.write() += n;
        }
        Ok(n)
    }

    pub fn seek(&self, offset: isize, whence: SeekWhence) -> Result<usize, Errno> {
        let mut pos = self.pos.write();

        let base: isize = match whence {
            SeekWhence::Set => 0,
            SeekWhence::Cur => isize::try_from(*pos).map_err(|_| Errno::EINVAL)?,
            SeekWhence::End => {
                let size = self
                    .dentry
                    .inode
                    .read()
                    .as_ref()
                    .map(|i| i.get_size())
                    .unwrap_or(0);
                isize::try_from(size).map_err(|_| Errno::EINVAL)?
            }
        };

        // checked_add: 溢出 → EINVAL；结果 < 0 → EINVAL
        let new = base.checked_add(offset).ok_or(Errno::EINVAL)?;
        if new < 0 {
            return Err(Errno::EINVAL);
        }

        *pos = new as usize;
        Ok(*pos)
    }

    pub fn getdents(&self, buf: &mut [u8]) -> (usize, usize) {
        if let Some(ref inode) = *self.dentry.inode.read() {
            let pos = *self.pos.read();
            let (new_pos, written) = inode.getdents(pos, buf);
            *self.pos.write() = new_pos;
            (new_pos, written)
        } else {
            (0, 0)
        }
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
