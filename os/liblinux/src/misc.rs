//! Miscellaneous Linux syscall implementations.

/// uname — syscall 160
pub fn sys_uname(buf_ptr: usize) -> u64 {
    // struct utsname { sysname[65], nodename[65], release[65], version[65], machine[65] }
    // Write a minimal uname to the user buffer.
    unsafe {
        let p = buf_ptr as *mut u8;
        // sysname = "Linux"
        let sysname = b"Linux\0";
        core::ptr::copy_nonoverlapping(sysname.as_ptr(), p, sysname.len());
        // nodename = "quackos"
        let nodename = b"quackos\0";
        core::ptr::copy_nonoverlapping(nodename.as_ptr(), p.add(65), nodename.len());
        // release = "5.0.0"
        let release = b"5.0.0\0";
        core::ptr::copy_nonoverlapping(release.as_ptr(), p.add(130), release.len());
        // machine = "aarch64"
        let machine = b"aarch64\0";
        core::ptr::copy_nonoverlapping(machine.as_ptr(), p.add(260), machine.len());
    }
    0
}

/// getrandom — syscall 278 (stub)
pub fn sys_getrandom(_buf: usize, _len: usize, _flags: usize) -> u64 {
    0
}

/// getcwd — syscall 17
pub fn sys_getcwd(task: &crate::task::TaskStruct, buf: usize, size: usize) -> u64 {
    if size == 0 { return (-crate::errno::EINVAL as u64); }
    let cwd_len = task.cwd.iter().position(|&b| b == 0).unwrap_or(0);
    let copy = cwd_len.min(size - 1);
    if buf != 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(task.cwd.as_ptr(), buf as *mut u8, copy);
            *((buf as *mut u8).add(copy)) = 0;
        }
    }
    buf as u64
}

/// getpgid(pid) — syscall 154
pub fn sys_getpgid(pid: usize, task_pid: u64) -> u64 {
    if pid == 0 { task_pid } else { task_pid }
}

/// setpgid(pid, pgid) — syscall 155
#[allow(unused_variables)]
pub fn sys_setpgid(pid: usize, pgid: usize) -> u64 {
    0
}

/// faccessat2(dfd, filename, mode, flags) — syscall 439
pub fn sys_faccessat2() -> u64 {
    0
}
