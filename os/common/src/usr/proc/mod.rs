//! User-space Process Manager (proc) — the policy-side of QuackOS's microkernel
//! process subsystem.
//!
//! # Architecture
//!
//! ```text
//!          User processes (Ring 3)
//!                │
//!                │ Spawn / Exit / Signal (IPC channel)
//!                ▼
//!   ┌────────────────────────┐
//!   │  ProcServer (Ring 3)   │  ← policy decision-maker (this module)
//!   │                        │
//!   │  ProcessTable ─── per-process metadata (state, threads, priority)
//!   │  PriorityPolicy ─ decides base priorities by process role
//!   │  SignalDispatch ─ POSIX-style signal delivery
//!   └────────┬───────────────┘
//!            │ Create / Destroy / SetPriority
//!            ▼
//!   ┌────────────────────────┐
//!   │  usr::task (Ring 3)    │  ← type-safe thread management
//!   └────────┬───────────────┘
//!            │
//!            │ syscall (sche::create_thread, …)
//!            ▼
//!   ┌────────────────────────┐
//!   │  Kernel sche (Ring 0)  │  ← mechanism provider
//!   │  Threads, ReadyQueue   │
//!   └────────────────────────┘
//!
//!   ProcServer also coordinates with:
//!   ┌────────────────────────┐
//!   │  MmServer              │  ← address-space creation / teardown
//!   │  Capability system     │  ← permission checks, capability grants
//!   └────────────────────────┘
//! ```
//!
//! # Sub-modules
//!
//! | Module       | Purpose                                          |
//! |--------------|--------------------------------------------------|
//! | `types`      | `ProcessId`, `Signal`, `ProcError`, `ProcRequest` |
//! | `proc_table` | `ProcessInfo`, `ProcessTable` — the process DB   |
//! | `server`     | `ProcServer` — IPC event loop, spawn/exit/signal  |
//!
//! # Relationship to the kernel
//!
//! The kernel (`sche`) only sees threads — it has no concept of a "process."
//! `ProcServer` builds processes on top:
//!
//! * **Process** = address space + thread set + capability set + name + priority
//! * The kernel's `ProcessId` (currently `u32`) is just an opaque token
//! * `ProcServer` owns the generational `ProcessId` namespace
//!
//! # Usage
//!
//! The Process Server runs as a standalone user-space process:
//!
//! ```ignore
//! use usr::proc::{ProcServer, ProcessPriority};
//!
//! // 1. Create the server on its well-known IPC channels.
//! let mut server = ProcServer::new(proc_channel, mm_channel);
//!
//! // 2. Bootstrap with init and pre-existing system servers.
//! server.bootstrap(init_asid, &[
//!     (mm_asid, b"mm-server", ProcessPriority::SYSTEM),
//! ])?;
//!
//! // 3. Enter the main event loop.
//! // loop {
//! //     let msg = sys_ipc_recv(server_pid, proc_channel);
//! //     let request = decode(msg);
//! //     server.handle_request(request);
//! // }
//! ```

pub mod proc_table;
pub mod server;
pub mod types;
pub mod elf_loader;

// Re-export the public API surface.
pub use proc_table::{ProcessInfo, ProcessTable, MAX_PROCESSES, MAX_THREADS_PER_PROCESS};
pub use server::ProcServer;
pub use types::{
    ProcError, ProcReplyOp, ProcRequest, ProcRequestOp, ProcResult, ProcessId, ProcessPriority,
    ProcessState, Signal, SignalAction,
};
