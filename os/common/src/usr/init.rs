//! Init process — loads `/bin/bash` + liblinux, spawns the liblinux user
//! thread, and enters the FsServer IPC loop.
//!
//! Phase 0 boot flow:
//!
//! 1. Mount ext4, load /bin/bash ELF, map segments into user space.
//! 2. Load liblinux ELF from embedded bytes, map into user space.
//! 3. Write BootInfo at a fixed user address (bash entry, stack, brk).
//! 4. Create a user-mode thread that runs liblinux.
//! 5. Boot thread becomes the FsServer IPC event loop.

use alloc::sync::Arc;
use alloc::vec;
use core::arch::asm;

use crate::kernel::bmm::MapFlags;
use crate::usr::drivers::VirtIOBlockDev;
use crate::usr::fs::dev::block_dev::BlockDevice;
use crate::usr::fs::ext4::fs::Ext4FileSystem;
use crate::usr::fs::ext4::inode::load_inode;
use crate::usr::fs::server::FsServer;
use crate::usr::fs::types::{OpenFlags, SeekWhence};
use crate::print_uart;
use crate::print_uart_hex;

use aarch64::base::mm::{
    alloc_page, VirtPageNum, PhysPageNum,
};
use aarch64::base::config::PAGE_SIZE;
use aarch64::base::mm::page_table::{
    PageTable, PTEFlags,
};

// ---------------------------------------------------------------------------
// Embedded liblinux ELF (built by Makefile before the kernel)
// ---------------------------------------------------------------------------
const LIBLINUX_ELF: &[u8] = include_bytes!("../../../liblinux/target/aarch64-unknown-none/release/liblinux");

// ---------------------------------------------------------------------------
// BootInfo — passed from kernel to liblinux
// ---------------------------------------------------------------------------

/// Fixed user-space address where BootInfo is placed.
/// Must match liblinux's expected address.
const BOOTINFO_ADDR: usize = 0x7FFF_FFEF_FFA0;

#[repr(C)]
struct BootInfo {
    bash_entry: u64,
    stack_top: u64,
    brk: u64,
}

// ---------------------------------------------------------------------------
// Page-table helpers
// ---------------------------------------------------------------------------

fn kernel_page_table() -> PageTable {
    let l0_pa = *crate::KERNEL_L0_PA.lock();
    PageTable::from_token(l0_pa)
}

fn map_user_page(pt: &mut PageTable, vaddr: usize, paddr: usize, flags: MapFlags) {
    let vpn = VirtPageNum::from(vaddr >> 12);
    let ppn = PhysPageNum::from(paddr >> 12);

    let mut ptef = PTEFlags::empty();
    ptef.insert(PTEFlags::V);
    ptef.insert(PTEFlags::A);
    ptef.insert(PTEFlags::D);
    if flags.contains(MapFlags::READ)  { ptef.insert(PTEFlags::R); }
    if flags.contains(MapFlags::WRITE) { ptef.insert(PTEFlags::W); }
    if flags.contains(MapFlags::EXEC)  { ptef.insert(PTEFlags::X); }
    if flags.contains(MapFlags::USER)  { ptef.insert(PTEFlags::U); }

    pt.map(vpn, ppn, ptef);
}

fn map_user_pages_anon(pt: &mut PageTable, vaddr_start: usize, count: usize, flags: MapFlags) {
    for i in 0..count {
        let va = vaddr_start + i * PAGE_SIZE;
        let pa = alloc_page().expect("OOM mapping user anon pages");
        unsafe { core::ptr::write_bytes(pa as *mut u8, 0, PAGE_SIZE); }
        map_user_page(pt, va, pa, flags);
    }
}

// ---------------------------------------------------------------------------
// ELF64 constants
// ---------------------------------------------------------------------------

const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

// ---------------------------------------------------------------------------
// ELF loader from byte slice (for embedded liblinux)
// ---------------------------------------------------------------------------

/// Load an ELF from a byte slice, mapping PT_LOAD segments into user space.
/// Returns (entry_point, data_end).
fn load_elf_from_bytes(pt: &mut PageTable, elf_bytes: &[u8]) -> (usize, usize) {
    assert!(elf_bytes.len() >= 64, "ELF too small");
    assert_eq!(elf_bytes[0..4], ELF_MAGIC, "Invalid ELF magic");

    let entry = u64::from_le_bytes(elf_bytes[0x18..0x20].try_into().unwrap()) as usize;
    let phoff = u64::from_le_bytes(elf_bytes[0x20..0x28].try_into().unwrap()) as usize;
    let phentsize = u16::from_le_bytes(elf_bytes[0x36..0x38].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(elf_bytes[0x38..0x3A].try_into().unwrap()) as usize;

    let mut data_end: usize = 0;

    for i in 0..phnum {
        let off = phoff + i * phentsize;
        if off + 48 > elf_bytes.len() { break; }

        let p_type = u32::from_le_bytes(elf_bytes[off..off+4].try_into().unwrap());
        if p_type != PT_LOAD { continue; }

        let p_flags  = u32::from_le_bytes(elf_bytes[off+4..off+8].try_into().unwrap());
        let p_offset = u64::from_le_bytes(elf_bytes[off+8..off+16].try_into().unwrap()) as usize;
        let p_vaddr  = u64::from_le_bytes(elf_bytes[off+16..off+24].try_into().unwrap()) as usize;
        let p_filesz = u64::from_le_bytes(elf_bytes[off+32..off+40].try_into().unwrap()) as usize;
        let p_memsz  = u64::from_le_bytes(elf_bytes[off+40..off+48].try_into().unwrap()) as usize;

        let mut mapf = MapFlags::empty();
        mapf.0 |= MapFlags::USER;
        if p_flags & PF_R != 0 { mapf.0 |= MapFlags::READ; }
        if p_flags & PF_W != 0 { mapf.0 |= MapFlags::WRITE; }
        if p_flags & PF_X != 0 { mapf.0 |= MapFlags::EXEC; }

        let seg_end = p_vaddr + p_memsz;
        if seg_end > data_end { data_end = seg_end; }

        let start_page = p_vaddr & !(PAGE_SIZE - 1);
        let end_page = (p_vaddr + p_memsz + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        for page_va in (start_page..end_page).step_by(PAGE_SIZE) {
            let pa = alloc_page().expect("OOM mapping ELF segment");

            let page_off = page_va.wrapping_sub(p_vaddr);
            let copy_start = page_off;
            let copy_end = (page_off + PAGE_SIZE).min(p_filesz);

            if copy_start < p_filesz {
                let len = copy_end - copy_start;
                let file_off = p_offset + copy_start;
                if file_off + len <= elf_bytes.len() {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            elf_bytes.as_ptr().add(file_off),
                            pa as *mut u8,
                            len,
                        );
                    }
                }
            }

            // Zero BSS portion
            if copy_end < PAGE_SIZE {
                let zero_start = if page_off > p_filesz { 0 } else { p_filesz - page_off };
                unsafe {
                    core::ptr::write_bytes((pa + zero_start) as *mut u8, 0, PAGE_SIZE - zero_start);
                }
            }

            map_user_page(pt, page_va, pa, mapf);
        }
    }

    (entry, data_end)
}

// ---------------------------------------------------------------------------
// run_init
// ---------------------------------------------------------------------------

pub fn run_init() -> ! {
    print_uart("\n=== Init: starting Phase 0 bootstrap ===\n");

    // ------------------------------------------------------------------
    // 1. Create the virtio-blk device + mount ext4
    // ------------------------------------------------------------------
    let virtio_blk = VirtIOBlockDev::new(0x0a000000)
        .expect("Failed to init virtio-blk at 0x0a000000");
    let block_device: Arc<dyn BlockDevice> = Arc::new(virtio_blk);

    print_uart("VirtIO block device ready\n");

    let ext4_fs = Ext4FileSystem::open(block_device.clone());
    let root_inode = load_inode(2, block_device.clone(), ext4_fs.clone());
    let fs = Arc::new(FsServer::new(root_inode));

    print_uart("Ext4 filesystem mounted\n");

    // ------------------------------------------------------------------
    // 2. Open and load /bin/bash (for file info we need to read)
    // ------------------------------------------------------------------
    let fd = fs.open(0, "/bin/bash", OpenFlags::O_RDONLY, 0)
        .expect("Failed to open /bin/bash");
    let stat = fs.fstat(0, fd).expect("Failed to stat /bin/bash");
    print_uart("/bin/bash opened: size=");
    print_uart_hex(stat.size);
    print_uart("\n");

    // ------------------------------------------------------------------
    // 3. Parse /bin/bash ELF header via FsServer
    // ------------------------------------------------------------------
    let mut elf_hdr = [0u8; 64];
    fs.read_to(0, fd, &mut elf_hdr).expect("Failed to read ELF header");
    assert_eq!(elf_hdr[0..4], ELF_MAGIC, "Invalid bash ELF magic");

    let bash_entry = u64::from_le_bytes(elf_hdr[0x18..0x20].try_into().unwrap()) as usize;
    let phoff     = u64::from_le_bytes(elf_hdr[0x20..0x28].try_into().unwrap()) as usize;
    let phentsize = u16::from_le_bytes(elf_hdr[0x36..0x38].try_into().unwrap()) as usize;
    let phnum     = u16::from_le_bytes(elf_hdr[0x38..0x3A].try_into().unwrap()) as usize;

    print_uart("bash ELF entry point: ");
    print_uart_hex(bash_entry as u64);
    print_uart("\n");

    // ------------------------------------------------------------------
    // 4. Load liblinux ELF from embedded bytes
    // ------------------------------------------------------------------
    let mut pt = kernel_page_table();
    print_uart("Loading liblinux from embedded ELF (");
    print_uart_hex(LIBLINUX_ELF.len() as u64);
    print_uart(" bytes)...\n");

    let (liblinux_entry, _liblinux_end) = load_elf_from_bytes(&mut pt, LIBLINUX_ELF);
    print_uart("liblinux entry: ");
    print_uart_hex(liblinux_entry as u64);
    print_uart("\n");

    // ------------------------------------------------------------------
    // 5. Load /bin/bash program headers and map segments (same as before)
    // ------------------------------------------------------------------
    let ph_size = phnum * phentsize;
    let mut ph_buf = vec![0u8; ph_size];
    fs.lseek(0, fd, phoff as isize, SeekWhence::Set).ok();
    fs.read_to(0, fd, &mut ph_buf).expect("Failed to read program headers");

    let mut data_end: usize = 0;

    for i in 0..phnum {
        let off = i * phentsize;
        let p_type  = u32::from_le_bytes(ph_buf[off..off+4].try_into().unwrap());
        if p_type != PT_LOAD { continue; }

        let p_flags  = u32::from_le_bytes(ph_buf[off+4..off+8].try_into().unwrap());
        let p_offset = u64::from_le_bytes(ph_buf[off+8..off+16].try_into().unwrap()) as usize;
        let p_vaddr  = u64::from_le_bytes(ph_buf[off+16..off+24].try_into().unwrap()) as usize;
        let p_filesz = u64::from_le_bytes(ph_buf[off+32..off+40].try_into().unwrap()) as usize;
        let p_memsz  = u64::from_le_bytes(ph_buf[off+40..off+48].try_into().unwrap()) as usize;

        let mut mapf = MapFlags::empty();
        mapf.0 |= MapFlags::USER;
        if p_flags & PF_R != 0 { mapf.0 |= MapFlags::READ; }
        if p_flags & PF_W != 0 { mapf.0 |= MapFlags::WRITE; }
        if p_flags & PF_X != 0 { mapf.0 |= MapFlags::EXEC; }

        print_uart("  PT_LOAD: va=");
        print_uart_hex(p_vaddr as u64);
        print_uart(" ms=");
        print_uart_hex(p_memsz as u64);
        print_uart("\n");

        let seg_end = p_vaddr + p_memsz;
        if seg_end > data_end { data_end = seg_end; }

        let start_page = p_vaddr & !(PAGE_SIZE - 1);
        let end_page = (p_vaddr + p_memsz + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        for page_va in (start_page..end_page).step_by(PAGE_SIZE) {
            let pa = alloc_page().expect("OOM mapping bash segment");

            let page_off = page_va.wrapping_sub(p_vaddr);
            let copy_start = page_off;
            let copy_end = (page_off + PAGE_SIZE).min(p_filesz);

            if copy_start < p_filesz {
                let len = copy_end - copy_start;
                let file_pos = p_offset + copy_start;
                fs.lseek(0, fd, file_pos as isize, SeekWhence::Set).ok();
                let mut tmp_buf = [0u8; 4096];
                fs.read_to(0, fd, &mut tmp_buf[..len]).expect("Failed to read segment data");
                unsafe {
                    core::ptr::copy_nonoverlapping(tmp_buf.as_ptr(), pa as *mut u8, len);
                }
            }

            // Zero BSS
            if copy_end < PAGE_SIZE {
                let zero_start = if page_off > p_filesz { 0 } else { p_filesz - page_off };
                unsafe {
                    core::ptr::write_bytes((pa + zero_start) as *mut u8, 0, PAGE_SIZE - zero_start);
                }
            }

            map_user_page(&mut pt, page_va, pa, mapf);
        }
    }

    fs.close(0, fd).ok();
    let initial_brk = (data_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    print_uart("bash segments mapped, brk=");
    print_uart_hex(initial_brk as u64);
    print_uart("\n");

    // ------------------------------------------------------------------
    // 6. Map user stack (128 KiB at top of user address space)
    // ------------------------------------------------------------------
    let stack_top = 0x7FFF_FFF0_0000;
    let stack_pages = 32;
    let stack_bottom = stack_top - stack_pages * PAGE_SIZE;
    let stack_flags = MapFlags(MapFlags::READ | MapFlags::WRITE | MapFlags::USER);
    map_user_pages_anon(&mut pt, stack_bottom, stack_pages, stack_flags);

    print_uart("User stack mapped: ");
    print_uart_hex(stack_bottom as u64);
    print_uart(" - ");
    print_uart_hex(stack_top as u64);
    print_uart("\n");

    // ------------------------------------------------------------------
    // 7. Write BootInfo for liblinux
    // ------------------------------------------------------------------
    let bootinfo = BootInfo {
        bash_entry: bash_entry as u64,
        stack_top: stack_top as u64,
        brk: initial_brk as u64,
    };
    // Map a page for BootInfo if not already mapped
    let bi_page = BOOTINFO_ADDR & !(PAGE_SIZE - 1);
    {
        let mut pt2 = kernel_page_table();
        let bi_pa = alloc_page().expect("OOM for bootinfo page");
        unsafe { core::ptr::write_bytes(bi_pa as *mut u8, 0, PAGE_SIZE); }
        map_user_page(&mut pt2, bi_page, bi_pa, stack_flags);
    }
    unsafe {
        core::ptr::write_volatile(BOOTINFO_ADDR as *mut BootInfo, bootinfo);
    }
    print_uart("BootInfo written at ");
    print_uart_hex(BOOTINFO_ADDR as u64);
    print_uart("\n");

    // ------------------------------------------------------------------
    // 8. TLB flush for new user mappings
    // ------------------------------------------------------------------
    unsafe {
        let l0_pa = *crate::KERNEL_L0_PA.lock();
        asm!("msr ttbr0_el1, {}", in(reg) l0_pa);
        asm!("dsb ish; isb");
        asm!("tlbi vmalle1is; dsb ish; isb");
    }
    print_uart("TLB flushed\n");

    // ------------------------------------------------------------------
    // 9. Create user thread for liblinux and enter FsServer IPC loop
    // ------------------------------------------------------------------
    use crate::kernel::sche::{self, enqueue_ready};
    use core::ptr::write_volatile;

    // Allocate kernel stack for liblinux user thread
    let ks_pa0 = alloc_page().expect("OOM for liblinux kernel stack");
    let ks_pa1 = alloc_page().expect("OOM for liblinux kernel stack");
    let ks_base = ks_pa0;
    let ks_top = ks_pa1 + PAGE_SIZE;
    let ks_size = 2 * PAGE_SIZE;
    unsafe { core::ptr::write_bytes(ks_pa0 as *mut u8, 0, ks_size); }

    // TaskContext at top of kernel stack
    let ctx_addr = ks_top - 128;
    // TrapFrame below TaskContext
    let tf_addr = ctx_addr - 288;

    // Build initial TrapFrame for liblinux
    // liblinux expects to run with:
    //   elr = liblinux_entry
    //   sp = stack_top (liblinux uses the same user stack initially)
    //   x0 = 0 (no args)
    let initial_sp = stack_top - 32; // 16-byte aligned

    let trapframe = crate::kernel::trap::TrapFrame {
        trap_num: 0,
        elr: liblinux_entry,
        spsr: 0, // EL0t
        sp: initial_sp,
        tpidr: 0,
        general: crate::kernel::trap::GeneralRegs {
            x0: 0,
            ..Default::default()
        },
    };
    unsafe { write_volatile(tf_addr as *mut crate::kernel::trap::TrapFrame, trapframe); }

    // Create TCB
    let lid = unsafe {
        sche::create_thread(128, ks_base, ctx_addr, 0, 0)
    }.expect("Failed to create liblinux thread");

    // Get TCB pointer for x19
    let tcb_addr = unsafe { sche::tcb_ptr(lid) } as usize;

    // Build TaskContext (must match __switch layout in switch.S)
    let ttbr1: usize;
    unsafe { asm!("mrs {}, ttbr1_el1", out(reg) ttbr1); }
    unsafe {
        write_volatile((ctx_addr + 0x00) as *mut usize, crate::kernel::trap::thread_trampoline_addr());
        write_volatile((ctx_addr + 0x08) as *mut usize, tcb_addr);
        write_volatile((ctx_addr + 0x70) as *mut usize, ttbr1);
    }

    // Enqueue the new thread
    let prio = sche::with_thread(lid, |t| t.effective_priority()).unwrap_or(128);
    enqueue_ready(lid, prio).expect("Failed to enqueue liblinux thread");

    print_uart("liblinux thread created (tid=");
    // print tid...
    print_uart("), entering FsServer IPC loop\n");

    // ------------------------------------------------------------------
    // 10. Boot thread becomes the FsServer IPC event loop
    // ------------------------------------------------------------------
    Arc::clone(&fs).run_ipc_loop();
}