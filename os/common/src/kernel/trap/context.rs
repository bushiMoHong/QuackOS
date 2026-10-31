//! Re-export of the architecture-specific trap frame types.
//!
//! These come from the `aarch64` library crate.  When additional
//! architectures are supported they will be selected via `#[cfg]`.

pub use crate::arch::base::trap::TrapFrame;
pub use crate::arch::base::trap::ExceptionKind;
pub use crate::arch::base::trap::ExceptionSource;
pub use crate::arch::base::trap::GeneralRegs;
pub use crate::arch::base::trap::UserContext;
