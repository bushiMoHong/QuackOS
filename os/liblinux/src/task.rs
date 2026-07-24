//! Linux TaskStruct — per-process state maintained in user space.
//!
//! Like the Linux kernel's `task_struct`, this holds all per-process
//! bookkeeping: file descriptor table, brk, signal handlers, etc.
//! Unlike Linux, this lives entirely in liblinux's heap.

use crate::fd_table::FdTable;

/// Pipe ring buffer (in-memory).
pub struct Pipe {
    pub buf: [u8; 4096],
    pub read_pos: usize,
    pub write_pos: usize,
    pub byte_count: usize,
    /// Number of open read fds referencing this pipe.
    pub readers: usize,
    /// Number of open write fds referencing this pipe.
    pub writers: usize,
}

const MAX_PIPES: usize = 16;

/// Task / process control block.
pub struct TaskStruct {
    /// File descriptor table (Linux fd → FsServer handle)
    pub fd_table: FdTable,
    /// Program break (end of data segment)
    pub brk: usize,
    /// Initial brk (set by kernel at load time, never shrinks below this)
    pub initial_brk: usize,
    /// Process ID (liblinux-managed, Linux-style small integer starting from 1)
    pub pid: u64,
    /// Next PID to allocate for child processes
    pub next_pid: u64,
    /// Map kernel ThreadId → liblinux PID (up to 16 children)
    pub tid_to_pid: [(u32, u64); 16],
    pub tid_to_pid_count: usize,
    /// Exit code (set by exit / exit_group)
    pub exit_code: i32,
    /// clear_child_tid pointer — kernel zeroes *clear_child_tid on thread exit.
    /// Set by sys_set_tid_address (syscall 96).
    pub clear_child_tid: usize,
    /// PR_SET_NO_NEW_PRIVS flag
    pub no_new_privs: bool,
    /// Next free virtual address for mmap (simple bump allocator).
    pub mmap_base: usize,
    /// Console termios state (kernel struct termios, 36 bytes).
    pub termios: [u8; 36],
    /// Pending canonical-mode input line (delivered across short reads).
    pub line_buf: [u8; 4096],
    pub line_len: usize,
    pub line_pos: usize,
    /// Current working directory (null-terminated).
    pub cwd: [u8; 256],
    /// Pipe table.
    pub pipes: [Option<Pipe>; MAX_PIPES],
}

impl TaskStruct {
    pub fn new(initial_brk: usize) -> Self {
        let mut t = TaskStruct {
            fd_table: FdTable::new(),
            brk: initial_brk,
            initial_brk,
            pid: 1,
            next_pid: 2,
            tid_to_pid: [(0, 0); 16],
            tid_to_pid_count: 0,
            exit_code: 0,
            clear_child_tid: 0,
            no_new_privs: false,
            mmap_base: 0x0001_0000_0000, // 4 GB, well above program segments
            termios: crate::fs::DEFAULT_TERMIOS,
            line_buf: [0; 4096],
            line_len: 0,
            line_pos: 0,
            cwd: [0; 256],
            pipes: [const { None }; MAX_PIPES],
        };
        t.cwd[0] = b'/';
        t
    }

    /// Extend (or shrink) the program break.
    /// Returns the new brk on success, or the old brk on failure.
    pub fn do_brk(&mut self, new_brk: usize) -> Result<usize, isize> {
        if new_brk < self.initial_brk {
            // Never shrink below the initial data segment end.
            return Ok(self.brk);
        }

        use crate::native;
        let page_size = 4096;

        if new_brk > self.brk {
            // Grow: map anonymous pages
            let start = (self.brk + page_size - 1) & !(page_size - 1);
            let end = (new_brk + page_size - 1) & !(page_size - 1);
            for va in (start..end).step_by(page_size) {
                // prot: READ | WRITE
                let ret = unsafe { native::map_page(va, 1 | 2) };
                if ret < 0 {
                    return Err(ret);
                }
            }
        } else if new_brk < self.brk {
            // Shrink: unmap pages
            let start = (new_brk + page_size - 1) & !(page_size - 1);
            let end = (self.brk + page_size - 1) & !(page_size - 1);
            for va in (start..end).step_by(page_size) {
                unsafe { native::unmap_page(va); }
            }
        }

        self.brk = new_brk;
        Ok(new_brk)
    }

    /// Allocate a new PID for a child, and store the ThreadId→PID mapping.
    pub fn alloc_child_pid(&mut self, tid: u32) -> u64 {
        let pid = self.next_pid;
        self.next_pid += 1;
        if self.tid_to_pid_count < self.tid_to_pid.len() {
            self.tid_to_pid[self.tid_to_pid_count] = (tid, pid);
            self.tid_to_pid_count += 1;
        }
        pid
    }

    /// Look up the PID for a child ThreadId, and remove the mapping.
    pub fn take_child_pid(&mut self, tid: u32) -> Option<u64> {
        for i in 0..self.tid_to_pid_count {
            if self.tid_to_pid[i].0 == tid {
                let pid = self.tid_to_pid[i].1;
                // Remove by swapping with last
                self.tid_to_pid_count -= 1;
                self.tid_to_pid[i] = self.tid_to_pid[self.tid_to_pid_count];
                return Some(pid);
            }
        }
        None
    }
}
