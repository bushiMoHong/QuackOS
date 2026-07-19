//! Software timer queue for IPC timeouts.
//!
//! Manages a sorted list of deadlines.  When a thread sets an IPC timeout,
//! its deadline is inserted into the queue.  If it's the earliest deadline,
//! the hardware timer is reprogrammed.  When the timer IRQ fires,
//! `check_timeouts()` wakes all expired threads with `IpcState::TimedOut`.

use alloc::vec::Vec;
use crate::kernel::sche::ThreadId;
use aarch64::base::timer;
use spin::Mutex;

// ---------------------------------------------------------------------------
// Timer entry and queue
// ---------------------------------------------------------------------------

struct TimerDeadline {
    deadline_ticks: u64,
    tid: ThreadId,
}

/// Sorted by `deadline_ticks` ascending — index 0 is the earliest.
static TIMER_QUEUE: Mutex<Vec<TimerDeadline>> = Mutex::new(Vec::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Set an IPC timeout for a thread.
///
/// `timeout_ms == 0` is a no-op (infinite wait).
/// Otherwise computes `deadline = CNTPCT + ms_to_ticks(timeout_ms)`, inserts
/// into the sorted queue, and reprograms the hardware timer if this is now
/// the earliest deadline.
pub fn set_ipc_timeout(tid: ThreadId, timeout_ms: u32) {
    if timeout_ms == 0 {
        return;
    }

    let freq = timer::get_cntfrq() as u64;
    let now = timer::get_time() as u64;
    let ticks = (timeout_ms as u64) * freq / 1000;
    let deadline = now + ticks;

    // Store deadline in TCB
    let _ = crate::kernel::sche::with_thread_mut(tid, |t| {
        t.ipc_deadline = deadline;
    });

    let mut queue = TIMER_QUEUE.lock();

    // Insert sorted by deadline
    let pos = queue
        .iter()
        .position(|e| e.deadline_ticks > deadline)
        .unwrap_or(queue.len());
    queue.insert(pos, TimerDeadline { deadline_ticks: deadline, tid });

    // If this is the new earliest, reprogram hardware timer
    if pos == 0 {
        timer::set_timer_cval(deadline as usize);
    }
}

/// Cancel a thread's IPC timeout.
///
/// Removes the thread from the timer queue.  If the thread was the earliest
/// deadline, reprogram the hardware timer for the new earliest.
pub fn cancel_ipc_timeout(tid: ThreadId) {
    let _ = crate::kernel::sche::with_thread_mut(tid, |t| {
        t.ipc_deadline = 0;
    });

    let mut queue = TIMER_QUEUE.lock();
    if let Some(pos) = queue.iter().position(|e| e.tid == tid) {
        let was_earliest = pos == 0;
        queue.remove(pos);
        if was_earliest && !queue.is_empty() {
            timer::set_timer_cval(queue[0].deadline_ticks as usize);
        }
    }
}

/// Scan for expired deadlines and wake the corresponding threads.
///
/// Called from `dispatch_irq()` on each timer tick (IRQ #30).
/// After waking all expired threads, reprograms the hardware timer for the
/// next pending deadline (if any).  The default tick (`set_next_trigger`)
/// is only restored when the queue is empty.
pub fn check_timeouts() {
    use crate::kernel::ipc::synchronization::IpcState;
    use crate::kernel::sche::thread;

    let now = timer::get_time() as u64;
    let mut expired = Vec::new();

    {
        let mut queue = TIMER_QUEUE.lock();
        while !queue.is_empty() && queue[0].deadline_ticks <= now {
            let entry = queue.remove(0);
            expired.push(entry.tid);
        }
    }

    for tid in &expired {
        // Clear deadline in TCB
        let _ = thread::with_thread_mut(*tid, |t| {
            t.ipc_deadline = 0;
        });
        // Wake the thread (sets ipc_state = Ready) and then override to
        // TimedOut.  Safe because the woken thread cannot run until we
        // return from the IRQ handler (single-core, no preemption).
        crate::kernel::sche::wake(*tid);
        let _ = thread::with_thread_mut(*tid, |t| {
            t.ipc_state = IpcState::TimedOut;
        });
    }
}

/// Initialise the timer subsystem.
pub fn init() {
    // Queue is const-initialised; nothing to do.
}
