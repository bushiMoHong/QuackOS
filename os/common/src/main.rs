#![no_std]
#![no_main]

extern crate alloc;

use core::arch::global_asm;
use core::panic::PanicInfo;
use linked_list_allocator::LockedHeap;

use kernel::trap;

pub mod kernel;
pub mod usr;

global_asm!(include_str!("boot_arm64.S"));

// ---------------------------------------------------------------------------
// Kernel heap — 256 KB static array
// ---------------------------------------------------------------------------

const HEAP_SIZE: usize = 8 * 1024 * 1024; // 8 MB — page cache needs ~4KB per cached page
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

const UART0_DR: *mut u8 = 0x09000000 as *mut u8;

pub fn print_uart(s: &str) {
    for byte in s.bytes() {
        unsafe {
            core::ptr::write_volatile(UART0_DR, byte);
        }
    }
}

pub fn print_uart_hex(mut v: u64) {
    print_uart("0x");
    for i in 0..16 {
        let nibble = ((v >> (60 - i * 4)) & 0xF) as u8;
        let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
        unsafe { core::ptr::write_volatile(UART0_DR, c) };
    }
}

pub fn print_uart_num(n: usize) {
    if n == 0 {
        print_uart("0");
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
        unsafe { core::ptr::write_volatile(UART0_DR, buf[i]) };
    }
}

// ---------------------------------------------------------------------------
// Linker symbols
// ---------------------------------------------------------------------------

extern "C" {
    static __bss_end: u8;
}

// ---------------------------------------------------------------------------
// MMU setup
// ---------------------------------------------------------------------------

/// Physical address of the L0 page table, stored so init.rs can add user
/// mappings into the same page table.
pub(crate) static KERNEL_L0_PA: spin::Mutex<usize> = spin::Mutex::new(0);

/// Physical address of the L2 table for VA sub-range 0x0–0x1FFFFF within
/// L0[0]→L1[0], where user pages and MMIO blocks coexist.
pub(crate) static KERNEL_L2_LOW_PA: spin::Mutex<usize> = spin::Mutex::new(0);

/// Set up page tables and enable the MMU.
///
/// Page table structure (4KB granule, 48-bit VA):
/// ```text
/// L0[0] → L1
///   L1[0] → L2 (VA 0x0–0x3FFF_FFFF)
///     L2[0x40] = 2MB device block for GIC   (VA 0x08000000)
///     L2[0x48] = 2MB device block for UART  (VA 0x09000000)
///     L2[0x50] = 2MB device block for virtio (VA 0x0A000000)
///     (remaining L2 entries: free for user L3 tables)
///   L1[1] = 1GB normal WB block for RAM (VA 0x40000000–0x7FFFFFFF)
/// ```
///
/// After this call, virtual addresses == physical addresses for all kernel
/// memory (identity mapped). User pages go into L0[0]→L1[0]'s unused L2 space.
fn setup_mmu() {
    use core::arch::asm;

    print_uart("[MMU] setting up page tables...\n");

    // Static BSS page tables — avoids allocator/memset issues.
    #[repr(align(4096))]
    struct PageTablePage([u64; 512]);

    static mut L0: PageTablePage = PageTablePage([0u64; 512]);
    static mut L1: PageTablePage = PageTablePage([0u64; 512]);
    static mut L2_LO: PageTablePage = PageTablePage([0u64; 512]);

    let l0 = unsafe { &raw mut L0.0 as *mut u64 };
    let l1 = unsafe { &raw mut L1.0 as *mut u64 };
    let l2_lo = unsafe { &raw mut L2_LO.0 as *mut u64 };
    let l0_pa = l0 as usize;
    let l1_pa = l1 as usize;
    print_uart("[MMU] PT addrs: L0=");
    print_uart_hex(l0_pa as u64);
    print_uart(" L1=");
    print_uart_hex(l1_pa as u64);
    print_uart(" L2=");
    print_uart_hex(l2_lo as usize as u64);
    print_uart("\n");

    // ---- L0[0] → L1 (VA 0x0–0x7F_FFFF_FFFF, 512GB) ----
    // ---- L1[0] → L2_lo (VA 0x0–0x3FFF_FFFF, 1GB, MMIO + user pages) ----
    // ---- L1[1] = 1GB RAM block (VA 0x40000000–0x7FFFFFFF) ----
    unsafe {
        l0.add(0).write_volatile((l1_pa as u64) | 0b11);
        l1.add(0).write_volatile((l2_lo as usize as u64) | 0b11);
        l1.add(1).write_volatile(
            (0x40000000u64)
                | (2 << 2)     // AttrIndx 2 = normal WB
                | (1 << 5)     // NS = non-secure output address
                | (0b11 << 8)  // inner shareable
                | (0b00 << 6)  // AP = EL1 RW, EL0 RW
                | (1 << 10)    // AF
                | (1 << 54)    // UXN
                // PXN = 0 — kernel code executable
                | 0b01,        // block, valid (at L1)
        );
    }
    print_uart("[MMU] L0/L1 entries written\n");

    // ---- L2_lo: 2MB device blocks for UART + VirtIO ----
    fn device_block(paddr: u64) -> u64 {
        (paddr & 0x0000_FFFF_FFE0_0000)
            | (0 << 2)     // AttrIndx 0 = device nGnRnE
            | (1 << 5)     // NS = non-secure output address
            | (0b11 << 8)  // inner shareable
            | (0b00 << 6)  // AP = EL1 RW, EL0 RW
            | (1 << 10)    // AF
            | (1 << 54)    // UXN
            | (1 << 53)    // PXN
            | 0b01
    }
    unsafe {
        l2_lo.add(0x40).write_volatile(device_block(0x08000000)); // GIC
        l2_lo.add(0x48).write_volatile(device_block(0x09000000)); // UART
        l2_lo.add(0x50).write_volatile(device_block(0x0A000000)); // VirtIO
    }
    print_uart("[MMU] device blocks written\n");

    // ---- Store roots for init.rs ----
    *KERNEL_L0_PA.lock() = l0_pa;
    *KERNEL_L2_LOW_PA.lock() = l2_lo as usize;

    // ---- Configure MAIR_EL1 ----
    let mair: u64 =
        0x00            // Attr0: Device-nGnRnE
        | (0x44 << 8)   // Attr1: Normal, non-cacheable
        | (0xFF << 16); // Attr2: Normal, WB, Read/Write Allocate
    unsafe { asm!("msr mair_el1, {}", in(reg) mair); }

    // ---- Configure TCR_EL1 ----
    let tcr: u64 =
        (16 << 0)      // T0SZ = 16 → 48-bit VA
        | (16 << 16)   // T1SZ = 16
        | (1 << 23)    // EPD1 = disable TTBR1 walks
        | (0b00 << 14) // TG0 = 4KB granule
        | (0b11 << 12) // SH0 = inner shareable
        | (0b01 << 10) // ORGN0 = normal WB cacheable
        | (0b01 << 8); // IRGN0 = normal WB cacheable
    unsafe { asm!("msr tcr_el1, {}", in(reg) tcr); }

    // ---- Set page table bases ----
    unsafe {
        asm!("msr ttbr0_el1, {}", in(reg) l0_pa);
        asm!("msr ttbr1_el1, {}", in(reg) 0u64);
    }

    // ---- Synchronise & enable MMU ----
    print_uart("[MMU] enabling MMU...\n");
    unsafe { asm!("dsb ishst; tlbi vmalle1is; dsb ish; isb"); }
    unsafe {
        asm!(
            "mrs x1, sctlr_el1",
            "orr x1, x1, #1",
            "msr sctlr_el1, x1",
            "isb",
            "mov w2, #77",
            "strb w2, [x3]",
            out("x1") _,
            in("x3") 0x09000000u64,
        );
    }
    print_uart("MU enabled\n");
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    // 1. Initialise the kernel heap allocator.
    unsafe {
        ALLOCATOR.lock().init(HEAP.as_mut_ptr(), HEAP_SIZE);
    }

    print_uart("Hello, QuackOS on QEMU ARM!\n");

    // 2. Initialise the trap subsystem.
    unsafe {
        trap::init();
    }
    print_uart("Trap system initialised.\n");

    // 3. Register physical memory after the kernel image.
    let bss_end = unsafe { &__bss_end as *const u8 as usize };
    // Align up to 2MB boundary for safety, then free up to 512MB mark.
    let free_start = (bss_end + 0x1FFFFF) & !0x1FFFFF;
    let ram_end = 0x40000000 + 512 * 1024 * 1024 - 0x100000; // 512MB - 1MB for DTB/QEMU
    if free_start < ram_end {
        aarch64::base::mm::free_page_range(free_start, ram_end);
        print_uart("Physical memory registered: ");
        print_uart_hex(free_start as u64);
        print_uart(" - ");
        print_uart_hex(ram_end as u64);
        print_uart("\n");
    }

    // 4. Enable MMU.
    setup_mmu();

    // 5. Initialise the scheduler.
    kernel::sche::init();
    print_uart("Scheduler initialised.\n");

    // 5.5. Initialise the IRQ subsystem.
    kernel::irq::init();

    // 5.6. Enable IRQs at the CPU level (clear DAIF.I).
    // Must be done after GIC init (in trap::init) and trap handler install.
    unsafe {
        trap::irq_enable();
    }
    print_uart("IRQs enabled.\n");

    // 6. Run the init process — load and execute /bin/bash.
    usr::init::run_init();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    print_uart("PANIC! ");
    if let Some(loc) = info.location() {
        print_uart(loc.file());
        print_uart(" L");
        print_uart_num(loc.line() as usize);
    }
    print_uart("\n");
    loop {}
}
