//! Memory Manager server — the policy-side of the microkernel memory system.
//!
//! # Role
//!
//! `MmServer` is a user-space (Ring 3) process that:
//!
//! 1. Receives `IpcPageFault` messages from the kernel's `bmm` via IPC.
//! 2. Resolves each fault by consulting the faulting process's VMA manager.
//! 3. Allocates physical pages from the global buddy allocator.
//! 4. Sends `MapRequest` / `UnmapRequest` / `KillProcess` back to the kernel.
//!
//! # Concurrency model
//!
//! - **VMA managers**: protected by a `RwLock<BTreeMap>` (reader = fault handler,
//!   writer = mmap / munmap / process exit).  When `alloc` is unavailable the
//!   map is backed by a fixed-size array.
//! - **Physical allocator**: per-CPU `PcpCache` for the hot path, global
//!   `BuddyAllocator` with a spinlock for the slow path.
//! - **IPC channel**: the server blocks on `sys_ipc_recv` on its well-known
//!   channel, processes one request at a time, then replies via `sys_ipc_send`.
//!
//! # Future: multi-worker
//!
//! For SMP scalability the single-threaded event loop can be upgraded to a
//! pool of worker threads, each with its own `PcpCache`, sharing the global
//! `BuddyAllocator` and the `RwLock<VmaManager>` map.

use crate::kernel::bmm::AddressSpaceId;
use crate::kernel::ipc::message::{IpcPageFault, ProcessId};
use crate::kernel::ipc::ChannelId;

use crate::usr::mm::page_fault::{resolve_page_fault, resolve_with_prefault};
use crate::usr::mm::types::{MmError, MmRequest, MmResult, OomPolicy, VmaEntry, VmPerms};
use crate::usr::mm::vma::VmaManager;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of processes the mm server can track simultaneously.
pub const MAX_PROCESSES: usize = 128;

// ---------------------------------------------------------------------------
// Per-process state
// ---------------------------------------------------------------------------

/// Everything the mm server knows about one user process.
pub struct ProcessMmState {
    pub pid: ProcessId,
    pub vma_manager: VmaManager,
    pub addr_space_id: AddressSpaceId,
}

impl ProcessMmState {
    pub fn new(pid: ProcessId, addr_space_id: AddressSpaceId) -> Self {
        ProcessMmState {
            pid,
            vma_manager: VmaManager::new(),
            addr_space_id,
        }
    }
}

// ---------------------------------------------------------------------------
// Process table
// ---------------------------------------------------------------------------

/// Fixed-size process table.
///
/// Indexed linearly; when `alloc` is available this can become a `BTreeMap`.
struct ProcessTable {
    slots: [Option<ProcessMmState>; MAX_PROCESSES],
    len: usize,
}

impl ProcessTable {
    const fn new() -> Self {
        ProcessTable {
            slots: [const { None }; MAX_PROCESSES],
            len: 0,
        }
    }

    fn get(&self, pid: ProcessId) -> Option<&ProcessMmState> {
        self.slots.iter().find_map(|s| s.as_ref().filter(|p| p.pid == pid))
    }

    fn get_mut(&mut self, pid: ProcessId) -> Option<&mut ProcessMmState> {
        self.slots.iter_mut().find_map(|s| s.as_mut().filter(|p| p.pid == pid))
    }

    fn insert(&mut self, state: ProcessMmState) -> Result<(), ProcessMmState> {
        for slot in self.slots.iter_mut() {
            if slot.is_none() {
                *slot = Some(state);
                self.len += 1;
                return Ok(());
            }
        }
        Err(state)
    }

    fn remove(&mut self, pid: ProcessId) -> Option<ProcessMmState> {
        for slot in self.slots.iter_mut() {
            if slot.as_ref().is_some_and(|p| p.pid == pid) {
                self.len -= 1;
                return slot.take();
            }
        }
        None
    }

    #[allow(dead_code)]
    fn iter_mut(&mut self) -> impl Iterator<Item = &mut ProcessMmState> {
        self.slots.iter_mut().filter_map(|s| s.as_mut())
    }
}

// ---------------------------------------------------------------------------
// MmServer
// ---------------------------------------------------------------------------

/// The memory-manager server.
pub struct MmServer {
    /// All processes currently tracked.
    processes: ProcessTable,
    /// IPC channel the kernel uses to deliver page faults to us.
    fault_channel: ChannelId,
    /// OOM policy.
    oom_policy: OomPolicy,
    /// Enable prefaulting (maps adjacent pages on each fault).
    prefault_enabled: bool,
}

impl MmServer {
    /// Create a new mm server.
    ///
    /// `fault_channel` is the IPC channel on which the kernel delivers
    /// `IpcPageFault` messages.  It must already be created and the mm
    /// server must hold RECV rights on it.
    pub fn new(fault_channel: ChannelId) -> Self {
        MmServer {
            processes: ProcessTable::new(),
            fault_channel,
            oom_policy: OomPolicy::default(),
            prefault_enabled: false,
        }
    }

    /// Enable or disable prefaulting at runtime.
    pub fn set_prefault(&mut self, enabled: bool) {
        self.prefault_enabled = enabled;
    }

    /// Set the OOM policy.
    pub fn set_oom_policy(&mut self, policy: OomPolicy) {
        self.oom_policy = policy;
    }

    // ------------------------------------------------------------------
    // Process registration
    // ------------------------------------------------------------------

    /// Register a new user process with the mm server.
    ///
    /// Must be called before the process starts executing (or at least before
    /// its first page fault).  The caller is responsible for creating the
    /// address space via `bmm` and providing the `AddressSpaceId`.
    pub fn register_process(
        &mut self,
        pid: ProcessId,
        addr_space_id: AddressSpaceId,
    ) -> Result<(), ProcessId> {
        let state = ProcessMmState::new(pid, addr_space_id);
        self.processes.insert(state).map_err(|s| s.pid)
    }

    /// Unregister a process (called on process exit).
    ///
    /// All VMA entries for the process are dropped.  Physical pages are **not**
    /// freed — that responsibility belongs to a separate reaper / the kernel's
    /// address-space teardown.
    pub fn unregister_process(&mut self, pid: ProcessId) {
        self.processes.remove(pid);
    }

    /// Set up initial VMAs for a newly loaded process.
    ///
    /// The caller provides the ELF-derived regions (code, data, stack).
    /// Guard pages are automatically placed:
    /// - One below the stack (stack overflow detection).
    /// - One at address 0 (null-pointer dereference detection).
    pub fn init_process_vma(
        &mut self,
        pid: ProcessId,
        code_start: usize,
        code_end: usize,
        data_start: usize,
        data_end: usize,
        stack_start: usize,
        stack_end: usize,
        heap_start: usize,
    ) -> Result<(), MmError> {
        let state = self
            .processes
            .get_mut(pid)
            .ok_or(MmError::NoVma)?;

        let mgr = &mut state.vma_manager;

        // Null-page guard.
        mgr.insert(VmaEntry::new_guard(0))?;

        // Code (RX).
        if code_end > code_start {
            mgr.insert(VmaEntry::new_code(code_start, code_end))?;
        }

        // Data (RW).
        if data_end > data_start {
            mgr.insert(VmaEntry::new_data(data_start, data_end))?;
        }

        // Heap (RW, starts empty or with a small initial region).
        if heap_start > 0 {
            mgr.insert(VmaEntry::new_heap(heap_start, heap_start))?; // 0-size placeholder
        }

        // Stack guard page (one page below stack).
        if stack_start > 0x1000 {
            mgr.insert(VmaEntry::new_guard(stack_start - 0x1000))?;
        }

        // Stack (RW).
        if stack_end > stack_start {
            mgr.insert(VmaEntry::new_stack(stack_start, stack_end))?;
        }

        Ok(())
    }

    // ------------------------------------------------------------------
    // mmap / munmap
    // ------------------------------------------------------------------

    /// Handle an `mmap` request from a user process.
    ///
    /// Allocates a VMA entry.  Physical pages are allocated lazily on first
    /// access (page fault).
    pub fn handle_mmap(
        &mut self,
        pid: ProcessId,
        start: usize,
        end: usize,
        read: bool,
        write: bool,
        exec: bool,
    ) -> Result<(), MmError> {
        let state = self.processes.get_mut(pid).ok_or(MmError::NoVma)?;
        let perms = VmPerms { read, write, exec };
        let entry = VmaEntry::new_mmap(start, end, perms);
        state.vma_manager.insert(entry)
    }

    /// Handle a `munmap` request from a user process.
    ///
    /// Removes the VMA entry.  The caller (kernel) is responsible for
    /// unmapping the pages before or after this call.
    pub fn handle_munmap(
        &mut self,
        pid: ProcessId,
        start: usize,
        end: usize,
    ) -> Result<(), MmError> {
        let state = self.processes.get_mut(pid).ok_or(MmError::NoVma)?;
        state.vma_manager.remove(start, end)
    }

    /// Handle a `brk` (heap extension) request.
    pub fn handle_brk(
        &mut self,
        pid: ProcessId,
        new_brk: usize,
    ) -> Result<usize, MmError> {
        let state = self.processes.get_mut(pid).ok_or(MmError::NoVma)?;

        // Find the heap VMA.
        let heap_idx = state
            .vma_manager
            .all()
            .iter()
            .position(|e| {
                e.as_ref()
                    .is_some_and(|e| e.region_type == crate::usr::mm::types::VmRegionType::Heap)
            })
            .ok_or(MmError::NoVma)?;

        let heap = state.vma_manager.all()[heap_idx].as_ref().unwrap();
        let current_brk = heap.end_vaddr;

        if new_brk == current_brk {
            return Ok(current_brk);
        }
        if new_brk < heap.start_vaddr {
            return Err(MmError::InvalidArgument);
        }

        // Extend or shrink by removing and re-inserting.
        let heap_start = heap.start_vaddr;
        let heap_perms = heap.perms;
        state.vma_manager.remove(heap_start, current_brk)?;

        if new_brk > heap_start {
            let new_heap = VmaEntry {
                start_vaddr: heap_start,
                end_vaddr: new_brk,
                perms: heap_perms,
                region_type: crate::usr::mm::types::VmRegionType::Heap,
                backing_offset: 0,
                cow: false,
            };
            state.vma_manager.insert(new_heap)?;
        }

        Ok(new_brk)
    }

    // ------------------------------------------------------------------
    // Page-fault handling — the main event loop
    // ------------------------------------------------------------------

    /// Process a single page-fault IPC message.
    ///
    /// This is the entry point called by the IPC dispatch loop.
    /// Returns a `MmRequest` to send back to the kernel.
    pub fn handle_page_fault(&mut self, fault: &IpcPageFault) -> MmResult<MmRequest> {
        // Map the fault's address space to a process.
        // We need to look up the process by addr_space_id.
        // For now, addr_space_id.0 doubles as pid (simplification).
        let pid = fault.addr_space_id.0 as ProcessId;

        // Ensure the process is registered.
        if self.processes.get(pid).is_none() {
            // Auto-register unknown address spaces (simplification).
            let _ = self.register_process(pid, fault.addr_space_id);
        }

        let state = self
            .processes
            .get_mut(pid)
            .ok_or(MmError::NoVma)?;

        if self.prefault_enabled {
            let (primary, _batch) =
                resolve_with_prefault(fault, &mut state.vma_manager, self.oom_policy)?;
            // TODO: send batch mappings as a single IPC when the kernel
            // supports batch map requests.
            Ok(primary)
        } else {
            resolve_page_fault(fault, &mut state.vma_manager, self.oom_policy)
        }
    }

    /// Get a reference to the VMA manager for `pid` (for debugging).
    pub fn vma_manager(&self, pid: ProcessId) -> Option<&VmaManager> {
        self.processes.get(pid).map(|s| &s.vma_manager)
    }

    /// Get a mutable reference to the VMA manager for `pid`.
    pub fn vma_manager_mut(&mut self, pid: ProcessId) -> Option<&mut VmaManager> {
        self.processes.get_mut(pid).map(|s| &mut s.vma_manager)
    }

    /// Return the fault channel ID.
    pub fn fault_channel(&self) -> ChannelId {
        self.fault_channel
    }

    /// Number of tracked processes.
    pub fn process_count(&self) -> usize {
        self.processes.len
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_lookup_process() {
        let mut server = MmServer::new(ChannelId(0));
        server.register_process(1, AddressSpaceId(1)).unwrap();
        assert!(server.vma_manager(1).is_some());
        assert!(server.vma_manager(2).is_none());
    }

    #[test]
    fn init_process_vma_sets_up_regions() {
        let mut server = MmServer::new(ChannelId(0));
        server.register_process(1, AddressSpaceId(1)).unwrap();

        server
            .init_process_vma(
                1,
                0x10000, 0x20000, // code
                0x20000, 0x30000, // data
                0x7FFF_F000, 0x8000_0000, // stack
                0x30000, // heap
            )
            .unwrap();

        let mgr = server.vma_manager(1).unwrap();
        assert!(mgr.find(0x15000).is_some()); // code
        assert!(mgr.find(0x25000).is_some()); // data
        assert!(mgr.find(0x7FFF_F100).is_some()); // stack
        assert!(mgr.find_guard(0).is_some()); // null guard
    }

    #[test]
    fn mmap_and_munmap() {
        let mut server = MmServer::new(ChannelId(0));
        server.register_process(2, AddressSpaceId(2)).unwrap();

        server
            .handle_mmap(2, 0x50000, 0x60000, true, true, false)
            .unwrap();

        let mgr = server.vma_manager(2).unwrap();
        assert!(mgr.find(0x55000).is_some());

        server.handle_munmap(2, 0x50000, 0x60000).unwrap();
        assert!(server.vma_manager(2).unwrap().is_empty());
    }

    #[test]
    fn unregister_removes_process() {
        let mut server = MmServer::new(ChannelId(0));
        server.register_process(3, AddressSpaceId(3)).unwrap();
        server.unregister_process(3);
        assert!(server.vma_manager(3).is_none());
    }
}
