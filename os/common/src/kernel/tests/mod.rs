//! Kernel test framework — runs tests during boot without `cargo test`.
//!
//! Each test is a `fn() -> bool`.  `run_one` calls the function and prints
//! a PASS / FAIL line via UART.

// ---------------------------------------------------------------------------
// UART output helpers
// ---------------------------------------------------------------------------

const UART0_DR: *mut u8 = 0x09000000 as *mut u8;

fn uart_write(s: &str) {
    for byte in s.bytes() {
        unsafe { core::ptr::write_volatile(UART0_DR, byte); }
    }
}

fn uart_num(n: usize) {
    if n == 0 {
        uart_write("0");
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 0;
    let mut v = n;
    while v > 0 {
        buf[i] = b'0' + (v % 10) as u8;
        i += 1;
        v /= 10;
    }
    while i > 0 {
        i -= 1;
        unsafe { core::ptr::write_volatile(UART0_DR, buf[i]); }
    }
}

// ---------------------------------------------------------------------------
// Sub-modules
// ---------------------------------------------------------------------------

mod bmm_test;
mod cap_test;
mod ipc_test;
mod sche_test;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn run_all() {
    uart_write("\n========== Kernel Unit Tests ==========\n\n");

    let (bmm_p, bmm_t)   = bmm_test::run();
    let (cap_p, cap_t)   = cap_test::run();
    let (ipc_p, ipc_t)   = ipc_test::run();
    let (sche_p, sche_t) = sche_test::run();

    let passed = bmm_p + cap_p + ipc_p + sche_p;
    let total  = bmm_t + cap_t + ipc_t + sche_t;

    uart_write("\n=======================================\n");
    uart_write("  Total: ");
    uart_num(passed);
    uart_write(" / ");
    uart_num(total);
    uart_write(" passed");

    if passed == total {
        uart_write("\n  >>> ALL TESTS PASSED <<<\n");
    } else {
        uart_write("\n  >>> SOME TESTS FAILED <<<\n");
    }
    uart_write("=======================================\n");
}

// ---------------------------------------------------------------------------
// Runner helper
// ---------------------------------------------------------------------------

pub(crate) fn run_one(name: &str, test: fn() -> bool) -> bool {
    uart_write("  ");
    uart_write(name);
    let pad = if name.len() < 50 { 50 - name.len() } else { 0 };
    for _ in 0..pad { uart_write("."); }
    uart_write(" ");
    let ok = test();
    if ok { uart_write("PASS\n"); } else { uart_write("FAIL\n"); }
    ok
}
