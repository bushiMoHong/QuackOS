//! Thread Control Block (TCB) and the global thread table.
//!
//! # TCB layout constraint
//!
//! `Thread.kernel_stack_top` **must** be the first field (offset 0) because
//! the assembly context-switch routine `__switch` writes the current stack
//! pointer via `str sp, [x19]` — i.e. into `TCB[0]`.
//!
//! # Locking
//!
//! The `ThreadTable` uses a single `Mutex` for allocation / deallocation
//! (create / destroy).  The hot path (`schedule()`) does **not** hold this
//! lock — it reads thread state via atomic loads and pushes / pops the
//! ready-queue independently.  See `context.rs`.

use super::error::ScheError;
use crate::kernel::ipc::synchronization::IpcState;
use crate::kernel::ipc::transfer::IpcBuffer;
use core::fmt;
use core::sync::atomic::{AtomicU8, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// ThreadId — generational index (ABA-proof)
// ---------------------------------------------------------------------------

/// Thread identifier with built-in ABA protection.
///
/// # Layout
///
/// ```text
/// bits 31:16  →  generation (u16, increments on every allocation at a slot)
/// bits 15:0   →  index      (u16, array index into ThreadTable)
/// ```
///
/// `ThreadId(0)` (`NULL`) is reserved and never allocated.
///
/// # Why generational?
///
/// If a thread is destroyed and its slot reused, a stale `ThreadId` held by
/// another thread for IPC will have the wrong generation — `lookup()` returns
/// `ScheError::InvalidThread` instead of silently operating on the wrong TCB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadId(pub u32);

impl ThreadId {
    /// The null / invalid thread ID.  Never allocated.
    pub const NULL: ThreadId = ThreadId(0);

    /// Maximum number of threads supported (limited by u16 index width).
    pub const MAX_INDEX: u16 = u16::MAX;

    /// Construct a `ThreadId` from an index and generation.
    #[inline]
    pub const fn new(index: u16, generation: u16) -> Self {
        ThreadId(((generation as u32) << 16) | (index as u32))
    }

    /// Extract the slot index.
    #[inline]
    pub fn index(self) -> u16 {
        (self.0 & 0xFFFF) as u16
    }

    /// Extract the generation.
    #[inline]
    pub fn generation(self) -> u16 {
        ((self.0 >> 16) & 0xFFFF) as u16
    }

    /// Return `true` if this is the null ID.
    #[inline]
    pub fn is_null(self) -> bool {
        self.0 == 0
    }
}

// Display / Debug — used in log messages throughout the IPC subsystem.
impl fmt::Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_null() {
            write!(f, "T(NULL)")
        } else {
            write!(f, "T({}:{})", self.index(), self.generation())
        }
    }
}

// ---------------------------------------------------------------------------
// ThreadState — encoded as u8 for atomic access
// ---------------------------------------------------------------------------

/// Thread lifecycle state.
///
/// Stored as a `u8` in `AtomicU8` so the scheduler can transition states
/// without holding the thread-table lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ThreadState {
    /// Slot is unused (free for allocation).
    Free = 0,
    /// Thread is runnable and waiting in the ReadyQueue.
    Ready = 1,
    /// Thread is currently executing on a CPU.
    Running = 2,
    /// Thread is blocked (on IPC, timer, notification, …).
    Blocked = 3,
    /// Thread is in the process of being destroyed.
    Dying = 4,
}

impl ThreadState {
    /// Convert a `u8` back to `ThreadState`.
    #[inline]
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(ThreadState::Free),
            1 => Some(ThreadState::Ready),
            2 => Some(ThreadState::Running),
            3 => Some(ThreadState::Blocked),
            4 => Some(ThreadState::Dying),
            _ => None,
        }
    }

    /// Return `true` if the thread is runnable (can be scheduled).
    #[inline]
    pub fn is_runnable(self) -> bool {
        matches!(self, ThreadState::Ready)
    }
}

// ---------------------------------------------------------------------------
// Thread (TCB)
// ---------------------------------------------------------------------------

/// Thread Control Block.
///
/// # Layout invariant (critical)
///
/// `kernel_stack_top` **must** be the first field.  The assembly
/// `__switch` routine writes the current SP to `*(x19 + 0)`, so
/// this field lives at offset 0 within the struct.
#[derive(Debug)]
#[repr(C)]
pub struct Thread {
    // ── offset 0 (read / written by __switch) ──
    /// Saved kernel stack pointer — points to the 128-byte `TaskContext`
    /// area on this thread's kernel stack.
    pub kernel_stack_top: usize,

    // ── identity ──
    /// Unique thread identifier (generational).
    pub id: ThreadId,

    // ── scheduling ──
    /// Effective scheduling priority (may be boosted by inheritance).
    /// The scheduler dequeues the highest `priority` first.
    pub priority: u8,
    /// Original (base) priority — restored when priority donation ends.
    pub base_priority: u8,
    /// Donated priority, or 0 if no donation is active.
    /// `priority == max(base_priority, donated_priority)`.
    pub donated_priority: u8,

    // ── execution context ──
    /// Base address of this thread's kernel stack.
    pub kernel_stack_base: usize,
    /// Size of the kernel stack (top - base).  Used by `destroy_thread`.
    pub kernel_stack_size: usize,
    /// Page-table root token (TTBR0_EL1 value).
    pub ttbr0: usize,
    /// Address-space identifier (for TLB tagging).
    pub asid: usize,

    // ── IPC integration ──
    /// Thread state (for atomic transitions in `schedule()`).
    pub state: AtomicU8,
    /// IPC blocking reason — meaningful only when `state == Blocked`.
    pub ipc_state: IpcState,
    /// Per-thread IPC receive buffer (moved from `ipc::transfer` global table).
    pub ipc_buffer: IpcBuffer,

    // ── Linux syscall exception reflection ──
    /// User-mode liblinux handler entry point for this thread (per-thread, §8.7).
    pub linux_handler_pc: Option<usize>,
    /// Per-thread save area vaddr for `LinuxContext` (§8.4).
    pub linux_save_area: Option<usize>,

    // ── IPC timeout ──
    /// Deadline in CNTPCT ticks for the current IPC operation (0 = no timeout).
    pub ipc_deadline: u64,

    // ── process / parent-child tracking ──
    /// Parent thread (set by clone).
    pub parent_tid: Option<ThreadId>,
    /// Exit code (set by exit_thread, read by wait4).
    pub exit_code: i32,
}

impl Thread {
    /// Create a new TCB in `Ready` state.
    ///
    /// # Safety
    ///
    /// The caller must ensure `kernel_stack_base` and `kernel_stack_top`
    /// point to a valid, exclusively-owned kernel stack for this thread.
    /// `ttbr0` must be a valid page-table root token.
    /// `kernel_stack_size` is the full allocated size of the kernel stack
    /// (used by `destroy_thread` to free the range).
    pub unsafe fn new(
        id: ThreadId,
        priority: u8,
        kernel_stack_base: usize,
        kernel_stack_top: usize,
        kernel_stack_size: usize,
        ttbr0: usize,
        asid: usize,
    ) -> Self {
        Thread {
            id,
            kernel_stack_top,
            priority,
            base_priority: priority,
            donated_priority: 0,
            kernel_stack_base,
            kernel_stack_size,
            ttbr0,
            asid,
            state: AtomicU8::new(ThreadState::Ready as u8),
            ipc_state: IpcState::Ready,
            ipc_buffer: IpcBuffer::empty(),
            linux_handler_pc: None,
            linux_save_area: None,
            ipc_deadline: 0,
            parent_tid: None,
            exit_code: 0,
        }
    }

    // ------------------------------------------------------------------
    // Atomic state helpers — used by context.rs (hot path, no table lock)
    // ------------------------------------------------------------------

    /// Atomically read the current `ThreadState`.
    #[inline]
    pub fn atomic_state(&self) -> ThreadState {
        let v = self.state.load(Ordering::Acquire);
        ThreadState::from_u8(v).unwrap_or(ThreadState::Free)
    }

    /// Atomically write a new `ThreadState`.
    #[inline]
    pub fn set_atomic_state(&self, s: ThreadState) {
        self.state.store(s as u8, Ordering::Release);
    }

    /// Compare-and-swap the thread state.  Returns `true` on success.
    #[inline]
    pub fn cas_state(&self, old: ThreadState, new: ThreadState) -> bool {
        self.state
            .compare_exchange(old as u8, new as u8, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    // ------------------------------------------------------------------
    // Priority inheritance hooks (reserved — wired to IPC later)
    // ------------------------------------------------------------------

    /// Effective priority used by the scheduler.
    #[inline]
    pub fn effective_priority(&self) -> u8 {
        self.priority
    }

    /// Boost this thread's effective priority (priority donation).
    ///
    /// Caller must hold any necessary IPC / channel locks to ensure the
    /// target thread isn't concurrently being modified.
    pub fn boost_priority(&mut self, donated: u8) {
        self.donated_priority = donated;
        self.priority = self.base_priority.max(donated);
    }

    /// Restore the thread's priority to its base value.
    pub fn restore_priority(&mut self) {
        self.donated_priority = 0;
        self.priority = self.base_priority;
    }
}

// ---------------------------------------------------------------------------
// Global thread table
// ---------------------------------------------------------------------------

/// Maximum number of kernel threads.
pub const MAX_THREADS: usize = 256;

/// Inner data for the global thread table.
///
/// The outer `Mutex` protects allocation / deallocation.  Hot-path
/// operations (`schedule()`, `wake()`) do not acquire this lock —
/// they operate on the `ReadyQueue` and atomically transition
/// per-thread states.
struct ThreadTableInner {
    slots: [Option<Thread>; MAX_THREADS],
    /// Generation counter per slot — incremented on every allocation.
    generations: [u16; MAX_THREADS],
    /// Number of currently-allocated threads.
    count: usize,
}

impl ThreadTableInner {
    const fn new() -> Self {
        ThreadTableInner {
            // SAFETY: `Option<Thread>` is not `Copy`, but we can construct
            // the array in a const context with `const { None }`.
            slots: [const { None }; MAX_THREADS],
            generations: [0u16; MAX_THREADS],
            count: 0,
        }
    }

    /// Allocate a slot and return the new `ThreadId`.
    fn alloc_slot(&mut self) -> Option<ThreadId> {
        for (i, slot) in self.slots.iter().enumerate() {
            if slot.is_none() {
                // Simple generation: start at 1 so 0 signals "never used".
                let gen = if self.generations[i] == 0 {
                    1
                } else {
                    self.generations[i].wrapping_add(1)
                };
                self.generations[i] = gen;
                self.count += 1;
                return Some(ThreadId::new(i as u16, gen));
            }
        }
        None
    }

    /// Free a slot.
    fn free_slot(&mut self, index: u16) {
        let i = index as usize;
        if i < MAX_THREADS && self.slots[i].is_some() {
            self.slots[i] = None;
            self.count = self.count.saturating_sub(1);
            // generation is intentionally **not** reset — this is the ABA guard.
        }
    }

    /// Look up a thread by `ThreadId`, verifying the generation.
    fn lookup(&self, id: ThreadId) -> Option<&Thread> {
        if id.is_null() {
            return None;
        }
        let i = id.index() as usize;
        if i >= MAX_THREADS {
            return None;
        }
        self.slots[i].as_ref().filter(|t| t.id.generation() == id.generation())
    }

    /// Look up a thread mutably, verifying the generation.
    fn lookup_mut(&mut self, id: ThreadId) -> Option<&mut Thread> {
        if id.is_null() {
            return None;
        }
        let i = id.index() as usize;
        if i >= MAX_THREADS {
            return None;
        }
        self.slots[i].as_mut().filter(|t| t.id.generation() == id.generation())
    }

    /// Insert a thread at a previously-allocated slot.
    fn insert(&mut self, thread: Thread) -> Result<(), ScheError> {
        let i = thread.id.index() as usize;
        if i >= MAX_THREADS {
            return Err(ScheError::InvalidThread);
        }
        if self.slots[i].is_some() {
            return Err(ScheError::InvalidThreadState);
        }
        self.slots[i] = Some(thread);
        Ok(())
    }
}

/// Global thread table.
static THREAD_TABLE: Mutex<ThreadTableInner> = Mutex::new(ThreadTableInner::new());

// ---------------------------------------------------------------------------
// Public API — thread lifecycle
// ---------------------------------------------------------------------------

/// Create a new thread and insert it into the global table.
///
/// Returns the new `ThreadId` on success.
///
/// `kernel_stack_size` is the full allocated byte-length of the kernel stack
/// (typically `8 * PAGE_SIZE`).  It is stored so `destroy_thread` can free the
/// entire contiguous range.
///
/// # Safety
///
/// `kernel_stack_base` / `kernel_stack_top` must reference a valid, exclusive
/// kernel stack.  `ttbr0` must be a valid page-table root token.
pub unsafe fn create_thread(
    priority: u8,
    kernel_stack_base: usize,
    kernel_stack_top: usize,
    kernel_stack_size: usize,
    ttbr0: usize,
    asid: usize,
) -> Result<ThreadId, ScheError> {
    let mut table = THREAD_TABLE.lock();

    let tid = table
        .alloc_slot()
        .ok_or(ScheError::ThreadTableFull)?;

    let thread = Thread::new(tid, priority, kernel_stack_base, kernel_stack_top, kernel_stack_size, ttbr0, asid);

    table.insert(thread)?;

    log::info!("sche: created thread {:?} prio={}", tid, priority);
    Ok(tid)
}

/// Destroy a thread and free its slot.
///
/// The thread must be in `Free` or `Dying` state.  Its kernel stack is
/// freed here — callers no longer need to deallocate it separately.
pub fn destroy_thread(id: ThreadId) -> Result<(), ScheError> {
    if id.is_null() {
        return Err(ScheError::NullThreadId);
    }

    let (ks_base, ks_end) = {
        let mut table = THREAD_TABLE.lock();

        // Verify the thread exists with matching generation.
        let thread = table.lookup(id).ok_or(ScheError::InvalidThread)?;

        let base = thread.kernel_stack_base;
        let end = base + thread.kernel_stack_size;

        // Mark as Free first, then release the slot.
        table.free_slot(id.index());

        (base, end)
    };

    // Free the kernel stack pages.
    if ks_base != 0 {
        aarch64::base::mm::free_page_range(ks_base, ks_end);
    }

    log::info!("sche: destroyed thread {:?}", id);
    Ok(())
}

/// Return the number of currently-allocated threads.
pub fn thread_count() -> usize {
    THREAD_TABLE.lock().count
}

// ---------------------------------------------------------------------------
// Public API — thread access (for IPC and other subsystems)
// ---------------------------------------------------------------------------

/// Run a closure over the thread identified by `id`.
///
/// The table lock is held for the duration of `f`.  Callers must not
/// attempt to `schedule()` or acquire other locks that may deadlock.
pub fn with_thread<R>(
    id: ThreadId,
    f: impl FnOnce(&Thread) -> R,
) -> Result<R, ScheError> {
    let table = THREAD_TABLE.lock();
    let thread = table.lookup(id).ok_or(ScheError::InvalidThread)?;
    Ok(f(thread))
}

/// Run a mutable closure over the thread identified by `id`.
///
/// Same locking caveat as `with_thread`.
pub fn with_thread_mut<R>(
    id: ThreadId,
    f: impl FnOnce(&mut Thread) -> R,
) -> Result<R, ScheError> {
    let mut table = THREAD_TABLE.lock();
    let thread = table.lookup_mut(id).ok_or(ScheError::InvalidThread)?;
    Ok(f(thread))
}

/// Check whether a thread with the given ID exists (generation check included).
pub fn thread_exists(id: ThreadId) -> bool {
    if id.is_null() {
        return false;
    }
    THREAD_TABLE.lock().lookup(id).is_some()
}

/// Run a closure over all allocated threads.
///
/// The table lock is held for the duration of `f`.
pub fn with_all_threads<R>(
    f: impl FnOnce(&[Thread]) -> R,
) -> R {
    let table = THREAD_TABLE.lock();
    // Collect all active threads into a temporary buffer and pass as slice.
    // MAX_THREADS is small, so this is cheap.
    let mut buf: [*const Thread; MAX_THREADS] = [core::ptr::null(); MAX_THREADS];
    let mut n = 0;
    for slot in table.slots.iter() {
        if let Some(ref t) = slot {
            buf[n] = t as *const Thread;
            n += 1;
        }
    }
    // SAFETY: the pointers are valid while the lock is held.
    let slice = unsafe { core::slice::from_raw_parts(buf.as_ptr() as *const Thread, n) };
    f(slice)
}

/// Return the kernel stack top of a thread (for `__switch`).
///
/// # Panics
///
/// Panics if `id` is invalid — this is a hot-path function called from
/// `context.rs::schedule()` where the ID was just taken from the ready
/// queue and is known to be valid.
#[inline]
pub fn kernel_stack_top(id: ThreadId) -> usize {
    // Hot path — avoid the closure overhead of `with_thread`.
    let table = THREAD_TABLE.lock();
    table
        .lookup(id)
        .expect("sche: kernel_stack_top on invalid thread")
        .kernel_stack_top
}

/// Return a raw pointer to the TCB identified by `id`.
///
/// The pointer is valid as long as the thread is not destroyed. It is used
/// to initialise x19 in the saved `TaskContext` of a newly-created thread
/// so that `__switch` can write the kernel SP into `TCB[0]`.
///
/// # Safety
///
/// The returned pointer becomes dangling if the thread is destroyed.
/// Callers must ensure the thread outlives the pointer.
pub unsafe fn tcb_ptr(id: ThreadId) -> *const Thread {
    let table = THREAD_TABLE.lock();
    let thread = table
        .lookup(id)
        .expect("sche: tcb_ptr on invalid thread");
    (thread as *const Thread)
}

/// Update the kernel stack top field after modifying the stack layout.
pub fn set_kernel_stack_top(id: ThreadId, top: usize) {
    let mut table = THREAD_TABLE.lock();
    let thread = table
        .lookup_mut(id)
        .expect("sche: set_kernel_stack_top on invalid thread");
    thread.kernel_stack_top = top;
}
