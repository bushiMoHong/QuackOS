//! ProcServer — the user-space Process Manager.
//!
//! # Role in the microkernel
//!
//! `ProcServer` is the **policy decision-maker** for process management.
//! The kernel only sees threads; `ProcServer` builds the "process" abstraction
//! on top:
//!
//! ```text
//!                  ┌──────────────────────────┐
//!                  │      ProcServer           │
//!                  │                           │
//!                  │  • Process lifecycle      │
//!                  │  • Priority policy        │
//!                  │  • Signal delivery        │
//!                  │  • Permission / parentage │
//!                  │                           │
//!                  │  Role: Planner            │
//!                  └──────────┬───────────────┘
//!                             │
//!              ┌──────────────┼──────────────┐
//!              │              │              │
//!              ▼              ▼              ▼
//!        usr::task      usr::mm       kernel::cap
//!     (thread mgmt)  (address space) (capabilities)
//! ```
//!
//! # Concurrency model
//!
//! Single-threaded IPC event loop (same as `MmServer`):
//!
//! 1. Block on `sys_ipc_recv` on the request channel.
//! 2. Decode the incoming `ProcRequest`.
//! 3. Dispatch to the appropriate handler.
//! 4. Send a reply (if the request requires one).
//! 5. Loop.
//!
//! # Future: multi-worker
//!
//! For SMP scalability the single-threaded event loop can be upgraded to a
//! pool of worker threads sharing a `RwLock<ProcessTable>`.

use super::proc_table::{ProcessInfo, ProcessTable, MAX_PROCESSES, MAX_THREADS_PER_PROCESS};
use super::types::*;
use crate::kernel::bmm::AddressSpaceId;
use crate::kernel::ipc::ChannelId;
use crate::usr::task::{TaskId, TaskManager, TaskPriority};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

#[allow(dead_code)]
/// Default kernel stack size for a new process's initial thread (bytes).
const DEFAULT_KERNEL_STACK_SIZE: usize = 4096;

#[allow(dead_code)]
/// Default user stack size for a new process (bytes).
const DEFAULT_USER_STACK_SIZE: usize = 0x1_0000; // 64 KiB

#[allow(dead_code)]
/// Default heap size for a new process (bytes — initial reserve, grows on demand).
const DEFAULT_HEAP_INITIAL: usize = 0x0; // start at 0, grow via brk

// ---------------------------------------------------------------------------
// ProcServer
// ---------------------------------------------------------------------------

/// The Process Manager server.
///
/// # Well-known channels
///
/// `ProcServer` owns two IPC channels:
///
/// * `request_channel` — clients send `ProcRequest` messages here.
/// * `mm_channel`      — used to communicate with `MmServer` (register
///                        address spaces, set up VMAs, kill on OOM).
pub struct ProcServer {
    /// Process table — the single source of truth for all processes.
    processes: ProcessTable,

    /// IPC channel on which clients send requests to us.
    request_channel: ChannelId,

    /// IPC channel to the Memory Manager server.
    mm_channel: ChannelId,

    /// Task manager — creates / destroys threads on behalf of processes.
    task_mgr: TaskManager,

    /// Default priority assigned to newly spawned processes.
    default_priority: ProcessPriority,

    /// Whether this server has been initialised (bootstrapped with init).
    initialised: bool,
}

impl ProcServer {
    /// Create a new Process Server.
    ///
    /// `request_channel` must already exist and the server must hold RECV
    /// rights on it.  `mm_channel` must already exist and the server must
    /// hold SEND rights on it.
    pub fn new(request_channel: ChannelId, mm_channel: ChannelId) -> Self {
        ProcServer {
            processes: ProcessTable::new(),
            request_channel,
            mm_channel,
            task_mgr: TaskManager::new(),
            default_priority: ProcessPriority::DEFAULT,
            initialised: false,
        }
    }

    /// Return the request channel ID (for the IPC dispatch loop).
    pub fn request_channel(&self) -> ChannelId {
        self.request_channel
    }

    /// Return the mm channel ID.
    pub fn mm_channel(&self) -> ChannelId {
        self.mm_channel
    }

    /// Set the default priority for new processes.
    pub fn set_default_priority(&mut self, priority: ProcessPriority) {
        self.default_priority = priority;
    }

    // ------------------------------------------------------------------
    // Bootstrap — register the init process
    // ------------------------------------------------------------------

    /// Register the init process (PID 1) and any pre-created system servers.
    ///
    /// Must be called before any other process operations.  This is the
    /// boot-time initialisation path:
    ///
    /// 1. Register init.
    /// 2. Register system servers that were started before ProcServer itself
    ///    (e.g. the Memory Manager, if ProcServer depends on it).
    ///
    /// After this call, `initialised` is set to `true` and the server is
    /// ready to handle spawn requests.
    pub fn bootstrap(
        &mut self,
        init_addr_space_id: AddressSpaceId,
        system_servers: &[(AddressSpaceId, &[u8], ProcessPriority)],
    ) -> ProcResult<()> {
        if self.initialised {
            return Err(ProcError::InvalidState);
        }

        // 1. Register init (PID 1, generation 1 at slot 0).
        //    We bypass `insert()` for init because we need a known PID.
        let init_pid = ProcessId::new(0, 1);
        let init_info = ProcessInfo::new(
            init_pid,
            b"init",
            ProcessPriority::SYSTEM,
            init_addr_space_id,
            ProcessId::NULL, // init has no parent
        );
        let registered_pid = self.processes.insert(init_info)?;
        debug_assert_eq!(registered_pid, init_pid, "init must occupy slot 0 generation 1");

        log::info!("proc: bootstrapped init {:?}", init_pid);

        // 2. Register pre-existing system servers.
        for &(addr_space_id, name, priority) in system_servers {
            self.register_process(init_pid, addr_space_id, name, priority)?;
        }

        self.initialised = true;
        log::info!(
            "proc: initialised ({} processes registered)",
            self.processes.len(),
        );

        Ok(())
    }

    /// Register an already-running process (used at boot time and when
    /// a server that started before ProcServer connects).
    pub fn register_process(
        &mut self,
        parent: ProcessId,
        addr_space_id: AddressSpaceId,
        name: &[u8],
        priority: ProcessPriority,
    ) -> ProcResult<ProcessId> {
        // Verify the parent exists (unless it's the init path, where
        // parent may be NULL).
        if !parent.is_null() && self.processes.get(parent).is_none() {
            return Err(ProcError::InvalidProcess);
        }

        // Use insert() to get a generational ProcessId.
        let info = ProcessInfo::new(
            ProcessId::NULL, // placeholder — insert() returns the real PID
            name,
            priority,
            addr_space_id,
            parent,
        );
        let pid = self.processes.insert(info)?;
        Ok(pid)
    }

    // ------------------------------------------------------------------
    // Process spawn
    // ------------------------------------------------------------------

    /// Spawn a new process as a child of `parent`.
    ///
    /// # What happens
    ///
    /// 1. Validate the parent exists and has permission.
    /// 2. Allocate a `ProcessId` and insert a `Created` entry.
    /// 3. Request `MmServer` to create a new address space.
    /// 4. Set up initial VMAs (code, data, stack, heap) via mm.
    /// 5. Allocate a kernel stack for the initial thread.
    /// 6. Create the initial thread via `TaskManager`.
    /// 7. Transition the process to `Running`.
    /// 8. Return the new `ProcessId` to the parent.
    ///
    /// # Limitations (current)
    ///
    /// Steps 3–5 require IPC to `MmServer` and a physical-page allocator.
    /// For now these are stubbed — the process record is created and the
    /// address space / stack are expected to be pre-set by the caller
    /// (bootstrap-style).
    pub fn spawn(
        &mut self,
        parent: ProcessId,
        name: &[u8],
        code_start: usize,
        code_end: usize,
        data_start: usize,
        data_end: usize,
        stack_start: usize,
        stack_end: usize,
        heap_start: usize,
    ) -> ProcResult<ProcessId> {
        // 1. Parent must exist and be alive.
        let parent_info = self
            .processes
            .get(parent)
            .ok_or(ProcError::InvalidProcess)?;

        if !parent_info.state.is_alive() {
            return Err(ProcError::InvalidState);
        }

        // 2. Create the process record.
        let info = ProcessInfo::new(
            ProcessId::NULL, // placeholder
            name,
            self.default_priority,
            // TODO: allocate real AddressSpaceId via mm-server IPC.
            AddressSpaceId(0),
            parent,
        );
        let child_pid = self.processes.insert(info)?;

        log::info!(
            "proc: spawned {:?} \"{}\" parent={:?}",
            child_pid,
            core::str::from_utf8(name).unwrap_or("<invalid>"),
            parent,
        );

        // 3–7. Address space + stack + thread creation.
        //
        // TODO: When the mm-server IPC path is plumbed:
        //
        // // 3. Request mm-server to create a new address space.
        // let addr_space_id = mm_server_create_address_space(self.mm_channel)?;
        //
        // // 4. Set up initial VMAs.
        // mm_server_init_vma(self.mm_channel, child_pid, addr_space_id,
        //     code_start, code_end, data_start, data_end,
        //     stack_start, stack_end, heap_start)?;
        //
        // // 5. Allocate kernel stack.
        // let (stack_base, stack_top) = allocate_kernel_stack(DEFAULT_KERNEL_STACK_SIZE)?;
        //
        // // 6. Create the initial thread.
        // let tid = self.task_mgr.create_task(
        //     TaskPriority(self.default_priority.0),
        //     stack_base, stack_top,
        //     ttbr0, addr_space_id.0,
        //     child_pid.into(),
        // )?;
        //
        // // 7. Register the thread with the process.
        // let child = self.processes.get_mut(child_pid).unwrap();
        // child.add_thread(tid).map_err(|_| ProcError::ThreadTableFull)?;
        // child.addr_space_id = addr_space_id;
        // self.processes.set_state(child_pid, ProcessState::Running)?;

        // For now, mark as Running immediately (bootstrap path: the caller
        // has already set up the address space and threads externally).
        let _ = (
            code_start, code_end, data_start, data_end,
            stack_start, stack_end, heap_start,
        );

        let child = self
            .processes
            .get_mut(child_pid)
            .ok_or(ProcError::InvalidProcess)?;
        child.state = ProcessState::Running;

        Ok(child_pid)
    }

    // ------------------------------------------------------------------
    // Process exit
    // ------------------------------------------------------------------

    /// Handle a process exit.
    ///
    /// # What happens
    ///
    /// 1. Validate the process exists and is in an exit-able state.
    /// 2. Transition to `Dying`.
    /// 3. Destroy all threads belonging to this process.
    /// 4. Request mm-server to release the address space.
    /// 5. Notify parent via `SIGCHLD`.
    /// 6. If parent has already exited, mark as `Zombie` for reaping;
    ///    otherwise remove from the table.
    /// 7. If this was the last process (init exited), halt the system.
    pub fn exit(&mut self, pid: ProcessId, exit_code: i32) -> ProcResult<()> {
        let info = self
            .processes
            .get(pid)
            .ok_or(ProcError::InvalidProcess)?;

        if !info.state.is_alive() {
            return Err(ProcError::InvalidState);
        }

        log::info!(
            "proc: exit {:?} \"{}\" code={}",
            pid,
            info.name_str(),
            exit_code,
        );

        // 1. Transition to Dying.
        self.processes.set_state(pid, ProcessState::Dying)?;

        // 2. Collect thread IDs (to avoid borrow issues during iteration).
        let thread_ids: ThreadIdArray = {
            let info = self.processes.get(pid).unwrap();
            info.thread_ids().collect::<ThreadIdArray>()
        };

        // 3. Destroy all threads.
        for tid in thread_ids.iter() {
            if self.task_mgr.task_exists(*tid) {
                let _ = self.task_mgr.destroy_task(*tid);
            }
        }

        // 4. TODO: Request mm-server to release the address space.
        // mm_server_release_address_space(self.mm_channel, addr_space_id)?;

        // 5. Notify parent.
        let parent_pid = {
            let info = self.processes.get(pid).unwrap();
            info.parent
        };

        if !parent_pid.is_null() {
            if let Some(parent_info) = self.processes.get(parent_pid) {
                if parent_info.state.can_receive_signal() {
                    // TODO: deliver SIGCHLD to parent via its IPC channel.
                    log::debug!(
                        "proc: SIGCHLD → {:?} (child {:?} exited)",
                        parent_pid,
                        pid,
                    );
                }
            }
        }

        // 6. Remove the process or leave as Zombie.
        let is_zombie = !parent_pid.is_null()
            && self
                .processes
                .get(parent_pid)
                .is_some_and(|p| p.state.is_alive());

        if is_zombie {
            // Parent is still alive — mark as Zombie for parent to reap.
            let info = self.processes.get_mut(pid).unwrap();
            info.state = ProcessState::Zombie;
            info.exit_code = Some(exit_code);
            // Clear the thread list.
            info.threads = [const { None }; MAX_THREADS_PER_PROCESS];
            info.thread_count = 0;
        } else {
            // Parent is dead or NULL (init) — remove outright.
            self.processes.remove(pid);

            // If init exited, the system should halt.
            if pid.index() == 0 && pid.generation() == 1 {
                log::warn!("proc: init process exited — system halt");
                // TODO: trigger system shutdown.
            }
        }

        Ok(())
    }

    /// Force-kill a process (SIGKILL).
    ///
    /// Equivalent to `exit()` but with signal semantics — no graceful
    /// teardown, no handler invocation.
    pub fn kill(&mut self, pid: ProcessId) -> ProcResult<()> {
        // In this minimal implementation, kill = exit with code -9.
        self.exit(pid, -9)
    }

    /// Reap a zombie process (called by parent via `wait`).
    ///
    /// Returns the exit code of the reaped child.
    pub fn reap(&mut self, parent: ProcessId, child: ProcessId) -> ProcResult<i32> {
        let child_info = self
            .processes
            .get(child)
            .ok_or(ProcError::InvalidProcess)?;

        if child_info.state != ProcessState::Zombie {
            return Err(ProcError::InvalidState);
        }
        if child_info.parent != parent {
            return Err(ProcError::PermissionDenied);
        }

        let exit_code = child_info.exit_code.unwrap_or(0);
        self.processes.remove(child);

        log::info!(
            "proc: reaped {:?} (parent={:?}, exit_code={})",
            child,
            parent,
            exit_code,
        );

        Ok(exit_code)
    }

    // ------------------------------------------------------------------
    // Signals
    // ------------------------------------------------------------------

    /// Send a signal to a process.
    ///
    /// # Default actions
    ///
    /// * `Terminate` → call `exit(pid, -signal_number)`.
    /// * `Stop`     → set state to `Stopped` (if alive).
    /// * `Continue`  → set state back to `Running` (if `Stopped`).
    /// * `Ignore`    → no-op.
    ///
    /// # Future
    ///
    /// Per-process signal handlers (registered via `sigaction`) will
    /// replace the default action for catchable signals.
    pub fn send_signal(&mut self, target: ProcessId, signal: Signal) -> ProcResult<()> {
        let info = self
            .processes
            .get(target)
            .ok_or(ProcError::InvalidProcess)?;

        if !info.state.can_receive_signal() {
            // Silently drop signals to processes that can't receive them
            // (Created, Dying, Zombie) — POSIX-conformant behaviour.
            log::debug!(
                "proc: dropped signal {:?} to {:?} (state={:?})",
                signal,
                target,
                info.state,
            );
            return Ok(());
        }

        log::debug!("proc: signal {:?} → {:?}", signal, target);

        match signal.default_action() {
            SignalAction::Terminate => {
                let sig_num = signal as i32;
                self.exit(target, -sig_num)?;
            }
            SignalAction::Stop => {
                self.processes.set_state(target, ProcessState::Stopped)?;
            }
            SignalAction::Continue => {
                if info.state == ProcessState::Stopped {
                    self.processes.set_state(target, ProcessState::Running)?;
                }
            }
            SignalAction::Ignore => {
                // No action.
            }
        }

        Ok(())
    }

    // ------------------------------------------------------------------
    // Priority policy — the "Planner" role
    // ------------------------------------------------------------------

    /// Set the priority of a process and propagate to all its threads.
    ///
    /// This is the policy entry-point: `ProcServer` decides the base
    /// priority for every thread belonging to this process.
    pub fn set_priority(
        &mut self,
        target: ProcessId,
        priority: ProcessPriority,
    ) -> ProcResult<()> {
        let info = self
            .processes
            .get_mut(target)
            .ok_or(ProcError::InvalidProcess)?;

        info.priority = priority;

        // Propagate to all threads.
        let thread_ids: ThreadIdArray = info.thread_ids().collect();
        drop(thread_ids); // release borrow on `info`

        // Re-borrow to update threads.
        let info = self
            .processes
            .get_mut(target)
            .ok_or(ProcError::InvalidProcess)?;

        for tid in info.thread_ids() {
            // The TaskManager's set_priority updates the kernel TCB.
            let task_prio = TaskPriority(priority.0);
            let _ = self.task_mgr.set_priority(tid, task_prio);
        }

        log::info!(
            "proc: set priority of {:?} to {}",
            target,
            priority.0,
        );

        Ok(())
    }

    /// Compute the default priority for a process based on its name.
    ///
    /// This is the "Planner" logic from `sche/README.md` — the Process
    /// Server decides who gets what priority based on the process role.
    ///
    /// # Convention
    ///
    /// * Names starting with `mm-`, `proc-`, `init`, `cap-` → `SYSTEM`
    /// * Names starting with `fs-`, `net-`, `drv-` → `SERVER`
    /// * Everything else → `USER`
    pub fn compute_default_priority(&self, name: &[u8]) -> ProcessPriority {
        if name.starts_with(b"init")
            || name.starts_with(b"mm-")
            || name.starts_with(b"proc-")
            || name.starts_with(b"cap-")
        {
            ProcessPriority::SYSTEM
        } else if name.starts_with(b"fs-")
            || name.starts_with(b"net-")
            || name.starts_with(b"drv-")
        {
            ProcessPriority::SERVER
        } else {
            ProcessPriority::USER
        }
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Get process information.
    pub fn process_info(&self, pid: ProcessId) -> ProcResult<&ProcessInfo> {
        self.processes.get(pid).ok_or(ProcError::InvalidProcess)
    }

    /// Return the total number of processes.
    pub fn process_count(&self) -> usize {
        self.processes.len()
    }

    /// List all process IDs.
    pub fn list_processes(&self) -> impl Iterator<Item = ProcessId> + '_ {
        self.processes.iter().map(|info| info.pid)
    }

    /// Return the children of a process.
    pub fn children_of(&self, parent: ProcessId) -> ProcResult<ArrayVec<ProcessId, { MAX_PROCESSES }>> {
        if !parent.is_null() && self.processes.get(parent).is_none() {
            return Err(ProcError::InvalidProcess);
        }
        Ok(self.processes.children_of(parent).collect())
    }

    // ------------------------------------------------------------------
    // IPC request dispatch
    // ------------------------------------------------------------------

    /// Decode and dispatch a single IPC request.
    ///
    /// The request is encoded as a `ShortPayload` where:
    ///
    /// * `words[0]` — opcode (`ProcRequestOp`)
    /// * `words[1..]` — per-variant payload
    ///
    /// After processing, the caller can query `process_info()` or
    /// `list_processes()` to build a reply message.
    pub fn handle_request(&mut self, request: &ProcRequest) -> ProcResult<()> {
        match *request {
            ProcRequest::Spawn {
                parent,
                ref name,
                name_len: _,
                code_start,
                code_end,
                data_start,
                data_end,
                stack_start,
                stack_end,
                heap_start,
            } => {
                let name_slice = &name[..name.len().min(32)];
                let _child_pid = self.spawn(
                    parent,
                    name_slice,
                    code_start,
                    code_end,
                    data_start,
                    data_end,
                    stack_start,
                    stack_end,
                    heap_start,
                )?;
                // Caller reads the new child PID via children_of() or
                // the last-inserted process entry.
                Ok(())
            }
            ProcRequest::Exit { pid, exit_code } => {
                self.exit(pid, exit_code)?;
                Ok(())
            }
            ProcRequest::Signal { target, signal } => {
                self.send_signal(target, signal)?;
                Ok(())
            }
            ProcRequest::SetPriority { target, priority } => {
                self.set_priority(target, priority)?;
                Ok(())
            }
            ProcRequest::Query { target } => {
                let _info = self.process_info(target)?;
                Ok(())
            }
            ProcRequest::Register {
                pid,
                addr_space_id,
                ref name,
                name_len: _,
                priority,
                parent,
            } => {
                let registered_pid =
                    self.register_process(parent, addr_space_id, &name[..name.len().min(32)], priority)?;
                if !pid.is_null() && registered_pid != pid {
                    log::warn!(
                        "proc: Register requested {:?} but got {:?}",
                        pid,
                        registered_pid,
                    );
                }
                Ok(())
            }
            ProcRequest::List => {
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SmallVec helpers — avoid pulling in the `smallvec` crate for tiny arrays.
// ---------------------------------------------------------------------------

/// A tiny inline-array type for collecting thread IDs (max 8 per process).
///
/// In production you'd use `smallvec::SmallVec<[TaskId; MAX_THREADS_PER_PROCESS]>`.
/// Here we use a fixed-capacity `Vec` emulation backed by a stack array to
/// keep the dependency footprint minimal.
pub type ThreadIdArray = ArrayVec<TaskId, MAX_THREADS_PER_PROCESS>;

pub struct ArrayVec<T, const N: usize> {
    data: [Option<T>; N],
    len: usize,
}

impl<T, const N: usize> ArrayVec<T, N> {
    fn new() -> Self {
        ArrayVec {
            data: [const { None }; N],
            len: 0,
        }
    }

    fn push(&mut self, item: T) {
        if self.len < N {
            self.data[self.len] = Some(item);
            self.len += 1;
        }
    }

    fn iter(&self) -> impl Iterator<Item = &T> {
        self.data[..self.len].iter().filter_map(|opt| opt.as_ref())
    }
}

impl<T, const N: usize> FromIterator<T> for ArrayVec<T, N> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut av = ArrayVec::new();
        for item in iter {
            av.push(item);
        }
        av
    }
}

// Needed for `IntoIterator` on owned `ArrayVec`.
impl<T, const N: usize> IntoIterator for ArrayVec<T, N> {
    type Item = T;
    type IntoIter = ArrayVecIter<T, N>;

    fn into_iter(self) -> Self::IntoIter {
        ArrayVecIter { inner: self, cursor: 0 }
    }
}

pub struct ArrayVecIter<T, const N: usize> {
    inner: ArrayVec<T, N>,
    cursor: usize,
}

impl<T, const N: usize> Iterator for ArrayVecIter<T, N> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        while self.cursor < self.inner.len {
            let item = self.inner.data[self.cursor].take();
            self.cursor += 1;
            if item.is_some() {
                return item;
            }
        }
        None
    }
}
