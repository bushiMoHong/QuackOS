//! IPC wrapper layer — communicates with FsServer, ProcServer, etc.
//! over microkernel IPC channels via SVC #1 native syscalls.

use crate::native;

/// Well-known channel for the filesystem server (Phase 1).
const FS_CHANNEL: u32 = 1;

/// Maximum IPC message payload size (ShortPayload = 256 bytes).
const IPC_MAX: usize = 256;

// ---------------------------------------------------------------------------
// FS protocol constants (must match kernel's FsServer dispatch)
// ---------------------------------------------------------------------------
const OP_OPEN:     u8 = 1;
const OP_READ:     u8 = 2;
const OP_CLOSE:    u8 = 3;
const OP_FSTAT:    u8 = 4;
const OP_LSEEK:    u8 = 5;
const OP_WRITE:    u8 = 6;
const OP_GETDENTS: u8 = 7;
const OP_READLINK: u8 = 8;

// ---------------------------------------------------------------------------
// FS operations
// ---------------------------------------------------------------------------

/// Open a file. Returns FsServer file identifier (fid) on success.
/// Errors are returned as positive errno values.
pub fn fs_open(path: &str) -> Result<u64, isize> {
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_OPEN;
    req[1] = 0; // O_RDONLY
    let n = path.as_bytes().len().min(IPC_MAX - 2);
    req[2..2 + n].copy_from_slice(&path.as_bytes()[..n]);

    let mut resp = [0u8; IPC_MAX];
    let ret = unsafe { native::ipc_call(FS_CHANNEL, req.as_ptr() as usize, IPC_MAX,
                                         resp.as_mut_ptr() as usize, IPC_MAX) };
    if ret < 0 { return Err(-ret); }
    if resp[0..8] == [0, 0, 0, 0, 0, 0, 0, 0] {
        // Success — fid is at bytes 8-15
        Ok(u64::from_le_bytes(resp[8..16].try_into().unwrap()))
    } else {
        // FsServer sends a negative errno; normalize to positive.
        Err(-(i64::from_le_bytes(resp[0..8].try_into().unwrap()) as isize))
    }
}

/// Read from a file.
pub fn fs_read(fid: u64, buf: &mut [u8]) -> Result<usize, isize> {
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_READ;
    req[1..9].copy_from_slice(&fid.to_le_bytes());
    req[9..17].copy_from_slice(&(buf.len() as u64).to_le_bytes());

    let mut resp = [0u8; IPC_MAX];
    let ret = unsafe { native::ipc_call(FS_CHANNEL, req.as_ptr() as usize, IPC_MAX,
                                         resp.as_mut_ptr() as usize, IPC_MAX) };
    if ret < 0 { return Err(-ret); }
    let err = i64::from_le_bytes(resp[0..8].try_into().unwrap());
    if err < 0 { return Err(-err as isize); }

    let n = u64::from_le_bytes(resp[8..16].try_into().unwrap()) as usize;
    let copy = n.min(buf.len());
    buf[..copy].copy_from_slice(&resp[16..16 + copy]);
    Ok(n)
}

/// Write to a file.
pub fn fs_write(fid: u64, buf: &[u8]) -> Result<usize, isize> {
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_WRITE;
    req[1..9].copy_from_slice(&fid.to_le_bytes());
    let data_len = buf.len().min(IPC_MAX - 17);
    req[9..17].copy_from_slice(&(data_len as u64).to_le_bytes());
    req[17..17 + data_len].copy_from_slice(&buf[..data_len]);

    let mut resp = [0u8; IPC_MAX];
    let ret = unsafe { native::ipc_call(FS_CHANNEL, req.as_ptr() as usize, IPC_MAX,
                                         resp.as_mut_ptr() as usize, IPC_MAX) };
    if ret < 0 { return Err(-ret); }
    let err = i64::from_le_bytes(resp[0..8].try_into().unwrap());
    if err < 0 { return Err(-err as isize); }
    Ok(u64::from_le_bytes(resp[8..16].try_into().unwrap()) as usize)
}

/// Close a file.
pub fn fs_close(fid: u64) -> Result<(), isize> {
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_CLOSE;
    req[1..9].copy_from_slice(&fid.to_le_bytes());

    let mut resp = [0u8; IPC_MAX];
    let ret = unsafe { native::ipc_call(FS_CHANNEL, req.as_ptr() as usize, IPC_MAX,
                                         resp.as_mut_ptr() as usize, IPC_MAX) };
    if ret < 0 { return Err(-ret); }
    let err = i64::from_le_bytes(resp[0..8].try_into().unwrap());
    if err < 0 { return Err(-err as isize); }
    Ok(())
}

/// Get file status (size).
pub fn fs_fstat(fid: u64) -> Result<u64, isize> {
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_FSTAT;
    req[1..9].copy_from_slice(&fid.to_le_bytes());

    let mut resp = [0u8; IPC_MAX];
    let ret = unsafe { native::ipc_call(FS_CHANNEL, req.as_ptr() as usize, IPC_MAX,
                                         resp.as_mut_ptr() as usize, IPC_MAX) };
    if ret < 0 { return Err(-ret); }
    let err = i64::from_le_bytes(resp[0..8].try_into().unwrap());
    if err < 0 { return Err(-err as isize); }
    Ok(u64::from_le_bytes(resp[8..16].try_into().unwrap()))
}

/// getdents64 — read directory entries.
pub fn fs_getdents(fid: u64, buf: &mut [u8]) -> Result<usize, isize> {
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_GETDENTS;
    req[1..9].copy_from_slice(&fid.to_le_bytes());
    req[9..17].copy_from_slice(&(buf.len() as u64).to_le_bytes());

    let mut resp = [0u8; IPC_MAX];
    let ret = unsafe {
        native::ipc_call(FS_CHANNEL, req.as_ptr() as usize, IPC_MAX,
                         resp.as_mut_ptr() as usize, IPC_MAX)
    };
    if ret < 0 { return Err(-ret); }
    let err = i64::from_le_bytes(resp[0..8].try_into().unwrap());
    if err < 0 { return Err(-err as isize); }
    let n = u64::from_le_bytes(resp[8..16].try_into().unwrap()) as usize;
    let copy = n.min(buf.len());
    buf[..copy].copy_from_slice(&resp[16..16 + copy]);
    Ok(n)
}

/// readlink — read symlink target path. Returns (target_bytes, actual_len).
pub fn fs_readlink(path: &str, buf: &mut [u8]) -> Result<usize, isize> {
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_READLINK;
    let n = path.as_bytes().len().min(IPC_MAX - 1);
    req[1..1 + n].copy_from_slice(&path.as_bytes()[..n]);

    let mut resp = [0u8; IPC_MAX];
    let ret = unsafe {
        native::ipc_call(FS_CHANNEL, req.as_ptr() as usize, IPC_MAX,
                         resp.as_mut_ptr() as usize, IPC_MAX)
    };
    if ret < 0 { return Err(-ret); }
    let err = i64::from_le_bytes(resp[0..8].try_into().unwrap());
    if err < 0 { return Err(-err as isize); }
    let len = u64::from_le_bytes(resp[8..16].try_into().unwrap()) as usize;
    let copy = len.min(buf.len());
    buf[..copy].copy_from_slice(&resp[16..16 + copy]);
    Ok(len)
}

/// Seek within a file.
pub fn fs_lseek(fid: u64, offset: isize, whence: u8) -> Result<usize, isize> {
    let mut req = [0u8; IPC_MAX];
    req[0] = OP_LSEEK;
    req[1..9].copy_from_slice(&fid.to_le_bytes());
    req[9..17].copy_from_slice(&(offset as i64).to_le_bytes());
    req[17] = whence;

    let mut resp = [0u8; IPC_MAX];
    let ret = unsafe { native::ipc_call(FS_CHANNEL, req.as_ptr() as usize, IPC_MAX,
                                         resp.as_mut_ptr() as usize, IPC_MAX) };
    if ret < 0 { return Err(-ret); }
    let err = i64::from_le_bytes(resp[0..8].try_into().unwrap());
    if err < 0 { return Err(-err as isize); }
    Ok(u64::from_le_bytes(resp[8..16].try_into().unwrap()) as usize)
}
