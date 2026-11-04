//! Filesystem operations via FsServer IPC.
//!
//! All filesystem I/O goes through the kernel's IPC channel to the FsServer
//! (well-known channel 1).  Each function serialises requests and
//! deserialises responses according to the FsServer protocol.
//!
//! The IPC transport itself (`sys_ipc_call`, nr=5) is a native `svc #1`
//! syscall.

use crate::syscall;

// ---------------------------------------------------------------------------
// Protocol constants (must match kernel's FsServer dispatch)
// ---------------------------------------------------------------------------

const FS_CHANNEL: u32 = 1;
const IPC_MAX: usize = 256;

const OP_OPEN:     u8 = 1;
const OP_READ:     u8 = 2;
const OP_CLOSE:    u8 = 3;
const OP_FSTAT:    u8 = 4;
const OP_LSEEK:    u8 = 5;
const OP_WRITE:    u8 = 6;
const OP_GETDENTS: u8 = 7;
const OP_READLINK: u8 = 8;
const OP_CREATE:   u8 = 9;
const OP_UNLINK:   u8 = 10;
const OP_RENAME:   u8 = 11;

// ---------------------------------------------------------------------------
// IPC helper
// ---------------------------------------------------------------------------

/// Issue a synchronous IPC call (send + receive) and interpret the kernel
/// return value.
fn ipc_call(req: &[u8; IPC_MAX], resp: &mut [u8; IPC_MAX]) -> Result<(), isize> {
    let ret = unsafe {
        syscall::syscall5(syscall::SYS_IPC_CALL,
                          FS_CHANNEL as usize,
                          req.as_ptr() as usize,
                          IPC_MAX,
                          resp.as_mut_ptr() as usize,
                          IPC_MAX) as isize
    };
    if ret < 0 {
        return Err(-ret);
    }
    Ok(())
}

/// Decode FsServer response: bytes 0–7 = error code (i64 LE), bytes 8+
/// depend on the operation.  Returns the embedded error.
fn resp_err(resp: &[u8; IPC_MAX]) -> Result<(), isize> {
    let err = i64::from_le_bytes(resp[0..8].try_into().unwrap());
    if err < 0 { Err((-err) as isize) } else { Ok(()) }
}

// ---------------------------------------------------------------------------
// Public filesystem API
// ---------------------------------------------------------------------------

/// BootInfo VA (must match kernel/liblinux).
const BOOTINFO_VA: usize = 0x22b000;

/// Cached CWD, initialised from BootInfo on first use.
/// Defaults to "/" until BootInfo is read.
static mut CACHED_CWD: [u8; 256] = [0u8; 256];
static mut CWD_LEN: usize = 256; // 256 = not yet read

/// Initialise the CWD from BootInfo (call once at startup if desired).
pub fn init_cwd() {
    let cwd_ptr = (BOOTINFO_VA + 2104) as *const u8;
    let mut len = 0usize;
    unsafe {
        while len < 256 {
            let b = core::ptr::read_volatile(cwd_ptr.add(len));
            if b == 0 { break; }
            CACHED_CWD[len] = b;
            len += 1;
        }
        if len == 0 {
            CACHED_CWD[0] = b'/';
            CWD_LEN = 1;
        } else {
            CWD_LEN = len;
        }
    }
}

/// Get the current working directory.
fn get_cwd(buf: &mut [u8; 256]) -> usize {
    unsafe {
        if CWD_LEN > 255 {
            // First call — initialise from BootInfo
            init_cwd();
        }
        let len = CWD_LEN;
        buf[..len].copy_from_slice(&CACHED_CWD[..len]);
        len
    }
}

/// Resolve a possibly-relative path against the CWD.
fn resolve_path<'a>(orig: &'a str, buf: &'a mut [u8]) -> &'a str {
    if orig.starts_with('/') {
        return orig;
    }
    let mut cwd = [0u8; 256];
    let cwd_len = get_cwd(&mut cwd);
    let mut wp = cwd_len;
    buf[..wp].copy_from_slice(&cwd[..wp]);
    if wp > 0 && buf[wp - 1] != b'/' {
        buf[wp] = b'/'; wp += 1;
    }
    let plen = orig.as_bytes().len().min(buf.len() - wp - 1);
    buf[wp..wp + plen].copy_from_slice(&orig.as_bytes()[..plen]);
    wp += plen;
    buf[wp] = 0;
    core::str::from_utf8(&buf[..wp]).unwrap_or("/")
}

/// Open a file or directory.
///
/// Returns an opaque **file identifier** (`fid`) that must be passed to
/// subsequent `read` / `getdents` / `close` calls.
pub fn open(path: &str) -> Result<u64, isize> {
    let mut path_buf = [0u8; 512];
    let resolved = resolve_path(path, &mut path_buf);

    let mut req = [0u8; IPC_MAX];
    req[0] = OP_OPEN;
    let n = resolved.as_bytes().len().min(IPC_MAX - 2);
    req[2..2 + n].copy_from_slice(&resolved.as_bytes()[..n]);

    let mut resp = [0u8; IPC_MAX];
    ipc_call(&req, &mut resp)?;
    resp_err(&resp)?;

    Ok(u64::from_le_bytes(resp[8..16].try_into().unwrap()))
}

/// Read from a file.
///
/// Returns the number of bytes actually read (may be less than `buf.len()`).
/// A return value of 0 indicates end-of-file.
pub fn read(fid: u64, buf: &mut [u8]) -> Result<usize, isize> {
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_READ;
    req[1..9].copy_from_slice(&fid.to_le_bytes());
    req[9..17].copy_from_slice(&(buf.len() as u64).to_le_bytes());

    let mut resp = [0u8; IPC_MAX];
    ipc_call(&req, &mut resp)?;
    resp_err(&resp)?;

    let n = u64::from_le_bytes(resp[8..16].try_into().unwrap()) as usize;
    let copy = n.min(buf.len());
    buf[..copy].copy_from_slice(&resp[16..16 + copy]);
    Ok(n)
}

/// Write to a file.
///
/// Returns the number of bytes written.
///
/// Protocol (must match FsServer's OP_WRITE): `[op:1][fd:8][len:2][data...]`.
/// Header is 11 bytes, leaving IPC_MAX - 11 = 245 bytes of data per message.
/// Large writes are split into multiple IPC calls.
pub fn write(fid: u64, buf: &[u8]) -> Result<usize, isize> {
    const HDR: usize = 11;
    const MAX_CHUNK: usize = IPC_MAX - HDR;

    let mut total = 0usize;
    while total < buf.len() {
        let chunk = (buf.len() - total).min(MAX_CHUNK);

        let mut req = [0u8; IPC_MAX];
        req[0] = OP_WRITE;
        req[1..9].copy_from_slice(&fid.to_le_bytes());
        req[9..11].copy_from_slice(&(chunk as u16).to_le_bytes());
        req[HDR..HDR + chunk].copy_from_slice(&buf[total..total + chunk]);

        let mut resp = [0u8; IPC_MAX];
        ipc_call(&req, &mut resp)?;
        resp_err(&resp)?;

        let written = u64::from_le_bytes(resp[8..16].try_into().unwrap()) as usize;
        if written == 0 {
            break; // avoid infinite loop
        }
        total += written;
    }
    Ok(total)
}

/// Close a file descriptor returned by [`open`].
pub fn close(fid: u64) -> Result<(), isize> {
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_CLOSE;
    req[1..9].copy_from_slice(&fid.to_le_bytes());

    let mut resp = [0u8; IPC_MAX];
    ipc_call(&req, &mut resp)?;
    resp_err(&resp)
}

/// Read directory entries (getdents64 format).
///
/// Fills `buf` with zero or more `linux_dirent64` structures.  Use
/// [`crate::dirent::parse_dirents`] to iterate over them.
pub fn getdents(fid: u64, buf: &mut [u8]) -> Result<usize, isize> {
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_GETDENTS;
    req[1..9].copy_from_slice(&fid.to_le_bytes());
    req[9..17].copy_from_slice(&(buf.len() as u64).to_le_bytes());

    let mut resp = [0u8; IPC_MAX];
    ipc_call(&req, &mut resp)?;
    resp_err(&resp)?;

    let n = u64::from_le_bytes(resp[8..16].try_into().unwrap()) as usize;
    let copy = n.min(buf.len());
    buf[..copy].copy_from_slice(&resp[16..16 + copy]);
    Ok(n)
}

/// Get file status (size in bytes).
pub fn fstat(fid: u64) -> Result<u64, isize> {
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_FSTAT;
    req[1..9].copy_from_slice(&fid.to_le_bytes());

    let mut resp = [0u8; IPC_MAX];
    ipc_call(&req, &mut resp)?;
    resp_err(&resp)?;

    Ok(u64::from_le_bytes(resp[8..16].try_into().unwrap()))
}

/// Seek within a file.
///
/// `whence`: 0 = SEEK_SET, 1 = SEEK_CUR, 2 = SEEK_END.
pub fn lseek(fid: u64, offset: isize, whence: u8) -> Result<usize, isize> {
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_LSEEK;
    req[1..9].copy_from_slice(&fid.to_le_bytes());
    req[9..17].copy_from_slice(&(offset as i64).to_le_bytes());
    req[17] = whence;

    let mut resp = [0u8; IPC_MAX];
    ipc_call(&req, &mut resp)?;
    resp_err(&resp)?;

    Ok(u64::from_le_bytes(resp[8..16].try_into().unwrap()) as usize)
}

/// Read the target of a symbolic link.
///
/// Returns the number of bytes in the link target.  `buf` receives as many
/// bytes as fit; if the target is longer, the returned value will be larger
/// than `buf.len()`.
pub fn readlink(path: &str, buf: &mut [u8]) -> Result<usize, isize> {
    let mut path_buf = [0u8; 512];
    let resolved = resolve_path(path, &mut path_buf);

    let mut req = [0u8; IPC_MAX];
    req[0] = OP_READLINK;
    let n = resolved.as_bytes().len().min(IPC_MAX - 1);
    req[1..1 + n].copy_from_slice(&resolved.as_bytes()[..n]);

    let mut resp = [0u8; IPC_MAX];
    ipc_call(&req, &mut resp)?;
    resp_err(&resp)?;

    let len = u64::from_le_bytes(resp[8..16].try_into().unwrap()) as usize;
    let copy = len.min(buf.len());
    buf[..copy].copy_from_slice(&resp[16..16 + copy]);
    Ok(len)
}

/// Create a new regular file.
pub fn create(path: &str) -> Result<(), isize> {
    let mut path_buf = [0u8; 512];
    let resolved = resolve_path(path, &mut path_buf);

    let mut req = [0u8; IPC_MAX];
    req[0] = OP_CREATE;
    // mode = 0o644 in little-endian u16
    req[1] = 0xA4; req[2] = 0x01;
    let n = resolved.as_bytes().len().min(IPC_MAX - 3);
    req[3..3 + n].copy_from_slice(&resolved.as_bytes()[..n]);

    let mut resp = [0u8; IPC_MAX];
    ipc_call(&req, &mut resp)?;
    resp_err(&resp)
}

/// Remove a file (unlink).
pub fn unlink(path: &str) -> Result<(), isize> {
    let mut path_buf = [0u8; 512];
    let resolved = resolve_path(path, &mut path_buf);

    let mut req = [0u8; IPC_MAX];
    req[0] = OP_UNLINK;
    let n = resolved.as_bytes().len().min(IPC_MAX - 1);
    req[1..1 + n].copy_from_slice(&resolved.as_bytes()[..n]);

    let mut resp = [0u8; IPC_MAX];
    ipc_call(&req, &mut resp)?;
    resp_err(&resp)
}

/// Rename (move) a file or directory.
pub fn rename(old_path: &str, new_path: &str) -> Result<(), isize> {
    let mut old_buf = [0u8; 512];
    let mut new_buf = [0u8; 512];
    let resolved_old = resolve_path(old_path, &mut old_buf);
    let resolved_new = resolve_path(new_path, &mut new_buf);

    let mut req = [0u8; IPC_MAX];
    req[0] = OP_RENAME;
    let old_len = resolved_old.as_bytes().len().min(255);
    req[1] = old_len as u8;
    req[2..2 + old_len].copy_from_slice(&resolved_old.as_bytes()[..old_len]);
    let new_len = resolved_new.as_bytes().len().min(IPC_MAX - 2 - old_len);
    let new_start = 2 + old_len;
    req[new_start..new_start + new_len].copy_from_slice(&resolved_new.as_bytes()[..new_len]);

    let mut resp = [0u8; IPC_MAX];
    ipc_call(&req, &mut resp)?;
    resp_err(&resp)
}
