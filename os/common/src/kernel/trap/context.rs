//! Re-export of the architecture-specific trap frame types.
//!
//! These come from the `aarch64` library crate.  When additional
//! architectures are supported they will be selected via `#[cfg]`.

pub use aarch64::base::trap::TrapFrame;
pub use aarch64::base::trap::ExceptionKind;
pub use aarch64::base::trap::ExceptionSource;
pub use aarch64::base::trap::GeneralRegs;
pub use aarch64::base::trap::UserContext;
