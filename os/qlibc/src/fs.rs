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

/// Open a file or directory.
///
/// Returns an opaque **file identifier** (`fid`) that must be passed to
/// subsequent `read` / `getdents` / `close` calls.
pub fn open(path: &str) -> Result<u64, isize> {
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_OPEN;
    let n = path.as_bytes().len().min(IPC_MAX - 2);
    req[2..2 + n].copy_from_slice(&path.as_bytes()[..n]);

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
pub fn write(fid: u64, buf: &[u8]) -> Result<usize, isize> {
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_WRITE;
    req[1..9].copy_from_slice(&fid.to_le_bytes());
    let data_len = buf.len().min(IPC_MAX - 17);
    req[9..17].copy_from_slice(&(data_len as u64).to_le_bytes());
    req[17..17 + data_len].copy_from_slice(&buf[..data_len]);

    let mut resp = [0u8; IPC_MAX];
    ipc_call(&req, &mut resp)?;
    resp_err(&resp)?;

    Ok(u64::from_le_bytes(resp[8..16].try_into().unwrap()) as usize)
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
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_READLINK;
    let n = path.as_bytes().len().min(IPC_MAX - 1);
    req[1..1 + n].copy_from_slice(&path.as_bytes()[..n]);

    let mut resp = [0u8; IPC_MAX];
    ipc_call(&req, &mut resp)?;
    resp_err(&resp)?;

    let len = u64::from_le_bytes(resp[8..16].try_into().unwrap()) as usize;
    let copy = len.min(buf.len());
    buf[..copy].copy_from_slice(&resp[16..16 + copy]);
    Ok(len)
}
