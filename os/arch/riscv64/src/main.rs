#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;

global_asm!(include_str!("boot.S"));

/// RISC-V QEMU virt 平台 UART0 基地址
const UART0_DR: *mut u8 = 0x10000000 as *mut u8;

fn print_uart(s: &str) {
    for byte in s.bytes() {
        unsafe {
            core::ptr::write_volatile(UART0_DR, byte);
        }
    }
}

/// 对应 boot.S 中 call rust_main 的符号
#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    print_uart("Hello, QuackOS on QEMU RiscV!\n");

    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
