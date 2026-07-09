//! User-space Memory Manager (mm) — the policy-side of QuackOS's microkernel
//! memory subsystem.
//!
//! # Architecture
//!
//! ```text
//!          User processes (Ring 3)
//!                │
//!                │ page fault (MMU trap)
//!                ▼
//!   ┌────────────────────────┐
//!   │  Kernel bmm (Ring 0)   │  ← mechanism provider
//!   │  catch #PF → IPC msg   │
//!   └────────┬───────────────┘
//!            │ IpcPageFault (IPC channel)
//!            ▼
//!   ┌────────────────────────┐
//!   │  Usermode mm (Ring 3)  │  ← policy decision-maker (this module)
//!   │                        │
//!   │  VmaManager  ─── per-process virtual address layout
//!   │  BuddyAlloc   ── global physical page allocator
//!   │  FaultResolver ─ decides map / kill / CoW
//!   └────────┬───────────────┘
//!            │ MapRequest (IPC channel)
//!            ▼
//!   ┌────────────────────────┐
//!   │  Kernel bmm (Ring 0)   │
//!   │  map() → set up PTE    │
//!   └────────────────────────┘
//! ```
//!
//! # Sub-modules
//!
//! | Module         | Purpose                                          |
//! |----------------|--------------------------------------------------|
//! | `types`        | Shared types: VMA entry, errors, IPC requests    |
//! | `vma`          | Per-process VMA manager (sorted array)           |
//! | `allocator`    | Buddy allocator + per-CPU page cache             |
//! | `page_fault`   | Fault resolution: VMA lookup → alloc → map       |
//! | `server`       | MmServer: process table, event loop, mmap/munmap |
//!
//! # Usage
//!
//! The mm server runs as a standalone user-space process.  During system init:
//!
//! ```ignore
//! use usr::mm::allocator;
//! use usr::mm::server::MmServer;
//!
//! // 1. Initialise physical-memory subsystem.
//! allocator::init(phys_mem_base, phys_mem_size);
//!
//! // 2. Create the mm server on its well-known IPC channel.
//! let mut server = MmServer::new(mm_channel_id);
//!
//! // 3. Register init process.
//! server.register_process(0, init_asid);
//! server.init_process_vma(0, code, data, stack, heap)?;
//!
//! // 4. Enter the main event loop (IPC receive / handle / reply).
//! // server.run();  // not yet implemented — see server.rs
//! ```

pub mod allocator;
pub mod page_fault;
pub mod server;
pub mod types;
pub mod vma;

// Re-export the public API surface.
pub use allocator::{alloc_page, alloc_pages, free_page, free_pages, free_count, init, total_pages};
pub use page_fault::{resolve_page_fault, resolve_with_prefault};
pub use server::MmServer;
pub use types::{
    BatchMappingArray, BatchMapping, MmError, MmRequest, MmResult, OomPolicy,
    VmaEntry, VmPerms, VmRegionType, PREFAULT_BATCH,
};
pub use vma::VmaManager;
