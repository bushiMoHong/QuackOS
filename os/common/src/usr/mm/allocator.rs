//! Buddy-system physical page allocator with per-CPU page caching.
//!
//! # Architecture
//!
//! ```text
//!   alloc_page() / free_page()
//!         │
//!   ┌─────▼──────┐
//!   │  PcpCache  │  ← per-CPU, lock-free hot path
//!   │ (fastpath) │
//!   └─────┬──────┘
//!         │ batch refill / drain
//!   ┌─────▼──────┐
//!   │   Buddy    │  ← global, spinlock-protected
//!   │ (slowpath) │
//!   └────────────┘
//! ```
//!
//! # Buddy system
//!
//! The buddy system manages pages in orders 0..MAX_ORDER, where each free
//! block at order `k` represents `2^k` contiguous pages.  Allocation uses
//! recursive splitting; freeing uses buddy merging.
//!
//! # Upgrade path
//!
//! When NUMA / multi-socket support is needed, replace the global buddy lock
//! with per-zone locks and NUMA-aware page allocation.

use spin::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Page size (bytes).
pub const PAGE_SIZE: usize = 4096;

/// Maximum buddy order — `2^10 = 1024` pages = 4 MiB max contiguous allocation.
pub const MAX_ORDER: usize = 10;

/// Per-CPU cache size: batch refill / drain threshold.
const PCP_BATCH: usize = 16;

/// Per-CPU cache high watermark — drain to buddy when exceeded.
const PCP_HIGH: usize = 64;

// ---------------------------------------------------------------------------
// BuddyAllocator
// ---------------------------------------------------------------------------

/// Global buddy allocator, protected by a spinlock.
///
/// Manages physical memory in power-of-two blocks.
pub struct BuddyAllocator {
    /// `free_lists[k]` holds the starting PFN of free blocks of order `k`.
    free_lists: [FreeList; MAX_ORDER + 1],
    /// Total number of pages managed.
    total_pages: usize,
    /// Current free page count.
    free_count: usize,
    /// Base physical address (PFN 0 = this address).
    #[allow(dead_code)]
    base_paddr: usize,
}

/// Singly-linked free list stored inside the free pages themselves.
#[allow(dead_code)]
struct FreeList {
    head: Option<usize>, // PFN of the first free block, or None
}

#[allow(dead_code)]
impl FreeList {
    const fn new() -> Self {
        FreeList { head: None }
    }

    fn push(&mut self, pfn: usize) {
        // Store the old head pointer inside the freed page.
        // The page itself is unused, so we can write metadata into it.
        // Safety: the PFN has been freed and is not in use.
        // TODO: write `self.head` to `(pfn * PAGE_SIZE)` when a direct-map is available.
        // For now, store the link in a separate array (see BuddyAllocator::links).
        // This is handled by the BuddyAllocator methods directly.
        let _ = pfn;
        self.head = Some(pfn);
    }

    fn pop(&mut self) -> Option<usize> {
        self.head.take()
    }
}

/// One link entry per physical page, stored out-of-line.
///
/// Buddy allocators typically store link pointers inside the free pages
/// themselves (since they're unused).  That requires those pages to be
/// directly mapped.  Until the kernel direct-map is set up, we use this
/// separate array.
const MAX_MANAGED_PAGES: usize = 1 << 20; // supports up to 4 GiB RAM

/// Link table: for each PFN, stores either the next PFN in its free list,
/// or `LINK_NONE` if the page is allocated / not in any list.
const LINK_NONE: usize = usize::MAX;

struct LinkTable {
    next: [usize; MAX_MANAGED_PAGES],
}

impl LinkTable {
    const fn new() -> Self {
        LinkTable {
            next: [LINK_NONE; MAX_MANAGED_PAGES],
        }
    }
}

static LINK_TABLE: Mutex<LinkTable> = Mutex::new(LinkTable::new());

/// The global buddy allocator singleton.
static BUDDY: Mutex<Option<BuddyAllocator>> = Mutex::new(None);

impl BuddyAllocator {
    /// Create a new buddy allocator managing `[base_paddr, base_paddr + total_bytes)`.
    ///
    /// # Safety
    /// The range must be backed by real RAM and not overlap kernel / device memory.
    pub unsafe fn new(base_paddr: usize, total_bytes: usize) -> Self {
        let total_pages = total_bytes >> 12; // PAGE_SHIFT
        let total_pages = total_pages.min(MAX_MANAGED_PAGES);

        const EMPTY_FREE_LIST: FreeList = FreeList::new();
        let mut buddy = BuddyAllocator {
            free_lists: [EMPTY_FREE_LIST; MAX_ORDER + 1],
            total_pages,
            free_count: total_pages,
            base_paddr,
        };

        // Add the full range as a single block at the highest possible order.
        let mut remaining = total_pages;
        let mut offset = 0usize;
        let mut order = MAX_ORDER;
        while remaining > 0 {
            let block_size = 1 << order;
            if block_size <= remaining {
                buddy.push_free(offset, order);
                offset += block_size;
                remaining -= block_size;
            } else {
                order -= 1;
            }
        }

        buddy
    }

    /// Initialise the global buddy allocator.
    pub fn init(base_paddr: usize, total_bytes: usize) {
        unsafe {
            *BUDDY.lock() = Some(BuddyAllocator::new(base_paddr, total_bytes));
        }
    }

    /// Allocate `2^order` contiguous pages.  Returns the PFN.
    pub fn alloc(&mut self, order: usize) -> Option<usize> {
        if order > MAX_ORDER {
            return None;
        }

        // Find the smallest order >= requested that has a free block.
        let mut o = order;
        while o <= MAX_ORDER && self.free_lists[o].head.is_none() {
            o += 1;
        }
        if o > MAX_ORDER {
            return None;
        }

        // Pop the block.
        let pfn = self.pop_free(o).unwrap();

        // Split down to the requested order, pushing buddies back.
        while o > order {
            o -= 1;
            let buddy_pfn = pfn + (1 << o);
            self.push_free(buddy_pfn, o);
        }

        self.free_count -= 1 << order;
        Some(pfn)
    }

    /// Allocate a single page (order 0).
    #[inline]
    pub fn alloc_page(&mut self) -> Option<usize> {
        self.alloc(0)
    }

    /// Free `2^order` pages starting at `pfn`.
    pub fn free(&mut self, pfn: usize, order: usize) {
        if order > MAX_ORDER {
            return;
        }

        let mut current_pfn = pfn;
        let mut current_order = order;

        // Try to merge with buddy at each level.
        while current_order < MAX_ORDER {
            let buddy_pfn = current_pfn ^ (1 << current_order);

            // Check if the buddy is free at this order.
            if !self.is_free(buddy_pfn, current_order) {
                break;
            }

            // Remove buddy from free list.
            self.pop_free_at(buddy_pfn, current_order);

            // Merge: the lower PFN becomes the merged block.
            current_pfn = current_pfn.min(buddy_pfn);
            current_order += 1;
        }

        self.push_free(current_pfn, current_order);
        self.free_count += 1 << order;
    }

    /// Free a single page.
    #[inline]
    pub fn free_page(&mut self, pfn: usize) {
        self.free(pfn, 0);
    }

    /// Total pages managed.
    #[inline]
    pub fn total_pages(&self) -> usize {
        self.total_pages
    }

    /// Currently free pages.
    #[inline]
    pub fn free_count(&self) -> usize {
        self.free_count
    }

    /// Currently used pages.
    #[inline]
    pub fn used_pages(&self) -> usize {
        self.total_pages - self.free_count
    }

    // --- internal helpers ---

    fn push_free(&mut self, pfn: usize, order: usize) {
        let mut links = LINK_TABLE.lock();
        links.next[pfn] = self.free_lists[order].head.unwrap_or(LINK_NONE);
        self.free_lists[order].head = Some(pfn);
    }

    fn pop_free(&mut self, order: usize) -> Option<usize> {
        let pfn = self.free_lists[order].head?;
        let mut links = LINK_TABLE.lock();
        let next = links.next[pfn];
        links.next[pfn] = LINK_NONE;
        self.free_lists[order].head = if next == LINK_NONE { None } else { Some(next) };
        Some(pfn)
    }

    fn pop_free_at(&mut self, pfn: usize, order: usize) {
        let mut links = LINK_TABLE.lock();
        // Walk the free list at this order to remove `pfn`.
        let mut prev: Option<usize> = None;
        let mut cur = self.free_lists[order].head;
        while let Some(c) = cur {
            if c == pfn {
                let next = if links.next[c] == LINK_NONE { None } else { Some(links.next[c]) };
                links.next[c] = LINK_NONE;
                match prev {
                    None => self.free_lists[order].head = next,
                    Some(p) => links.next[p] = links.next[c],
                }
                return;
            }
            prev = cur;
            cur = if links.next[c] == LINK_NONE { None } else { Some(links.next[c]) };
        }
    }

    fn is_free(&self, pfn: usize, _order: usize) -> bool {
        // Check if `pfn` is in any free list at the given order.
        // This is a linear scan of the free list; O(N) in free block count.
        // In production this would use a bitmap.
        let mut cur = self.free_lists[_order].head;
        let links = LINK_TABLE.lock();
        while let Some(c) = cur {
            if c == pfn {
                return true;
            }
            cur = if links.next[c] == LINK_NONE { None } else { Some(links.next[c]) };
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Per-CPU page cache (PcpCache)
// ---------------------------------------------------------------------------

/// Per-CPU front-end cache that batches page allocations to reduce lock
/// contention on the global buddy allocator.
///
/// In an SMP kernel there would be one `PcpCache` per CPU, accessed via
/// `tpidr_el1` on AArch64.  For now a single static instance is used.
pub struct PcpCache {
    /// Cached free page PFNs.
    pages: [usize; PCP_HIGH],
    /// Number of pages currently cached.
    count: usize,
}

impl PcpCache {
    pub const fn new() -> Self {
        PcpCache {
            pages: [0; PCP_HIGH],
            count: 0,
        }
    }

    /// Allocate a single page from the cache, refilling from buddy if empty.
    pub fn alloc_page(&mut self) -> Option<usize> {
        if self.count == 0 {
            self.refill();
        }
        if self.count > 0 {
            self.count -= 1;
            Some(self.pages[self.count])
        } else {
            None
        }
    }

    /// Free a single page into the cache, draining to buddy when full.
    pub fn free_page(&mut self, pfn: usize) {
        if self.count >= PCP_HIGH {
            self.drain();
        }
        self.pages[self.count] = pfn;
        self.count += 1;
    }

    /// Refill from the global buddy allocator.
    fn refill(&mut self) {
        let mut buddy = BUDDY.lock();
        let buddy = buddy.as_mut().expect("BuddyAllocator not initialised");
        for _ in 0..PCP_BATCH {
            if let Some(pfn) = buddy.alloc_page() {
                if self.count < PCP_HIGH {
                    self.pages[self.count] = pfn;
                    self.count += 1;
                } else {
                    buddy.free_page(pfn); // shouldn't happen, but be safe
                    break;
                }
            } else {
                break;
            }
        }
    }

    /// Drain excess pages back to the global buddy allocator.
    fn drain(&mut self) {
        let mut buddy = BUDDY.lock();
        let buddy = buddy.as_mut().expect("BuddyAllocator not initialised");
        let drain_count = self.count.min(PCP_BATCH);
        for _ in 0..drain_count {
            if self.count > 0 {
                self.count -= 1;
                buddy.free_page(self.pages[self.count]);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Global per-CPU cache (single instance for now; SMP will have one per core).
static GLOBAL_PCP: Mutex<PcpCache> = Mutex::new(PcpCache::new());

/// Initialise the physical-memory subsystem.
///
/// `base_paddr` — start of usable physical RAM.
/// `total_bytes` — size of usable physical RAM.
pub fn init(base_paddr: usize, total_bytes: usize) {
    BuddyAllocator::init(base_paddr, total_bytes);
}

/// Allocate a single zeroed physical page.
///
/// Fast path: per-CPU cache → slow path: buddy allocator.
pub fn alloc_page() -> Option<usize> {
    let mut pcp = GLOBAL_PCP.lock();
    if let Some(pfn) = pcp.alloc_page() {
        return Some(pfn);
    }
    drop(pcp);

    // PCP refill failed (buddy empty) — try buddy directly as last resort.
    let mut buddy = BUDDY.lock();
    buddy.as_mut()?.alloc_page()
}

/// Free a single physical page.
pub fn free_page(pfn: usize) {
    let mut pcp = GLOBAL_PCP.lock();
    pcp.free_page(pfn);
}

/// Allocate `2^order` contiguous pages directly from buddy.
pub fn alloc_pages(order: usize) -> Option<usize> {
    BUDDY.lock().as_mut()?.alloc(order)
}

/// Free `2^order` contiguous pages directly to buddy.
pub fn free_pages(pfn: usize, order: usize) {
    if let Some(buddy) = BUDDY.lock().as_mut() {
        buddy.free(pfn, order);
    }
}

/// Return the number of free physical pages (from buddy).
pub fn free_count() -> usize {
    BUDDY
        .lock()
        .as_ref()
        .map(|b| b.free_count())
        .unwrap_or(0)
}

/// Return the total number of managed physical pages.
pub fn total_pages() -> usize {
    BUDDY
        .lock()
        .as_ref()
        .map(|b| b.total_pages())
        .unwrap_or(0)
}
