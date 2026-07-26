#![no_std]
#![no_main]

use core::arch::asm;
use core::ffi::CStr;

// ---------------------------------------------------------------------------
// Native microkernel syscall numbers (SVC #1)
// ---------------------------------------------------------------------------
const SYS_IPC_CALL:       u64 = 5;
const SYS_CONSOLE_WRITE:  u64 = 11;
const SYS_EXIT_THREAD:    u64 = 7;

// ---------------------------------------------------------------------------
// FsServer constants
// ---------------------------------------------------------------------------
const FS_CHANNEL: u32 = 1;
const IPC_MAX: usize = 256;

const OP_OPEN:     u8 = 1;
const OP_GETDENTS: u8 = 7;
const OP_CLOSE:    u8 = 3;

// ---------------------------------------------------------------------------
// Native syscall wrappers
// ---------------------------------------------------------------------------

unsafe fn ipc_call(ch: u32, send_ptr: usize, send_len: usize,
                   recv_buf: usize, recv_len: usize) -> isize {
    let ret: isize;
    unsafe {
        asm!(
            "svc #1",
            in("x8") SYS_IPC_CALL,
            in("x0") ch as usize,
            in("x1") send_ptr,
            in("x2") send_len,
            in("x3") recv_buf,
            in("x4") recv_len,
            lateout("x0") ret,
        );
    }
    ret
}

unsafe fn console_write(buf: *const u8, len: usize) -> isize {
    let ret: isize;
    unsafe {
        asm!(
            "svc #1",
            in("x8") SYS_CONSOLE_WRITE,
            in("x0") buf,
            in("x1") len,
            lateout("x0") ret,
        );
    }
    ret
}

unsafe fn exit_thread(code: i32) -> ! {
    unsafe {
        asm!(
            "svc #1",
            in("x8") SYS_EXIT_THREAD,
            in("x0") code as u64,
            options(noreturn),
        );
    }
}

// ---------------------------------------------------------------------------
// FsServer IPC helpers (inlined protocol — no liblinux dependency)
// ---------------------------------------------------------------------------

/// Open a file/directory via FsServer IPC.  Returns fid on success.
unsafe fn fs_open(path: &str) -> Result<u64, isize> {
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_OPEN;
    let n = path.as_bytes().len().min(IPC_MAX - 2);
    req[2..2 + n].copy_from_slice(&path.as_bytes()[..n]);

    let mut resp = [0u8; IPC_MAX];
    let ret = unsafe { ipc_call(FS_CHANNEL, req.as_ptr() as usize, IPC_MAX,
                                 resp.as_mut_ptr() as usize, IPC_MAX) };
    if ret < 0 { return Err(-ret); }
    if resp[0..8] == [0, 0, 0, 0, 0, 0, 0, 0] {
        Ok(u64::from_le_bytes(resp[8..16].try_into().unwrap()))
    } else {
        Err(-(i64::from_le_bytes(resp[0..8].try_into().unwrap()) as isize))
    }
}

/// Read directory entries via FsServer IPC.
unsafe fn fs_getdents(fid: u64, buf: &mut [u8]) -> Result<usize, isize> {
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_GETDENTS;
    req[1..9].copy_from_slice(&fid.to_le_bytes());
    req[9..17].copy_from_slice(&(buf.len() as u64).to_le_bytes());

    let mut resp = [0u8; IPC_MAX];
    let ret = unsafe { ipc_call(FS_CHANNEL, req.as_ptr() as usize, IPC_MAX,
                                 resp.as_mut_ptr() as usize, IPC_MAX) };
    if ret < 0 { return Err(-ret); }
    let err = i64::from_le_bytes(resp[0..8].try_into().unwrap());
    if err < 0 { return Err(-err as isize); }
    let n = u64::from_le_bytes(resp[8..16].try_into().unwrap()) as usize;
    let copy = n.min(buf.len());
    buf[..copy].copy_from_slice(&resp[16..16 + copy]);
    Ok(n)
}

/// Close a file/directory via FsServer IPC.
unsafe fn fs_close(fid: u64) -> Result<(), isize> {
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_CLOSE;
    req[1..9].copy_from_slice(&fid.to_le_bytes());

    let mut resp = [0u8; IPC_MAX];
    let ret = unsafe { ipc_call(FS_CHANNEL, req.as_ptr() as usize, IPC_MAX,
                                 resp.as_mut_ptr() as usize, IPC_MAX) };
    if ret < 0 { return Err(-ret); }
    let err = i64::from_le_bytes(resp[0..8].try_into().unwrap());
    if err < 0 { return Err(-err as isize); }
    Ok(())
}

// ---------------------------------------------------------------------------
// linux_dirent64 decoder
// ---------------------------------------------------------------------------
// struct linux_dirent64 {
//     ino64_t        d_ino;       // 8 bytes
//     off64_t        d_off;       // 8 bytes
//     unsigned short d_reclen;    // 2 bytes
//     unsigned char  d_type;      // 1 byte
//     char           d_name[];    // null-terminated
// };

fn parse_dirent(buf: &[u8], offset: usize) -> Option<(&[u8], usize)> {
    if offset + 19 > buf.len() { return None; }
    let reclen = u16::from_le_bytes([buf[offset + 16], buf[offset + 17]]) as usize;
    if reclen == 0 || offset + reclen > buf.len() { return None; }
    // d_name starts at offset+19
    let name_start = offset + 19;
    let name_max = (offset + reclen).min(buf.len()) - name_start;
    let mut name_len = 0;
    while name_len < name_max && buf[name_start + name_len] != 0 {
        name_len += 1;
    }
    Some((&buf[name_start..name_start + name_len], offset + reclen))
}

// ---------------------------------------------------------------------------
// strlen stub
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
unsafe extern "C" fn strlen(s: *const u8) -> usize {
    let mut len = 0;
    unsafe {
        while *s.add(len) != 0 {
            len += 1;
        }
    }
    len
}

#[unsafe(no_mangle)]
unsafe extern "C" fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    unsafe {
        let mut i = 0;
        while i < n {
            *dst.add(i) = *src.add(i);
            i += 1;
        }
    }
    dst
}

#[unsafe(no_mangle)]
unsafe extern "C" fn memset(s: *mut u8, c: i32, n: usize) -> *mut u8 {
    unsafe {
        let mut i = 0;
        while i < n {
            *s.add(i) = c as u8;
            i += 1;
        }
    }
    s
}

// ---------------------------------------------------------------------------
// panic handler
// ---------------------------------------------------------------------------

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe { exit_thread(1); }
}

// ---------------------------------------------------------------------------
// _start entry point
// ---------------------------------------------------------------------------

core::arch::global_asm!(
    ".globl _start",
    "_start:",
    "mov x29, #0",
    "mov x30, #0",
    "mov x2, sp",
    "and sp, x2, #-16",
    "ldr x0, [x2]",          // argc from orig sp
    "add x1, x2, #8",        // argv from orig sp
    "b   {ls_main}",
    ls_main = sym ls_main,
);

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

/// Create a CStr from a byte string literal (no_std compatible).
fn make_dir_path(argc: usize, argv: *const *const u8) -> &'static CStr {
    if argc > 1 {
        let ptr = unsafe { *argv.add(1) };
        if !ptr.is_null() {
            // We need to return a reference to the user-provided C string.
            // This is safe because argv strings live for the program's lifetime.
            return unsafe { CStr::from_ptr(ptr) };
        }
    }
    // Default: "."
    unsafe { CStr::from_bytes_with_nul_unchecked(b".\0") }
}

#[unsafe(no_mangle)]
unsafe fn ls_main(argc: usize, argv: *const *const u8) -> ! {
    let dir = make_dir_path(argc, argv);
    let path_str = match dir.to_str() {
        Ok(s) => s,
        Err(_) => {
            unsafe { console_write(b"ls: invalid path\n".as_ptr(), 18) };
            unsafe { exit_thread(1) };
        }
    };

    // Open directory
    let fid = match unsafe { fs_open(path_str) } {
        Ok(f) => f,
        Err(_e) => {
            unsafe { console_write(b"ls: cannot open '".as_ptr(), 17) };
            unsafe { console_write(path_str.as_ptr(), path_str.len()) };
            unsafe { console_write(b"'\n".as_ptr(), 2) };
            unsafe { exit_thread(1) };
        }
    };

    // Read directory entries
    let mut buf = [0u8; IPC_MAX];
    match unsafe { fs_getdents(fid, &mut buf) } {
        Ok(_n) => {}
        Err(_e) => {
            unsafe { fs_close(fid).ok(); }
            unsafe { console_write(b"ls: read error\n".as_ptr(), 15) };
            unsafe { exit_thread(1) };
        }
    }

    // Parse and print entries (skip . and ..)
    let mut offset = 0;
    while offset < buf.len() {
        match parse_dirent(&buf, offset) {
            Some((name, next)) => {
                if name != b"." && name != b".." {
                    unsafe { console_write(name.as_ptr(), name.len()) };
                    unsafe { console_write(b"  ".as_ptr(), 2) };
                }
                offset = next;
            }
            None => break,
        }
    }
    unsafe { console_write(b"\n".as_ptr(), 1) };

    // Cleanup
    unsafe { fs_close(fid).ok(); }
    unsafe { exit_thread(0) };
}
