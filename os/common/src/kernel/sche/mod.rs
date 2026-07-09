//! Kernel scheduler (`sche`) — the executor.
//!
//! # Architecture (microkernel)
//!
//! ```text
//! ┌──────────────────────────────────────────────────────┐
//! │ Kernel (sche)                                        │
//! │                                                      │
//! │  • Physical CPU allocation                           │
//! │  • Execution context switching                       │
//! │  • Only sees "Thread" — the schedulable execution flow│
//! │                                                      │
//! │  Role: Executor — dispatches by priority             │
//! └──────────────────────────────────────────────────────┘
//!
//! ┌──────────────────────────────────────────────────────┐
//! │ User-space (proc server)                             │
//! │                                                      │
//! │  • Logical process management                        │
//! │  • Creation, termination, naming, permissions        │
//! │  • Priority policy decisions                         │
//! │                                                      │
//! │  Role: Planner — decides who gets what priority      │
//! └──────────────────────────────────────────────────────┘
//! ```
//!
//! # Sub-modules
//!
//! | Module      | Purpose                                         |
//! |-------------|-------------------------------------------------|
//! | `thread`    | `ThreadId` (generational), `Thread` (TCB), table |
//! | `queue`     | O(1) bitmap-based `ReadyQueue`                   |
//! | `context`   | `schedule()`, `block_current()`, `wake()`        |
//! | `error`     | `ScheError` enum                                 |
//!
//! # Integration with IPC
//!
//! The IPC subsystem (`kernel::ipc`) delegates its synchronization
//! primitives here:
//!
//! ```text
//! ipc::sys_ipc_send(…)
//!   → ipc::synchronization::block_current(state)
//!     → sche::block_current(state)
//!       → sche::context::schedule()
//!         → sche::queue::dequeue_ready()
//!         → __switch()
//! ```
//!
//! # Usage from kernel binary
//!
//! ```ignore
//! use common::kernel::sche;
//!
//! // Create the boot thread.
//! let tid = sche::create_thread(128, stack_base, stack_top, ttbr0, asid)?;
//! unsafe { sche::bootstrap_idle(tid); }
//! ```

pub mod context;
pub mod error;
pub mod queue;
pub mod thread;

// Re-export everything public so users only need `use kernel::sche`.
pub use context::{block_current, bootstrap_idle, current_thread, schedule, wake};
pub use error::ScheError;
pub use queue::{dequeue_ready, enqueue_ready, is_ready_empty, runnable_count, DEFAULT_PRIORITY, MAX_PRIORITY, NUM_PRIORITIES};
pub use thread::{create_thread, destroy_thread, thread_count, thread_exists, with_thread, with_thread_mut, kernel_stack_top, Thread, ThreadId, ThreadState};

// Re-export IpcState from IPC so the sche module can use it without
// circular dependency — sche::IpcState is the canonical name.
pub use crate::kernel::ipc::synchronization::IpcState;

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the scheduler subsystem.
///
/// # What it does
///
/// 1. Logs initialisation.
/// 2. (Future) Initialises per-CPU data structures for SMP.
/// 3. (Future) Creates the idle thread.
///
/// Must be called once during kernel boot, before any thread operations.
pub fn init() {
    log::info!("sche: subsystem initialised ({} priorities, {} max threads)",
        NUM_PRIORITIES,
        thread::MAX_THREADS,
    );
}
