//! TaskManager — user-space task lifecycle and query operations.
//!
//! `TaskManager` is a **zero-state struct**: it holds no data of its own.
//! All authoritative state lives in the kernel's `sche::ThreadTable`.
//! Every method here is a thin, type-safe wrapper around a kernel `sche`
//! operation, translating kernel error types into user-space `TaskError`.
//!
//! # Design
//!
//! ```text
//! User-space code
//!       │
//!       │ TaskManager::create_task(…)
//!       ▼
//!   usr::task::manager       ← type-safe API, user-friendly errors
//!       │
//!       │ sche::create_thread(…)
//!       ▼
//!   kernel::sche             ← mechanism provider (ThreadId, TCB, ReadyQueue)
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use usr::task::{TaskManager, TaskPriority};
//!
//! let tm = TaskManager::new();
//!
//! // Create a new task.
//! let tid = tm.create_task(
//!     TaskPriority::USER,
//!     stack_base, stack_top,
//!     ttbr0, asid,
//! )?;
//!
//! // Query task info.
//! let info = tm.task_info(tid)?;
//!
//! // Destroy the task when done.
//! tm.destroy_task(tid)?;
//! ```

use super::types::*;
use crate::kernel::ipc::message::ProcessId;
use crate::kernel::sche;

// ---------------------------------------------------------------------------
// TaskManager
// ---------------------------------------------------------------------------

/// User-space task manager.
///
/// A zero-size struct — all state is kernel-resident.  Multiple
/// `TaskManager` instances are indistinguishable; the type exists
/// only to namespace the operations.
pub struct TaskManager;

impl TaskManager {
    /// Create a `TaskManager`.
    pub const fn new() -> Self {
        TaskManager
    }

    // ------------------------------------------------------------------
    // Lifecycle
    // ------------------------------------------------------------------

    /// Create a new task (kernel thread) and return its `TaskId`.
    ///
    /// # Arguments
    ///
    /// * `priority`          — scheduling priority (0–255).
    /// * `kernel_stack_base` — base address of the new task's kernel stack.
    /// * `kernel_stack_top`  — initial stack pointer (must point to a valid
    ///                         `TaskContext` frame on that stack).
    /// * `ttbr0`             — page-table root token for the task's address
    ///                         space (TTBR0_EL1 value on AArch64).
    /// * `asid`              — address-space identifier (TLB tag).
    /// * `owner_pid`         — owning process (for accounting; pass
    ///                         `ProcessId::NULL` for kernel threads).
    ///
    /// # Safety
    ///
    /// The caller must ensure that `kernel_stack_base` / `kernel_stack_top`
    /// reference a valid, exclusively-owned kernel stack and that `ttbr0`
    /// is a valid page-table root.
    ///
    /// # Errors
    ///
    /// * `TaskError::TableFull` — the global thread table is exhausted.
    pub fn create_task(
        &self,
        priority: TaskPriority,
        kernel_stack_base: usize,
        kernel_stack_top: usize,
        ttbr0: usize,
        asid: usize,
        _owner_pid: ProcessId,
    ) -> TaskResult<TaskId> {
        // SAFETY: delegated to the caller — see above.
        unsafe {
            sche::create_thread(
                priority.0,
                kernel_stack_base,
                kernel_stack_top,
                ttbr0,
                asid,
            )
            .map(TaskId)
            .map_err(map_sche_error)
        }
    }

    /// Destroy a task and free its slot in the thread table.
    ///
    /// The task must be in `Free` or `Dying` state.  Its kernel stack is
    /// **not** deallocated by this call — that is the caller's responsibility.
    ///
    /// # Errors
    ///
    /// * `TaskError::InvalidTask` — the task does not exist.
    /// * `TaskError::NullTaskId`  — `TaskId::NULL` was passed.
    pub fn destroy_task(&self, task: TaskId) -> TaskResult<()> {
        sche::destroy_thread(task.0).map_err(map_sche_error)
    }

    // ------------------------------------------------------------------
    // Priority
    // ------------------------------------------------------------------

    /// Set the base priority of a task.
    ///
    /// The effective priority is `max(base_priority, donated_priority)`.
    /// If the task is currently in the ready queue at a different priority,
    /// the caller must additionally remove and re-insert it (priority-donation
    /// logic, handled by IPC).
    ///
    /// # Errors
    ///
    /// * `TaskError::InvalidTask` — the task does not exist.
    pub fn set_priority(&self, task: TaskId, priority: TaskPriority) -> TaskResult<()> {
        sche::with_thread_mut(task.0, |t| {
            t.base_priority = priority.0;
            // If no donation is active, also update the effective priority.
            if t.donated_priority == 0 {
                t.priority = priority.0;
            } else {
                t.priority = t.base_priority.max(t.donated_priority);
            }
        })
        .map_err(map_sche_error)
    }

    /// Get the effective scheduling priority of a task.
    ///
    /// # Errors
    ///
    /// * `TaskError::InvalidTask` — the task does not exist.
    pub fn get_priority(&self, task: TaskId) -> TaskResult<TaskPriority> {
        sche::with_thread(task.0, |t| TaskPriority(t.effective_priority()))
            .map_err(map_sche_error)
    }

    /// Get the base priority of a task (before any donation).
    ///
    /// # Errors
    ///
    /// * `TaskError::InvalidTask` — the task does not exist.
    pub fn get_base_priority(&self, task: TaskId) -> TaskResult<TaskPriority> {
        sche::with_thread(task.0, |t| TaskPriority(t.base_priority))
            .map_err(map_sche_error)
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Return the `TaskId` of the currently executing task.
    ///
    /// Returns `TaskId::NULL` only during early boot, before the
    /// scheduler is initialised.
    pub fn current_task(&self) -> TaskId {
        TaskId(sche::current_thread())
    }

    /// Get a snapshot of task metadata.
    ///
    /// The snapshot is a copy — it does not hold the thread-table lock
    /// after returning.
    ///
    /// # Errors
    ///
    /// * `TaskError::InvalidTask` — the task does not exist.
    pub fn task_info(&self, task: TaskId) -> TaskResult<TaskInfo> {
        sche::with_thread(task.0, |t| {
            TaskInfo {
                id: task,
                owner_pid: 0, // TODO: stored in TCB, not yet plumbed
                priority: TaskPriority(t.effective_priority()),
                base_priority: TaskPriority(t.base_priority),
                state: TaskState::from(t.atomic_state()),
                asid: t.asid,
            }
        })
        .map_err(map_sche_error)
    }

    /// Return `true` if a task with the given ID exists.
    pub fn task_exists(&self, task: TaskId) -> bool {
        sche::thread_exists(task.0)
    }

    /// Return the total number of allocated tasks.
    pub fn task_count(&self) -> usize {
        sche::thread_count()
    }

    /// Return the number of runnable tasks (in the ready queue).
    pub fn runnable_count(&self) -> usize {
        sche::runnable_count()
    }

    // ------------------------------------------------------------------
    // Scheduling control
    // ------------------------------------------------------------------

    /// Voluntarily yield the CPU.
    ///
    /// The calling task is placed back on the ready queue (at its current
    /// effective priority) and the scheduler picks the next runnable task.
    ///
    /// # Safety
    ///
    /// Must only be called from kernel mode or via a syscall that
    /// transitions to kernel mode.  No spin locks may be held.
    pub fn yield_now(&self) {
        sche::schedule();
    }

    /// Block the current task until it is woken by `wake()`.
    ///
    /// The task is removed from the ready queue and its state is set to
    /// `Blocked`.  When `wake()` is called (typically by an IPC partner
    /// or interrupt handler), the task is re-enqueued and resumes
    /// execution after this call returns.
    ///
    /// # Safety
    ///
    /// * All locks must be released before calling.
    /// * The task must have a valid kernel stack to which it can return.
    pub unsafe fn block_current(&self, ipc_state: crate::kernel::ipc::IpcState) {
        sche::block_current(ipc_state);
    }

    /// Wake a blocked task and enqueue it on the ready queue.
    ///
    /// # Precondition
    ///
    /// Any IPC data must already have been delivered to the target's
    /// receive buffer before calling `wake()`.
    pub fn wake_task(&self, task: TaskId) {
        sche::wake(task.0);
    }

    // ------------------------------------------------------------------
    // Kernel stack access (for bootstrap / debug)
    // ------------------------------------------------------------------

    /// Return the kernel stack top pointer for a task.
    ///
    /// This is the value that `__switch` reads from the TCB at offset 0.
    ///
    /// # Panics
    ///
    /// Panics if the task does not exist — this is a hot-path function
    /// that assumes validity (caller must check `task_exists()` first).
    pub fn kernel_stack_top(&self, task: TaskId) -> usize {
        sche::kernel_stack_top(task.0)
    }
}
