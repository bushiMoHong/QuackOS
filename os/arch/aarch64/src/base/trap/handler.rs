//! Top-level C-ABI trap handler called from `vector.S`.
//!
//! # Calling convention
//!
//! `vector.S` calls `extern "C" fn trap_handler(tf: &mut TrapFrame)`.
//!
//! `tf` points to the fully saved register state on the stack.
//! On return, the assembly will restore context from this frame and `eret`.
//!
//! # Handler storage
//!
//! Each handler callback is a standalone thin function pointer (single `usize`).
//! No trait objects, no fat pointers, no niche optimisation — just six
//! `static mut` values.  This avoids the rlib linking issue where
//! `Option<&dyn TrapHandler>` fat-pointer storage behaves unpredictably
//! when the arch crate is an rlib linked into a separate binary.

use super::context::{ExceptionKind, ExceptionSource, TrapFrame};
use core::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Six handler function pointers — one per exception category
// ---------------------------------------------------------------------------

type TrapFn = unsafe fn(tf: &mut TrapFrame);

static mut HANDLE_USER_SYNC:   Option<TrapFn> = None;
static mut HANDLE_USER_IRQ:    Option<TrapFn> = None;
static mut HANDLE_KERNEL_SYNC: Option<TrapFn> = None;
static mut HANDLE_KERNEL_IRQ:  Option<TrapFn> = None;
static mut HANDLE_FIQ:         Option<TrapFn> = None;
static mut HANDLE_SERROR:      Option<TrapFn> = None;

/// Set once `set_trap_handler` has been called.
static HANDLER_INSTALLED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Install / register
// ---------------------------------------------------------------------------

/// Install the per-category trap handler functions.
///
/// Each `TrapFn` is a thin `unsafe fn(&mut TrapFrame)` — no data pointer,
/// no vtable.  If the handler needs state, use a `static` or `#[no_mangle]`
/// global in the binary crate and reference it from within the function.
///
/// # Safety
///
/// Call once during boot, before enabling IRQs or entering userspace.
pub fn set_trap_fns(
    user_sync: TrapFn,
    user_irq: TrapFn,
    kernel_sync: TrapFn,
    kernel_irq: TrapFn,
    fiq: TrapFn,
    serror: TrapFn,
) {
    unsafe {
        HANDLE_USER_SYNC   = Some(user_sync);
        HANDLE_USER_IRQ    = Some(user_irq);
        HANDLE_KERNEL_SYNC = Some(kernel_sync);
        HANDLE_KERNEL_IRQ  = Some(kernel_irq);
        HANDLE_FIQ         = Some(fiq);
        HANDLE_SERROR      = Some(serror);
    }
    HANDLER_INSTALLED.store(true, Ordering::Release);
}

/// Return `true` if a trap handler has been installed.
pub fn is_handler_installed() -> bool {
    HANDLER_INSTALLED.load(Ordering::Acquire)
}

// ---------------------------------------------------------------------------
// Trait interface (for convenience in the binary crate)
// ---------------------------------------------------------------------------

/// Trait for implementing a complete set of trap handlers.
///
/// Implement this, then call `install_trap_handler::<T>()` to register
/// the trait methods as individual function pointers.
pub trait TrapHandler {
    fn handle_user_sync(tf: &mut TrapFrame);
    fn handle_user_irq(tf: &mut TrapFrame);
    fn handle_kernel_sync(tf: &mut TrapFrame);
    fn handle_kernel_irq(tf: &mut TrapFrame);
    fn handle_fiq(tf: &mut TrapFrame);
    fn handle_serror(tf: &mut TrapFrame);
}

/// Register a `TrapHandler` implementation as the global trap handler.
///
/// Example:
/// ```ignore
/// struct MyHandler;
/// impl TrapHandler for MyHandler { ... }
/// install_trap_handler::<MyHandler>();
/// ```
pub fn install_trap_handler<H: TrapHandler>() {
    set_trap_fns(
        H::handle_user_sync,
        H::handle_user_irq,
        H::handle_kernel_sync,
        H::handle_kernel_irq,
        H::handle_fiq,
        H::handle_serror,
    );
}

// ---------------------------------------------------------------------------
// C entry point — called from vector.S
// ---------------------------------------------------------------------------

/// The assembly-level trap entry dispatches here.
#[no_mangle]
pub unsafe extern "C" fn trap_handler(tf: &mut TrapFrame) {
    let source = tf.source();
    let kind = tf.kind();

    match (source, kind) {
        // ---------- userspace ----------
        (ExceptionSource::LowerAArch64, ExceptionKind::Synchronous) => {
            HANDLE_USER_SYNC.expect("user sync handler not installed")(tf);
        }
        (ExceptionSource::LowerAArch64, ExceptionKind::Irq) => {
            HANDLE_USER_IRQ.expect("user IRQ handler not installed")(tf);
        }
        (ExceptionSource::LowerAArch64, ExceptionKind::Fiq) => {
            HANDLE_FIQ.expect("FIQ handler not installed")(tf);
        }
        (ExceptionSource::LowerAArch64, ExceptionKind::SError) => {
            HANDLE_SERROR.expect("SError handler not installed")(tf);
        }

        // ---------- kernel ----------
        (ExceptionSource::CurrentSpEl0 | ExceptionSource::CurrentSpElx, ExceptionKind::Synchronous) => {
            HANDLE_KERNEL_SYNC.expect("kernel sync handler not installed")(tf);
        }
        (ExceptionSource::CurrentSpEl0 | ExceptionSource::CurrentSpElx, ExceptionKind::Irq) => {
            HANDLE_KERNEL_IRQ.expect("kernel IRQ handler not installed")(tf);
        }
        (ExceptionSource::CurrentSpEl0 | ExceptionSource::CurrentSpElx, ExceptionKind::Fiq) => {
            HANDLE_FIQ.expect("FIQ handler not installed")(tf);
        }
        (ExceptionSource::CurrentSpEl0 | ExceptionSource::CurrentSpElx, ExceptionKind::SError) => {
            HANDLE_SERROR.expect("SError handler not installed")(tf);
        }

        // ---------- AArch32 (unexpected) ----------
        (ExceptionSource::LowerAArch32, _) => {
            panic!("unexpected AArch32 exception: {:?} {:?}", source, kind);
        }
    }
}

// ---------------------------------------------------------------------------
// Default handler
// ---------------------------------------------------------------------------

/// Register a handler that panics on every unhandled trap.
pub fn install_default_handler() {
    set_trap_fns(
        default_user_sync,
        default_unhandled_irq,
        default_kernel_sync,
        default_unhandled_irq,
        default_unhandled_irq,
        default_unhandled_irq,
    );
}

unsafe fn default_user_sync(tf: &mut TrapFrame) {
    panic!("unhandled user sync: elr={:#018x}", tf.elr);
}

unsafe fn default_kernel_sync(tf: &mut TrapFrame) {
    panic!("unhandled kernel sync: elr={:#018x}", tf.elr);
}

unsafe fn default_unhandled_irq(tf: &mut TrapFrame) {
    panic!("unhandled exception at elr={:#018x}", tf.elr);
}
