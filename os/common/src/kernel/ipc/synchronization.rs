//! Thread-state transitions for synchronous IPC.
//!
//! # Design
//!
//! This module is a thin wrapper around `kernel::sche` that provides
//! IPC-specific thread-state semantics.  The scheduler's `ReadyQueue`
//! is the single source of truth for which threads are runnable.
//!
//! | Function          | Effect                                         |
//! |-------------------|------------------------------------------------|
//! | `block_current()` | Mark current TCB as Blocked → `schedule()`     |
//! | `wake()`          | Mark target TCB as Ready → push to ReadyQueue  |
//!
//! # Locking rule
//!
//! `schedule()` must **never** be called while any IPC lock (channel lock,
//! channel-table lock) is held.  The caller must release all locks before
//! calling `block_current()`.

use super::channel::{ChannelId, ThreadId};
use super::notification::NotificationId;

// ---------------------------------------------------------------------------
// IPC thread state
// ---------------------------------------------------------------------------

/// Thread state specific to IPC — stored in the TCB.
///
/// The scheduler uses this to decide whether a thread is runnable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcState {
    /// Thread is runnable (no IPC operation pending).
    Ready,
    /// Blocked until a receiver arrives on the given channel.
    BlockedOnSend(ChannelId),
    /// Blocked until a sender arrives on the given channel.
    BlockedOnReceive(ChannelId),
    /// Sent a message and blocked waiting for a reply.
    BlockedOnCall(ChannelId),
    /// Blocked waiting for an asynchronous notification.
    BlockedOnNotify(NotificationId),
    /// IPC operation timed out.
    TimedOut,
    /// Blocked in wait4, waiting for a child thread to exit.
    BlockedOnWait4,
}

impl IpcState {
    /// Return `true` if the thread is blocked (not runnable).
    pub fn is_blocked(&self) -> bool {
        !matches!(self, IpcState::Ready)
    }
}

// ---------------------------------------------------------------------------
// Current thread identity — delegated to the scheduler
// ---------------------------------------------------------------------------

/// Return the calling thread's ID.
///
/// Delegates to `kernel::sche::current_thread()` which reads from the
/// per-CPU current-thread tracker (updated on every context switch).
pub fn current_thread() -> ThreadId {
    crate::kernel::sche::current_thread()
}

// ---------------------------------------------------------------------------
// Block / wake — delegated to the scheduler
// ---------------------------------------------------------------------------

/// Block the calling thread on an IPC operation.
///
/// # What happens
///
/// 1. Mark the current TCB with `state`.
/// 2. Invoke the scheduler to switch to the next runnable thread.
///
/// When the thread is later woken by `wake()`, it resumes execution
/// *after* this call with its IPC receive buffer already populated.
///
/// # Safety
///
/// The caller must guarantee:
/// - **All** IPC locks (channel, channel-table) have been released.
/// - The thread's register context has been saved (by the trap frame).
/// - The message (if sender) is already stashed in the channel's `WaitEntry`.
pub unsafe fn block_current(state: IpcState) {
    log::debug!("thread {:?} blocking: {:?}", current_thread(), state);
    crate::kernel::sche::block_current(state);
}

/// Wake a blocked thread and return it to the scheduler's ready queue.
///
/// # Precondition
///
/// The caller must have already completed `transfer::deliver()` for this
/// thread, so that when the thread resumes its receive buffer is filled.
///
/// # What happens
///
/// 1. Mark the target TCB with `IpcState::Ready`.
/// 2. Push the TCB onto the scheduler's `ReadyQueue`.
pub fn wake(thread_id: ThreadId) {
    log::debug!("thread {:?} woken", thread_id);
    crate::kernel::sche::wake(thread_id);
}
