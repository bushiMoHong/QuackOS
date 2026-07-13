//! File System server — user-space IPC server that handles all filesystem
//! requests.  Single-threaded event loop + worker thread pool for block I/O.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use spin::RwLock;

use super::dentry::{Dentry, DentryFlags};
use super::file::{FdTable, File};
use super::inode::InodeOp;
use super::types::{Errno, Kstat, OpenFlags, SeekWhence};

use crate::kernel::ipc::channel::{ChannelId, with_channel, SendMatch, RecvMatch};
use crate::kernel::ipc::message::{Message, ShortPayload, ProcessId};
use crate::kernel::ipc::{deliver, wake, get_ipc_buffer};
use crate::kernel::sche::{self, block_current, IpcState};

// ---------------------------------------------------------------------------
// FS IPC protocol constants (Phase 0)
// ---------------------------------------------------------------------------

/// Well-known channel ID for filesystem requests.
pub const FS_CHANNEL: u32 = 1;

// Operation codes
const OP_OPEN:   u8 = 1;
const OP_READ:   u8 = 2;
const OP_CLOSE:  u8 = 3;
const OP_FSTAT:  u8 = 4;
const OP_LSEEK:  u8 = 5;
const OP_WRITE:  u8 = 6;

// ---------------------------------------------------------------------------
// Kernel-internal IPC helpers
// ---------------------------------------------------------------------------

/// Receive raw bytes from `channel_id`, blocking if necessary.
fn kernel_recv(channel_id: ChannelId) -> Vec<u8> {
    let tid = sche::current_thread();

    let action = with_channel(channel_id, |inner| inner.match_receiver(tid))
        .expect("kernel_recv: channel not found");

    match action {
        RecvMatch::Matched(sender_entry) => {
            let msg = sender_entry.msg.unwrap_or_else(|| {
                Message::new_short(0, ShortPayload { words: [0; 8], len: 0 })
            });
            let sender_tid = sender_entry.thread_id;
            let _ = deliver(&msg, tid, None, None);
            wake(sender_tid);

            if let Message::Short(_, ref payload) = msg {
                let n = payload.len.min(64) as usize;
                let mut buf = vec![0u8; n];
                for i in 0..((n + 7) / 8) {
                    let bytes = payload.words[i].to_le_bytes();
                    let off = i * 8;
                    let m = (n - off).min(8);
                    buf[off..off + m].copy_from_slice(&bytes[..m]);
                }
                buf
            } else {
                vec![]
            }
        }
        RecvMatch::Parked => {
            unsafe { block_current(IpcState::BlockedOnReceive(channel_id)); }
            let ipc_buf = get_ipc_buffer(tid).expect("kernel_recv: no buffer");
            if let Some(payload) = ipc_buf.read_short() {
                let n = payload.len.min(64) as usize;
                let mut buf = vec![0u8; n];
                for i in 0..((n + 7) / 8) {
                    let bytes = payload.words[i].to_le_bytes();
                    let off = i * 8;
                    let m = (n - off).min(8);
                    buf[off..off + m].copy_from_slice(&bytes[..m]);
                }
                buf
            } else {
                vec![]
            }
        }
    }
}

/// Send raw bytes through `channel_id`.  Blocks if no receiver is waiting.
fn kernel_send(channel_id: ChannelId, data: &[u8]) {
    let tid = sche::current_thread();
    let n = data.len().min(64);

    let mut words = [0usize; 8];
    let mut buf = [0u8; 64];
    buf[..n].copy_from_slice(&data[..n]);
    for i in 0..((n + 7) / 8) {
        let off = i * 8;
        let end = (off + 8).min(n);
        let mut w: usize = 0;
        for j in off..end {
            w |= (buf[j] as usize) << ((j - off) * 8);
        }
        words[i] = w;
    }
    let payload = ShortPayload { words, len: n as u8 };
    let msg = Message::new_short(1, payload);

    let action = with_channel(channel_id, |inner| inner.match_sender(tid, &msg))
        .expect("kernel_send: channel not found");

    match action {
        SendMatch::Matched(receiver_tid) => {
            let _ = deliver(&msg, receiver_tid, None, None);
            wake(receiver_tid);
        }
        SendMatch::Parked => {
            unsafe { block_current(IpcState::BlockedOnSend(channel_id)); }
        }
    }
}

// ---------------------------------------------------------------------------
// FsServer
// ---------------------------------------------------------------------------

pub struct FsServer {
    /// Global root dentry.
    pub root: Arc<Dentry>,
    /// Per-process file descriptor tables.
    pub fd_tables: RwLock<BTreeMap<u32, FdTable>>,
}

impl FsServer {
    pub fn new(root_inode: Arc<dyn InodeOp>) -> Self {
        let root = Dentry::new("/".into(), None, DentryFlags::DIRECTORY, Some(root_inode));
        Self {
            root,
            fd_tables: RwLock::new(BTreeMap::new()),
        }
    }

    /// Path walk: resolve `path` relative to `root`, returning the target
    /// Dentry and its parent.
    pub fn path_walk(
        root: &Arc<Dentry>,
        path: &str,
    ) -> Result<(Arc<Dentry>, Arc<Dentry>), Errno> {
        let components: Vec<&str> = path
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if components.is_empty() {
            return Ok((root.clone(), root.clone()));
        }

        let mut current = root.clone();
        for &name in &components[..components.len() - 1] {
            let next = if let Some(child) = current.get_child(name) {
                child
            } else {
                let inode_opt = current.inode.read();
                if let Some(ref inode) = *inode_opt {
                    let child = inode.lookup(name, current.clone());
                    current
                        .children
                        .write()
                        .insert(name.into(), Arc::downgrade(&child));
                    child
                } else {
                    return Err(Errno::ENOENT);
                }
            };
            current = next;
        }

        let last_name = components.last().unwrap();
        let target = if let Some(child) = current.get_child(last_name) {
            child
        } else {
            let inode_opt = current.inode.read();
            if let Some(ref inode) = *inode_opt {
                let child = inode.lookup(last_name, current.clone());
                current
                    .children
                    .write()
                    .insert((*last_name).into(), Arc::downgrade(&child));
                child
            } else {
                return Err(Errno::ENOENT);
            }
        };

        Ok((target, current))
    }

    /// Open a file for a process, returning the fd.
    pub fn open(
        &self,
        pid: u32,
        path: &str,
        flags: OpenFlags,
        _mode: u16,
    ) -> Result<usize, Errno> {
        let (dentry, _parent) = Self::path_walk(&self.root, path)?;
        if dentry.is_negative() && !flags.contains(OpenFlags::O_CREAT) {
            return Err(Errno::ENOENT);
        }
        let file = Arc::new(File::new(dentry, flags));
        let mut tables = self.fd_tables.write();
        let table = tables.entry(pid).or_insert_with(FdTable::new);
        table
            .alloc_fd(file)
            .ok_or(Errno::ENOMEM)
    }

    /// Read from a file descriptor.
    pub fn read(&self, pid: u32, fd: usize, count: usize) -> Result<Vec<u8>, Errno> {
        let tables = self.fd_tables.read();
        let table = tables.get(&pid).ok_or(Errno::EBADF)?;
        let file = table.get_file(fd).ok_or(Errno::EBADF)?;
        let mut buf = vec![0u8; count];
        let n = file.read(&mut buf);
        buf.truncate(n);
        Ok(buf)
    }

    /// Read into a caller-provided buffer — no allocation.
    pub fn read_to(&self, pid: u32, fd: usize, buf: &mut [u8]) -> Result<usize, Errno> {
        let tables = self.fd_tables.read();
        let table = tables.get(&pid).ok_or(Errno::EBADF)?;
        let file = table.get_file(fd).ok_or(Errno::EBADF)?;
        Ok(file.read(buf))
    }

    /// Write to a file descriptor.
    pub fn write(&self, pid: u32, fd: usize, data: &[u8]) -> Result<usize, Errno> {
        let tables = self.fd_tables.read();
        let table = tables.get(&pid).ok_or(Errno::EBADF)?;
        let file = table.get_file(fd).ok_or(Errno::EBADF)?;
        Ok(file.write(data))
    }

    /// Close a file descriptor.
    pub fn close(&self, pid: u32, fd: usize) -> Result<(), Errno> {
        let mut tables = self.fd_tables.write();
        let table = tables.get_mut(&pid).ok_or(Errno::EBADF)?;
        if table.close(fd) {
            Ok(())
        } else {
            Err(Errno::EBADF)
        }
    }

    /// Seek within a file.
    pub fn lseek(&self, pid: u32, fd: usize, offset: isize, whence: SeekWhence) -> Result<usize, Errno> {
        let tables = self.fd_tables.read();
        let table = tables.get(&pid).ok_or(Errno::EBADF)?;
        let file = table.get_file(fd).ok_or(Errno::EBADF)?;
        Ok(file.seek(offset, whence))
    }

    /// Get file metadata by path.
    pub fn stat(&self, _pid: u32, path: &str) -> Result<Kstat, Errno> {
        let (dentry, _) = Self::path_walk(&self.root, path)?;
        let inode_opt = dentry.inode.read().clone();
        if let Some(ref inode) = inode_opt {
            Ok(inode.get_stat())
        } else {
            Err(Errno::ENOENT)
        }
    }

    /// Get file metadata by fd.
    pub fn fstat(&self, pid: u32, fd: usize) -> Result<Kstat, Errno> {
        let tables = self.fd_tables.read();
        let table = tables.get(&pid).ok_or(Errno::EBADF)?;
        let file = table.get_file(fd).ok_or(Errno::EBADF)?;
        let inode_opt = file.dentry.inode.read().clone();
        if let Some(ref inode) = inode_opt {
            Ok(inode.get_stat())
        } else {
            Err(Errno::EBADF)
        }
    }

    /// Read directory entries.
    pub fn getdents(&self, pid: u32, fd: usize, count: usize) -> Result<Vec<u8>, Errno> {
        let tables = self.fd_tables.read();
        let table = tables.get(&pid).ok_or(Errno::EBADF)?;
        let file = table.get_file(fd).ok_or(Errno::EBADF)?;
        if file.dentry.flags.read().contains(DentryFlags::DIRECTORY) {
            let mut buf = vec![0u8; count];
            let (n, _) = file.getdents(&mut buf);
            buf.truncate(n);
            Ok(buf)
        } else {
            Err(Errno::ENOTDIR)
        }
    }

    /// Create a new file.
    pub fn create(&self, _pid: u32, path: &str, mode: u16) -> Result<(), Errno> {
        let (dentry, parent_dentry) = Self::path_walk(&self.root, path)?;
        if !dentry.is_negative() {
            return Err(Errno::EEXIST);
        }
        let pinode_opt = parent_dentry.inode.read().clone();
        if let Some(ref inode) = pinode_opt {
            inode.create(dentry.clone(), mode);
            Ok(())
        } else {
            Err(Errno::ENOENT)
        }
    }

    /// Create a new directory.
    pub fn mkdir(&self, _pid: u32, path: &str, mode: u16) -> Result<(), Errno> {
        let (dentry, parent_dentry) = Self::path_walk(&self.root, path)?;
        if !dentry.is_negative() {
            return Err(Errno::EEXIST);
        }
        let pinode_opt = parent_dentry.inode.read().clone();
        if let Some(ref inode) = pinode_opt {
            inode.mkdir(dentry.clone(), mode);
            Ok(())
        } else {
            Err(Errno::ENOENT)
        }
    }

    // -----------------------------------------------------------------------
    // IPC event loop
    // -----------------------------------------------------------------------

    /// Dispatch a single request and return the response bytes.
    fn dispatch(&self, req: &[u8]) -> Vec<u8> {
        if req.is_empty() { return vec![0, 0, 0, 0, 0, 0, 0, 0]; }

        let op = req[0];
        let mut resp = vec![0u8; 64];

        match op {
            OP_OPEN => {
                // req: [op:u8][flags:u8][path:...]
                let flags_byte = *req.get(1).unwrap_or(&0);
                let path_bytes = &req[2..];
                let path_len = path_bytes.iter().position(|&b| b == 0).unwrap_or(path_bytes.len());
                let path = core::str::from_utf8(&path_bytes[..path_len]).unwrap_or("/");
                let oflags = OpenFlags::from_raw(flags_byte as u32);

                match self.open(0, path, oflags, 0) {
                    Ok(fd) => {
                        resp[0..8].copy_from_slice(&0i64.to_le_bytes());
                        resp[8..16].copy_from_slice(&(fd as u64).to_le_bytes());
                    }
                    Err(e) => {
                        resp[0..8].copy_from_slice(&(-(e as i64)).to_le_bytes());
                    }
                }
            }
            OP_READ => {
                // req: [op:u8][fd:u64 LE][count:u64 LE]
                let fd = u64::from_le_bytes(req.get(1..9).map(|s| s.try_into().unwrap()).unwrap_or([0; 8])) as usize;
                let count = u64::from_le_bytes(req.get(9..17).map(|s| s.try_into().unwrap()).unwrap_or([0; 8])) as usize;

                let tables = self.fd_tables.read();
                if let Some(table) = tables.get(&0) {
                    if let Some(file) = table.get_file(fd) {
                        let mut buf = vec![0u8; count.min(48)]; // 64 - 8 retval - 8 actual_len
                        let n = file.read(&mut buf);
                        buf.truncate(n);
                        resp[0..8].copy_from_slice(&0i64.to_le_bytes());
                        resp[8..16].copy_from_slice(&(n as u64).to_le_bytes());
                        let copy_len = n.min(resp.len() - 16);
                        resp[16..16 + copy_len].copy_from_slice(&buf[..copy_len]);
                    } else {
                        resp[0..8].copy_from_slice(&(-(Errno::EBADF as i64)).to_le_bytes());
                    }
                } else {
                    resp[0..8].copy_from_slice(&(-(Errno::EBADF as i64)).to_le_bytes());
                }
            }
            OP_CLOSE => {
                // req: [op:u8][fd:u64 LE]
                let fd = u64::from_le_bytes(req.get(1..9).map(|s| s.try_into().unwrap()).unwrap_or([0; 8])) as usize;
                match self.close(0, fd) {
                    Ok(()) => resp[0..8].copy_from_slice(&0i64.to_le_bytes()),
                    Err(e) => resp[0..8].copy_from_slice(&(-(e as i64)).to_le_bytes()),
                }
            }
            OP_FSTAT => {
                // req: [op:u8][fd:u64 LE]
                let fd = u64::from_le_bytes(req.get(1..9).map(|s| s.try_into().unwrap()).unwrap_or([0; 8])) as usize;
                match self.fstat(0, fd) {
                    Ok(stat) => {
                        resp[0..8].copy_from_slice(&0i64.to_le_bytes());
                        resp[8..16].copy_from_slice(&stat.size.to_le_bytes());
                    }
                    Err(e) => {
                        resp[0..8].copy_from_slice(&(-(e as i64)).to_le_bytes());
                    }
                }
            }
            OP_LSEEK => {
                // req: [op:u8][fd:u64 LE][offset:i64 LE][whence:u8]
                let fd = u64::from_le_bytes(req.get(1..9).map(|s| s.try_into().unwrap()).unwrap_or([0; 8])) as usize;
                let offset = i64::from_le_bytes(req.get(9..17).map(|s| s.try_into().unwrap()).unwrap_or([0; 8])) as isize;
                let whence_byte = *req.get(17).unwrap_or(&0);
                let whence = match whence_byte {
                    0 => SeekWhence::Set,
                    1 => SeekWhence::Cur,
                    2 => SeekWhence::End,
                    _ => SeekWhence::Set,
                };
                match self.lseek(0, fd, offset, whence) {
                    Ok(pos) => {
                        resp[0..8].copy_from_slice(&0i64.to_le_bytes());
                        resp[8..16].copy_from_slice(&(pos as u64).to_le_bytes());
                    }
                    Err(e) => {
                        resp[0..8].copy_from_slice(&(-(e as i64)).to_le_bytes());
                    }
                }
            }
            OP_WRITE => {
                // req: [op:u8][fid:u64 LE][len:u64 LE][data...]
                let fd = u64::from_le_bytes(req.get(1..9).map(|s| s.try_into().unwrap()).unwrap_or([0; 8])) as usize;
                let data_len = u64::from_le_bytes(req.get(9..17).map(|s| s.try_into().unwrap()).unwrap_or([0; 8])) as usize;
                let data = &req.get(17..).unwrap_or(&[]);
                let n = data.len().min(data_len);
                match self.write(0, fd, &data[..n]) {
                    Ok(written) => {
                        resp[0..8].copy_from_slice(&0i64.to_le_bytes());
                        resp[8..16].copy_from_slice(&(written as u64).to_le_bytes());
                    }
                    Err(e) => {
                        resp[0..8].copy_from_slice(&(-(e as i64)).to_le_bytes());
                    }
                }
            }
            _ => {
                resp[0..8].copy_from_slice(&(-(Errno::ENOSYS as i64)).to_le_bytes());
            }
        }

        resp
    }

    /// Run the IPC event loop on the well-known FS channel.
    ///
    /// This function never returns — it services filesystem requests
    /// in a loop.  Create a dedicated kernel thread for it.
    pub fn run_ipc_loop(self: Arc<Self>) -> ! {
        use crate::print_uart;

        // create_channel allocates sequential IDs starting at 0.
        // Call it twice so the FS channel gets ID 1 (FS_CHANNEL).
        let _ = crate::kernel::ipc::channel::create_channel(0); // ChannelId(0)
        let fs_channel = crate::kernel::ipc::channel::create_channel(0).unwrap(); // ChannelId(1)
        debug_assert!(fs_channel.0 == FS_CHANNEL);

        print_uart("[FsServer] IPC loop starting on channel ");
        // print_uart_hex not readily available here; skip hex print

        loop {
            let request = kernel_recv(fs_channel);
            let response = self.dispatch(&request);
            kernel_send(fs_channel, &response);
        }
    }
}
