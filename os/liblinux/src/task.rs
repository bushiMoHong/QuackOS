//! Linux TaskStruct — per-process state maintained in user space.
//!
//! Like the Linux kernel's `task_struct`, this holds all per-process
//! bookkeeping: file descriptor table, brk, signal handlers, etc.
//! Unlike Linux, this lives entirely in liblinux's heap.

use crate::fd_table::FdTable;

/// Task / process control block.
pub struct TaskStruct {
    /// File descriptor table (Linux fd → FsServer handle)
    pub fd_table: FdTable,
    /// Program break (end of data segment)
    pub brk: usize,
    /// Initial brk (set by kernel at load time, never shrinks below this)
    pub initial_brk: usize,
    /// Process ID (from the kernel's perspective — ThreadId)
    pub pid: u64,
    /// Exit code (set by exit / exit_group)
    pub exit_code: i32,
    /// clear_child_tid pointer — kernel zeroes *clear_child_tid on thread exit.
    /// Set by sys_set_tid_address (syscall 96).
    pub clear_child_tid: usize,
    /// PR_SET_NO_NEW_PRIVS flag
    pub no_new_privs: bool,
    /// Next free virtual address for mmap (simple bump allocator).
    pub mmap_base: usize,
}

impl TaskStruct {
    pub fn new(initial_brk: usize) -> Self {
        TaskStruct {
            fd_table: FdTable::new(),
            brk: initial_brk,
            initial_brk,
            pid: 0,
            exit_code: 0,
            clear_child_tid: 0,
            no_new_privs: false,
            mmap_base: 0x0001_0000_0000, // 4 GB, well above program segments
        }
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
}
