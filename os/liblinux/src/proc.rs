//! Linux process management syscall implementations.

use crate::task::TaskStruct;
use crate::native;

/// exit_group(status) — syscall 94
pub fn sys_exit_group(_task: &mut TaskStruct, status: usize) -> ! {
    unsafe { native::exit_thread(status); }
}

/// exit(status) — syscall 93
pub fn sys_exit(task: &mut TaskStruct, status: usize) -> ! {
    task.exit_code = status as i32;
    // If clear_child_tid is set, zero it before exit (Linux ABI).
    if task.clear_child_tid != 0 {
        unsafe { *(task.clear_child_tid as *mut u32) = 0; }
    }
    unsafe { native::exit_thread(status); }
}

/// set_tid_address(tidptr) — syscall 96
pub fn sys_set_tid_address(task: &mut TaskStruct, tidptr: usize) -> u64 {
    task.clear_child_tid = tidptr;
    task.pid
}

/// getpid() — syscall 172
pub fn sys_getpid(task: &TaskStruct) -> u64 {
    task.pid
}

/// getuid() — syscall 174
pub fn sys_getuid(_task: &TaskStruct) -> u64 {
    0 // root
}

/// geteuid() — syscall 175
pub fn sys_geteuid(_task: &TaskStruct) -> u64 {
    0 // root
}

/// getgid() — syscall 176
pub fn sys_getgid(_task: &TaskStruct) -> u64 {
    0 // root
}

/// getegid() — syscall 177
pub fn sys_getegid(_task: &TaskStruct) -> u64 {
    0 // root
}
