//! Process table — the central data structure of the Process Server.
//!
//! # Design
//!
//! The process table is a fixed-size array of `ProcessInfo` slots indexed by
//! the lower 16 bits of `ProcessId`.  Each slot has an associated generation
//! counter that provides ABA protection — a stale `ProcessId` referencing a
//! reused slot will fail the generation check.
//!
//! # Locking
//!
//! The `ProcessTable` is not internally synchronised.  The `ProcServer` that
//! owns it serialises all access through its single-threaded IPC event loop.
//! When the server is upgraded to multi-worker, a `RwLock<ProcessTable>`
//! should wrap it.
//!
//! # Upgrade path
//!
//! When a global allocator is available, the fixed-size array can be replaced
//! with a `BTreeMap<u16, ProcessInfo>` keyed by slot index, removing the
//! `MAX_PROCESSES` cap.

use super::types::*;
use crate::kernel::bmm::AddressSpaceId;
use crate::usr::task::TaskId;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of processes the table can hold.
///
/// 128 processes is sufficient for embedded / microkernel workloads.
/// Desktop-class systems would need a dynamic structure.
pub const MAX_PROCESSES: usize = 128;

/// Maximum number of threads a single process can own.
///
/// This is a hard limit to keep `ProcessInfo` small (fixed-size array
/// of thread IDs).  Most microkernel processes use 1–4 threads.
pub const MAX_THREADS_PER_PROCESS: usize = 8;

// ---------------------------------------------------------------------------
// ProcessInfo
// ---------------------------------------------------------------------------

/// Everything the Process Server knows about one process.
///
/// Stored inline in the `ProcessTable` array.
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    /// Process identifier (generational).
    pub pid: ProcessId,

    /// Human-readable name (e.g. "init", "mm-server", "shell").
    /// Not null-terminated — `name_len` gives the valid prefix.
    pub name: [u8; 32],

    /// Number of valid bytes in `name`.
    pub name_len: u8,

    /// Current lifecycle state.
    pub state: ProcessState,

    /// Process-level priority — base priority for all threads in this process.
    pub priority: ProcessPriority,

    /// Address-space identifier (for TLB tagging, page-table lookups).
    pub addr_space_id: AddressSpaceId,

    /// Parent process (ProcessId::NULL for init).
    pub parent: ProcessId,

    /// Threads owned by this process.
    /// The first `thread_count` entries are valid.
    pub threads: [Option<TaskId>; MAX_THREADS_PER_PROCESS],

    /// Number of valid entries in `threads`.
    pub thread_count: u8,

    /// Exit code — meaningful only when `state == Zombie`.
    pub exit_code: Option<i32>,
}

impl ProcessInfo {
    /// Create a new `ProcessInfo` in `Created` state.
    pub fn new(
        pid: ProcessId,
        name: &[u8],
        priority: ProcessPriority,
        addr_space_id: AddressSpaceId,
        parent: ProcessId,
    ) -> Self {
        let mut name_buf = [0u8; 32];
        let name_len = name.len().min(32);
        name_buf[..name_len].copy_from_slice(&name[..name_len]);

        ProcessInfo {
            pid,
            name: name_buf,
            name_len: name_len as u8,
            state: ProcessState::Created,
            priority,
            addr_space_id,
            parent,
            threads: [const { None }; MAX_THREADS_PER_PROCESS],
            thread_count: 0,
            exit_code: None,
        }
    }

    /// Return the process name as a `&str` (lossy — invalid UTF-8 is replaced).
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize])
            .unwrap_or("<invalid-utf8>")
    }

    /// Return `true` if this process can accept another thread.
    #[inline]
    pub fn can_add_thread(&self) -> bool {
        (self.thread_count as usize) < MAX_THREADS_PER_PROCESS
    }

    /// Add a thread to this process's thread list.
    ///
    /// Returns `Err(())` if the thread list is full.
    pub fn add_thread(&mut self, tid: TaskId) -> Result<(), ()> {
        if !self.can_add_thread() {
            return Err(());
        }
        // Find first free slot.
        for slot in self.threads.iter_mut() {
            if slot.is_none() {
                *slot = Some(tid);
                self.thread_count += 1;
                return Ok(());
            }
        }
        Err(())
    }

    /// Remove a thread from this process's thread list.
    ///
    /// Returns `true` if the thread was found and removed.
    pub fn remove_thread(&mut self, tid: TaskId) -> bool {
        for slot in self.threads.iter_mut() {
            if *slot == Some(tid) {
                *slot = None;
                self.thread_count = self.thread_count.saturating_sub(1);
                return true;
            }
        }
        false
    }

    /// Return an iterator over this process's threads.
    pub fn thread_ids(&self) -> impl Iterator<Item = TaskId> + '_ {
        self.threads[..self.thread_count as usize]
            .iter()
            .filter_map(|opt| *opt)
    }

    /// Return `true` if all threads are in `Blocked` or `Dying` state
    /// (i.e. the process as a whole is blocked).
    ///
    /// The caller must provide a function that resolves a `TaskId` to its
    /// current `TaskState`, since `ProcessInfo` does not hold live state.
    pub fn all_threads_blocked(&self, state_of: impl Fn(TaskId) -> Option<crate::usr::task::TaskState>) -> bool {
        if self.thread_count == 0 {
            return true;
        }
        self.thread_ids().all(|tid| {
            state_of(tid).is_some_and(|s| {
                matches!(s, crate::usr::task::TaskState::Blocked | crate::usr::task::TaskState::Dying)
            })
        })
    }
}

// ---------------------------------------------------------------------------
// ProcessTable
// ---------------------------------------------------------------------------

/// Fixed-size process table with generational ABA protection.
///
/// # Slot lifecycle
///
/// ```text
/// Free → (alloc_slot, gen++) → Occupied → (free_slot) → Free
///                                                gen unchanged (ABA guard)
/// ```
///
/// The generation is *not* reset on free — a stale `ProcessId` that references
/// a freed-and-reused slot will have a mismatched generation and fail lookup.
pub struct ProcessTable {
    /// Process slots — `None` means free.
    slots: [Option<ProcessInfo>; MAX_PROCESSES],

    /// Number of currently-occupied slots.
    len: usize,

    /// Generation counter per slot — incremented on every allocation at
    /// that slot.  This provides the ABA-proof guarantee for `ProcessId`.
    generations: [u16; MAX_PROCESSES],
}

impl ProcessTable {
    /// Create an empty process table.
    pub const fn new() -> Self {
        ProcessTable {
            slots: [const { None }; MAX_PROCESSES],
            len: 0,
            generations: [0u16; MAX_PROCESSES],
        }
    }

    // ------------------------------------------------------------------
    // Capacity queries
    // ------------------------------------------------------------------

    /// Number of occupied slots.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// `true` when the table is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// `true` when the table is full.
    #[inline]
    pub fn is_full(&self) -> bool {
        self.len >= MAX_PROCESSES
    }

    // ------------------------------------------------------------------
    // Slot allocation / deallocation
    // ------------------------------------------------------------------

    /// Allocate a free slot and return a new `ProcessId`.
    ///
    /// Returns `None` if the table is full.
    fn alloc_slot(&mut self) -> Option<ProcessId> {
        for (i, slot) in self.slots.iter().enumerate() {
            if slot.is_none() {
                // Start generation at 1 so 0 (NULL) is never allocated.
                let gen = if self.generations[i] == 0 {
                    1
                } else {
                    self.generations[i].wrapping_add(1)
                };
                self.generations[i] = gen;
                return Some(ProcessId::new(i as u16, gen));
            }
        }
        None
    }

    /// Free a slot.
    ///
    /// The generation is intentionally **not** reset — this is the ABA guard.
    fn free_slot(&mut self, index: u16) {
        let i = index as usize;
        if i < MAX_PROCESSES {
            self.slots[i] = None;
            self.len = self.len.saturating_sub(1);
        }
    }

    // ------------------------------------------------------------------
    // CRUD
    // ------------------------------------------------------------------

    /// Insert a `ProcessInfo` into the table.
    ///
    /// Consumes the `ProcessInfo` and returns the allocated `ProcessId`.
    ///
    /// # Errors
    ///
    /// * `ProcError::ProcessTableFull` — no free slots.
    pub fn insert(&mut self, info: ProcessInfo) -> ProcResult<ProcessId> {
        let pid = self.alloc_slot().ok_or(ProcError::ProcessTableFull)?;

        let i = pid.index() as usize;
        self.slots[i] = Some(info);
        self.len += 1;

        log::info!("proc: registered process {:?} \"{}\"", pid, self.slots[i].as_ref().unwrap().name_str());
        Ok(pid)
    }

    /// Remove a process and return its `ProcessInfo`.
    ///
    /// The generation check ensures this `ProcessId` has not been reused
    /// since it was issued.
    pub fn remove(&mut self, pid: ProcessId) -> Option<ProcessInfo> {
        if pid.is_null() {
            return None;
        }
        let i = pid.index() as usize;
        if i >= MAX_PROCESSES {
            return None;
        }
        // Generation check.
        if self.generations[i] != pid.generation() {
            return None;
        }
        let info = self.slots[i].take();
        if info.is_some() {
            self.free_slot(pid.index());
            log::info!("proc: removed process {:?}", pid);
        }
        info
    }

    /// Look up a process by `ProcessId` (immutable).
    ///
    /// Returns `None` if the ID is invalid, the generation mismatches, or
    /// the slot is free.
    pub fn get(&self, pid: ProcessId) -> Option<&ProcessInfo> {
        if pid.is_null() {
            return None;
        }
        let i = pid.index() as usize;
        if i >= MAX_PROCESSES {
            return None;
        }
        if self.generations[i] != pid.generation() {
            return None;
        }
        self.slots[i].as_ref()
    }

    /// Look up a process by `ProcessId` (mutable).
    pub fn get_mut(&mut self, pid: ProcessId) -> Option<&mut ProcessInfo> {
        if pid.is_null() {
            return None;
        }
        let i = pid.index() as usize;
        if i >= MAX_PROCESSES {
            return None;
        }
        if self.generations[i] != pid.generation() {
            return None;
        }
        self.slots[i].as_mut()
    }

    // ------------------------------------------------------------------
    // Convenience queries
    // ------------------------------------------------------------------

    /// Find a process by name (exact match).  O(N).
    pub fn find_by_name(&self, name: &[u8]) -> Option<&ProcessInfo> {
        let name_len = name.len().min(32);
        self.slots.iter().find_map(|slot| {
            slot.as_ref().filter(|info| {
                info.name_len as usize == name_len
                    && &info.name[..name_len] == &name[..name_len]
            })
        })
    }

    /// Return PIDs of all children of `parent`.  O(N).
    pub fn children_of(&self, parent: ProcessId) -> impl Iterator<Item = ProcessId> + '_ {
        self.slots.iter().filter_map(move |slot| {
            slot.as_ref()
                .filter(|info| info.parent == parent)
                .map(|info| info.pid)
        })
    }

    /// Return an iterator over all valid `ProcessInfo` entries.
    pub fn iter(&self) -> impl Iterator<Item = &ProcessInfo> + '_ {
        self.slots.iter().filter_map(|slot| slot.as_ref())
    }

    /// Return an iterator over all valid `ProcessInfo` entries (mutable).
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut ProcessInfo> + '_ {
        self.slots.iter_mut().filter_map(|slot| slot.as_mut())
    }

    /// Transition a process's state (with generation check).
    ///
    /// Returns `ProcError::InvalidProcess` if the process doesn't exist.
    pub fn set_state(&mut self, pid: ProcessId, new_state: ProcessState) -> ProcResult<()> {
        let info = self.get_mut(pid).ok_or(ProcError::InvalidProcess)?;
        log::debug!(
            "proc: {:?} state {:?} → {:?}",
            pid,
            info.state,
            new_state,
        );
        info.state = new_state;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::bmm::AddressSpaceId;

    fn dummy_info(pid_hint: u16) -> ProcessInfo {
        let pid = ProcessId::new(pid_hint, 1);
        ProcessInfo::new(
            pid,
            b"test-process",
            ProcessPriority::DEFAULT,
            AddressSpaceId(pid_hint as usize),
            ProcessId::NULL,
        )
    }

    #[test]
    fn insert_and_lookup() {
        let mut table = ProcessTable::new();
        let info = dummy_info(0);
        let pid = table.insert(info).unwrap();
        assert!(!pid.is_null());
        assert_eq!(pid.generation(), 1);

        let found = table.get(pid).unwrap();
        assert_eq!(found.name_str(), "test-process");
        assert_eq!(found.state, ProcessState::Created);
    }

    #[test]
    fn remove_frees_slot() {
        let mut table = ProcessTable::new();
        let info = dummy_info(0);
        let pid = table.insert(info).unwrap();
        assert_eq!(table.len(), 1);

        let removed = table.remove(pid).unwrap();
        assert_eq!(removed.pid, pid);
        assert_eq!(table.len(), 0);
        assert!(table.get(pid).is_none());
    }

    #[test]
    fn generation_mismatch_rejected() {
        let mut table = ProcessTable::new();
        let info = dummy_info(0);
        let pid = table.insert(info).unwrap();
        assert_eq!(pid.generation(), 1);

        // Try to access with wrong generation.
        let wrong_pid = ProcessId::new(pid.index(), 99);
        assert!(table.get(wrong_pid).is_none());
    }

    #[test]
    fn null_pid_always_returns_none() {
        let table = ProcessTable::new();
        assert!(table.get(ProcessId::NULL).is_none());
        assert!(table.remove(ProcessId::NULL).is_none());
    }

    #[test]
    fn find_by_name_works() {
        let mut table = ProcessTable::new();
        let info = dummy_info(0);
        let _pid = table.insert(info).unwrap();

        assert!(table.find_by_name(b"test-process").is_some());
        assert!(table.find_by_name(b"nonexistent").is_none());
    }

    #[test]
    fn thread_list_management() {
        let mut info = dummy_info(0);
        assert_eq!(info.thread_count, 0);

        // Add threads.
        for i in 0..MAX_THREADS_PER_PROCESS {
            let tid = TaskId(crate::kernel::sche::ThreadId::new(i as u16, 1));
            assert!(info.add_thread(tid).is_ok());
        }
        assert_eq!(info.thread_count as usize, MAX_THREADS_PER_PROCESS);

        // Should be full now.
        let extra = TaskId(crate::kernel::sche::ThreadId::new(99, 1));
        assert!(info.add_thread(extra).is_err());

        // Remove one.
        let first = TaskId(crate::kernel::sche::ThreadId::new(0, 1));
        assert!(info.remove_thread(first));
        assert_eq!(info.thread_count as usize, MAX_THREADS_PER_PROCESS - 1);
    }
}
