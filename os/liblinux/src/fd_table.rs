//! File descriptor table — maps Linux fd numbers to filesystem server
//! file identifiers (fid).

/// Opaque file identifier returned by FsServer on open.
pub type FsFid = u64;

/// Maximum number of open files per process.
const MAX_FDS: usize = 256;

/// An entry in the fd table.
#[derive(Clone, Copy)]
struct FdEntry {
    /// FsServer file identifier (0 = free slot)
    fid: FsFid,
    /// Open flags (O_RDONLY, O_WRONLY, O_RDWR, etc.)
    flags: u32,
}

/// Per-process file descriptor table.
pub struct FdTable {
    entries: [FdEntry; MAX_FDS],
}

impl FdTable {
    pub fn new() -> Self {
        FdTable {
            entries: [FdEntry { fid: 0, flags: 0 }; MAX_FDS],
        }
    }

    /// Allocate a new fd, storing the given fid.
    pub fn alloc(&mut self, fid: FsFid, flags: u32) -> Option<usize> {
        for (i, entry) in self.entries.iter_mut().enumerate() {
            if entry.fid == 0 {
                entry.fid = fid;
                entry.flags = flags;
                return Some(i);
            }
        }
        None
    }

    /// Look up the FsServer fid for a given Linux fd.
    pub fn get(&self, fd: usize) -> Option<FsFid> {
        self.entries.get(fd).filter(|e| e.fid != 0).map(|e| e.fid)
    }

    /// Close a fd, freeing its slot.
    pub fn close(&mut self, fd: usize) -> bool {
        if let Some(entry) = self.entries.get_mut(fd) {
            if entry.fid != 0 {
                entry.fid = 0;
                entry.flags = 0;
                return true;
            }
        }
        false
    }
}
