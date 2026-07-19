//! IRQ manager — binds hardware interrupt lines to Notification objects.
//!
//! # Flow
//!
//! 1. User calls `sys_irq_register(irq_num)` → kernel creates a Notification,
//!    enables the IRQ line in the GIC, and returns the NotificationId.
//! 2. User calls `sys_notify_wait(nid)` to block until the IRQ fires.
//! 3. When the IRQ fires, `dispatch_irq()` reads GICC_IAR, looks up the
//!    notification, signals it, and the blocked user thread wakes up.
//! 4. User handles the IRQ, then calls `sys_irq_ack(irq_num)` → kernel
//!    writes GICC_EOIR to re-enable the IRQ line.
//!
//! # Locking rule
//!
//! `signal_notification()` must never be called while `IRQ_TABLE` is locked
//! (the notification module internally takes its own lock).  `dispatch_irq`
//! extracts the nid under the lock, then releases it before signalling.

use crate::kernel::ipc::notification::{create_notification, signal_notification, NotificationId};
use aarch64::base::gic;
use spin::Mutex;

// ---------------------------------------------------------------------------
// IRQ entry and table
// ---------------------------------------------------------------------------

const MAX_IRQS: usize = 256;
const SPI_BASE: u32 = 32;

struct IrqEntry {
    nid: NotificationId,
}

static IRQ_TABLE: Mutex<[Option<IrqEntry>; MAX_IRQS]> = Mutex::new([const { None }; MAX_IRQS]);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a hardware IRQ line and create a notification for it.
///
/// # Returns
/// `Ok(NotificationId)` — the notification is created and the IRQ is enabled
/// in the GIC.  The caller should wait on this notification via
/// `sys_notify_wait`.
///
/// # Errors
/// * `-EINVAL` (22) — `irq_num` is not in the valid SPI range (32..256).
/// * `-EEXIST` (17) — this IRQ is already registered.
/// * `-ENOMEM` (12) — notification table is full.
pub fn register_irq(irq_num: u32) -> Result<NotificationId, isize> {
    const EINVAL: isize = 22;
    const EEXIST: isize = 17;
    const ENOMEM:  isize = 12;

    if irq_num < SPI_BASE || irq_num >= MAX_IRQS as u32 {
        return Err(EINVAL);
    }

    let mut table = IRQ_TABLE.lock();
    let idx = irq_num as usize;
    if table[idx].is_some() {
        return Err(EEXIST);
    }

    let nid = create_notification().map_err(|_| ENOMEM)?;

    table[idx] = Some(IrqEntry { nid });

    // Enable the IRQ in the GIC *before* releasing the table lock to
    // ensure no IRQ can fire before the mapping is installed.
    unsafe { gic::gic_enable_irq(irq_num); }

    Ok(nid)
}

/// Acknowledge (EOI) a hardware IRQ line.
///
/// Writes GICC_EOIR unconditionally — even if the IRQ is not currently
/// registered — to prevent a misconfigured interrupt from locking up the
/// system.
///
/// # Errors
/// * `-EINVAL` (22) — `irq_num` is not in the valid SPI range.
pub fn ack_irq(irq_num: u32) -> Result<(), isize> {
    const EINVAL: isize = 22;

    if irq_num < SPI_BASE || irq_num >= MAX_IRQS as u32 {
        return Err(EINVAL);
    }

    unsafe { gic::gic_write_eoir(irq_num); }
    Ok(())
}

/// Dispatch a pending IRQ — called from `handle_user_irq`.
///
/// Reads GICC_IAR to identify the interrupt source, then:
/// - Spurious (IAR=1023) → return immediately.
/// - Timer PPI (#30) → re-arm timer, call `schedule()`.
/// - Registered SPI → signal the associated notification.
/// - Unregistered SPI → write EOI (prevent lock-up).
pub fn dispatch_irq() {
    let iar = unsafe { gic::gic_read_iar() };

    if gic::is_spurious(iar) {
        return;
    }

    let irq_num = gic::iar_irq_num(iar);

    // Timer PPI — handle inline (not GIC-routed; uses system registers)
    if irq_num == 30 {
        aarch64::base::timer::set_next_trigger();
        crate::kernel::timer::check_timeouts();
        crate::kernel::sche::schedule();
        return;
    }

    // Look up registered handler
    let nid = {
        let table = IRQ_TABLE.lock();
        table[irq_num as usize].as_ref().map(|entry| entry.nid)
    }; // lock released

    match nid {
        Some(nid) => {
            // Signal the notification — the waiting user thread will wake
            // and process the IRQ.  We do NOT write EOI here; the user
            // calls sys_irq_ack after handling.
            let _ = signal_notification(nid);
        }
        None => {
            // Unregistered IRQ — write EOI immediately to re-enable the
            // line and prevent the system from hanging on a spurious source.
            unsafe { gic::gic_write_eoir(iar); }
        }
    }
}

/// Initialise the IRQ subsystem.
pub fn init() {
    // Table is const-initialised; nothing to do.  Hook for future setup.
}
