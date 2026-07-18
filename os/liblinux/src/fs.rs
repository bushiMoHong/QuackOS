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
    match _task.fd_table.get(fd) {
        Some(FdKind::Console) => {
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
        Some(FdKind::File(fid)) => {
            let len = count.min(4096);
            let mut tmp = [0u8; 4096];
            unsafe { core::ptr::copy_nonoverlapping(buf_ptr as *const u8, tmp.as_mut_ptr(), len); }
            match ipc::fs_write(fid, &tmp[..len]) {
                Ok(n) => n as u64,
                Err(e) => (-e as u64),
            }
        }
        Some(FdKind::PipeWrite(idx)) => {
            pipe_write(_task, idx, buf_ptr, count)
        }
        _ => (-errno::EBADF as u64),
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
        Some(FdKind::PipeRead(idx)) => {
            pipe_read(_task, idx, buf_ptr, count)
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
        Some(FdKind::PipeRead(idx)) => {
            if let Some(ref mut pipe) = task.pipes[idx] {
                pipe.readers -= 1;
                if pipe.readers == 0 && pipe.writers == 0 {
                    task.pipes[idx] = None;
                }
            }
            0
        }
        Some(FdKind::PipeWrite(idx)) => {
            if let Some(ref mut pipe) = task.pipes[idx] {
                pipe.writers -= 1;
                if pipe.readers == 0 && pipe.writers == 0 {
                    task.pipes[idx] = None;
                }
            }
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
            ret as u64 // negative values are already -errno
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
pub fn sys_fstat(task: &TaskStruct, fd: usize, statbuf_ptr: usize) -> u64 {
    sys_fstat_fd(task, fd, statbuf_ptr)
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
pub fn sys_dup(task: &mut TaskStruct, oldfd: usize) -> u64 {
    // Allocate the lowest free fd pointing to the same object as oldfd.
    let entry = match task.fd_table.get(oldfd) {
        Some(kind) => kind,
        _ => return (-errno::EBADF as u64),
    };
    match task.fd_table.alloc_kind(entry, 0) {
        Some(newfd) => newfd as u64,
        None => (-errno::EMFILE as u64),
    }
}

/// dup3(oldfd, newfd, flags) — syscall 24
pub fn sys_dup3(task: &mut TaskStruct, oldfd: usize, newfd: usize, _flags: usize) -> u64 {
    if oldfd == newfd { return (-errno::EINVAL as u64); }
    let entry = match task.fd_table.get(oldfd) {
        Some(kind) => kind,
        _ => return (-errno::EBADF as u64),
    };
    // Close newfd first if open, then assign
    task.fd_table.close(newfd);
    task.fd_table.alloc_kind(entry, 0);
    newfd as u64
}

/// chdir(path) — syscall 49
pub fn sys_chdir(task: &mut TaskStruct, path_ptr: usize) -> u64 {
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
    task.cwd = [0u8; 256];
    task.cwd[..len.min(255)].copy_from_slice(&path[..len.min(255)]);
    0
}

/// getdents64(fd, buf, count) — syscall 61
pub fn sys_getdents64(task: &TaskStruct, fd: usize, buf_ptr: usize, count: usize) -> u64 {
    if let Some(fid) = task.fd_table.get_fid(fd) {
        let len = count.min(4096);
        let mut tmp = [0u8; 4096];
        match ipc::fs_getdents(fid, &mut tmp[..len]) {
            Ok(n) => {
                unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf_ptr as *mut u8, n); }
                n as u64
            }
            Err(e) => (-e as u64),
        }
    } else {
        (-errno::EBADF as u64)
    }
}

/// readlinkat(dfd, path, buf, bufsize) — syscall 78
pub fn sys_readlinkat(_task: &TaskStruct, _dfd: usize, path_ptr: usize, buf_ptr: usize, bufsize: usize) -> u64 {
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
    let path_str = core::str::from_utf8(&path[..len]).unwrap_or("");
    let mut tmp = [0u8; 256];
    match ipc::fs_readlink(path_str, &mut tmp) {
        Ok(n) => {
            let copy = n.min(bufsize);
            if copy > 0 && buf_ptr != 0 {
                unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf_ptr as *mut u8, copy); }
            }
            copy as u64
        }
        Err(e) => (-e as u64),
    }
}

/// newfstatat(dfd, path, statbuf, flags) — syscall 79
/// Implemented as open + fstat + close directly via IPC.
pub fn sys_newfstatat(task: &TaskStruct, dfd: usize, path_ptr: usize, statbuf_ptr: usize, flags: usize) -> u64 {
    const AT_EMPTY_PATH: usize = 0x1000;

    if flags & AT_EMPTY_PATH != 0 {
        return sys_fstat_fd(task, dfd, statbuf_ptr);
    }

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
            let ret = match ipc::fs_fstat(fid) {
                Ok(size) => {
                    unsafe {
                        core::ptr::write_bytes(statbuf_ptr as *mut u8, 0, 128);
                        *((statbuf_ptr + 16) as *mut u32) = 0x8000 | 0o644;
                        *((statbuf_ptr + 20) as *mut u32) = 1;
                        *((statbuf_ptr + 48) as *mut u64) = size;
                        *((statbuf_ptr + 56) as *mut u32) = 4096;
                    }
                    0
                }
                Err(e) => (-e as u64),
            };
            ipc::fs_close(fid).ok();
            ret
        }
        Err(e) => (-e as u64),
    }
}

/// Write stat to user buffer (shared by fstat and newfstatat Console path).
fn sys_fstat_fd(task: &TaskStruct, fd: usize, statbuf_ptr: usize) -> u64 {
    match task.fd_table.get(fd) {
        Some(FdKind::Console) => {
            unsafe {
                core::ptr::write_bytes(statbuf_ptr as *mut u8, 0, 128);
                *((statbuf_ptr + 16) as *mut u32) = 0x2000 | 0o620;
                *((statbuf_ptr + 20) as *mut u32) = 1;
                *((statbuf_ptr + 32) as *mut u64) = (5 << 8) | 1;
                *((statbuf_ptr + 56) as *mut u32) = 1024;
            }
            0
        }
        Some(FdKind::File(fid)) => {
            match ipc::fs_fstat(fid) {
                Ok(size) => {
                    unsafe {
                        core::ptr::write_bytes(statbuf_ptr as *mut u8, 0, 128);
                        *((statbuf_ptr + 16) as *mut u32) = 0x8000 | 0o644;
                        *((statbuf_ptr + 20) as *mut u32) = 1;
                        *((statbuf_ptr + 48) as *mut u64) = size;
                        *((statbuf_ptr + 56) as *mut u32) = 4096;
                    }
                    0
                }
                Err(e) => (-e as u64),
            }
        }
        _ => (-errno::EBADF as u64),
    }
}

// ---------------------------------------------------------------------------
// Pipe helpers
// ---------------------------------------------------------------------------

fn pipe_read(task: &mut TaskStruct, idx: usize, buf_ptr: usize, count: usize) -> u64 {
    let pipe = match &mut task.pipes[idx] {
        Some(p) => p,
        None => return (-errno::EBADF as u64),
    };

    if pipe.byte_count == 0 {
        // No data available — return 0 (EOF if writer closed, or would-block)
        if pipe.writers == 0 {
            return 0; // EOF
        }
        return 0; // non-blocking empty
    }

    let n = count.min(pipe.byte_count);
    let pipe_buf_size = 4096;

    for i in 0..n {
        let b = pipe.buf[pipe.read_pos];
        pipe.read_pos = (pipe.read_pos + 1) % pipe_buf_size;
        pipe.byte_count -= 1;
        unsafe {
            *((buf_ptr as *mut u8).add(i)) = b;
        }
    }

    n as u64
}

fn pipe_write(task: &mut TaskStruct, idx: usize, buf_ptr: usize, count: usize) -> u64 {
    let pipe = match &mut task.pipes[idx] {
        Some(p) => p,
        None => return (-errno::EBADF as u64),
    };

    if pipe.readers == 0 {
        // SIGPIPE would normally be delivered; return -EPIPE
        return (-crate::errno::EPIPE as u64);
    }

    let pipe_buf_size = 4096;
    let free = pipe_buf_size - pipe.byte_count;
    let n = count.min(free);

    for i in 0..n {
        let b = unsafe { *((buf_ptr as *const u8).add(i)) };
        pipe.buf[pipe.write_pos] = b;
        pipe.write_pos = (pipe.write_pos + 1) % pipe_buf_size;
        pipe.byte_count += 1;
    }

    n as u64
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

/// pipe2(fds_ptr, flags) — syscall 59
///
/// Creates an anonymous pipe.  Writes fds[0] (read end) and fds[1] (write end)
/// into the user-space array at `fds_ptr`.
pub fn sys_pipe2(task: &mut TaskStruct, fds_ptr: usize, _flags: usize) -> u64 {
    // Find a free pipe slot
    let pipe_idx = match task.pipes.iter().position(|p| p.is_none()) {
        Some(i) => i,
        None => return (-crate::errno::EMFILE as u64),
    };

    // Create pipe
    task.pipes[pipe_idx] = Some(crate::task::Pipe {
        buf: [0u8; 4096],
        read_pos: 0,
        write_pos: 0,
        byte_count: 0,
        readers: 1,
        writers: 1,
    });

    // Allocate read fd
    let read_fd = match task.fd_table.alloc_kind(FdKind::PipeRead(pipe_idx), 0) {
        Some(fd) => fd,
        None => {
            task.pipes[pipe_idx] = None;
            return (-crate::errno::EMFILE as u64);
        }
    };

    // Allocate write fd
    let write_fd = match task.fd_table.alloc_kind(FdKind::PipeWrite(pipe_idx), 0) {
        Some(fd) => fd,
        None => {
            task.fd_table.close(read_fd);
            task.pipes[pipe_idx] = None;
            return (-crate::errno::EMFILE as u64);
        }
    };

    // Write fds to user-space
    unsafe {
        core::ptr::write_volatile(fds_ptr as *mut i32, read_fd as i32);
        core::ptr::write_volatile((fds_ptr as *mut i32).add(1), write_fd as i32);
    }

    0
}
