//! Linux file I/O syscall implementations.
//!
//! Each function translates a Linux syscall into one or more FsServer IPC
//! calls, using the per-process fd table stored in `TaskStruct`.
//! fds 0/1/2 are pre-reserved as UART-backed Console (tty) entries.

use crate::errno;
use crate::fd_table::FdKind;
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
pub fn sys_write(_task: &mut TaskStruct, fd: usize, buf_ptr: usize, count: usize) -> u64 {
    if fd <= 2 {
        // Console: stdout (1) / stderr (2) → write to UART
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
    } else {
        if let Some(fid) = _task.fd_table.get_fid(fd) {
            let len = count.min(4096);
            let mut tmp = [0u8; 4096];
            unsafe { core::ptr::copy_nonoverlapping(buf_ptr as *const u8, tmp.as_mut_ptr(), len); }
            match ipc::fs_write(fid, &tmp[..len]) {
                Ok(n) => n as u64,
                Err(e) => (-e as u64),
            }
        } else {
            (-errno::EBADF as u64)
        }
    }
}

/// read(fd, buf, count) — syscall 63
pub fn sys_read(_task: &mut TaskStruct, fd: usize, buf_ptr: usize, count: usize) -> u64 {
    match _task.fd_table.get(fd) {
        Some(FdKind::Console) => {
            if count == 0 { return 0; }
            let lflag = termios_lflag(&_task.termios);
            if lflag & 0x0002 != 0 {
                console_read_canonical(_task, buf_ptr, count)
            } else {
                console_read_raw(_task, buf_ptr, count)
            }
        }
        Some(FdKind::File(fid)) => {
            let len = count.min(4096);
            let mut tmp = [0u8; 4096];
            match ipc::fs_read(fid, &mut tmp[..len]) {
                Ok(n) => {
                    unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf_ptr as *mut u8, n); }
                    n as u64
                }
                Err(e) => (-e as u64),
            }
        }
        _ => (-errno::EBADF as u64),
    }
}

/// openat(dirfd, path, flags, mode) — syscall 56 (openat is 56 on aarch64)
pub fn sys_openat(task: &mut TaskStruct, _dirfd: usize, path_ptr: usize, flags: usize, _mode: usize) -> u64 {
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
            if let Some(fd) = task.fd_table.alloc_file(fid, flags as u32) {
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
    match task.fd_table.close_kind(fd) {
        Some(FdKind::Console) => 0,
        Some(FdKind::File(fid)) => {
            ipc::fs_close(fid).ok();
            0
        }
        _ => (-errno::EBADF as u64),
    }
}

// Terminal ioctl constants
const TCGETS: usize     = 0x5401;
const TCSETS: usize     = 0x5402;
const TCSETSW: usize    = 0x5403;
const TCSETSF: usize    = 0x5404;
const TIOCGWINSZ: usize = 0x5413;
const TIOCSWINSZ: usize = 0x5414;
const TIOCGPGRP: usize  = 0x540F;
const TIOCSPGRP: usize  = 0x5410;

// termios flag bits we honor
const ICRNL: u32 = 0x0100; // c_iflag: translate CR → NL on input
const ECHO:  u32 = 0x0008; // c_lflag: echo input characters

/// Kernel struct termios (36 bytes): c_iflag, c_oflag, c_cflag, c_lflag
/// (4×u32 LE), c_line (u8), c_cc[19].
pub const DEFAULT_TERMIOS: [u8; 36] = [
    0x00, 0x05, 0x00, 0x00, // c_iflag: ICRNL|IXON
    0x05, 0x00, 0x00, 0x00, // c_oflag: OPOST|ONLCR
    0xBF, 0x04, 0x00, 0x00, // c_cflag: B38400|CS8|CREAD|HUPCL
    0x3B, 0x8A, 0x00, 0x00, // c_lflag: ISIG|ICANON|ECHO|ECHOE|ECHOK|ECHOCTL|ECHOKE|IEXTEN
    0x00,                   // c_line
    // c_cc: Linux INIT_C_CC (VINTR=^C, VERASE=DEL, VEOF=^D, VMIN=1, ...)
    0x03, 0x1C, 0x7F, 0x15, 0x04, 0x00, 0x01, 0x00,
    0x11, 0x13, 0x1A, 0x00, 0x12, 0x0F, 0x17, 0x16,
    0x00, 0x00, 0x00,
];

fn termios_iflag(t: &[u8; 36]) -> u32 { u32::from_le_bytes(t[0..4].try_into().unwrap()) }
fn termios_lflag(t: &[u8; 36]) -> u32 { u32::from_le_bytes(t[12..16].try_into().unwrap()) }

/// Canonical (line-edited) console read.  Builds a full line in the task's
/// pending buffer — handling erase and echo locally — and only delivers
/// bytes to the caller once Enter is seen.  Partial deliveries (e.g. bash's
/// `read` builtin doing 1-byte reads) resume from the pending buffer.
fn console_read_canonical(task: &mut TaskStruct, buf_ptr: usize, count: usize) -> u64 {
    // Deliver leftover bytes from a previously completed line first.
    if task.line_pos < task.line_len {
        let avail = task.line_len - task.line_pos;
        let n = avail.min(count);
        unsafe {
            core::ptr::copy_nonoverlapping(
                task.line_buf.as_ptr().add(task.line_pos), buf_ptr as *mut u8, n);
        }
        task.line_pos += n;
        return n as u64;
    }

    let iflag = termios_iflag(&task.termios);
    let lflag = termios_lflag(&task.termios);
    let echo_on = lflag & ECHO != 0;

    let mut line = [0u8; 4096];
    let mut len = 0usize;

    loop {
        let mut b = 0u8;
        let n = unsafe { crate::native::console_read(&mut b, 1) };
        if n < 0 { return (-errno::EIO as u64); }
        if n == 0 {
            unsafe { crate::native::yield_cpu(); }
            continue;
        }

        if b == b'\r' && iflag & ICRNL != 0 {
            b = b'\n';
        }

        match b {
            0x7F | 0x08 => {
                // Backspace: erase last char from the line and the screen.
                if len > 0 {
                    len -= 1;
                    if echo_on {
                        unsafe { crate::native::console_write(b"\x08 \x08".as_ptr(), 3); }
                    }
                }
            }
            0x04 => {
                // ^D (VEOF): empty line → EOF; otherwise deliver what we have.
                if len == 0 { return 0; }
                break;
            }
            b'\n' => {
                line[len] = b'\n';
                len += 1;
                if echo_on {
                    unsafe { crate::native::console_write(b"\n".as_ptr(), 1); }
                }
                break;
            }
            _ => {
                if len < line.len() - 1 {
                    line[len] = b;
                    len += 1;
                    if echo_on {
                        unsafe { crate::native::console_write(&b, 1); }
                    }
                }
            }
        }
    }

    task.line_buf[..len].copy_from_slice(&line[..len]);
    task.line_len = len;
    task.line_pos = 0;

    let n = len.min(count);
    unsafe { core::ptr::copy_nonoverlapping(line.as_ptr(), buf_ptr as *mut u8, n); }
    task.line_pos = n;
    n as u64
}

/// Raw-mode console read: deliver bytes as they arrive, no line editing.
/// ICRNL/ECHO are still honored if the application left them enabled.
fn console_read_raw(task: &mut TaskStruct, buf_ptr: usize, count: usize) -> u64 {
    let iflag = termios_iflag(&task.termios);
    let lflag = termios_lflag(&task.termios);
    let len = count.min(4096);
    let mut tmp = [0u8; 4096];
    loop {
        let n = unsafe { crate::native::console_read(tmp.as_mut_ptr(), len) };
        if n < 0 { return (-errno::EIO as u64); }
        if n == 0 {
            unsafe { crate::native::yield_cpu(); }
            continue;
        }
        let n = n as usize;
        if iflag & ICRNL != 0 {
            for b in tmp[..n].iter_mut() {
                if *b == b'\r' { *b = b'\n'; }
            }
        }
        if lflag & ECHO != 0 {
            // Translate backspace to BS-SP-BS for visual erase.
            let mut eb = [0u8; 4096];
            let mut ei = 0;
            for &b in &tmp[..n] {
                if b == 0x7F || b == 0x08 {
                    eb[ei] = 0x08; ei += 1;
                    eb[ei] = b' '; ei += 1;
                    eb[ei] = 0x08; ei += 1;
                } else {
                    eb[ei] = b; ei += 1;
                }
            }
            unsafe { crate::native::console_write(eb.as_ptr(), ei); }
        }
        unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf_ptr as *mut u8, n); }
        return n as u64;
    }
}

fn console_ioctl(task: &mut TaskStruct, request: usize, arg: usize) -> isize {
    match request {
        TCGETS => {
            // glibc's tcgetattr passes a 36-byte stack buffer — writing more
            // smashes the caller's stack canary.
            if arg != 0 {
                unsafe {
                    core::ptr::copy_nonoverlapping(task.termios.as_ptr(), arg as *mut u8, 36);
                }
            }
            0
        }
        TCSETS | TCSETSW | TCSETSF => {
            if arg != 0 {
                unsafe {
                    core::ptr::copy_nonoverlapping(arg as *const u8, task.termios.as_mut_ptr(), 36);
                }
            }
            0
        }
        TIOCGWINSZ => {
            // winsize: ws_row(u16) ws_col(u16) ws_xpixel(u16) ws_ypixel(u16)
            if arg != 0 {
                unsafe {
                    core::ptr::write_bytes(arg as *mut u8, 0, 8);
                    core::ptr::write_volatile(arg as *mut u16, 24);
                    core::ptr::write_volatile((arg + 2) as *mut u16, 80);
                }
            }
            0
        }
        TIOCSWINSZ => 0,
        TIOCGPGRP => {
            if arg != 0 {
                unsafe { core::ptr::write_volatile(arg as *mut i32, task.pid as i32); }
            }
            0
        }
        TIOCSPGRP => 0,
        _ => (-crate::errno::ENOTTY) as isize,
    }
}

/// ioctl(fd, request, arg) — syscall 29
pub fn sys_ioctl(task: &mut TaskStruct, fd: usize, request: usize, arg: usize) -> u64 {
    match task.fd_table.get(fd) {
        Some(FdKind::Console) => {
            let ret = console_ioctl(task, request, arg);
            if ret < 0 { (-ret as u64) } else { ret as u64 }
        }
        _ => (-crate::errno::ENOTTY as u64),
    }
}

/// writev(fd, iov, iovcnt) — syscall 66
pub fn sys_writev(task: &mut TaskStruct, fd: usize, iov: usize, iovcnt: usize) -> u64 {
    let mut total: u64 = 0;
    for i in 0..iovcnt {
        let base = unsafe { *((iov + i * 16) as *const usize) };
        let len  = unsafe { *((iov + i * 16 + 8) as *const usize) };
        if len == 0 { continue; }
        let n = sys_write(task, fd, base, len);
        if (n as i64) < 0 {
            return n;
        }
        total += n;
    }
    total
}

/// fstat(fd, statbuf) — syscall 80
pub fn sys_fstat(task: &mut TaskStruct, fd: usize, statbuf_ptr: usize) -> u64 {
    match task.fd_table.get(fd) {
        Some(FdKind::Console) => {
            // Character device: S_IFCHR | 0620, rdev = (5,1) like /dev/console
            unsafe {
                core::ptr::write_bytes(statbuf_ptr as *mut u8, 0, 128);
                *((statbuf_ptr + 16) as *mut u32) = 0x2000 | 0o620; // st_mode
                *((statbuf_ptr + 20) as *mut u32) = 1;              // st_nlink
                *((statbuf_ptr + 32) as *mut u64) = (5 << 8) | 1;   // st_rdev
                *((statbuf_ptr + 56) as *mut u32) = 1024;           // st_blksize
            }
            0
        }
        Some(FdKind::File(fid)) => {
            match ipc::fs_fstat(fid) {
                Ok(size) => {
                    unsafe {
                        core::ptr::write_bytes(statbuf_ptr as *mut u8, 0, 128);
                        *((statbuf_ptr + 16) as *mut u32) = 0x8000 | 0o644; // st_mode: S_IFREG
                        *((statbuf_ptr + 20) as *mut u32) = 1;              // st_nlink
                        *((statbuf_ptr + 48) as *mut u64) = size;           // st_size
                        *((statbuf_ptr + 56) as *mut u32) = 4096;           // st_blksize
                    }
                    0
                }
                Err(e) => (-e as u64),
            }
        }
        _ => (-errno::EBADF as u64),
    }
}

/// lseek(fd, offset, whence) — syscall 62
pub fn sys_lseek(task: &mut TaskStruct, fd: usize, offset: isize, whence: usize) -> u64 {
    if let Some(fid) = task.fd_table.get_fid(fd) {
        match ipc::fs_lseek(fid, offset, whence as u8) {
            Ok(pos) => pos as u64,
            Err(e) => (-e as u64),
        }
    } else {
        (-errno::EBADF as u64)
    }
}

/// dup(oldfd) — syscall 23
pub fn sys_dup(task: &TaskStruct, oldfd: usize) -> u64 {
    if task.fd_table.get(oldfd).is_some() {
        oldfd as u64
    } else {
        (-errno::EBADF as u64)
    }
}

/// fcntl(fd, cmd, arg) — syscall 25
pub fn sys_fcntl(task: &mut TaskStruct, fd: usize, cmd: usize, arg: usize) -> u64 {
    match cmd {
        0 => {
            if task.fd_table.get(fd).is_some() {
                for newfd in arg..256 {
                    if task.fd_table.get(newfd).is_none() {
                        if let Some(fid) = task.fd_table.get_fid(fd) {
                            task.fd_table.alloc_file(fid, 0);
                        } else {
                            task.fd_table.alloc_kind(FdKind::Console, 0);
                        }
                        return newfd as u64;
                    }
                }
                (-errno::EMFILE as u64)
            } else {
                (-errno::EBADF as u64)
            }
        }
        1 => 0,
        2 => 0,
        3 => {
            if task.fd_table.get(fd).is_some() { 0 } else { (-errno::EBADF as u64) }
        }
        4 => 0,
        _ => (-errno::EINVAL as u64),
    }
}
