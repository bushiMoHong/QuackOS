//! /dev/null — discards all writes, returns EOF on reads.

use alloc::string::String;
use alloc::sync::Arc;
use core::any::Any;

use super::super::dentry::Dentry;
use super::super::inode::InodeOp;
use super::super::types::{Errno, FileType, Kstat, SyscallRet};

pub struct NullInode;

impl InodeOp for NullInode {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn read(&self, _offset: usize, _buf: &mut [u8]) -> usize {
        0
    }
    fn write(&self, _offset: usize, buf: &[u8]) -> usize {
        buf.len()
    }
    fn lookup(&self, _name: &str, _parent: Arc<Dentry>) -> Arc<Dentry> {
        Dentry::negative(String::new(), None)
    }
    fn create(&self, _dentry: Arc<Dentry>, _mode: u16) {}
    fn mkdir(&self, _dentry: Arc<Dentry>, _mode: u16) {}
    fn symlink(&self, _dentry: Arc<Dentry>, _target: &str) {}
    fn mknod(&self, _dentry: Arc<Dentry>, _mode: u16, _rdev: u64) {}
    fn unlink(&self, _dentry: Arc<Dentry>) -> SyscallRet {
        Err(Errno::EPERM)
    }
    fn truncate(&self, _size: usize) -> SyscallRet {
        Ok(0)
    }
    fn fsync(&self) -> SyscallRet {
        Ok(0)
    }
    fn get_stat(&self) -> Kstat {
        let mut st = Kstat::default();
        st.file_type = FileType::ChrDev;
        st.rdev = 0x0103; // major 1, minor 3
        st
    }
    fn get_size(&self) -> usize {
        0
    }
    fn getdents(&self, _offset: usize, _buf: &mut [u8]) -> (usize, usize) {
        (0, 0)
    }
}
