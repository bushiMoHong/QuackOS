//! Console input / output via the kernel's UART driver.
//!
//! Wraps `sys_console_write` (nr=11) and `sys_console_read` (nr=15).

use core::fmt;

// ---------------------------------------------------------------------------
// Raw wrappers (internal)
// ---------------------------------------------------------------------------

fn raw_write(buf: &[u8]) -> isize {
    unsafe {
        crate::syscall::syscall2(
            crate::syscall::SYS_CONSOLE_WRITE,
            buf.as_ptr() as usize,
            buf.len(),
        ) as isize
    }
}

fn raw_read(buf: &mut [u8]) -> isize {
    unsafe {
        crate::syscall::syscall2(
            crate::syscall::SYS_CONSOLE_READ,
            buf.as_mut_ptr() as usize,
            buf.len(),
        ) as isize
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Write bytes directly to the UART console.
///
/// Returns the number of bytes written (capped to 4096 by kernel).
pub fn write(buf: &[u8]) -> usize {
    let ret = raw_write(buf);
    if ret >= 0 { ret as usize } else { 0 }
}

/// Read pending bytes from the UART console (non-blocking).
///
/// Returns the number of bytes read, or 0 if the RX FIFO is empty.
pub fn read(buf: &mut [u8]) -> usize {
    let ret = raw_read(buf);
    if ret >= 0 { ret as usize } else { 0 }
}

/// Print a string to the console.
pub fn print(s: &str) {
    write(s.as_bytes());
}

/// Print a string followed by a newline.
pub fn println(s: &str) {
    write(s.as_bytes());
    write(b"\n");
}

// ---------------------------------------------------------------------------
// fmt::Write support (enables `write!` and `writeln!` macros)
// ---------------------------------------------------------------------------

/// A writer that sends formatted text to the UART console.
pub struct ConsoleWriter;

impl fmt::Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write(s.as_bytes());
        Ok(())
    }
}

/// Prints to the console using `core::fmt`.
///
/// Equivalent to [`print`] but accepts format arguments.
#[macro_export]
macro_rules! qprint {
    ($($arg:tt)*) => {
        {
            use core::fmt::Write;
            let _ = write!($crate::console::ConsoleWriter, $($arg)*);
        }
    };
}

/// Prints to the console with a newline, using `core::fmt`.
///
/// Equivalent to [`println`] but accepts format arguments.
#[macro_export]
macro_rules! qprintln {
    () => { $crate::console::write(b"\n"); };
    ($($arg:tt)*) => {
        {
            use core::fmt::Write;
            let _ = write!($crate::console::ConsoleWriter, "{}\n", format_args!($($arg)*));
        }
    };
}
