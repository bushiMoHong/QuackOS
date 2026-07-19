//! File handle and per-process file descriptor table.

use alloc::sync::Arc;
use spin::RwLock;

use super::dentry::Dentry;
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
        if let Some(ref inode) = *self.dentry.inode.read() {
            let pos = *self.pos.read();
            let n = inode.read(pos, buf);
            *self.pos.write() += n;
            n
        } else {
            0
        }
    }

    pub fn write(&self, buf: &[u8]) -> usize {
        if let Some(ref inode) = *self.dentry.inode.read() {
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
        let size = self
            .dentry
            .inode
            .read()
            .as_ref()
            .map(|i| i.get_size())
            .unwrap_or(0);
        match whence {
            SeekWhence::Set => *pos = offset as usize,
            SeekWhence::Cur => *pos = ((*pos as isize) + offset) as usize,
            SeekWhence::End => *pos = ((size as isize) + offset) as usize,
        }
        *pos
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
