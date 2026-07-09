//! Virtual Memory Area (VMA) manager.
//!
//! Each user process has one `VmaManager` that tracks its virtual address-space
//! layout — code, data, heap, stack, mmap regions, and guard pages.
//!
//! # Data structure
//!
//! Entries are stored in a fixed-size array sorted by `start_vaddr`.
//! Lookup uses binary search (O(log N)); insert / remove are O(N) but bounded
//! by `MAX_VMA_ENTRIES` (64 per process).
//!
//! # Upgrade path
//!
//! When a global allocator is available, `VmaManager` can be migrated to a
//! `BTreeMap<usize, VmaEntry>` keyed by `start_vaddr`, giving O(log N) for
//! all operations without the fixed cap.

use crate::usr::mm::types::{MmError, MmResult, VmaEntry, VmPerms, VmRegionType};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum VMA entries per process.
///
/// 64 entries is sufficient for typical embedded / microkernel workloads.
/// A desktop process with heavy mmap usage can have hundreds, at which point
/// the `BTreeMap` migration becomes necessary.
pub const MAX_VMA_ENTRIES: usize = 64;

/// Page size (bytes).  Matches `aarch64::base::config::PAGE_SIZE`.
pub const PAGE_SIZE: usize = 4096;
pub const PAGE_MASK: usize = PAGE_SIZE - 1;

// ---------------------------------------------------------------------------
// VmaManager
// ---------------------------------------------------------------------------

/// Per-process Virtual Memory Area manager.
///
/// ```text
///  0x0000_0000_0000_0000                             0x0000_7FFF_FFFF_FFFF
///  ├──────────┬─────────┬────────┬──────────┬──────────┤
///  │  .text   │  .data  │  heap  │  stack   │  mmap    │
///  │  RX      │  RW     │  RW    │  RW      │  RW      │  (simplified)
///  └──────────┴─────────┴────────┴──────────┴──────────┘
/// ```
pub struct VmaManager {
    entries: [Option<VmaEntry>; MAX_VMA_ENTRIES],
    len: usize,
}

impl VmaManager {
    /// Create an empty VMA manager.
    pub const fn new() -> Self {
        VmaManager {
            entries: [const { None }; MAX_VMA_ENTRIES],
            len: 0,
        }
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Number of VMA entries currently tracked.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when no VMA entries exist.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Find the VMA that contains `vaddr`, if any.
    ///
    /// Uses binary search on the sorted array.
    pub fn find(&self, vaddr: usize) -> Option<&VmaEntry> {
        if self.len == 0 {
            return None;
        }
        // Binary search: find the rightmost entry whose start <= vaddr.
        let mut lo = 0usize;
        let mut hi = self.len;
        let mut best: Option<usize> = None;

        while lo < hi {
            let mid = (lo + hi) / 2;
            let start = self.entries[mid].as_ref().unwrap().start_vaddr;
            if start <= vaddr {
                best = Some(mid);
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }

        match best {
            Some(idx) => {
                let entry = self.entries[idx].as_ref().unwrap();
                if entry.contains(vaddr) { Some(entry) } else { None }
            }
            None => None,
        }
    }

    /// Return the guard-page entry at `vaddr`, if one exists.
    ///
    /// Guard pages are checked first during page-fault resolution:
    /// accessing a guard page always means SIGSEGV.
    pub fn find_guard(&self, vaddr: usize) -> Option<&VmaEntry> {
        self.find(vaddr).filter(|e| e.is_guard())
    }

    /// Check whether the given access is permitted at `vaddr`.
    pub fn is_valid_access(&self, vaddr: usize, needs: VmPerms) -> bool {
        self.find(vaddr).is_some_and(|e| e.permits(needs))
    }

    /// Get the VMA entry that would contain the stack extension area.
    ///
    /// The stack grows downward; when a fault occurs just below the current
    /// stack VMA, the stack can be extended into the adjacent region (provided
    /// no other VMA occupies it).
    pub fn find_stack_to_extend(&self, fault_vaddr: usize) -> Option<&VmaEntry> {
        self.find(fault_vaddr).or_else(|| {
            // Look at the VMA whose start is just above fault_vaddr.
            // If it's a stack, extension may be possible.
            self.entries[..self.len]
                .iter()
                .filter_map(|e| e.as_ref())
                .find(|e| e.region_type == VmRegionType::Stack && fault_vaddr < e.start_vaddr)
        })
    }

    /// Check whether `start..end` overlaps any existing VMA.
    pub fn overlaps(&self, start: usize, end: usize) -> bool {
        for i in 0..self.len {
            let e = self.entries[i].as_ref().unwrap();
            if start < e.end_vaddr && end > e.start_vaddr {
                return true;
            }
        }
        false
    }

    /// Return a slice of all valid entries, for iteration.
    pub fn all(&self) -> &[Option<VmaEntry>] {
        &self.entries[..self.len]
    }

    // ------------------------------------------------------------------
    // Mutations
    // ------------------------------------------------------------------

    /// Insert a VMA entry, maintaining sorted order.
    ///
    /// Returns `MmError::VmaOverlap` if the new region overlaps an existing
    /// one.  Adjacent regions of the same type and permissions are merged.
    pub fn insert(&mut self, entry: VmaEntry) -> MmResult<()> {
        if entry.is_empty() {
            return Err(MmError::InvalidArgument);
        }
        if self.len >= MAX_VMA_ENTRIES {
            return Err(MmError::OutOfMemory); // reused; table is full
        }

        // Find insertion position (by start_vaddr).
        let pos = self
            .entries[..self.len]
            .iter()
            .position(|e| e.as_ref().unwrap().start_vaddr > entry.start_vaddr)
            .unwrap_or(self.len);

        // Check overlap with predecessor.
        if pos > 0 {
            let prev = self.entries[pos - 1].as_ref().unwrap();
            if prev.end_vaddr > entry.start_vaddr {
                return Err(MmError::VmaOverlap);
            }
            // Merge with predecessor if adjacent and compatible.
            if prev.end_vaddr == entry.start_vaddr && vma_can_merge(prev, &entry) {
                return self.merge_into_prev(pos - 1, &entry);
            }
        }

        // Check overlap with successor.
        if pos < self.len {
            let next = self.entries[pos].as_ref().unwrap();
            if entry.end_vaddr > next.start_vaddr {
                return Err(MmError::VmaOverlap);
            }
            // Merge with successor if adjacent and compatible.
            if entry.end_vaddr == next.start_vaddr && vma_can_merge(&entry, next) {
                return self.merge_with_next(pos, &entry);
            }
        }

        // Shift entries right and insert.
        self.shift_right(pos);
        self.entries[pos] = Some(entry);
        self.len += 1;
        Ok(())
    }

    /// Remove (or shrink) the VMA covering `start..end`.
    ///
    /// If the range splits a VMA in two, two entries will result.
    /// Pages that fall partially inside the range are handled by the caller
    /// (they should already be unmapped before calling this).
    pub fn remove(&mut self, start: usize, end: usize) -> MmResult<()> {
        if start >= end {
            return Err(MmError::InvalidArgument);
        }

        // Find the first VMA that touches the range.
        let idx = match self.find_index_touching(start) {
            Some(i) => i,
            None => return Ok(()), // nothing to remove
        };

        let mut i = idx;
        while i < self.len {
            let entry = self.entries[i].as_ref().unwrap();
            if entry.start_vaddr >= end {
                break; // past the removal range
            }

            let e_start = entry.start_vaddr;
            let e_end = entry.end_vaddr;

            if end <= e_start {
                break;
            }

            if start <= e_start && end >= e_end {
                // Fully covered — remove entire entry.
                self.shift_left(i);
                self.len -= 1;
                // Don't increment i; the next entry shifted into this slot.
                continue;
            } else if start > e_start && end < e_end {
                // Split: keep left and right fragments.
                let right = VmaEntry {
                    start_vaddr: end,
                    ..entry.clone()
                };
                // Shrink the current entry to become the left fragment.
                self.entries[i].as_mut().unwrap().end_vaddr = start;
                // Insert the right fragment after i.
                if self.len >= MAX_VMA_ENTRIES {
                    return Err(MmError::OutOfMemory);
                }
                self.shift_right(i + 1);
                self.entries[i + 1] = Some(right);
                self.len += 1;
                break;
            } else if start <= e_start {
                // Truncate left side.
                self.entries[i].as_mut().unwrap().start_vaddr = end;
            } else {
                // Truncate right side (start > e_start).
                self.entries[i].as_mut().unwrap().end_vaddr = start;
            }
            i += 1;
        }
        Ok(())
    }

    /// Split the VMA containing `vaddr` at that address.
    ///
    /// After the split there will be two VMAs: `[old_start, vaddr)` and
    /// `[vaddr, old_end)`.  Used for CoW break and partial mprotect.
    ///
    /// Returns the index of the right (upper) fragment.
    pub fn split_at(&mut self, vaddr: usize) -> MmResult<usize> {
        let idx = self.find_index_containing(vaddr).ok_or(MmError::NoVma)?;

        let entry = self.entries[idx].as_ref().unwrap();
        if vaddr == entry.start_vaddr || vaddr == entry.end_vaddr {
            // Already split at this boundary.
            return Ok(idx);
        }
        if self.len >= MAX_VMA_ENTRIES {
            return Err(MmError::OutOfMemory);
        }

        let right = VmaEntry {
            start_vaddr: vaddr,
            end_vaddr: entry.end_vaddr,
            ..entry.clone()
        };
        self.entries[idx].as_mut().unwrap().end_vaddr = vaddr;

        self.shift_right(idx + 1);
        self.entries[idx + 1] = Some(right);
        self.len += 1;
        Ok(idx + 1)
    }

    /// Extend the stack VMA downward to `new_start` (page-aligned).
    ///
    /// `new_start` must be < the current stack start and must not overlap
    /// any other VMA.
    pub fn extend_stack(&mut self, new_start: usize) -> MmResult<()> {
        // Find the stack VMA.
        let idx = self
            .entries[..self.len]
            .iter()
            .position(|e| {
                let e = e.as_ref().unwrap();
                e.region_type == VmRegionType::Stack
            })
            .ok_or(MmError::NoVma)?;

        let new_end = self.entries[idx].as_ref().unwrap().start_vaddr;
        if new_start >= new_end {
            return Err(MmError::InvalidArgument);
        }

        // Check no other VMA overlaps the extension.
        if self.overlaps(new_start, new_end) {
            return Err(MmError::VmaOverlap);
        }

        self.entries[idx].as_mut().unwrap().start_vaddr = new_start;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Find the index of the VMA containing `vaddr`.
    fn find_index_containing(&self, vaddr: usize) -> Option<usize> {
        self.find(vaddr)?; // validate existence
        // Re-find via binary search.
        self.bsearch(vaddr)
    }

    /// Find the index of the first VMA whose `end_vaddr > start`.
    fn find_index_touching(&self, start: usize) -> Option<usize> {
        for i in 0..self.len {
            let e = self.entries[i].as_ref().unwrap();
            if e.end_vaddr > start {
                return Some(i);
            }
        }
        None
    }

    /// Binary search for the rightmost entry with start <= vaddr.
    fn bsearch(&self, vaddr: usize) -> Option<usize> {
        let mut lo = 0usize;
        let mut hi = self.len;
        let mut best: Option<usize> = None;

        while lo < hi {
            let mid = (lo + hi) / 2;
            let start = self.entries[mid].as_ref().unwrap().start_vaddr;
            if start <= vaddr {
                best = Some(mid);
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        best
    }

    /// Shift entries `[pos..len)` one slot right.
    fn shift_right(&mut self, pos: usize) {
        let mut i = self.len;
        while i > pos {
            self.entries[i] = self.entries[i - 1].take();
            i -= 1;
        }
    }

    /// Shift entries `[pos+1..len)` one slot left.
    fn shift_left(&mut self, pos: usize) {
        for i in pos..self.len - 1 {
            self.entries[i] = self.entries[i + 1].take();
        }
        self.entries[self.len - 1] = None;
    }

    /// Merge `entry` into the predecessor at `prev_idx`.
    fn merge_into_prev(&mut self, prev_idx: usize, entry: &VmaEntry) -> MmResult<()> {
        let prev = self.entries[prev_idx].as_mut().unwrap();
        prev.end_vaddr = prev.end_vaddr.max(entry.end_vaddr);
        Ok(())
    }

    /// Merge `entry` with the successor at `next_idx`.
    fn merge_with_next(&mut self, next_idx: usize, entry: &VmaEntry) -> MmResult<()> {
        let next = self.entries[next_idx].as_mut().unwrap();
        next.start_vaddr = entry.start_vaddr;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Debug
    // ------------------------------------------------------------------

    /// Produce a human-readable dump of the address-space layout.
    ///
    /// Format is similar to `/proc/<pid>/maps` on Linux.
    pub fn dump(&self) -> VmaDump<'_> {
        VmaDump { manager: self, cursor: 0 }
    }
}

// ---------------------------------------------------------------------------
// Iterator for debug dumping
// ---------------------------------------------------------------------------

pub struct VmaDump<'a> {
    manager: &'a VmaManager,
    cursor: usize,
}

impl<'a> Iterator for VmaDump<'a> {
    type Item = &'a VmaEntry;

    fn next(&mut self) -> Option<Self::Item> {
        while self.cursor < self.manager.len {
            let entry = self.manager.entries[self.cursor].as_ref();
            self.cursor += 1;
            if entry.is_some() {
                return entry;
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Two VMAs can be merged into one contiguous region when they are adjacent,
/// have identical permissions, the same `cow` status, and the same region type.
fn vma_can_merge(a: &VmaEntry, b: &VmaEntry) -> bool {
    a.perms == b.perms
        && a.region_type == b.region_type
        && a.cow == b.cow
        && a.backing_offset == b.backing_offset
}

// ---------------------------------------------------------------------------
// Convenience constructors
// ---------------------------------------------------------------------------

impl VmaEntry {
    /// Create a code region (RX).
    pub fn new_code(start: usize, end: usize) -> Self {
        VmaEntry {
            start_vaddr: start,
            end_vaddr: end,
            perms: VmPerms::RX,
            region_type: VmRegionType::Code,
            backing_offset: 0,
            cow: false,
        }
    }

    /// Create a data region (RW).
    pub fn new_data(start: usize, end: usize) -> Self {
        VmaEntry {
            start_vaddr: start,
            end_vaddr: end,
            perms: VmPerms::RW,
            region_type: VmRegionType::Data,
            backing_offset: 0,
            cow: false,
        }
    }

    /// Create a heap region (RW, grows upward).
    pub fn new_heap(start: usize, end: usize) -> Self {
        VmaEntry {
            start_vaddr: start,
            end_vaddr: end,
            perms: VmPerms::RW,
            region_type: VmRegionType::Heap,
            backing_offset: 0,
            cow: false,
        }
    }

    /// Create a stack region (RW, grows downward).
    pub fn new_stack(start: usize, end: usize) -> Self {
        VmaEntry {
            start_vaddr: start,
            end_vaddr: end,
            perms: VmPerms::RW,
            region_type: VmRegionType::Stack,
            backing_offset: 0,
            cow: false,
        }
    }

    /// Create an anonymous mmap region.
    pub fn new_mmap(start: usize, end: usize, perms: VmPerms) -> Self {
        VmaEntry {
            start_vaddr: start,
            end_vaddr: end,
            perms,
            region_type: VmRegionType::Mmap,
            backing_offset: 0,
            cow: false,
        }
    }

    /// Create a guard page (one page, no access).
    pub fn new_guard(addr: usize) -> Self {
        VmaEntry {
            start_vaddr: addr,
            end_vaddr: addr + PAGE_SIZE,
            perms: VmPerms::NONE,
            region_type: VmRegionType::Guard,
            backing_offset: 0,
            cow: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_manager_finds_nothing() {
        let mgr = VmaManager::new();
        assert!(mgr.find(0x1000).is_none());
    }

    #[test]
    fn insert_and_find() {
        let mut mgr = VmaManager::new();
        mgr.insert(VmaEntry::new_code(0x1000, 0x2000)).unwrap();
        assert!(mgr.find(0x1000).is_some());
        assert!(mgr.find(0x1FFF).is_some());
        assert!(mgr.find(0x2000).is_none());
        assert!(mgr.find(0x0FFF).is_none());
    }

    #[test]
    fn overlap_rejected() {
        let mut mgr = VmaManager::new();
        mgr.insert(VmaEntry::new_code(0x1000, 0x3000)).unwrap();
        assert!(mgr.insert(VmaEntry::new_data(0x2000, 0x4000)).is_err());
    }

    #[test]
    fn adjacent_merge() {
        let mut mgr = VmaManager::new();
        mgr.insert(VmaEntry::new_code(0x1000, 0x2000)).unwrap();
        // Adjacent code regions with same perms should merge.
        mgr.insert(VmaEntry::new_code(0x2000, 0x3000)).unwrap();
        assert_eq!(mgr.len(), 1);
        let entry = mgr.find(0x2500).unwrap();
        assert_eq!(entry.start_vaddr, 0x1000);
        assert_eq!(entry.end_vaddr, 0x3000);
    }

    #[test]
    fn remove_full_entry() {
        let mut mgr = VmaManager::new();
        mgr.insert(VmaEntry::new_data(0x2000, 0x3000)).unwrap();
        mgr.remove(0x2000, 0x3000).unwrap();
        assert!(mgr.is_empty());
    }

    #[test]
    fn remove_partial_left() {
        let mut mgr = VmaManager::new();
        mgr.insert(VmaEntry::new_data(0x2000, 0x5000)).unwrap();
        mgr.remove(0x2000, 0x3000).unwrap();
        let entry = mgr.find(0x4000).unwrap();
        assert_eq!(entry.start_vaddr, 0x3000);
        assert_eq!(entry.end_vaddr, 0x5000);
    }

    #[test]
    fn remove_partial_right() {
        let mut mgr = VmaManager::new();
        mgr.insert(VmaEntry::new_data(0x2000, 0x5000)).unwrap();
        mgr.remove(0x4000, 0x5000).unwrap();
        let entry = mgr.find(0x3000).unwrap();
        assert_eq!(entry.start_vaddr, 0x2000);
        assert_eq!(entry.end_vaddr, 0x4000);
    }

    #[test]
    fn remove_split() {
        let mut mgr = VmaManager::new();
        mgr.insert(VmaEntry::new_data(0x2000, 0x6000)).unwrap();
        mgr.remove(0x3000, 0x4000).unwrap();
        assert_eq!(mgr.len(), 2);
        let left = mgr.find(0x2500).unwrap();
        assert_eq!(left.start_vaddr, 0x2000);
        assert_eq!(left.end_vaddr, 0x3000);
        let right = mgr.find(0x5000).unwrap();
        assert_eq!(right.start_vaddr, 0x4000);
        assert_eq!(right.end_vaddr, 0x6000);
    }

    #[test]
    fn split_at_mid() {
        let mut mgr = VmaManager::new();
        mgr.insert(VmaEntry::new_data(0x2000, 0x6000)).unwrap();
        let right_idx = mgr.split_at(0x4000).unwrap();
        assert_eq!(mgr.len(), 2);
        let left = mgr.find(0x3000).unwrap();
        assert_eq!(left.end_vaddr, 0x4000);
        let right = mgr.find(0x5000).unwrap();
        assert_eq!(right.start_vaddr, 0x4000);
        assert_eq!(right.end_vaddr, 0x6000);
    }

    #[test]
    fn guard_page_detected() {
        let mut mgr = VmaManager::new();
        mgr.insert(VmaEntry::new_code(0x1000, 0x2000)).unwrap();
        mgr.insert(VmaEntry::new_guard(0x2000)).unwrap();
        assert!(mgr.find_guard(0x2000).is_some());
        assert!(mgr.find_guard(0x1500).is_none());
    }

    #[test]
    fn permission_check() {
        let mut mgr = VmaManager::new();
        mgr.insert(VmaEntry::new_code(0x1000, 0x2000)).unwrap(); // RX
        assert!(mgr.is_valid_access(0x1500, VmPerms::R));
        assert!(mgr.is_valid_access(0x1500, VmPerms::RX));
        assert!(!mgr.is_valid_access(0x1500, VmPerms::RW));
    }
}
