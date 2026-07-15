//! Linux file I/O syscall implementations.
//!
//! Each function translates a Linux syscall into one or more FsServer IPC
//! calls, using the per-process fd table stored in `TaskStruct`.

use crate::errno;
use crate::ipc;
use crate::task::TaskStruct;

// Linux open flags (partial)
pub const O_RDONLY: u32 = 0x0000;
pub const O_WRONLY: u32 = 0x0001;
pub const O_RDWR:   u32 = 0x0002;
pub const O_CREAT:  u32 = 0x0040;
pub const O_TRUNC:  u32 = 0x0200;
pub const O_APPEND: u32 = 0x0400;

/// write(fd, buf, count) — syscall 64
pub fn sys_write(task: &mut TaskStruct, fd: usize, buf_ptr: usize, count: usize) -> u64 {
    if fd > 2 {
        // Not stdout/stderr — redirect to FsServer via fd table
        if let Some(fid) = task.fd_table.get(fd) {
            // Read from user buffer (in the Linux binary's address space)
            let len = count.min(4096);
            let mut tmp = [0u8; 4096];
            unsafe {
                core::ptr::copy_nonoverlapping(buf_ptr as *const u8, tmp.as_mut_ptr(), len);
            }
            match ipc::fs_write(fid, &tmp[..len]) {
                Ok(n) => n as u64,
                Err(e) => (-e as u64),
            }
        } else {
            (-errno::EBADF as u64)
        }
    } else {
        // stdout (1) / stderr (2) → write to UART via native syscall
        // fd 0 (stdin) writes → not supported yet
        if fd == 0 {
            return (-errno::EBADF as u64);
        }
        let len = count.min(4096);
        let ret = unsafe { crate::native::console_write(buf_ptr as *const u8, len) };
        if ret < 0 {
            (-crate::errno::EIO as u64)
        } else {
            ret as u64
        }
    }
}

/// read(fd, buf, count) — syscall 63
pub fn sys_read(task: &mut TaskStruct, fd: usize, buf_ptr: usize, count: usize) -> u64 {
    if let Some(fid) = task.fd_table.get(fd) {
        let len = count.min(4096);
        let mut tmp = [0u8; 4096];
        match ipc::fs_read(fid, &mut tmp[..len]) {
            Ok(n) => {
                unsafe {
                    core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf_ptr as *mut u8, n);
                }
                n as u64
            }
            Err(e) => (-e as u64),
        }
    } else {
        (-errno::EBADF as u64)
    }
}

/// open(path, flags, mode) — syscall 56 (openat is 56 on aarch64)
pub fn sys_openat(task: &mut TaskStruct, _dirfd: usize, path_ptr: usize, flags: usize, _mode: usize) -> u64 {
    // Read path string from user memory
    let mut path = [0u8; 256];
    let len = unsafe {
        let mut l = 0;
        while l < 256 {
            let b = *((path_ptr as *const u8).add(l));
            if b == 0 { break; }
            path[l] = b;
            l += 1;
        }
        l
    };
    let path_str = core::str::from_utf8(&path[..len]).unwrap_or("/");

    match ipc::fs_open(path_str) {
        Ok(fid) => {
            if let Some(fd) = task.fd_table.alloc(fid, flags as u32) {
                fd as u64
            } else {
                ipc::fs_close(fid).ok();
                (-errno::EMFILE as u64)
            }
        }
        Err(e) => (-e as u64),
    }
}

/// close(fd) — syscall 57
pub fn sys_close(task: &mut TaskStruct, fd: usize) -> u64 {
    if let Some(fid) = task.fd_table.get(fd) {
        if ipc::fs_close(fid).is_ok() {
            task.fd_table.close(fd);
            0
        } else {
            (-errno::EIO as u64)
        }
    } else {
        (-errno::EBADF as u64)
    }
}

/// ioctl(fd, request, arg) — syscall 29
pub fn sys_ioctl(fd: usize, _request: usize, _arg: usize) -> u64 {
    // All standard fds are non-terminal; return ENOTTY so musl treats them
    // as regular files instead of trying terminal ioctls.
    if fd <= 2 {
        (-crate::errno::ENOTTY as u64)
    } else {
        (-crate::errno::ENOTTY as u64)
    }
}

/// writev(fd, iov, iovcnt) — syscall 66
pub fn sys_writev(task: &mut TaskStruct, fd: usize, iov: usize, iovcnt: usize) -> u64 {
    // struct iovec { void *iov_base; size_t iov_len; } — 16 bytes each
    let mut total: u64 = 0;
    for i in 0..iovcnt {
        let base = unsafe { *((iov + i * 16) as *const usize) };
        let len  = unsafe { *((iov + i * 16 + 8) as *const usize) };
        if len == 0 { continue; }
        let n = sys_write(task, fd, base, len);
        // sys_write returns a u64; negative errno values are encoded as large
        // u64 (e.g. -25u64 = 0xffffffe7).  Check the sign bit.
        if (n as i64) < 0 {
            return n;
        }
        total += n;
    }
    total
}

/// fstat(fd, statbuf) — syscall 80
pub fn sys_fstat(task: &mut TaskStruct, fd: usize, statbuf_ptr: usize) -> u64 {
    if let Some(fid) = task.fd_table.get(fd) {
        match ipc::fs_fstat(fid) {
            Ok(size) => {
                // Write a minimal stat struct to user space:
                // struct stat { ... st_size at offset 48 (on aarch64) ... }
                unsafe {
                    // Zero the stat buffer
                    core::ptr::write_bytes(statbuf_ptr as *mut u8, 0, 128);
                    // st_size at offset 48
                    *((statbuf_ptr + 48) as *mut u64) = size;
                }
                0
            }
            Err(e) => (-e as u64),
        }
    } else {
        (-errno::EBADF as u64)
    }
}
