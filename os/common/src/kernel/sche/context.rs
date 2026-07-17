//! Context switching — the heart of the scheduler.
//!
//! # Anatomy of a context switch
//!
//! ```text
//! schedule()
//!   ├── dequeue next thread from ReadyQueue  (O(1) bitmap, brief lock)
//!   ├── CAS current.state: Running → Ready   (atomic, no table lock)
//!   ├── CAS next.state:    Ready   → Running (atomic, no table lock)
//!   ├── update CURRENT_THREAD
//!   └── __switch(next.kernel_stack_top)      (arch asm, NO locks held)
//! ```
//!
//! # TCB pointer convention
//!
//! The current TCB address is passed explicitly to `__switch(current_tcb, next_sp)`.
//! It is also saved/restored in the TaskContext at offset 0x08 (x19 slot)
//! so that it persists across context switches.
//!
//! `kernel_stack_top` (the first field at offset 0 in TCB) is
//! automatically updated by `__switch` via `str sp, [current_tcb]`.
//!
//! # Locking rule
//!
//! `schedule()` **must never** be called while any spin lock (ReadyQueue,
//! channel lock, …) is held.  The caller must release all locks before
//! calling `schedule()`.  This matches the IPC channel locking rule.

use super::queue;
use super::thread::{self, ThreadId, ThreadState};
use aarch64::base::switch::__switch;
use spin::Mutex;

// ---------------------------------------------------------------------------
// Per-CPU current thread
// ---------------------------------------------------------------------------

/// Tracks which thread is currently running on this CPU.
///
/// Updated by `schedule()` just before `__switch`.  Read by
/// `current_thread()` from any context (IRQ, trap handler, …).
static CURRENT: Mutex<ThreadId> = Mutex::new(ThreadId::NULL);

/// Return the `ThreadId` of the currently executing thread.
///
/// # Returns
///
/// `ThreadId::NULL` if the scheduler has not yet been initialised
/// (e.g. during early boot before `sche::init()`).
pub fn current_thread() -> ThreadId {
    *CURRENT.lock()
}

/// Set the current-thread tracker — called from `schedule()` and `init()`.
fn set_current(tid: ThreadId) {
    *CURRENT.lock() = tid;
}

// ---------------------------------------------------------------------------
// schedule() — pick next and switch
// ---------------------------------------------------------------------------

/// Yield the CPU to the next runnable thread.
///
/// # What happens
///
/// 1. Dequeue the highest-priority thread from the `ReadyQueue`.
/// 2. If none is runnable → idle loop (WFI stub for now).
/// 3. Transition current thread from `Running` to its new state:
///    * If still runnable → `Ready` + re-enqueue.
///    * If `Blocked` → left alone (caller is `block_current`).
///    * If `Dying` → left alone for cleanup.
/// 4. Transition next thread from `Ready` to `Running`.
/// 5. Update `CURRENT`.
/// 6. Invoke `__switch(next.kernel_stack_top)`.
///
/// # Safety
///
/// * Must only be called from kernel mode with a valid kernel stack.
/// * **No** spin locks may be held across this call (deadlock risk —
///   another thread might try to acquire the same lock before this
///   thread runs again).
pub fn schedule() {
    let current_tid = current_thread();

    // 1. Pick next thread (briefly locks ReadyQueue).
    let maybe_next = queue::dequeue_ready();

    // 2. Handle idle: no runnable thread available.
    let next_tid = match maybe_next {
        Some(tid) => tid,
        None => {
            // All threads are blocked.  In production we'd execute WFI
            // (wait-for-interrupt) and resume when a timer or device IRQ
            // fires.  For now spin — acceptable on single-core.
            log::warn!("sche: idle (no runnable threads); spinning");
            return;
        }
    };

    // 3. Transition current thread's state (briefly locks thread table).
    if !current_tid.is_null() {
        let _ = thread::with_thread(current_tid, |t| {
            let state = t.atomic_state();
            match state {
                ThreadState::Running => {
                    t.set_atomic_state(ThreadState::Ready);
                }
                ThreadState::Blocked => {
                    // `block_current()` set this before calling schedule().
                    // The thread is already on a wait-queue (e.g. a channel).
                    // Leave it alone — `wake()` will re-enqueue it later.
                }
                ThreadState::Dying => {
                    // Thread is being destroyed.  Leave it for cleanup.
                }
                _ => {
                    log::error!(
                        "sche: unexpected current state {:?} for {:?}",
                        state,
                        current_tid,
                    );
                }
            }
        });

        // Re-enqueue if the current thread was Running (it is now Ready).
        if let Ok(should_reenqueue) = thread::with_thread(current_tid, |t| {
            t.atomic_state() == ThreadState::Ready
        }) {
            if should_reenqueue {
                let prio = thread::with_thread(current_tid, |t| t.effective_priority())
                    .unwrap_or(queue::DEFAULT_PRIORITY);
                let _ = queue::enqueue_ready(current_tid, prio);
            }
        }
    }

    // 4. Transition next thread: Ready → Running.
    let _ = thread::with_thread(next_tid, |t| {
        t.set_atomic_state(ThreadState::Running);
    });

    // 5. Update per-CPU tracking.
    set_current(next_tid);

    // 6. Read the next thread's kernel stack pointer and current TCB address.
    let next_sp = thread::kernel_stack_top(next_tid);
    let current_tcb = unsafe { thread::tcb_ptr(current_tid) } as usize;

    // 7. Context switch — **no locks held from this point**.
    //
    // SAFETY:
    // - current_tcb points to the current thread's TCB (Thread struct),
    //   obtained from the thread table while current_tid is still valid.
    // - next_sp points to a valid 128-byte saved TaskContext on the
    //   next thread's kernel stack.
    // - All spin locks (thread table, ready queue, IPC channels) are
    //   released — no deadlock risk.
    unsafe {
        __switch(current_tcb, next_sp);
    }

    // Execution resumes here when another thread switches **back** to us.
}

// ---------------------------------------------------------------------------
// block_current() — block the calling thread and yield
// ---------------------------------------------------------------------------

use crate::kernel::ipc::synchronization::IpcState;

/// Block the calling thread and immediately yield the CPU.
///
/// # Flow
///
/// 1. Set current thread's state to `Blocked` with the given `ipc_state`.
/// 2. Call `schedule()` — which will **not** re-enqueue us.
///
/// # Safety
///
/// Same preconditions as `schedule()`: all locks released, kernel stack valid.
pub unsafe fn block_current(ipc_state: IpcState) {
    let current_tid = current_thread();

    if !current_tid.is_null() {
        let _ = thread::with_thread_mut(current_tid, |t| {
            t.set_atomic_state(ThreadState::Blocked);
            t.ipc_state = ipc_state;
        });
        log::debug!(
            "sche: blocked {:?} state={:?}",
            current_tid,
            ipc_state,
        );
    }

    schedule();
}

// ---------------------------------------------------------------------------
// wake() — mark a thread ready and enqueue it
// ---------------------------------------------------------------------------

/// Wake a blocked thread and push it onto the ready queue.
///
/// # Precondition
///
/// The caller must have completed any data delivery (IPC buffer, page
/// mapping) **before** calling `wake()`, so the thread sees valid data
/// when it resumes.
pub fn wake(tid: ThreadId) {
    if tid.is_null() {
        return;
    }

    let prio = thread::with_thread_mut(tid, |t| {
        t.set_atomic_state(ThreadState::Ready);
        t.ipc_state = IpcState::Ready;
        t.effective_priority()
    });

    match prio {
        Ok(p) => {
            if let Err(e) = queue::enqueue_ready(tid, p) {
                log::error!("sche: wake {:?} failed to enqueue: {:?}", tid, e);
            } else {
                log::debug!("sche: woke {:?} prio={}", tid, p);
            }
        }
        Err(e) => {
            log::error!("sche: wake {:?} failed: {:?}", tid, e);
        }
    }
}

// ---------------------------------------------------------------------------
// Idle thread bootstrapping
// ---------------------------------------------------------------------------

/// Bootstrap the first (idle / init) thread into the scheduler.
///
/// Called once during `sche::init()`.  Registers the boot thread as the
/// currently-running thread and initialises x19 to point to its TCB.
///
/// # Safety
///
/// Must be called exactly once, before any other scheduler operation.
pub unsafe fn bootstrap_idle(tid: ThreadId) {
    set_current(tid);

    // Mark the boot thread as Running.
    let _ = thread::with_thread(tid, |t| {
        t.set_atomic_state(ThreadState::Running);
    });

    // Initialise x19 = address of the boot thread's TCB.
    //
    // We can't take the address of the TCB directly from safe Rust
    // (it lives inside a Mutex).  Instead we use `with_thread` to get
    // the pointer and then write it into x19.
    //
    // TODO: use inline asm to set x19.  For now the warm-up trick:
    // call `__switch(self)` once so that x19 is loaded from the context.
    //
    // Actually, on the very first call to __switch, x19 must already
    // be set.  We do this in the arch init path or with a small asm
    // trampoline.  For now, the boot thread never blocks — we skip
    // the __switch path until there are at least 2 threads.

    log::info!("sche: bootstrapped idle thread {:?}", tid);
}
