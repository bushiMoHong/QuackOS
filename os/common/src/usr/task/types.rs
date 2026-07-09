//! Task / thread related types for user-space.
//!
//! These types provide a user-space-friendly vocabulary for operating on
//! kernel threads.  They wrap the kernel `sche` primitives without exposing
//! internal details like raw `ThreadId` layout or TCB field offsets.

use crate::kernel::ipc::message::ProcessId;
use crate::kernel::sche::{ThreadId, ThreadState};

// ---------------------------------------------------------------------------
// TaskId — user-space handle for a kernel thread
// ---------------------------------------------------------------------------

/// User-space identifier for a schedulable task (thread).
///
/// `TaskId` wraps the kernel's `ThreadId`.  User-space code should never
/// inspect or construct a `ThreadId` directly — `TaskId` provides the
/// public API boundary.
///
/// `TaskId::NULL` (value 0) is never allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskId(pub ThreadId);

impl TaskId {
    /// The null / invalid task ID.  Never allocated by the kernel.
    pub const NULL: TaskId = TaskId(ThreadId::NULL);

    /// Return `true` if this is the null ID.
    #[inline]
    pub fn is_null(self) -> bool {
        self.0.is_null()
    }

    /// Extract the slot index (for debugging).
    #[inline]
    pub fn index(self) -> u16 {
        self.0.index()
    }

    /// Extract the generation (for debugging).
    #[inline]
    pub fn generation(self) -> u16 {
        self.0.generation()
    }
}

// ---------------------------------------------------------------------------
// TaskPriority
// ---------------------------------------------------------------------------

/// Task scheduling priority.
///
/// Range: 0 (lowest) – 255 (highest), matching the kernel's `ReadyQueue`
/// priority scheme.
///
/// # Convention
///
/// | Range   | Class        |
/// |---------|--------------|
/// | 200–255 | System       |
/// | 128–199 | Server       |
/// | 64–127  | User         |
/// | 0–63    | Background   |
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskPriority(pub u8);

impl TaskPriority {
    /// Default priority for new tasks.
    pub const DEFAULT: TaskPriority = TaskPriority(128);

    /// Minimum possible priority.
    pub const MIN: TaskPriority = TaskPriority(0);

    /// Maximum possible priority.
    pub const MAX: TaskPriority = TaskPriority(255);

    /// System-class floor (e.g. init, proc-server, mm-server).
    pub const SYSTEM: TaskPriority = TaskPriority(200);

    /// Server-class default.
    pub const SERVER: TaskPriority = TaskPriority(150);

    /// User-class default.
    pub const USER: TaskPriority = TaskPriority(100);

    /// Background / idle class.
    pub const BACKGROUND: TaskPriority = TaskPriority(50);

    /// Return `true` if the priority is in the valid range.
    /// Always `true` for `u8`-backed priorities (0–255 maps directly).
    #[inline]
    pub fn is_valid(self) -> bool {
        true // u8 is always in [0, 255]
    }
}

// ---------------------------------------------------------------------------
// TaskState — user-space view of thread lifecycle
// ---------------------------------------------------------------------------

/// User-space view of a task's lifecycle state.
///
/// This is a semantic re-export of the kernel's `ThreadState` with
/// user-friendly variant names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Task is runnable and waiting in the ready queue.
    Ready,
    /// Task is currently executing on a CPU.
    Running,
    /// Task is blocked (on IPC, timer, notification, …).
    Blocked,
    /// Task is in the process of being destroyed.
    Dying,
    /// Slot is unused.
    Free,
}

impl From<ThreadState> for TaskState {
    fn from(s: ThreadState) -> Self {
        match s {
            ThreadState::Free => TaskState::Free,
            ThreadState::Ready => TaskState::Ready,
            ThreadState::Running => TaskState::Running,
            ThreadState::Blocked => TaskState::Blocked,
            ThreadState::Dying => TaskState::Dying,
        }
    }
}

impl From<TaskState> for ThreadState {
    fn from(s: TaskState) -> Self {
        match s {
            TaskState::Free => ThreadState::Free,
            TaskState::Ready => ThreadState::Ready,
            TaskState::Running => ThreadState::Running,
            TaskState::Blocked => ThreadState::Blocked,
            TaskState::Dying => ThreadState::Dying,
        }
    }
}

// ---------------------------------------------------------------------------
// TaskInfo — snapshot of task metadata
// ---------------------------------------------------------------------------

/// Immutable snapshot of a task's metadata at a point in time.
///
/// Obtained via `TaskManager::task_info()`.  The snapshot avoids holding
/// the thread-table lock across user-space decision-making.
#[derive(Debug, Clone)]
pub struct TaskInfo {
    /// Task identifier.
    pub id: TaskId,
    /// Owning process (ProcessId::NULL if kernel-internal thread).
    pub owner_pid: ProcessId,
    /// Effective scheduling priority.
    pub priority: TaskPriority,
    /// Base priority (before any donation).
    pub base_priority: TaskPriority,
    /// Current lifecycle state.
    pub state: TaskState,
    /// Address-space identifier (for TLB tagging).
    pub asid: usize,
}

// ---------------------------------------------------------------------------
// TaskError
// ---------------------------------------------------------------------------

/// Errors returned by user-space task operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskError {
    /// The given `TaskId` does not reference a valid task.
    InvalidTask,
    /// The global thread table is full.
    TableFull,
    /// The priority value is out of range.
    InvalidPriority,
    /// The task is in an unexpected state for the operation.
    InvalidState,
    /// `TaskId::NULL` was passed where a real task was required.
    NullTaskId,
    /// A bad argument was supplied.
    InvalidArgument,
    /// The requested operation is not yet implemented.
    NotImplemented,
}

/// Convenience alias.
pub type TaskResult<T> = Result<T, TaskError>;

// ---------------------------------------------------------------------------
// Conversion helpers — map kernel errors to user-space errors
// ---------------------------------------------------------------------------

/// Map a kernel `ScheError` to a user-space `TaskError`.
///
/// This function lives here (rather than in `manager.rs`) so it can be
/// shared across multiple user-space modules that interact with the
/// scheduler.
pub fn map_sche_error(e: crate::kernel::sche::ScheError) -> TaskError {
    match e {
        crate::kernel::sche::ScheError::InvalidThread => TaskError::InvalidTask,
        crate::kernel::sche::ScheError::ThreadTableFull => TaskError::TableFull,
        crate::kernel::sche::ScheError::PriorityQueueFull => TaskError::TableFull,
        crate::kernel::sche::ScheError::InvalidThreadState => TaskError::InvalidState,
        crate::kernel::sche::ScheError::NoRunnableThread => TaskError::InvalidState,
        crate::kernel::sche::ScheError::NullThreadId => TaskError::NullTaskId,
        crate::kernel::sche::ScheError::InvalidArgument => TaskError::InvalidArgument,
        crate::kernel::sche::ScheError::NotImplemented => TaskError::NotImplemented,
    }
}
