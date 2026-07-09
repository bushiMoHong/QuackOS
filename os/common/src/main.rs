#![no_std]
#![no_main]

extern crate alloc;

use core::arch::global_asm;
use core::panic::PanicInfo;
use linked_list_allocator::LockedHeap;

use kernel::trap::{self, TrapFrame, TrapHandler};

pub mod kernel;
pub mod usr;

global_asm!(include_str!("boot_arm64.S"));

// ---------------------------------------------------------------------------
// Kernel heap — 256 KB static array
// ---------------------------------------------------------------------------

const HEAP_SIZE: usize = 256 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

const UART0_DR: *mut u8 = 0x09000000 as *mut u8;

fn print_uart(s: &str) {
    for byte in s.bytes() {
        unsafe {
            core::ptr::write_volatile(UART0_DR, byte);
        }
    }
}

fn print_uart_hex(mut v: u64) {
    print_uart("0x");
    for i in 0..16 {
        let nibble = ((v >> (60 - i * 4)) & 0xF) as u8;
        let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
        unsafe { core::ptr::write_volatile(UART0_DR, c) };
    }
}

// ---------------------------------------------------------------------------
// Trap test infrastructure
// ---------------------------------------------------------------------------

/// Static buffer for dumping registers after trap return.
/// Layout: [x1, x2, x3, x4, x5, x6] — 6 slots
static mut REG_DUMP: [u64; 6] = [0; 6];

/// Trap handler used during the register save/restore test.
struct TestHandler;

impl TrapHandler for TestHandler {
    fn handle_user_sync(_tf: &mut TrapFrame) {
        print_uart("[FAIL] Unexpected user sync trap!\n");
    }

    fn handle_user_irq(_tf: &mut TrapFrame) {
        // ignore for now
    }

    fn handle_kernel_sync(tf: &mut TrapFrame) {
        print_uart("\n>>> Kernel sync trap caught (SVC #0)\n");

        // Print saved register values to verify the save path
        print_uart("Saved registers from trap frame:\n");
        print_uart("  x1 = ");
        print_uart_hex(tf.general.x1 as u64);
        print_uart("\n  x2 = ");
        print_uart_hex(tf.general.x2 as u64);
        print_uart("\n  x3 = ");
        print_uart_hex(tf.general.x3 as u64);
        print_uart("\n  x4 = ");
        print_uart_hex(tf.general.x4 as u64);
        print_uart("\n  x5 = ");
        print_uart_hex(tf.general.x5 as u64);
        print_uart("\n  x6 = ");
        print_uart_hex(tf.general.x6 as u64);
        print_uart("\n");

        // Check save: verify that the values we loaded before SVC arrived intact
        if tf.general.x1 == 0x1111
            && tf.general.x2 == 0x2222
            && tf.general.x3 == 0x3333
            && tf.general.x4 == 0x4444
            && tf.general.x5 == 0x5555
            && tf.general.x6 == 0x6666
        {
            print_uart("  [PASS] Register save verified!\n");
        } else {
            print_uart("  [FAIL] Register save mismatch!\n");
        }

        // Modify registers — these must be restored by trap_return
        tf.general.x1 = 0xBEEF;
        tf.general.x2 = 0xDEAD;
        tf.general.x3 = 0xCAFE;
        tf.general.x4 = 0xFACE;
        tf.general.x5 = 0x1234;
        tf.general.x6 = 0x5678;

        print_uart("Modified trap frame: x1->BEEF x2->DEAD x3->CAFE x4->FACE x5->1234 x6->5678\n");
    }

    fn handle_kernel_irq(_tf: &mut TrapFrame) {
        // ignore timer IRQs for now
    }

    fn handle_fiq(_tf: &mut TrapFrame) {
        print_uart("[FAIL] Unexpected FIQ!\n");
    }

    fn handle_serror(_tf: &mut TrapFrame) {
        print_uart("[FAIL] Unexpected SError!\n");
    }
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    // Initialise the kernel heap allocator.
    unsafe {
        ALLOCATOR.lock().init(HEAP.as_mut_ptr(), HEAP_SIZE);
    }

    print_uart("Hello, QuackOS on QEMU ARM!\n");

    // --- Initialise the trap subsystem ---
    unsafe {
        trap::init();
    }
    trap::install_trap_handler::<TestHandler>();
    print_uart("Trap system initialised (VBAR + handler installed).\n");

    // --- Run kernel unit tests ---
    kernel::tests::run_all();

    // --- Test register save/restore via SVC round-trip ---
    print_uart("\n=== Testing register save/restore via SVC #0 ===\n");
    print_uart("Loading test values into regs...\n");

    unsafe {
        core::arch::asm!(
            // Load known values into registers
            "mov x1, #0x1111",
            "mov x2, #0x2222",
            "mov x3, #0x3333",
            "mov x4, #0x4444",
            "mov x5, #0x5555",
            "mov x6, #0x6666",

            // Trigger SVC — traps into handle_kernel_sync above
            "svc #0",

            // After trap_return, registers hold the values the handler set.
            // Dump them to the static buffer for Rust-side verification.
            "adrp x0, {reg_dump}",
            "add  x0, x0, :lo12:{reg_dump}",
            "stp  x1,  x2,  [x0]",
            "stp  x3,  x4,  [x0, #16]",
            "stp  x5,  x6,  [x0, #32]",

            reg_dump = sym REG_DUMP,
            clobber_abi("C"),
        );
    }

    // --- Verify restored register values ---
    let regs = unsafe { REG_DUMP };
    print_uart("\nRegisters after trap return:\n");
    print_uart("  x1 = ");
    print_uart_hex(regs[0]);
    print_uart("\n  x2 = ");
    print_uart_hex(regs[1]);
    print_uart("\n  x3 = ");
    print_uart_hex(regs[2]);
    print_uart("\n  x4 = ");
    print_uart_hex(regs[3]);
    print_uart("\n  x5 = ");
    print_uart_hex(regs[4]);
    print_uart("\n  x6 = ");
    print_uart_hex(regs[5]);
    print_uart("\n");

    let mut all_pass = true;

    if regs[0] == 0xBEEF
        && regs[1] == 0xDEAD
        && regs[2] == 0xCAFE
        && regs[3] == 0xFACE
        && regs[4] == 0x1234
        && regs[5] == 0x5678
    {
        print_uart("[PASS] Register restore verified for all 6 regs!\n");
    } else {
        print_uart("[FAIL] Register restore mismatch!\n");
        print_uart("       Expected: x1=BEEF x2=DEAD x3=CAFE x4=FACE x5=1234 x6=5678\n");
        all_pass = false;
    }

    if all_pass {
        print_uart("\n=== All tests PASSED ===\n");
    } else {
        print_uart("\n=== Some tests FAILED ===\n");
    }

    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    print_uart("PANIC! ");
    if let Some(loc) = info.location() {
        print_uart(loc.file());
        print_uart(" L");
        let line = loc.line();
        print_uart_hex(line as u64);
    }
    print_uart("\n");
    loop {}
}
