//! Linux memory management syscall implementations.

use crate::errno;
use crate::task::TaskStruct;

/// brk(new_brk) — syscall 214
///
/// Sets the program break. If new_brk is 0, returns the current brk.
pub fn sys_brk(task: &mut TaskStruct, new_brk: usize) -> u64 {
    if new_brk == 0 {
        return task.brk as u64;
    }
    match task.do_brk(new_brk) {
        Ok(brk) => brk as u64,
        Err(e) => (-e as u64),
    }
}

/// mmap — syscall 222 (stub)
pub fn sys_mmap(_task: &mut TaskStruct, _addr: usize, _len: usize, _prot: usize,
                _flags: usize, _fd: usize, _off: usize) -> u64 {
    (-errno::ENOSYS as u64)
}

/// munmap — syscall 215 (stub)
pub fn sys_munmap(_task: &mut TaskStruct, _addr: usize, _len: usize) -> u64 {
    (-errno::ENOSYS as u64)
}
