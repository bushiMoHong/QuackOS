#![no_std]

extern crate alloc;

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        let _ = core::fmt::Write::write_fmt(
            &mut $crate::base::sbi::SbiConsole,
            format_args!($($arg)*),
        );
    }};
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

pub mod base;
// 兼容旧代码中的 `crate::arch::*` 路径。
pub use base as arch;
