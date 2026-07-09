//! Channel management — the kernel object that connects two processes for IPC.
//!
//! # Design
//!
//! A `Channel` is the rendezvous point for synchronous IPC.  Each channel has:
//!
//! * `receiver_queue` — threads waiting to receive (arrived before sender)
//! * `sender_queue`   — threads waiting to send    (arrived before receiver)
//!
//! # Locking
//!
//! A single global `Mutex<ChannelTable>` protects all channels.  The
//! `with_channel()` closure pattern keeps lock scoping explicit.
//!
//! **Rule:** `schedule()` must **never** be called while a channel lock is
//! held.  The closure returns the matching decision; the caller drops the
//! lock, then performs transfer and scheduling outside the lock.
//!
//! # Future: fine-grained locking
//!
//! When SMP support lands the global table lock will be split into per-channel
//! locks.  The `with_channel()` closure signature stays the same — only the
//! internals change.  This will likely require `unsafe` or a separate inner
//! array to satisfy the borrow checker when releasing the outer lock while
//! holding the inner one.

use super::error::IpcError;
use super::message::Message;
use spin::Mutex;

// ---------------------------------------------------------------------------
// Thread / channel identifiers
// ---------------------------------------------------------------------------

/// Thread identifier — re-exported from the kernel scheduler.
pub use crate::kernel::sche::ThreadId;

/// Channel identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelId(pub u32);

// ---------------------------------------------------------------------------
// WaitEntry — a thread parked on a channel
// ---------------------------------------------------------------------------

/// Why a thread is blocked on a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    /// Sender arrived first, waiting for a receiver.
    Send,
    /// Receiver arrived first, waiting for a sender.
    Receive,
    /// Caller sent and is waiting for a reply.
    Call,
}

/// An entry in a channel's wait-queue.
///
/// When the sender arrives first the message travels with the `WaitEntry`
/// so that the later receiver can retrieve it directly.  When the receiver
/// arrives first `msg` is `None`.
#[derive(Debug, Clone)]
pub struct WaitEntry {
    pub thread_id: ThreadId,
    pub reason: BlockReason,
    pub msg: Option<Message>,
}

// ---------------------------------------------------------------------------
// WaitQueue — fixed-size ring buffer (matches `bmm::FaultQueue` style)
// ---------------------------------------------------------------------------

const WAIT_QUEUE_CAP: usize = 32;

pub(crate) struct WaitQueue {
    items: [Option<WaitEntry>; WAIT_QUEUE_CAP],
    head: usize,
    tail: usize,
    count: usize,
}

impl WaitQueue {
    const fn new() -> Self {
        WaitQueue {
            items: [const { None }; WAIT_QUEUE_CAP],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    pub(crate) fn push(&mut self, entry: WaitEntry) -> Result<(), WaitEntry> {
        if self.count >= WAIT_QUEUE_CAP {
            return Err(entry);
        }
        self.items[self.tail] = Some(entry);
        self.tail = (self.tail + 1) % WAIT_QUEUE_CAP;
        self.count += 1;
        Ok(())
    }

    pub(crate) fn pop(&mut self) -> Option<WaitEntry> {
        if self.count == 0 {
            return None;
        }
        let entry = self.items[self.head].take();
        self.head = (self.head + 1) % WAIT_QUEUE_CAP;
        self.count -= 1;
        entry
    }

    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.count == 0
    }
}

// ---------------------------------------------------------------------------
// Channel
// ---------------------------------------------------------------------------

/// Mutable interior of a channel.
///
/// Fields are `pub(crate)` because `WaitQueue` is intentionally opaque
/// outside of the IPC subsystem — all queue manipulation goes through
/// `match_sender()` / `match_receiver()`.
pub struct ChannelInner {
    pub(crate) receiver_queue: WaitQueue,
    pub(crate) sender_queue: WaitQueue,
}

impl ChannelInner {
    pub(crate) const fn new() -> Self {
        ChannelInner {
            receiver_queue: WaitQueue::new(),
            sender_queue: WaitQueue::new(),
        }
    }
}

/// An IPC channel connecting two (or more) processes.
pub struct Channel {
    pub channel_id: ChannelId,
    pub capability_id: u32,
    pub inner: ChannelInner,
}

impl Channel {
    const fn new(id: ChannelId, capability_id: u32) -> Self {
        Channel {
            channel_id: id,
            capability_id,
            inner: ChannelInner::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Matching result enums (module scope so they name-resolve)
// ---------------------------------------------------------------------------

/// Outcome of `ChannelInner::match_sender()`.
pub enum SendMatch {
    /// A receiver was already waiting — deliver to this thread.
    Matched(ThreadId),
    /// No receiver yet — message was cloned into sender queue.
    Parked,
}

/// Outcome of `ChannelInner::match_receiver()`.
pub enum RecvMatch {
    /// A sender was waiting — its `WaitEntry` carries the message.
    Matched(WaitEntry),
    /// No sender yet — receiver is parked.
    Parked,
}

// ---------------------------------------------------------------------------
// Matching helpers
// ---------------------------------------------------------------------------

impl ChannelInner {
    /// Try to match an incoming sender against a waiting receiver.
    ///
    /// Takes `msg` by reference so the caller retains ownership for the
    /// `Matched` case (needed by `transfer::deliver`).  A clone is stored
    /// in the queue for the `Parked` case.
    pub fn match_sender(
        &mut self,
        thread_id: ThreadId,
        msg: &Message,
    ) -> Result<SendMatch, IpcError> {
        if let Some(receiver) = self.receiver_queue.pop() {
            Ok(SendMatch::Matched(receiver.thread_id))
        } else {
            let entry = WaitEntry {
                thread_id,
                reason: BlockReason::Send,
                msg: Some(msg.clone()),
            };
            self.sender_queue
                .push(entry)
                .map_err(|_| IpcError::ChannelTableFull)?;
            Ok(SendMatch::Parked)
        }
    }

    /// Try to match an incoming receiver against a waiting sender.
    ///
    /// Returns the sender's `WaitEntry` (which carries the message) on
    /// match; otherwise enqueues the receiver.
    pub fn match_receiver(
        &mut self,
        thread_id: ThreadId,
    ) -> Result<RecvMatch, IpcError> {
        if let Some(sender) = self.sender_queue.pop() {
            Ok(RecvMatch::Matched(sender))
        } else {
            let entry = WaitEntry {
                thread_id,
                reason: BlockReason::Receive,
                msg: None,
            };
            self.receiver_queue
                .push(entry)
                .map_err(|_| IpcError::ChannelTableFull)?;
            Ok(RecvMatch::Parked)
        }
    }
}

// ---------------------------------------------------------------------------
// Global channel table
// ---------------------------------------------------------------------------

const MAX_CHANNELS: usize = 64;

struct ChannelTable {
    slots: [Option<Channel>; MAX_CHANNELS],
    next_id: u32,
}

impl ChannelTable {
    const fn new() -> Self {
        ChannelTable {
            slots: [const { None }; MAX_CHANNELS],
            next_id: 0,
        }
    }

    fn get(&self, id: ChannelId) -> Option<&Channel> {
        self.slots
            .iter()
            .find_map(|slot| slot.as_ref().filter(|ch| ch.channel_id == id))
    }

    fn get_mut(&mut self, id: ChannelId) -> Option<&mut Channel> {
        self.slots
            .iter_mut()
            .find_map(|slot| slot.as_mut().filter(|ch| ch.channel_id == id))
    }

    fn insert(&mut self, channel: Channel) -> Result<ChannelId, IpcError> {
        let id = channel.channel_id;
        for slot in self.slots.iter_mut() {
            if slot.is_none() {
                *slot = Some(channel);
                return Ok(id);
            }
        }
        Err(IpcError::ChannelTableFull)
    }

    fn remove(&mut self, id: ChannelId) -> Result<(), IpcError> {
        for slot in self.slots.iter_mut() {
            if slot.as_ref().is_some_and(|ch| ch.channel_id == id) {
                *slot = None;
                return Ok(());
            }
        }
        Err(IpcError::InvalidChannel)
    }

    fn alloc_id(&mut self) -> ChannelId {
        let id = ChannelId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        id
    }
}

static CHANNEL_TABLE: Mutex<ChannelTable> = Mutex::new(ChannelTable::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new IPC channel.
///
/// The returned `ChannelId` is a capability-bearing handle — the caller is
/// expected to distribute capabilities to processes that need to communicate
/// through this channel.
pub fn create_channel(capability_id: u32) -> Result<ChannelId, IpcError> {
    let mut table = CHANNEL_TABLE.lock();
    let id = table.alloc_id();
    let channel = Channel::new(id, capability_id);
    table.insert(channel)?;
    Ok(id)
}

/// Destroy an IPC channel.
///
/// Any threads currently blocked on the channel will **not** be woken
/// (they will remain blocked indefinitely).  In production this should
/// be preceded by a channel-teardown protocol that wakes waiters first.
pub fn destroy_channel(id: ChannelId) -> Result<(), IpcError> {
    CHANNEL_TABLE.lock().remove(id)
}

/// Access a channel's inner state through a closure.
///
/// The table lock is held for the duration of `f`.  **Callers must not
/// call `schedule()` inside `f`** — that would deadlock if another thread
/// tries to access the same table.
///
/// # Future
///
/// When SMP lands this will be upgraded to two-level locking where the table
/// lock is released before `f` executes.
pub fn with_channel<R>(
    id: ChannelId,
    f: impl FnOnce(&mut ChannelInner) -> Result<R, IpcError>,
) -> Result<R, IpcError> {
    let mut table = CHANNEL_TABLE.lock();
    let channel = table
        .get_mut(id)
        .ok_or(IpcError::InvalidChannel)?;
    f(&mut channel.inner)
}

/// Return the number of currently active channels (for debugging).
pub fn channel_count() -> usize {
    CHANNEL_TABLE
        .lock()
        .slots
        .iter()
        .filter(|s| s.is_some())
        .count()
}

/// Return `true` if a channel with the given ID exists.
pub fn channel_exists(id: ChannelId) -> bool {
    CHANNEL_TABLE.lock().get(id).is_some()
}
