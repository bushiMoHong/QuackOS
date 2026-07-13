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
    unsafe { native::exit_thread(status); }
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
