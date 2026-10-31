//! 架构抽象层：根据目标架构 re-export 对应 arch crate 的内容。
//!
//! 上层代码通过 `crate::arch::base::*` 访问架构相关功能。

#[cfg(target_arch = "aarch64")]
pub use aarch64::*;

#[cfg(target_arch = "riscv64")]
pub use riscv64::*;
