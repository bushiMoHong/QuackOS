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
pub use thread::{create_thread, destroy_thread, thread_count, thread_exists, with_all_threads, with_thread, with_thread_mut, kernel_stack_top, set_kernel_stack_top, tcb_ptr, Thread, ThreadId, ThreadState};

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
/// 1. Creates a TCB for the boot thread so it can participate in scheduling.
/// 2. Bootstraps the boot thread as current.
/// 3. Sets x19 to point to the boot thread's TCB (required by `__switch`).
///
/// Must be called once during kernel boot, before any thread operations.
pub fn init() {
    log::info!("sche: subsystem initialised ({} priorities, {} max threads)",
        NUM_PRIORITIES,
        thread::MAX_THREADS,
    );

    // The boot thread (FsServer) runs on the boot stack defined in
    // boot_arm64.S (256 KB in .bss.stack).  Register it as the boot
    // thread's kernel stack so the TCB tracks the correct base address
    // and the sp validation passes.
    extern "C" {
        static boot_stack: u8;
        static boot_stack_top: u8;
    }
    let boot_stack_base = unsafe { &boot_stack as *const u8 as usize };
    let boot_stack_top_addr = unsafe { &boot_stack_top as *const u8 as usize };

    let tid = unsafe {
        thread::create_thread(
            128,                    // default priority
            boot_stack_base,        // kernel_stack_base = boot stack
            boot_stack_top_addr,    // kernel_stack_top  = top of boot stack
            0,                      // ttbr0
            0,                      // asid
        )
    }.expect("Failed to create boot thread TCB");

    unsafe {
        bootstrap_idle(tid);
    }
}
