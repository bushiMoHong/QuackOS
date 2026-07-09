//! ReadyQueue — O(1) priority-based scheduler queue with a 256-bit bitmap.
//!
//! # Design
//!
//! ```text
//! bitmap: [u64; 4]         ← one bit per priority level (0=lowest, 255=highest)
//! queues: [RingBuffer; 256] ← FIFO ring buffer per priority level
//! ```
//!
//! * `enqueue(tid, prio)` — set bit in bitmap, push to ring buffer  → O(1)
//! * `dequeue() → ThreadId` — find highest set bit via `leading_zeros()`
//!   (compiles to `clz` on ARM64), pop from ring buffer, clear bit if
//!   empty → O(1)
//!
//! # Priority inheritance integration
//!
//! When a thread's effective priority changes (due to priority donation),
//! the caller must **remove** it from its old queue and **re-insert** it
//! at the new priority.  The `remove()` method on each ring buffer
//! supports this, but it is O(n) in the queue depth — acceptable because
//! priority donation is a rare event.
//!
//! # "Ready" vs "Blocked"
//!
//! This queue only ever holds threads in `ThreadState::Ready`.  A thread
//! that blocks is **removed** from the ReadyQueue (it never sits in the
//! queue with a Blocked state).  `wake()` re-enqueues it.

use super::error::ScheError;
use super::thread::ThreadId;
use spin::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of priority levels.
pub const NUM_PRIORITIES: usize = 256;

/// Max priority value (highest).
pub const MAX_PRIORITY: u8 = 255;

/// Default priority for new threads.
pub const DEFAULT_PRIORITY: u8 = 128;

/// Capacity of each per-priority ring buffer.
const RING_CAP: usize = 64;

/// Number of u64 words needed for a 256-bit bitmap.
const BITMAP_WORDS: usize = 4;

// ---------------------------------------------------------------------------
// RingBuffer — per-priority FIFO queue
// ---------------------------------------------------------------------------

/// Fixed-size ring buffer for one priority level.
///
/// Follows the same pattern as `WaitQueue` in `ipc::channel` and
/// `FaultQueue` in `kernel::bmm`.
struct RingBuffer {
    items: [Option<ThreadId>; RING_CAP],
    head: usize,
    tail: usize,
    count: usize,
}

impl RingBuffer {
    const fn new() -> Self {
        RingBuffer {
            items: [const { None }; RING_CAP],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    /// Push a thread onto the tail of the queue.
    fn push(&mut self, tid: ThreadId) -> Result<(), ScheError> {
        if self.count >= RING_CAP {
            return Err(ScheError::PriorityQueueFull);
        }
        self.items[self.tail] = Some(tid);
        self.tail = (self.tail + 1) % RING_CAP;
        self.count += 1;
        Ok(())
    }

    /// Pop a thread from the head of the queue.
    /// Skips holes left by `remove()` (which already decremented count).
    fn pop(&mut self) -> Option<ThreadId> {
        while self.count > 0 {
            let tid = self.items[self.head].take();
            self.head = (self.head + 1) % RING_CAP;
            if tid.is_some() {
                self.count -= 1;
                return tid;
            }
            // Hole — remove() already decremented count for this slot.
        }
        None
    }

    /// Remove a specific thread from the queue (for priority donation re-insert).
    fn remove(&mut self, tid: ThreadId) -> bool {
        for slot in self.items.iter_mut() {
            if *slot == Some(tid) {
                *slot = None;
                self.count = self.count.saturating_sub(1);
                return true;
            }
        }
        false
    }

    fn is_empty(&self) -> bool {
        self.count == 0
    }
}

// ---------------------------------------------------------------------------
// ReadyQueue
// ---------------------------------------------------------------------------

/// O(1) priority-based ready queue.
///
/// # Locking
///
/// The outer `Mutex` protects both the bitmap and the ring buffers.
/// `enqueue` and `dequeue` are the only public mutators; both are O(1).
pub struct ReadyQueue {
    /// Bitmap: word `w` bit `b` corresponds to priority `w * 64 + b`.
    bitmap: [u64; BITMAP_WORDS],
    /// Per-priority FIFO queues.
    queues: [RingBuffer; NUM_PRIORITIES],
}

impl ReadyQueue {
    /// Create an empty ready queue.
    pub const fn new() -> Self {
        ReadyQueue {
            bitmap: [0u64; BITMAP_WORDS],
            queues: [const { RingBuffer::new() }; NUM_PRIORITIES],
        }
    }

    // ------------------------------------------------------------------
    // Public API
    // ------------------------------------------------------------------

    /// Push a thread onto the ready queue at the given priority.
    ///
    /// The thread must be in `ThreadState::Ready`.
    pub fn enqueue(&mut self, tid: ThreadId, priority: u8) -> Result<(), ScheError> {
        let p = priority as usize;
        if p >= NUM_PRIORITIES {
            return Err(ScheError::InvalidArgument);
        }

        self.queues[p].push(tid)?;

        // Set the corresponding bit in the bitmap.
        let word = p / 64;
        let bit = p % 64;
        self.bitmap[word] |= 1u64 << bit;

        Ok(())
    }

    /// Pop the highest-priority runnable thread.
    ///
    /// Returns `None` if the queue is empty (all threads blocked — idle).
    pub fn dequeue(&mut self) -> Option<ThreadId> {
        // Find the highest non-empty priority using `leading_zeros()`.
        // Iterate words from high to low: higher word index = higher priority.
        for (wi, &word) in self.bitmap.iter().enumerate().rev() {
            if word == 0 {
                continue;
            }
            // `leading_zeros()` on u64: the highest set bit is the highest
            // priority within this word.  `63 - lz` gives the bit index.
            let bit = 63 - word.leading_zeros() as usize;
            let p = wi * 64 + bit;

            let tid = self.queues[p].pop().expect("bitmap set but queue empty");

            // If that priority level is now empty, clear the bitmap bit.
            if self.queues[p].is_empty() {
                self.bitmap[wi] &= !(1u64 << bit);
            }

            return Some(tid);
        }
        None
    }

    /// Remove a specific thread from the queue (O(n) in queue depth of
    /// its priority level).
    ///
    /// Used when a thread's priority changes (priority donation boost /
    /// restore) so it can be re-inserted at the new priority.
    pub fn remove(&mut self, tid: ThreadId, priority: u8) -> bool {
        let p = priority as usize;
        if p >= NUM_PRIORITIES {
            return false;
        }
        let removed = self.queues[p].remove(tid);
        if removed && self.queues[p].is_empty() {
            let word = p / 64;
            let bit = p % 64;
            self.bitmap[word] &= !(1u64 << bit);
        }
        removed
    }

    /// Return `true` if the ready queue is completely empty.
    pub fn is_empty(&self) -> bool {
        self.bitmap.iter().all(|&w| w == 0)
    }

    /// Return the number of runnable threads across all priorities.
    pub fn total_runnable(&self) -> usize {
        self.queues.iter().map(|q| q.count).sum()
    }
}

// ---------------------------------------------------------------------------
// Global ready queue
// ---------------------------------------------------------------------------

/// The global ready queue — the scheduler's single source of truth for
/// which threads are runnable.
static READY_QUEUE: Mutex<ReadyQueue> = Mutex::new(ReadyQueue::new());

// ---------------------------------------------------------------------------
// Public helpers used by context.rs and mod.rs
// ---------------------------------------------------------------------------

/// Enqueue a thread onto the global ready queue.
///
/// Called by `wake()` and at thread creation time.
pub fn enqueue_ready(tid: ThreadId, priority: u8) -> Result<(), ScheError> {
    READY_QUEUE.lock().enqueue(tid, priority)
}

/// Dequeue the highest-priority thread from the global ready queue.
///
/// Called by `schedule()`.  Returns `None` if no thread is runnable.
pub fn dequeue_ready() -> Option<ThreadId> {
    READY_QUEUE.lock().dequeue()
}

/// Remove a thread from the global ready queue (for priority-change re-insert).
pub fn remove_ready(tid: ThreadId, priority: u8) -> bool {
    READY_QUEUE.lock().remove(tid, priority)
}

/// Return `true` if the ready queue is empty.
pub fn is_ready_empty() -> bool {
    READY_QUEUE.lock().is_empty()
}

/// Return the total number of runnable threads.
pub fn runnable_count() -> usize {
    READY_QUEUE.lock().total_runnable()
}
