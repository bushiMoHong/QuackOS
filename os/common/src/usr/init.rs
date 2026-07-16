//! Init process — loads `/bin/helloworld` + liblinux, spawns the liblinux user
//! thread, and enters the FsServer IPC loop.
//!
//! Phase 0 boot flow:
//!
//! 1. Mount ext4, load /bin/helloworld ELF, map segments into user space.
//! 2. Load liblinux ELF from embedded bytes, map into user space.
//! 3. Write BootInfo at a fixed user address (bash entry, stack, brk).
//! 4. Create a user-mode thread that runs liblinux.
//! 5. Boot thread becomes the FsServer IPC event loop.

use alloc::sync::Arc;
use core::arch::asm;

use crate::usr::drivers::VirtIOBlockDev;
use crate::usr::fs::dev::block_dev::BlockDevice;
use crate::usr::fs::ext4::fs::Ext4FileSystem;
use crate::usr::fs::ext4::inode::load_inode;
use crate::usr::fs::server::FsServer;
use crate::usr::fs::types::OpenFlags;
use crate::print_uart;
use crate::print_uart_hex;

use aarch64::base::mm::page_table::PageTable;

use crate::usr::proc::elf_loader::{
    load_elf_bytes, map_user_stack, spawn_user_thread_in_as, USER_STACK_TOP,
};
use crate::kernel::bmm::{self, create_kernel_mapped_page_table};

// ---------------------------------------------------------------------------
// Embedded liblinux ELF (built by Makefile before the kernel)
// ---------------------------------------------------------------------------
const LIBLINUX_ELF: &[u8] = include_bytes!("../../../liblinux/target/aarch64-unknown-none/release/liblinux");

// ---------------------------------------------------------------------------
// BootInfo — passed from kernel to liblinux
// ---------------------------------------------------------------------------

/// Fixed user-space address where BootInfo is placed.
/// Must match liblinux's expected address.
///
/// Placed at liblinux's SAVE_AREA address, which is already
/// mapped as RW by `load_elf_bytes`.  This avoids needing a separately
/// mapped page.
/// The BootInfo data is consumed before any Linux syscalls overwrite
/// the SAVE_AREA with the LinuxContext.
const BOOTINFO_ADDR: usize = 0x204028;

#[repr(C)]
struct BootInfo {
    program_entry: u64,
    stack_top: u64,
    brk: u64,
    phdr_addr: u64,
    phent_size: u64,
    phnum: u64,
}

// ---------------------------------------------------------------------------
// Page-table helpers
// ---------------------------------------------------------------------------

fn kernel_page_table() -> PageTable {
    let l0_pa = *crate::KERNEL_L0_PA.lock();
    PageTable::from_token(l0_pa)
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
    // 2. Create isolated page table and load liblinux ELF
    // ------------------------------------------------------------------
    let mut pt = create_kernel_mapped_page_table()
        .expect("Failed to create isolated page table");
    print_uart("Isolated page table created, loading liblinux from embedded ELF (");
    print_uart_hex(LIBLINUX_ELF.len() as u64);
    print_uart(" bytes)...\n");

    let liblinux = load_elf_bytes(&mut pt, LIBLINUX_ELF)
        .expect("Failed to load liblinux ELF");
    print_uart("liblinux entry: ");
    print_uart_hex(liblinux.entry as u64);
    print_uart(" brk: ");
    print_uart_hex(liblinux.brk as u64);
    print_uart("\n");

    // ------------------------------------------------------------------
    // 3. Read and load /bin/helloworld ELF
    // ------------------------------------------------------------------
    let fd = fs.open(0, "/bin/helloworld", OpenFlags::O_RDONLY, 0)
        .expect("Failed to open /bin/helloworld");
    let stat = fs.fstat(0, fd).expect("Failed to stat /bin/helloworld");
    print_uart("/bin/helloworld size: ");
    print_uart_hex(stat.size);
    print_uart("\n");

    let elf_data = fs.read(0, fd, stat.size as usize)
        .expect("Failed to read /bin/helloworld");
    fs.close(0, fd).ok();

    let bash = load_elf_bytes(&mut pt, &elf_data)
        .expect("Failed to load bash ELF");
    print_uart("bash entry: ");
    print_uart_hex(bash.entry as u64);
    print_uart(" brk: ");
    print_uart_hex(bash.brk as u64);
    print_uart("\n");

    // ------------------------------------------------------------------
    // 4. Map user stack
    // ------------------------------------------------------------------
    map_user_stack(&mut pt);
    print_uart("User stack mapped at ");
    print_uart_hex(USER_STACK_TOP as u64);
    print_uart("\n");

    // ------------------------------------------------------------------
    // 5. Write BootInfo for liblinux
    // ------------------------------------------------------------------
    let bootinfo = BootInfo {
        program_entry: bash.entry as u64,
        stack_top: USER_STACK_TOP as u64,
        brk: bash.brk as u64,
        phdr_addr: bash.phdr_addr as u64,
        phent_size: bash.phent_size as u64,
        phnum: bash.phnum as u64,
    };
    unsafe {
        core::ptr::write_volatile(BOOTINFO_ADDR as *mut BootInfo, bootinfo);
    }
    unsafe { asm!("dsb ishst"); }

    // Verify from kernel side
    let verify = unsafe { core::ptr::read_volatile(BOOTINFO_ADDR as *const BootInfo) };
    print_uart("BootInfo written at ");
    print_uart_hex(BOOTINFO_ADDR as u64);
    print_uart(" entry=");
    print_uart_hex(verify.program_entry);
    print_uart(" stack=");
    print_uart_hex(verify.stack_top);
    print_uart(" brk=");
    print_uart_hex(verify.brk);
    print_uart("\n");

    // ------------------------------------------------------------------
    // 6. TLB flush — ensure all new user mappings are visible
    // ------------------------------------------------------------------
    unsafe {
        let l0_pa = *crate::KERNEL_L0_PA.lock();
        asm!("msr ttbr0_el1, {}", in(reg) l0_pa);
        asm!("dsb ish; isb");
        asm!("tlbi vmalle1is; dsb ish; isb");
    }
    print_uart("TLB done\n");

    // ------------------------------------------------------------------
    // 7. Spawn liblinux user thread
    // ------------------------------------------------------------------
    let tid = spawn_user_thread(liblinux.entry, USER_STACK_TOP)
        .expect("Failed to spawn liblinux thread");
    print_uart("liblinux thread spawned (tid=");
    print_uart_hex(tid.0 as u64);
    print_uart("), entering FsServer IPC loop\n");

    // ------------------------------------------------------------------
    // 8. Boot thread becomes the FsServer IPC event loop
    // ------------------------------------------------------------------
    Arc::clone(&fs).run_ipc_loop();
}
