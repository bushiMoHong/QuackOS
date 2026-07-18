//! File descriptor table — maps Linux fd numbers to backing objects.
//!
//! fds 0/1/2 start out as the console (UART-backed tty); other fds are
//! FsServer files identified by an opaque fid.

/// Opaque file identifier returned by FsServer on open.
pub type FsFid = u64;

/// Maximum number of open files per process.
const MAX_FDS: usize = 256;

/// What an fd refers to.
#[derive(Clone, Copy, PartialEq)]
pub enum FdKind {
    Free,
    /// UART console (tty semantics).
    Console,
    /// FsServer-backed file.
    File(FsFid),
    /// Pipe read end (index into TaskStruct.pipes).
    PipeRead(usize),
    /// Pipe write end (index into TaskStruct.pipes).
    PipeWrite(usize),
}

/// An entry in the fd table.
#[derive(Clone, Copy)]
struct FdEntry {
    kind: FdKind,
    /// Open flags (O_RDONLY, O_WRONLY, O_RDWR, etc.)
    flags: u32,
}

/// Per-process file descriptor table.
pub struct FdTable {
    entries: [FdEntry; MAX_FDS],
}

impl FdTable {
    pub fn new() -> Self {
        let mut t = FdTable {
            entries: [FdEntry { kind: FdKind::Free, flags: 0 }; MAX_FDS],
        };
        // stdin / stdout / stderr → console
        for fd in 0..3 {
            t.entries[fd] = FdEntry { kind: FdKind::Console, flags: 2 /* O_RDWR */ };
        }
        t
    }

    /// Allocate the lowest free fd, storing the given File fid.
    pub fn alloc_file(&mut self, fid: FsFid, flags: u32) -> Option<usize> {
        self.alloc_kind(FdKind::File(fid), flags)
    }

    /// Allocate the lowest free fd, storing the given kind.
    pub fn alloc_kind(&mut self, kind: FdKind, flags: u32) -> Option<usize> {
        for i in 0..MAX_FDS {
            if self.entries[i].kind == FdKind::Free {
                self.entries[i] = FdEntry { kind, flags };
                return Some(i);
            }
        }
        None
    }

    /// Look up what an fd refers to.
    pub fn get(&self, fd: usize) -> Option<FdKind> {
        self.entries
            .get(fd)
            .filter(|e| e.kind != FdKind::Free)
            .map(|e| e.kind)
    }

    /// Look up the FsServer fid for a given Linux fd (legacy API).
    pub fn get_fid(&self, fd: usize) -> Option<FsFid> {
        match self.get(fd) {
            Some(FdKind::File(fid)) => Some(fid),
            _ => None,
        }
    }

    /// Close a fd, freeing its slot.  Returns true if the fd was open.
    pub fn close(&mut self, fd: usize) -> bool {
        if let Some(entry) = self.entries.get_mut(fd) {
            if entry.kind != FdKind::Free {
                entry.kind = FdKind::Free;
                entry.flags = 0;
                return true;
            }
        }
        false
    }

    /// Close a fd and return the old kind (for FsServer cleanup).
    pub fn close_kind(&mut self, fd: usize) -> Option<FdKind> {
        if let Some(entry) = self.entries.get_mut(fd) {
            if entry.kind != FdKind::Free {
                let old = entry.kind;
                entry.kind = FdKind::Free;
                entry.flags = 0;
                return Some(old);
            }
        }
        None
    }
}
