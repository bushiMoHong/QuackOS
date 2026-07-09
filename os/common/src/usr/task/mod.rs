//! User-space task / thread management helpers.
//!
//! # Architecture (microkernel)
//!
//! ```text
//! ┌──────────────────────────────────────────────────────┐
//! │ Kernel (sche)                                        │
//! │                                                      │
//! │  • Thread creation / destruction                     │
//! │  • ReadyQueue, context switching                     │
//! │  • Atomic state transitions                          │
//! │                                                      │
//! │  Role: Mechanism provider                            │
//! └────────────────────────┬─────────────────────────────┘
//!                           │
//!                           │ sche::create_thread()
//!                           │ sche::schedule()
//!                           │ sche::block_current()
//!                           ▼
//! ┌──────────────────────────────────────────────────────┐
//! │ User-space (task)                                    │
//! │                                                      │
//! │  • Type-safe API (TaskId, TaskPriority, TaskState)   │
//! │  • User-friendly error types (TaskError)             │
//! │  • Snapshot queries (TaskInfo)                       │
//! │                                                      │
//! │  Role: Convenience / type-safety layer               │
//! └──────────────────────────────────────────────────────┘
//! ```
//!
//! # Sub-modules
//!
//! | Module      | Purpose                                         |
//! |-------------|-------------------------------------------------|
//! | `types`     | `TaskId`, `TaskPriority`, `TaskState`, `TaskInfo`, `TaskError` |
//! | `manager`   | `TaskManager` — lifecycle, priority, queries    |
//!
//! # Relationship to `usr/proc`
//!
//! `usr/task` is a **library**, not a server.  It provides building blocks
//! that `usr/proc` (the Process Server) uses to manage threads on behalf
//! of processes.  End-user processes should go through `usr/proc` rather
//! than calling `usr/task` directly — the Process Server is the policy
//! authority for thread creation.
//!
//! ```text
//! User process
//!      │
//!      │ IPC: ProcRequest::Spawn
//!      ▼
//! usr/proc (ProcServer)    ← policy: "who can create threads, at what priority"
//!      │
//!      │ usr/task::TaskManager::create_task()
//!      ▼
//! kernel::sche              ← mechanism: CPU scheduling
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use usr::task::{TaskManager, TaskPriority};
//!
//! let tm = TaskManager::new();
//!
//! // Who am I?
//! let me = tm.current_task();
//!
//! // Create a child task.
//! let child = tm.create_task(
//!     TaskPriority::USER,
//!     stack_base, stack_top,
//!     ttbr0, asid, owner_pid,
//! )?;
//!
//! // Query its state.
//! let info = tm.task_info(child)?;
//! assert_eq!(info.priority, TaskPriority::USER);
//!
//! // Tear it down.
//! tm.destroy_task(child)?;
//! ```

pub mod manager;
pub mod types;

// Re-export everything public so users only need `use usr::task`.
pub use manager::TaskManager;
pub use types::{
    map_sche_error, TaskError, TaskId, TaskInfo, TaskPriority, TaskResult, TaskState,
};
