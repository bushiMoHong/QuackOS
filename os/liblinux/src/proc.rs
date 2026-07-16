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

/// gettid() — syscall 178
pub fn sys_gettid(task: &TaskStruct) -> u64 {
    task.pid
}

/// sched_yield() — syscall 124
pub fn sys_sched_yield() -> u64 {
    unsafe { crate::native::yield_cpu(); }
    0
}

/// prctl(option, arg2, arg3, arg4, arg5) — syscall 167
///
/// Minimal implementation covering common operations.
#[allow(non_upper_case_globals)]
pub fn sys_prctl(task: &mut TaskStruct, option: usize, arg2: usize, _arg3: usize, _arg4: usize, _arg5: usize) -> u64 {
    // PR_SET_NAME (15) — set process name
    const PR_SET_NAME: usize = 15;
    // PR_GET_NAME (16) — get process name
    const PR_GET_NAME: usize = 16;
    // PR_SET_SECCOMP (22) — set seccomp mode
    const PR_SET_SECCOMP: usize = 22;
    // PR_CAPBSET_DROP (24) — drop capability
    const PR_CAPBSET_DROP: usize = 24;
    // PR_SET_NO_NEW_PRIVS (36) — set no_new_privs
    const PR_SET_NO_NEW_PRIVS: usize = 36;
    // PR_GET_NO_NEW_PRIVS (39) — get no_new_privs
    const PR_GET_NO_NEW_PRIVS: usize = 39;
    // PR_SET_VMA (0x53564d41) — set VMA properties
    const PR_SET_VMA: usize = 0x53564d41;

    match option {
        PR_SET_NAME => 0,
        PR_GET_NAME => {
            // Write placeholder process name to *arg2
            if arg2 != 0 {
                unsafe {
                    let name = b"quackos\0";
                    core::ptr::copy_nonoverlapping(name.as_ptr(), arg2 as *mut u8, name.len());
                }
            }
            0
        }
        PR_SET_NO_NEW_PRIVS => {
            task.no_new_privs = true;
            0
        }
        PR_GET_NO_NEW_PRIVS => task.no_new_privs as u64,
        PR_SET_SECCOMP => 0, // accept but ignore
        PR_CAPBSET_DROP => 0,
        PR_SET_VMA => 0,
        _ => (-crate::errno::EINVAL as u64),
    }
}
