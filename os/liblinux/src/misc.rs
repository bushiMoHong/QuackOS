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

/// getcwd — syscall 17 (stub)
pub fn sys_getcwd(_buf: usize, _size: usize) -> u64 {
    // Write "/" as the current working directory
    if _size > 0 {
        unsafe {
            *(_buf as *mut u8) = b'/';
            *(_buf as *mut u8).add(1) = 0;
        }
    }
    _buf as u64
}
