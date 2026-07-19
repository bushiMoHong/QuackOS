//! Notification kernel objects — asynchronous binary-semaphore signals.
//!
//! # Design
//!
//! A `Notification` is a lightweight kernel object implementing a binary
//! semaphore.  It has no message payload — just a "signaled" flag and a
//! FIFO wait queue of blocked threads.
//!
//! | Operation | Behaviour |
//! |-----------|-----------|
//! | `signal`  | Pop and wake the first waiter, or set `signaled = true` if none. |
//! | `wait`    | Clear `signaled` and return immediately if set; otherwise park the thread. |
//!
//! # Locking rule
//!
//! `wake()` and `block_current()` must never be called while the global
//! `NOTIFICATION_TABLE` lock is held.  Both `signal_notification` and
//! `wait_on_notification` release the lock before calling these functions.

use super::error::IpcError;
use super::synchronization::{block_current, current_thread, wake, IpcState};
use crate::kernel::sche::ThreadId;
use spin::Mutex;

// ---------------------------------------------------------------------------
// Identifier
// ---------------------------------------------------------------------------

/// Unique identifier for a notification object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationId(pub u32);

// ---------------------------------------------------------------------------
// Wait queue (ThreadId only — no message payload)
// ---------------------------------------------------------------------------

const WAIT_QUEUE_CAP: usize = 32;

struct NotifyWaitQueue {
    items: [Option<ThreadId>; WAIT_QUEUE_CAP],
    head: usize,
    tail: usize,
    count: usize,
}

impl NotifyWaitQueue {
    const fn new() -> Self {
        NotifyWaitQueue {
            items: [const { None }; WAIT_QUEUE_CAP],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    fn push(&mut self, tid: ThreadId) -> Result<(), ThreadId> {
        if self.count >= WAIT_QUEUE_CAP {
            return Err(tid);
        }
        self.items[self.tail] = Some(tid);
        self.tail = (self.tail + 1) % WAIT_QUEUE_CAP;
        self.count += 1;
        Ok(())
    }

    fn pop(&mut self) -> Option<ThreadId> {
        if self.count == 0 {
            return None;
        }
        let entry = self.items[self.head].take();
        self.head = (self.head + 1) % WAIT_QUEUE_CAP;
        self.count -= 1;
        entry
    }
}

// ---------------------------------------------------------------------------
// Notification object
// ---------------------------------------------------------------------------

pub struct Notification {
    pub id: NotificationId,
    pub signaled: bool,
    pub wait_queue: NotifyWaitQueue,
}

impl Notification {
    const fn new(id: NotificationId) -> Self {
        Notification {
            id,
            signaled: false,
            wait_queue: NotifyWaitQueue::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Global notification table
// ---------------------------------------------------------------------------

const MAX_NOTIFICATIONS: usize = 64;

struct NotificationTable {
    slots: [Option<Notification>; MAX_NOTIFICATIONS],
    next_id: u32,
}

impl NotificationTable {
    const fn new() -> Self {
        NotificationTable {
            slots: [const { None }; MAX_NOTIFICATIONS],
            next_id: 0,
        }
    }

    fn get_mut(&mut self, id: NotificationId) -> Option<&mut Notification> {
        self.slots
            .iter_mut()
            .find_map(|slot| slot.as_mut().filter(|n| n.id == id))
    }

    fn insert(&mut self, notification: Notification) -> Result<NotificationId, IpcError> {
        let id = notification.id;
        for slot in self.slots.iter_mut() {
            if slot.is_none() {
                *slot = Some(notification);
                return Ok(id);
            }
        }
        Err(IpcError::ChannelTableFull)
    }

    fn remove(&mut self, id: NotificationId) -> Result<(), IpcError> {
        for slot in self.slots.iter_mut() {
            if slot.as_ref().is_some_and(|n| n.id == id) {
                *slot = None;
                return Ok(());
            }
        }
        Err(IpcError::InvalidChannel)
    }

    fn alloc_id(&mut self) -> NotificationId {
        let id = NotificationId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        id
    }
}

static NOTIFICATION_TABLE: Mutex<NotificationTable> = Mutex::new(NotificationTable::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new notification object.  Returns its `NotificationId`.
pub fn create_notification() -> Result<NotificationId, IpcError> {
    let mut table = NOTIFICATION_TABLE.lock();
    let id = table.alloc_id();
    let notification = Notification::new(id);
    table.insert(notification)
}

/// Destroy a notification object.
///
/// Any threads currently blocked on this notification will remain blocked
/// indefinitely — this matches the existing `destroy_channel` behaviour.
pub fn destroy_notification(id: NotificationId) -> Result<(), IpcError> {
    NOTIFICATION_TABLE.lock().remove(id)
}

/// Signal a notification — binary-semaphore post (V) operation.
///
/// If a thread is waiting on this notification, the first waiter is popped
/// and woken.  Otherwise the `signaled` flag is set so the next call to
/// `wait_on_notification` returns immediately.
pub fn signal_notification(id: NotificationId) -> Result<(), IpcError> {
    let waiter = {
        let mut table = NOTIFICATION_TABLE.lock();
        let notif = table.get_mut(id).ok_or(IpcError::InvalidChannel)?;

        if let Some(waiter) = notif.wait_queue.pop() {
            Some(waiter)
        } else {
            notif.signaled = true;
            None
        }
    }; // table lock dropped

    if let Some(waiter) = waiter {
        wake(waiter);
    }
    Ok(())
}

/// Wait on a notification — binary-semaphore wait (P) operation.
///
/// If the notification is already signaled, the flag is cleared and the call
/// returns immediately.  Otherwise the calling thread is parked in the wait
/// queue and blocks until another thread calls `signal_notification`.
pub fn wait_on_notification(id: NotificationId) -> Result<(), IpcError> {
    let tid = current_thread();

    let should_block = {
        let mut table = NOTIFICATION_TABLE.lock();
        let notif = table.get_mut(id).ok_or(IpcError::InvalidChannel)?;

        if notif.signaled {
            notif.signaled = false;
            false
        } else {
            notif.wait_queue.push(tid).map_err(|_| IpcError::ChannelTableFull)?;
            true
        }
    }; // table lock dropped

    if should_block {
        unsafe { block_current(IpcState::BlockedOnNotify(id)); }
    }
    Ok(())
}

/// Return `true` if a notification with the given ID exists.
pub fn notification_exists(id: NotificationId) -> bool {
    NOTIFICATION_TABLE.lock().get_mut(id).is_some()
}
