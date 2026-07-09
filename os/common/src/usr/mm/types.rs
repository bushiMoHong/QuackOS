//! Common types for the user-space memory manager.
//!
//! These types define the vocabulary used across VMA management, physical-page
//! allocation, page-fault resolution, and IPC with the kernel's `bmm` subsystem.

use crate::kernel::bmm::{AddressSpaceId, MapFlags};
use crate::kernel::ipc::message::ProcessId;

// ---------------------------------------------------------------------------
// Virtual Memory Area (VMA) types
// ---------------------------------------------------------------------------

/// Semantic category of a virtual-memory region.
///
/// Used by the page-fault resolver to decide whether an access is valid
/// and by VMA utilities to determine merge / split policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmRegionType {
    /// Executable code (`.text`).
    Code,
    /// Initialised / uninitialised data (`.data`, `.bss`).
    Data,
    /// Dynamic heap (`brk` / `sbrk`).
    Heap,
    /// Main thread stack (grows downward).
    Stack,
    /// `mmap`-backed region (anonymous or file).
    Mmap,
    /// Guard page — accessing it always triggers a segmentation fault.
    Guard,
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

/// Page-level access permissions for a VMA.
///
/// These are the user-space counterpart of the kernel's `MapFlags`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmPerms {
    pub read:  bool,
    pub write: bool,
    pub exec:  bool,
}

impl VmPerms {
    pub const R:   VmPerms = VmPerms { read: true,  write: false, exec: false };
    pub const RW:  VmPerms = VmPerms { read: true,  write: true,  exec: false };
    pub const RX:  VmPerms = VmPerms { read: true,  write: false, exec: true  };
    pub const RWX: VmPerms = VmPerms { read: true,  write: true,  exec: true  };

    /// No access — used for guard pages.
    pub const NONE: VmPerms = VmPerms { read: false, write: false, exec: false };

    /// Convert to kernel-side `MapFlags`, always adding the USER bit so the
    /// mapping is accessible from EL0.
    pub fn to_map_flags(&self) -> MapFlags {
        let mut f = MapFlags::empty();
        if self.read  { f.0 |= MapFlags::READ;  }
        if self.write { f.0 |= MapFlags::WRITE; }
        if self.exec  { f.0 |= MapFlags::EXEC;  }
        f.0 |= MapFlags::USER;
        f
    }

    /// Return the raw `usize` bitmask suitable for IPC request payloads.
    pub fn as_bits(&self) -> usize {
        self.to_map_flags().0 as usize
    }
}

// ---------------------------------------------------------------------------
// VMA entry
// ---------------------------------------------------------------------------

/// A single contiguous virtual-memory region.
///
/// `[start_vaddr, end_vaddr)` — the region is *half-open*.
#[derive(Debug, Clone)]
pub struct VmaEntry {
    /// Start virtual address (page-aligned).
    pub start_vaddr: usize,
    /// End virtual address (page-aligned, exclusive).
    pub end_vaddr: usize,
    /// Access permissions.
    pub perms: VmPerms,
    /// Region category.
    pub region_type: VmRegionType,
    /// Offset into a backing file for file-backed mmaps; 0 for anonymous.
    pub backing_offset: usize,
    /// Whether this VMA tracks Copy-on-Write semantics.
    pub cow: bool,
}

impl VmaEntry {
    /// Size of the region in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.end_vaddr - self.start_vaddr
    }

    /// True when the region is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.start_vaddr >= self.end_vaddr
    }

    /// True when `vaddr` falls within this region.
    #[inline]
    pub fn contains(&self, vaddr: usize) -> bool {
        vaddr >= self.start_vaddr && vaddr < self.end_vaddr
    }

    /// True when this region is a guard page.
    #[inline]
    pub fn is_guard(&self) -> bool {
        self.region_type == VmRegionType::Guard
    }

    /// True when `perms` satisfies the requested access for this region.
    pub fn permits(&self, needs: VmPerms) -> bool {
        (!needs.read  || self.perms.read)
            && (!needs.write || self.perms.write)
            && (!needs.exec  || self.perms.exec)
    }

    /// Readable description for debug output.
    pub fn desc(&self) -> &'static str {
        match self.region_type {
            VmRegionType::Code  => "code",
            VmRegionType::Data  => "data",
            VmRegionType::Heap  => "heap",
            VmRegionType::Stack => "stack",
            VmRegionType::Mmap  => "mmap",
            VmRegionType::Guard => "guard",
        }
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by the user-space mm subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmError {
    /// No VMA covers the faulting address.
    NoVma,
    /// The accessing VMA exists but lacks the required permissions.
    PermissionDenied,
    /// Physical memory exhausted — OOM kill triggered.
    OutOfMemory,
    /// The address is already mapped (double-map attempt).
    AlreadyMapped,
    /// The address is not mapped (unmap of unmapped address).
    NotMapped,
    /// Invalid argument (null pointer, bad alignment, …).
    InvalidArgument,
    /// VMA ranges overlap — insertion refused.
    VmaOverlap,
}

/// Convenience alias.
pub type MmResult<T> = Result<T, MmError>;

// ---------------------------------------------------------------------------
// OOM policy
// ---------------------------------------------------------------------------

/// What the memory manager does when `alloc_page()` returns `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OomPolicy {
    /// Immediately send a kill signal to the faulting process.
    Kill,
    /// Attempt page reclamation (LRU scan → swap / discard).
    /// **Not yet implemented** — reserved for future swap support.
    Reclaim,
    /// Try reclamation first; kill the process if it fails.
    ReclaimThenKill,
}

impl Default for OomPolicy {
    fn default() -> Self {
        OomPolicy::ReclaimThenKill
    }
}

// ---------------------------------------------------------------------------
// IPC request types (mm → kernel)
// ---------------------------------------------------------------------------

/// A request the mm server sends to the kernel's `bmm` subsystem.
///
/// Carried via IPC `ShortInfo` messages; the kernel dispatches on a per-variant
/// opcode encoded in the first register word.
#[derive(Debug, Clone)]
pub enum MmRequest {
    /// Map a single page.
    MapSingle {
        addr_space_id: AddressSpaceId,
        vaddr: usize,
        paddr: usize,
        flags: usize,
    },
    /// Unmap a single page.
    UnmapSingle {
        addr_space_id: AddressSpaceId,
        vaddr: usize,
    },
    /// Transfer a physical frame from one address space to another.
    Grant {
        src_asid: AddressSpaceId,
        dst_asid: AddressSpaceId,
        vaddr: usize,
        flags: usize,
    },
    /// Kill a process (OOM / segmentation fault).
    KillProcess {
        pid: ProcessId,
    },
}

// ---------------------------------------------------------------------------
// Batch mapping (prefault optimisation)
// ---------------------------------------------------------------------------

/// Maximum number of pages mapped in a single prefault batch.
pub const PREFAULT_BATCH: usize = 5; // 1 faulting page + up to 4 adjacent

/// A single entry in a batch-map request.
#[derive(Debug, Clone, Copy, Default)]
pub struct BatchMapping {
    pub vaddr: usize,
    pub paddr: usize,
    pub flags: usize,
}

/// Fixed-size batch of mapping requests.
///
/// The `count` field says how many of `entries` are valid.
#[derive(Debug, Clone, Copy)]
pub struct BatchMappingArray {
    pub entries: [BatchMapping; PREFAULT_BATCH],
    pub count: u8,
}

impl BatchMappingArray {
    pub const fn new() -> Self {
        BatchMappingArray {
            entries: [BatchMapping { vaddr: 0, paddr: 0, flags: 0 }; PREFAULT_BATCH],
            count: 0,
        }
    }

    pub fn push(&mut self, vaddr: usize, paddr: usize, flags: usize) {
        if (self.count as usize) < PREFAULT_BATCH {
            self.entries[self.count as usize] = BatchMapping { vaddr, paddr, flags };
            self.count += 1;
        }
    }

    pub fn is_empty(&self) -> bool { self.count == 0 }

    pub fn iter(&self) -> impl Iterator<Item = &BatchMapping> {
        self.entries[..self.count as usize].iter()
    }
}

// ---------------------------------------------------------------------------
// Opcodes for MmRequest encoding in IPC short messages
// ---------------------------------------------------------------------------

/// Opcode stored in `ShortPayload.words[0]` to tell the kernel which
/// mm-server request is being sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MmRequestOp {
    MapSingle   = 1,
    UnmapSingle = 2,
    Grant       = 3,
    KillProcess = 4,
}
