//! Native (SVC #1) syscall dispatch table and implementations.
//!
//! Each function reads arguments from `tf.general` registers and writes the
//! return value to `tf.general.x0`.  Syscalls that do not return (exit,
//! linux_syscall_done) directly manipulate the trap frame and eret.
//!
//! # Syscall table (see §3.3 of the Linux syscall plan)
//!
//! | nr | Name                       | x0            | x1         | x2        | x3        |
//! |----|----------------------------|---------------|------------|-----------|-----------|
//! | 1  | sys_map_page               | vaddr         | prot_flags | –         | –         |
//! | 2  | sys_unmap_page             | vaddr         | –          | –         | –         |
//! | 3  | sys_ipc_send               | endpoint_cptr | msg_ptr    | msg_len   | –         |
//! | 4  | sys_ipc_recv               | endpoint_cptr | buf_ptr    | buf_len   | –         |
//! | 5  | sys_ipc_call               | endpoint_cptr | send_ptr   | send_len  | recv_buf  |
//! |    |                            |               |            |           | recv_len  |
//! | 6  | sys_create_thread          | entry_pc      | stack_top  | arg       | –         |
//! | 7  | sys_exit_thread            | exit_code     | –          | –         | –         |
//! | 8  | sys_register_linux_handler | handler_pc    | save_area  | –         | –         |
//! | 9  | sys_linux_syscall_done     | return_value  | –          | –         | –         |
//! | 10 | sys_yield                  | –             | –          | –         | –         |
//! | 11 | sys_console_write          | buf_ptr       | len        | –         | –         |
//! | 12 | sys_mprotect               | vaddr         | prot       | –         | –         |
//! | 13 | sys_spawn                  | elf_ptr       | elf_len    | stack_top | –         |
//! | 14 | sys_clone                  | flags         | child_sp   | par_tid   | child_tid |
//! |    |                            | tls           |            |           |           |
//! | 15 | sys_console_read           | buf_ptr       | len        | –         | –         |
//! | 16 | sys_exec                   | elf_ptr       | elf_len    | stack_top | bootinfo  |
//! |    |                            |               |            |           | ptr       |
//! | 17 | sys_wait4                  | –             | –          | –         | –         |
//! | 18 | sys_create_notification    | –             | –          | –         | –         |
//! | 19 | sys_notify_send            | notify_id     | –          | –         | –         |
//! | 20 | sys_notify_wait            | notify_id     | –          | –         | –         |
//! | 21 | sys_irq_register           | irq_num       | –          | –         | –         |
//! | 22 | sys_irq_ack                | irq_num       | –          | –         | –         |
//! | 23 | sys_ipc_recv_timeout       | endpoint_cptr | buf_ptr    | buf_len   | timeout_ms|
//! | 24 | sys_ipc_call_timeout       | endpoint_cptr | send_ptr   | send_len  | recv_buf  |
//! |    |                            | recv_len      | timeout_ms |           |           |
//! | 25 | sys_cspace_mint            | obj_id        | cap_type   | rights    | –         |
//! | 26 | sys_cspace_derive          | src_cptr      | new_rights | –         | –         |
//! | 27 | sys_cspace_revoke          | cptr          | –          | –         | –         |
//! | 28 | sys_cspace_move            | src_cptr      | dest_cptr  | –         | –         |
//! | 29 | sys_cspace_delete          | cptr          | –          | –         | –         |

use super::{LinuxContext, TrapFrame};

// ---------------------------------------------------------------------------
// liblinux VA range — stored during init, used by sys_exec
// ---------------------------------------------------------------------------
static LIBLINUX_VA_START: spin::Mutex<usize> = spin::Mutex::new(0);
static LIBLINUX_VA_END: spin::Mutex<usize> = spin::Mutex::new(0);
static LIBLINUX_ENTRY: spin::Mutex<usize> = spin::Mutex::new(0);

/// Store liblinux's VA boundaries after init loads liblinux ELF.
pub fn store_liblinux_range(start: usize, end: usize, entry: usize) {
    *LIBLINUX_VA_START.lock() = start;
    *LIBLINUX_VA_END.lock() = end;
    *LIBLINUX_ENTRY.lock() = entry;
}

// ---------------------------------------------------------------------------
// Linux-compatible errno constants (negative return values)
// ---------------------------------------------------------------------------

const EINVAL:   isize = 22;
const ENOMEM:   isize = 12;
const ENOSYS:   isize = 38;
const EBADF:    isize = 9;
const EACCES:   isize = 13;
const EEXIST:   isize = 17;
const ENOTSUP:  isize = 95;

// ---------------------------------------------------------------------------
// Top-level dispatch
// ---------------------------------------------------------------------------

/// Dispatch an SVC #1 native syscall.
///
/// Called from `handle_user_sync` when ESR_EL1 indicates SVC with imm=1.
/// The syscall number is in `tf.general.x8`.
pub fn native_syscall_dispatch(nr: u64, tf: &mut TrapFrame) {
    let tid = crate::kernel::sche::current_thread();
    // Syscall tracing disabled for quiet production output.
    // Enable the block below to debug syscall dispatch.
    /*
    if tid.0 & 0xFFFF == 2 || tid.0 & 0xFFFF == 0 {
        let name = super::native_syscall_name(nr as usize);
        let sp: usize;
        unsafe { core::arch::asm!("mov {}, sp", out(reg) sp); }
        // crate::print_uart("[N:");
        // crate::print_uart(name);
        // crate::print_uart("] tid=");
        // crate::print_uart_hex(tid.0 as u64);
        // crate::print_uart(" elr=");
        // crate::print_uart_hex(tf.elr as u64);
        // crate::print_uart(" tf=");
        // crate::print_uart_hex(tf as *const TrapFrame as u64);
        // crate::print_uart(" sp=");
        // crate::print_uart_hex(sp as u64);
        // For linux_syscall_done, show return value and restore target
        if nr == 9 {
            // crate::print_uart(" ret=");
            // crate::print_uart_hex(tf.general.x0 as u64);
            let save_area = crate::kernel::sche::with_thread(tid, |t| t.linux_save_area).unwrap_or(None);
            if let Some(sa) = save_area {
                let ctx = unsafe { core::ptr::read_volatile(sa as *const super::LinuxContext) };
                // crate::print_uart(" linux_elr=");
                // crate::print_uart_hex(ctx.elr);
                // crate::print_uart(" linux_nr=");
                // crate::print_uart_hex(ctx.x8);
                let lname = super::linux_syscall_name(ctx.x8 as usize);
                // crate::print_uart("(");
                // crate::print_uart(lname);
                // crate::print_uart(")");
            }
        }
        crate::print_uart("\n");
    }
    */
    match nr {
        1  => sys_map_page(tf),
        2  => sys_unmap_page(tf),
        3  => sys_ipc_send(tf),
        4  => sys_ipc_recv(tf),
        5  => sys_ipc_call(tf),
        6  => sys_create_thread(tf),
        7  => sys_exit_thread(tf),
        8  => sys_register_linux_handler(tf),
        9  => sys_linux_syscall_done(tf),
        10 => sys_yield(tf),
        11 => sys_console_write(tf),
        12 => sys_mprotect(tf),
        13 => sys_spawn(tf),
        14 => sys_clone(tf),
        15 => sys_console_read(tf),
        16 => sys_exec(tf),
        17 => sys_wait4(tf),
        18 => sys_create_notification(tf),
        19 => sys_notify_send(tf),
        20 => sys_notify_wait(tf),
        21 => sys_irq_register(tf),
        22 => sys_irq_ack(tf),
        23 => sys_ipc_recv_timeout(tf),
        24 => sys_ipc_call_timeout(tf),
        25 => sys_cspace_mint(tf),
        26 => sys_cspace_derive(tf),
        27 => sys_cspace_revoke(tf),
        28 => sys_cspace_move(tf),
        29 => sys_cspace_delete(tf),
        _  => {
            tf.general.x0 = (-ENOSYS) as usize;
        }
    }

    // Debug: show final ELR after dispatch, before trap_return uses it
    // (disabled — enable for debugging specific syscalls)
    /*
    let tid = crate::kernel::sche::current_thread();
    if tid.0 & 0xFFFF == 2 || tid.0 & 0xFFFF == 0 {
        let sp: usize;
        unsafe { core::arch::asm!("mov {}, sp", out(reg) sp); }
        crate::print_uart("[N:done] tid=");
        crate::print_uart_hex(tid.0 as u64);
        crate::print_uart(" nr=");
        crate::print_uart_hex(nr);
        crate::print_uart(" final_elr=");
        crate::print_uart_hex(tf.elr as u64);
        crate::print_uart(" tf=");
        crate::print_uart_hex(tf as *const TrapFrame as u64);
        crate::print_uart(" sp=");
        crate::print_uart_hex(sp as u64);
        crate::print_uart("\n");
    }
    */
}

// ---------------------------------------------------------------------------
// Helper: get a PageTable for the *current* thread's address space.
// Reads TTBR0_EL1 so we operate on the correct isolated page table, not
// the kernel shared one.  Boot threads with TTBR0=0 fall back to KERNEL_L0_PA.
fn current_page_table() -> aarch64::base::mm::page_table::PageTable {
    let ttbr0: usize;
    unsafe { core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr0); }
    if ttbr0 != 0 {
        aarch64::base::mm::page_table::PageTable::from_token(ttbr0)
    } else {
        let l0 = crate::KERNEL_L0_PA.lock();
        aarch64::base::mm::page_table::PageTable::from_token(*l0)
    }
}

// 1. sys_map_page — map a physical page at vaddr in current AS
// ---------------------------------------------------------------------------
// COMPROMISE: bypassing Capability check for frame allocation.
// The plan requires `x0=frame_cptr`, but Untyped memory is not yet
// implemented.  For Phase 0 we accept `x0=vaddr, x1=prot_flags` and
// let the kernel allocate the physical frame directly.
//
// prot_flags: bit 0=READ, bit 1=WRITE, bit 2=EXEC
fn sys_map_page(tf: &mut TrapFrame) {
    let vaddr = tf.general.x0;
    let prot = tf.general.x1;

    // Align vaddr to page boundary
    if vaddr & 0xFFF != 0 {
        tf.general.x0 = (-EINVAL) as usize;
        return;
    }

    // Check vaddr is in user space (top bit clear, below kernel base)
    // For 48-bit VA, user space is 0x0000_0000_0000_0000 – 0x0000_7FFF_FFFF_FFFF
    if vaddr >= 0x0000_8000_0000_0000 {
        tf.general.x0 = (-EINVAL) as usize;
        return;
    }

    // Build PTEFlags
    let mut flags = aarch64::base::mm::page_table::PTEFlags::empty();
    flags.insert(aarch64::base::mm::page_table::PTEFlags::V);
    flags.insert(aarch64::base::mm::page_table::PTEFlags::A);
    flags.insert(aarch64::base::mm::page_table::PTEFlags::D);
    flags.insert(aarch64::base::mm::page_table::PTEFlags::U);
    if prot & 1 != 0 { flags.insert(aarch64::base::mm::page_table::PTEFlags::R); }
    if prot & 2 != 0 { flags.insert(aarch64::base::mm::page_table::PTEFlags::W); }
    if prot & 4 != 0 { flags.insert(aarch64::base::mm::page_table::PTEFlags::X); }

    use aarch64::base::mm::VirtPageNum;
    let vpn = VirtPageNum::from(vaddr >> 12);
    let mut pt = current_page_table();

    // If the page is already mapped (e.g. TLS area pre-mapped by elf_loader),
    // just update permissions — don't leak the existing physical frame.
    if pt.find_pte(vpn).map_or(false, |pte| pte.is_valid()) {
        pt.remap(vpn, flags);
        tf.general.x0 = 0;
        return;
    }

    // Allocate a physical page
    let pa = match aarch64::base::mm::alloc_page() {
        Some(pa) => pa,
        None => {
            tf.general.x0 = (-ENOMEM) as usize;
            return;
        }
    };

    // Zero the page
    unsafe { core::ptr::write_bytes(pa as *mut u8, 0, aarch64::base::config::PAGE_SIZE); }

    // Map into current address space
    use aarch64::base::mm::PhysPageNum;
    let ppn = PhysPageNum::from(pa >> 12);

    pt.map(vpn, ppn, flags);

    tf.general.x0 = 0; // success
}

// ---------------------------------------------------------------------------
// 2. sys_unmap_page — unmap and free a page at vaddr
// ---------------------------------------------------------------------------
fn sys_unmap_page(tf: &mut TrapFrame) {
    let vaddr = tf.general.x0;

    if vaddr & 0xFFF != 0 || vaddr >= 0x0000_8000_0000_0000 {
        tf.general.x0 = (-EINVAL) as usize;
        return;
    }

    let mut pt = current_page_table();
    let vpn = aarch64::base::mm::VirtPageNum::from(vaddr >> 12);

    // Find the PTE to get the physical address for freeing
    if let Some(pte) = pt.find_pte(vpn) {
        let pa = pte.ppn().0 << aarch64::base::config::PAGE_SHIFT;
        pt.unmap(vpn);
        aarch64::base::mm::free_page(pa);
        tf.general.x0 = 0;
    } else {
        tf.general.x0 = (-EINVAL) as usize;
    }
}

// ---------------------------------------------------------------------------
// IPC helpers: pack/extract raw bytes to/from ShortPayload
// ---------------------------------------------------------------------------

/// Pack raw bytes into a `ShortPayload` (max 256 bytes).
fn pack_short(msg_ptr: usize, msg_len: usize) -> crate::kernel::ipc::message::ShortPayload {
    let mut words = [0usize; 32];
    let n = msg_len.min(256);
    let mut buf = [0u8; 256];
    unsafe { core::ptr::copy_nonoverlapping(msg_ptr as *const u8, buf.as_mut_ptr(), n); }
    for i in 0..((n + 7) / 8) {
        let off = i * 8;
        let end = (off + 8).min(n);
        let mut w: usize = 0;
        for j in off..end {
            w |= (buf[j] as usize) << ((j - off) * 8);
        }
        words[i] = w;
    }
    crate::kernel::ipc::message::ShortPayload { words, len: n as u16 }
}

/// Extract raw bytes from a `ShortPayload` and copy to user buffer.
fn unpack_short(payload: &crate::kernel::ipc::message::ShortPayload, buf_ptr: usize, buf_len: usize) -> usize {
    let n = (payload.len as usize).min(buf_len).min(256);
    let mut buf = [0u8; 256];
    for i in 0..32 {
        let bytes = payload.words[i].to_le_bytes();
        let off = i * 8;
        if off < n {
            let m = (n - off).min(8);
            buf[off..off+m].copy_from_slice(&bytes[..m]);
        }
    }
    unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), buf_ptr as *mut u8, n); }
    n
}

// ---------------------------------------------------------------------------
// 3. sys_ipc_send — send an IPC message via endpoint_cptr
// ---------------------------------------------------------------------------
// COMPROMISE (Phase 0): endpoint_cptr is used as raw ChannelId without
// CSpace lookup.  Capability checks are bypassed.
fn sys_ipc_send(tf: &mut TrapFrame) {
    let ch_raw = tf.general.x0 as u32;
    let msg_ptr = tf.general.x1;
    let msg_len = tf.general.x2;

    if msg_len > 256 {
        tf.general.x0 = (-EINVAL) as usize;
        return;
    }

    let channel_id = crate::kernel::ipc::channel::ChannelId(ch_raw);
    let payload = pack_short(msg_ptr, msg_len);
    let msg = crate::kernel::ipc::message::Message::new_short(
        1, payload
    );

    let tid = crate::kernel::sche::current_thread();
    use crate::kernel::ipc::channel::{with_channel, SendMatch};
    use crate::kernel::ipc::{deliver, wake};
    use crate::kernel::sche::{block_current, IpcState};

    let action = match with_channel(channel_id, |inner| inner.match_sender(tid, &msg)) {
        Ok(a) => a,
        Err(_) => { tf.general.x0 = (-EBADF) as usize; return; }
    };

    match action {
        SendMatch::Matched(receiver_tid) => {
            let _ = deliver(&msg, receiver_tid, None, None);
            wake(receiver_tid);
            tf.general.x0 = 0;
        }
        SendMatch::Parked => {
            unsafe { block_current(IpcState::BlockedOnSend(channel_id)); }
            tf.general.x0 = 0;
        }
    }
}

// ---------------------------------------------------------------------------
// 4. sys_ipc_recv — receive an IPC message via endpoint_cptr
// ---------------------------------------------------------------------------
fn sys_ipc_recv(tf: &mut TrapFrame) {
    let ch_raw = tf.general.x0 as u32;
    let buf_ptr = tf.general.x1;
    let buf_len = tf.general.x2;

    let channel_id = crate::kernel::ipc::channel::ChannelId(ch_raw);
    let tid = crate::kernel::sche::current_thread();
    use crate::kernel::ipc::channel::{with_channel, RecvMatch};
    use crate::kernel::ipc::{deliver, wake, get_ipc_buffer};
    use crate::kernel::sche::{block_current, IpcState};
    use crate::kernel::ipc::message::Message;

    let action = match with_channel(channel_id, |inner| inner.match_receiver(tid)) {
        Ok(a) => a,
        Err(_) => { tf.general.x0 = (-EBADF) as usize; return; }
    };

    match action {
        RecvMatch::Matched(sender_entry) => {
            let msg = sender_entry.msg.unwrap_or_else(|| {
                Message::new_short(
                    0,
                    crate::kernel::ipc::message::ShortPayload { words: [0; 32], len: 0 }
                )
            });
            let sender_tid = sender_entry.thread_id;
            let _ = deliver(&msg, tid, None, None);
            wake(sender_tid);

            if let Message::Short(_, ref payload) = msg {
                tf.general.x0 = unpack_short(payload, buf_ptr, buf_len);
            } else {
                tf.general.x0 = 0;
            }
        }
        RecvMatch::Parked => {
            unsafe { block_current(IpcState::BlockedOnReceive(channel_id)); }

            let buf = match get_ipc_buffer(tid) {
                Ok(b) => b,
                Err(_) => { tf.general.x0 = 0; return; }
            };
            if let Some(payload) = buf.read_short() {
                let n = unpack_short(&payload, buf_ptr, buf_len);
                tf.general.x0 = n;
            } else {
                tf.general.x0 = 0;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 5. sys_ipc_call — synchronous IPC (send + recv on same channel)
// ---------------------------------------------------------------------------
// **#[inline(never)] is REQUIRED.**  If inlined into native_syscall_dispatch
// the compiler may repurpose x19 (which holds `tf`) for something else,
// causing `tf` to be reconstructed from a stale spill slot after the
// block/resume cycle inside sys_ipc_recv.
#[inline(never)]
fn sys_ipc_call(tf: &mut TrapFrame) {
    let ch_raw = tf.general.x0 as u32;
    let send_ptr = tf.general.x1;
    let send_len = tf.general.x2;
    let recv_buf = tf.general.x3;
    let recv_len = tf.general.x4;

    // Save tf pointer for debug comparison
    let tf_ptr_before: usize = tf as *const TrapFrame as usize;
    // Read x19 via asm to compare with Rust-level tf reference
    let x19_before: usize;
    unsafe { core::arch::asm!("mov {}, x19", out(reg) x19_before); }

    // Send phase: set up registers and call sys_ipc_send
    let saved_x0 = tf.general.x0; let saved_x1 = tf.general.x1; let saved_x2 = tf.general.x2;
    tf.general.x0 = ch_raw as usize; tf.general.x1 = send_ptr; tf.general.x2 = send_len;
    sys_ipc_send(tf);
    if tf.general.x0 != 0 { return; }

    // Receive phase
    tf.general.x0 = ch_raw as usize; tf.general.x1 = recv_buf; tf.general.x2 = recv_len;
    sys_ipc_recv(tf);

    // Minimal debug right after recv returns
    // crate::print_uart("[call_post_recv]\n");

    // DEBUG: check if tf changed during recv
    let tf_ptr_after: usize = tf as *const TrapFrame as usize;
    let x19_after: usize;
    unsafe { core::arch::asm!("mov {}, x19", out(reg) x19_after); }
    let tid = crate::kernel::sche::current_thread();
    if false && tid.0 & 0xFFFF <= 3 {
        // crate::print_uart("[ipc_call_debug] tid=");
        // crate::print_uart_hex(tid.0 as u64);
        // crate::print_uart(" tf_before=");
        // crate::print_uart_hex(tf_ptr_before as u64);
        // crate::print_uart(" tf_after=");
        // crate::print_uart_hex(tf_ptr_after as u64);
        // crate::print_uart(" x19_before=");
        // crate::print_uart_hex(x19_before as u64);
        // crate::print_uart(" x19_after=");
        // crate::print_uart_hex(x19_after as u64);
        // crate::print_uart(" elr=");
        // crate::print_uart_hex(tf.elr as u64);
        // crate::print_uart("\n");
    }

    // Restore original registers (caller may need them)
    tf.general.x0 = tf.general.x0; // keep return value from recv
    tf.general.x1 = saved_x1; tf.general.x2 = saved_x2;
}

// ---------------------------------------------------------------------------
// 6. sys_create_thread — create a new user-space thread
// ---------------------------------------------------------------------------

extern "C" fn thread_trampoline(_next_sp: usize) -> ! {
    // tf_addr was stored in x20 when the TaskContext was built.
    // We read it via inline asm to avoid any dependency on x0,
    // which can be clobbered between __switch's `mov x0, x1` and here.
    let tf_ptr: usize;
    unsafe { core::arch::asm!("mov {}, x20", out(reg) tf_ptr); }
    let ctx = unsafe { &mut *(tf_ptr as *mut crate::kernel::trap::UserContext) };
    ctx.run();
    // run() delegates to assembly run_user → trap_return → eret.
    // It never returns to this call frame; when this thread traps, the
    // exception handler runs and trap_return eret's back to user space.
    loop { unsafe { core::arch::asm!("wfi"); } }
}

/// Return the address of `thread_trampoline` for use by init.rs when
/// constructing the initial TaskContext for a new user thread.
pub fn thread_trampoline_addr() -> usize {
    thread_trampoline as *const () as usize
}

fn sys_create_thread(tf: &mut TrapFrame) {
    let entry_pc = tf.general.x0;
    let stack_top = tf.general.x1;
    let arg = tf.general.x2;

    use aarch64::base::mm::alloc_page;
    use aarch64::base::config::PAGE_SIZE;
    use crate::kernel::sche::{self, enqueue_ready};
    use core::ptr::write_volatile;

    // Allocate kernel stack (8 physically contiguous pages = 32 KB, zeroed)
    let stack_base = match aarch64::base::mm::alloc_pages_contig(8) {
        Some(p) => p,
        None => { tf.general.x0 = (-ENOMEM) as usize; return; }
    };
    let stack_top_ks = stack_base + 8 * PAGE_SIZE;

    // Compute pointers within the kernel stack:
    // TaskContext (128 bytes) right below the top
    let ctx_addr = stack_top_ks - 128; // next_sp = address of TaskContext
    // TrapFrame (288 bytes) right below the TaskContext
    let tf_addr = ctx_addr - 288;      // address of TrapFrame

    // ── 1. Build TrapFrame ──
    let trapframe = crate::kernel::trap::TrapFrame {
        trap_num: 0,
        elr: entry_pc,
        spsr: 0, // EL0t
        sp: stack_top,
        tpidr: 0,
        general: crate::kernel::trap::GeneralRegs {
            x0: arg,
            ..Default::default()
        },
    };
    unsafe { write_volatile(tf_addr as *mut crate::kernel::trap::TrapFrame, trapframe); }

    // ── 2. Create TCB ──
    let tid = match unsafe {
        sche::create_thread(
            128,                // default priority
            stack_base,
            ctx_addr,           // kernel_stack_top = next_sp (points to TaskContext)
            8 * PAGE_SIZE,      // kernel_stack_size
            0,                  // ttbr0 (shares kernel page table)
            0,                  // asid
        )
    } {
        Ok(tid) => tid,
        Err(_) => {
            aarch64::base::mm::free_page_range(stack_base, stack_base + 8 * PAGE_SIZE);
            tf.general.x0 = (-ENOMEM) as usize;
            return;
        }
    };

    // ── 3. Get TCB pointer for x19 ──
    let tcb_addr = unsafe { sche::tcb_ptr(tid) } as usize;

    // ── 4. Build TaskContext ──
    // Layout must match __switch (switch.S):
    //   0x00: lr,   0x08: x19,  0x10: x20,  0x18: x21
    //   0x20: x22,  0x28: x23,  0x30: x24,  0x38: x25
    //   0x40: x26,  0x48: x27,  0x50: x28,  0x58: fp
    //   0x70: ttbr1_el1,   0x78: ttbr0_el1
    unsafe {
        write_volatile((ctx_addr + 0x00) as *mut usize, thread_trampoline as *const () as usize); // lr
        write_volatile((ctx_addr + 0x08) as *mut usize, tcb_addr);                   // x19 = TCB ptr
        write_volatile((ctx_addr + 0x10) as *mut usize, tf_addr);                    // x20 = tf_addr
        write_volatile((ctx_addr + 0x70) as *mut usize, 0u64 as usize);              // ttbr1_el1 = 0
        write_volatile((ctx_addr + 0x78) as *mut usize, *crate::KERNEL_L0_PA.lock()); // ttbr0 = shared page table
    }

    // ── 5. Enqueue the new thread ──
    let prio = sche::with_thread(tid, |t| t.effective_priority()).unwrap_or(128);
    let _ = enqueue_ready(tid, prio);

    // ── 6. Return ThreadId ──
    tf.general.x0 = tid.0 as usize;
}

// ---------------------------------------------------------------------------
// 7. sys_exit_thread — exit the current thread
// ---------------------------------------------------------------------------
fn sys_exit_thread(tf: &mut TrapFrame) {
    let exit_code = tf.general.x0 as i32;

    let current_tid = crate::kernel::sche::current_thread();

    // Store exit code for wait4
    let _ = crate::kernel::sche::with_thread_mut(current_tid, |t| {
        t.exit_code = exit_code;
    });

    // Mark as Dying — scheduler will skip re-enqueuing.
    let parent_tid = crate::kernel::sche::with_thread(current_tid, |t| {
        t.set_atomic_state(crate::kernel::sche::ThreadState::Dying);
        t.parent_tid
    }).ok().flatten();

    // If the parent is blocked in wait4, wake it so it can reap us.
    if let Some(pid) = parent_tid {
        let blocked = crate::kernel::sche::with_thread(pid, |t| {
            t.ipc_state == crate::kernel::ipc::synchronization::IpcState::BlockedOnWait4
        }).unwrap_or(false);
        if blocked {
            crate::kernel::sche::wake(pid);
        }
    }

    // Yield CPU — we never return from this call because the current
    // thread is Dying and won't be re-enqueued.
    crate::kernel::sche::schedule();

    // If we get here (idle loop with no runnable threads), spin.
    loop {
        unsafe { core::arch::asm!("wfi"); }
    }
}

// ---------------------------------------------------------------------------
// 8. sys_register_linux_handler — register Linux syscall exception handler
// ---------------------------------------------------------------------------
/// Register per-thread Linux syscall handler entry and save_area.
/// Operates on the **current** thread (§8.7 of the plan).
///
/// args: x0=handler_pc (user-mode liblinux handler entry VA)
///       x1=save_area_vaddr (per-thread save area VA for LinuxContext)
fn sys_register_linux_handler(tf: &mut TrapFrame) {
    // Read from x0/x1 — the natural ARM64 calling convention.
    // liblinux passes handler_pc in x0 and save_area in x1 via inout("x0")/in("x1").
    let handler_pc = tf.general.x0;
    let save_area = tf.general.x1;

    let tid = crate::kernel::sche::current_thread();
    let result = crate::kernel::sche::with_thread_mut(tid, |thread| {
        thread.linux_handler_pc = Some(handler_pc);
        thread.linux_save_area = Some(save_area);
    });

    match result {
        Ok(()) => tf.general.x0 = 0,
        Err(_) => tf.general.x0 = (-EINVAL) as usize,
    }
}

// ---------------------------------------------------------------------------
// 9. sys_linux_syscall_done — complete Linux syscall, restore context
// ---------------------------------------------------------------------------
/// Restores the original Linux program context from per-thread save_area,
/// sets x0 to the syscall return value, and erets back to the Linux binary.
///
/// args: x0 = return_value (the Linux syscall result)
fn sys_linux_syscall_done(tf: &mut TrapFrame) {
    let return_value = tf.general.x0;

    let tid = crate::kernel::sche::current_thread();

    // Read save_area vaddr from per-thread TCB
    let save_area = crate::kernel::sche::with_thread(tid, |thread| {
        thread.linux_save_area
    });

    let save_area = match save_area {
        Ok(Some(addr)) => addr,
        _ => loop { unsafe { core::arch::asm!("wfi"); } },
    };

    // Read LinuxContext from user-mode save_area
    // SAFETY: save_area points to liblinux-allocated memory in user space.
    // The address is trusted because it was registered by liblinux itself.
    let ctx = unsafe {
        let ctx_ptr = save_area as *const LinuxContext;
        core::ptr::read_volatile(ctx_ptr)
    };

    // Restore the original Linux program context (u64 → usize casts are
    // no-ops on AArch64 where both types are 64-bit).
    tf.elr = ctx.elr as usize; // ELR already points past SVC (hardware saves PC+4)
    tf.spsr = ctx.spsr as usize;
    tf.sp = ctx.sp as usize;
    tf.general.x0 = return_value;
    tf.general.x1  = ctx.x1  as usize; tf.general.x2  = ctx.x2  as usize;
    tf.general.x3  = ctx.x3  as usize; tf.general.x4  = ctx.x4  as usize;
    tf.general.x5  = ctx.x5  as usize; tf.general.x6  = ctx.x6  as usize;
    tf.general.x7  = ctx.x7  as usize; tf.general.x8  = ctx.x8  as usize;
    tf.general.x9  = ctx.x9  as usize; tf.general.x10 = ctx.x10 as usize;
    tf.general.x11 = ctx.x11 as usize; tf.general.x12 = ctx.x12 as usize;
    tf.general.x13 = ctx.x13 as usize; tf.general.x14 = ctx.x14 as usize;
    tf.general.x15 = ctx.x15 as usize; tf.general.x16 = ctx.x16 as usize;
    tf.general.x17 = ctx.x17 as usize; tf.general.x18 = ctx.x18 as usize;
    tf.general.x19 = ctx.x19 as usize; tf.general.x20 = ctx.x20 as usize;
    tf.general.x21 = ctx.x21 as usize; tf.general.x22 = ctx.x22 as usize;
    tf.general.x23 = ctx.x23 as usize; tf.general.x24 = ctx.x24 as usize;
    tf.general.x25 = ctx.x25 as usize; tf.general.x26 = ctx.x26 as usize;
    tf.general.x27 = ctx.x27 as usize; tf.general.x28 = ctx.x28 as usize;
    tf.general.x29 = ctx.x29 as usize; tf.general.x30 = ctx.x30 as usize;
    // After this, the trap handler returns to trap_return, which will
    // restore from tf and eret — back to the Linux program.
}

// ---------------------------------------------------------------------------
// 10. sys_yield — yield the CPU
// ---------------------------------------------------------------------------
fn sys_yield(tf: &mut TrapFrame) {
    crate::kernel::sche::schedule();
    tf.general.x0 = 0; // success (after we're scheduled back)
}

// ---------------------------------------------------------------------------
// 12. sys_mprotect — change page permissions
// ---------------------------------------------------------------------------
fn sys_mprotect(tf: &mut TrapFrame) {
    let vaddr = tf.general.x0;
    let prot = tf.general.x1;

    if vaddr & 0xFFF != 0 || vaddr >= 0x0000_8000_0000_0000 {
        tf.general.x0 = (-EINVAL) as usize;
        return;
    }

    let mut flags = aarch64::base::mm::page_table::PTEFlags::empty();
    flags.insert(aarch64::base::mm::page_table::PTEFlags::V);
    flags.insert(aarch64::base::mm::page_table::PTEFlags::A);
    flags.insert(aarch64::base::mm::page_table::PTEFlags::D);
    flags.insert(aarch64::base::mm::page_table::PTEFlags::U);
    if prot & 1 != 0 { flags.insert(aarch64::base::mm::page_table::PTEFlags::R); }
    if prot & 2 != 0 { flags.insert(aarch64::base::mm::page_table::PTEFlags::W); }
    if prot & 4 != 0 { flags.insert(aarch64::base::mm::page_table::PTEFlags::X); }

    let mut pt = current_page_table();
    let vpn = aarch64::base::mm::VirtPageNum::from(vaddr >> 12);

    match pt.find_pte(vpn) {
        Some(_) => {
            pt.remap(vpn, flags);
            tf.general.x0 = 0;
        }
        None => {
            tf.general.x0 = (-ENOMEM) as usize;
        }
    }
}

// ---------------------------------------------------------------------------
// 13. sys_spawn — create an isolated process with its own address space
// ---------------------------------------------------------------------------
// args: x0 = elf_data_ptr (user-space ELF buffer)
//       x1 = elf_len       (length in bytes)
//       x2 = stack_top     (top of user stack)
// returns: x0 = ThreadId on success / negative errno on failure
fn sys_spawn(tf: &mut TrapFrame) {
    use crate::kernel::bmm;
    use crate::usr::proc::elf_loader;
    use alloc::vec;

    let elf_ptr  = tf.general.x0;
    let elf_len  = tf.general.x1;
    let stack_top = tf.general.x2;

    // Validate arguments
    if elf_len == 0 || elf_len > 1024 * 1024 {
        tf.general.x0 = (-EINVAL) as usize;
        return;
    }
    if stack_top & 0xF != 0 || stack_top >= 0x0000_8000_0000_0000 {
        tf.general.x0 = (-EINVAL) as usize;
        return;
    }

    // Copy ELF data from user space to kernel heap
    let mut elf_buf = vec![0u8; elf_len as usize];
    unsafe {
        core::ptr::copy_nonoverlapping(
            elf_ptr as *const u8,
            elf_buf.as_mut_ptr(),
            elf_len as usize,
        );
    }

    // Create isolated page table with kernel identity mappings
    let mut pt = match bmm::create_kernel_mapped_page_table() {
        Ok(pt) => pt,
        Err(_) => {
            tf.general.x0 = (-ENOMEM) as usize;
            return;
        }
    };

    // Load ELF segments into the new page table
    let loaded = match elf_loader::load_elf_bytes(&mut pt, &elf_buf) {
        Ok(l) => l,
        Err(_) => {
            tf.general.x0 = (-EINVAL) as usize;
            return;
        }
    };

    // Map user stack
    elf_loader::map_user_stack(&mut pt);

    // Register in global AS table
    let (asid, ttbr0) = match bmm::register_page_table(pt) {
        Some((id, token)) => (id, token),
        None => {
            tf.general.x0 = (-ENOMEM) as usize;
            return;
        }
    };

    // Create initial thread in the new address space
    let tid = match elf_loader::spawn_user_thread_in_as(
        loaded.entry,
        stack_top,
        ttbr0,
        asid.0,
    ) {
        Ok(tid) => tid,
        Err(_) => {
            bmm::unregister_address_space(asid);
            tf.general.x0 = (-ENOMEM) as usize;
            return;
        }
    };

    // Full TLB flush so the new page table can be used
    unsafe {
        core::arch::asm!("dsb ish; tlbi vmalle1is; dsb ish; isb");
    }

    tf.general.x0 = tid.0 as usize;
}

// ---------------------------------------------------------------------------
// 14. sys_clone — create a child process that shares a copy of the parent
// ---------------------------------------------------------------------------
// args: x0 = flags            (clone flags, Linux ABI)
//       x1 = child_stack      (new sp for child, or 0 to use parent's sp)
//       x2 = parent_tid       (ptr to write child TID in parent's memory)
//       x3 = child_tid        (ptr for CLONE_CHILD_SETTID in child's memory)
//       x4 = tls              (TPIDR_EL0 value for child)
// returns: x0 = child ThreadId (parent) / child wakes up with x0=0
//
// Clone flags we handle:
//   CLONE_CHILD_SETTID (0x01000000) — write child TID to *child_tid
//   CLONE_SETTLS       (0x00080000) — set child's TPIDR_EL0
fn sys_clone(tf: &mut TrapFrame) {
    use crate::kernel::bmm;
    use crate::kernel::sche;
    use crate::usr::proc::elf_loader;
    use aarch64::base::mm::{alloc_page, alloc_pages_contig, free_page, free_page_range, watch_pa_range, VirtAddr};
    use core::ptr::write_volatile;

    let flags          = tf.general.x0;
    let child_stack    = tf.general.x1;
    let parent_tid_ptr = tf.general.x2;
    let child_tid_ptr  = tf.general.x3;
    let tls            = tf.general.x4;

    const CLONE_CHILD_SETTID: usize = 0x01000000;
    const CLONE_SETTLS:       usize = 0x00080000;

    // ── 1. Read current thread info ──────────────────────────────────────
    let tid = sche::current_thread();
    let (asid, save_area, handler_pc) = sche::with_thread(tid, |t| {
        (t.asid, t.linux_save_area, t.linux_handler_pc)
    }).unwrap_or((0, None, None));

    let save_area_va = match save_area {
        Some(va) => va,
        None => { tf.general.x0 = (-EINVAL) as usize; return; }
    };
    let handler_pc = match handler_pc {
        Some(pc) => pc,
        None => { tf.general.x0 = (-EINVAL) as usize; return; }
    };

    // ── 2. Read parent's LinuxContext from the user-space save_area ─────
    // (Runs with the parent's TTBR0 active, so the VA resolves correctly.)
    let parent_ctx = unsafe {
        core::ptr::read_volatile(save_area_va as *const LinuxContext)
    };

    // ── 3. Create child page table + clone all user mappings ────────────
    let mut child_pt = match bmm::create_kernel_mapped_page_table() {
        Ok(pt) => pt,
        Err(_) => { tf.general.x0 = (-ENOMEM) as usize; return; }
    };

    let parent_asid = bmm::AddressSpaceId(asid);
    match bmm::with_page_table_mut(parent_asid, |parent_pt| {
        bmm::clone_user_mappings(parent_pt, &mut child_pt)
    }) {
        Some(Ok(())) => {}
        Some(Err(_)) => { tf.general.x0 = (-ENOMEM) as usize; return; }
        None => { tf.general.x0 = (-EINVAL) as usize; return; }
    }

    // ── 4. Patch child's save_area copy — x0 = 0 (fork returns 0) ──────
    let child_save_pa = match child_pt.translate_va_to_pa(VirtAddr::from(save_area_va)) {
        Some(pa) => pa,
        None => { tf.general.x0 = (-ENOMEM) as usize; return; }
    };
    unsafe { write_volatile(child_save_pa as *mut u64, 0u64); }

    // ── 5. Allocate kernel stack for the child (contiguous, zeroed) ─────
    let ks_base = match aarch64::base::mm::alloc_pages_contig(8) {
        Some(p) => p,
        None => { tf.general.x0 = (-ENOMEM) as usize; return; }
    };
    let ks_top = ks_base + elf_loader::KERNEL_STACK_SIZE;

    // DEBUG: check if VA 0x564000 maps to kernel stack range in child PT
    // (disabled for production output)
    /*
    {
        let va_check = 0x564000;
        let saved_x30_addr = ks_top - 0x5D8;
        let saved_x30_page = saved_x30_addr & !0xFFF;
        match child_pt.translate_va_to_pa(VirtAddr::from(va_check)) {
            Some(pa) => {
                crate::print_uart("[clone_debug] VA 0x564000 -> PA=");
                crate::print_uart_hex(pa as u64);
                if pa >= ks_base && pa < ks_top {
                    crate::print_uart(" *** ALIAS! overlaps ks ***");
                }
                crate::print_uart("\n");
            }
            None => {
                crate::print_uart("[clone_debug] VA 0x564000 -> unmapped\n");
            }
        }
        for &va in &[0x564000usize, 0x565000, 0x566000, 0x563000, 0x562000, 0x561000, 0x560000, 0x550000, 0x500000, 0x400000] {
            if let Some(pa) = child_pt.translate_va_to_pa(VirtAddr::from(va)) {
                if pa >= ks_base && pa < ks_top {
                    crate::print_uart("[clone_debug] *** ALIAS: VA=");
                    crate::print_uart_hex(va as u64);
                    crate::print_uart(" -> PA=");
                    crate::print_uart_hex(pa as u64);
                    crate::print_uart(" (in ks range) ***\n");
                }
            }
        }
    }
    */

    // PA watch disabled — was triggering [WATCH#N] spam on every destroy_thread.
    // Register PA watch on child's kernel stack to detect any alloc/free/map touching these pages
    // watch_pa_range(ks_base, ks_top);
    // crate::print_uart("[clone] watching child ks PA [");
    // crate::print_uart_hex(ks_base as u64);
    // crate::print_uart(",");
    // crate::print_uart_hex(ks_top as u64);
    // crate::print_uart(")\n");

    let ctx_addr = ks_top - 128;  // TaskContext
    let tf_addr  = ctx_addr - 288; // TrapFrame below TaskContext

    // ── 6. Build child's initial TrapFrame ──────────────────────────────
    let child_sp = if child_stack != 0 { child_stack }
                   else { parent_ctx.sp as usize };
    // Without CLONE_SETTLS the child inherits the parent's TLS pointer.
    // tf.tpidr holds the caller's TPIDR_EL0 (saved on trap entry), which is
    // the Linux program's TLS — liblinux never modifies it.
    let child_tpidr = if flags & CLONE_SETTLS != 0 { tls } else { tf.tpidr };

    let trapframe = TrapFrame {
        trap_num: 0,
        elr:  parent_ctx.elr as usize,
        spsr: parent_ctx.spsr as usize,
        sp:   child_sp,
        tpidr: child_tpidr,
        general: crate::kernel::trap::GeneralRegs {
            x0:  0,
            x1:  parent_ctx.x1  as usize, x2:  parent_ctx.x2  as usize,
            x3:  parent_ctx.x3  as usize, x4:  parent_ctx.x4  as usize,
            x5:  parent_ctx.x5  as usize, x6:  parent_ctx.x6  as usize,
            x7:  parent_ctx.x7  as usize, x8:  parent_ctx.x8  as usize,
            x9:  parent_ctx.x9  as usize, x10: parent_ctx.x10 as usize,
            x11: parent_ctx.x11 as usize, x12: parent_ctx.x12 as usize,
            x13: parent_ctx.x13 as usize, x14: parent_ctx.x14 as usize,
            x15: parent_ctx.x15 as usize, x16: parent_ctx.x16 as usize,
            x17: parent_ctx.x17 as usize, x18: parent_ctx.x18 as usize,
            x19: parent_ctx.x19 as usize, x20: parent_ctx.x20 as usize,
            x21: parent_ctx.x21 as usize, x22: parent_ctx.x22 as usize,
            x23: parent_ctx.x23 as usize, x24: parent_ctx.x24 as usize,
            x25: parent_ctx.x25 as usize, x26: parent_ctx.x26 as usize,
            x27: parent_ctx.x27 as usize, x28: parent_ctx.x28 as usize,
            x29: parent_ctx.x29 as usize, x30: parent_ctx.x30 as usize,
        },
    };
    unsafe { write_volatile(tf_addr as *mut TrapFrame, trapframe); }

    // ── 7. Register page table, get ASID + TTBR0 ───────────────────────
    let (child_asid, child_ttbr0) = match bmm::register_page_table(child_pt) {
        Some((id, token)) => (id, token),
        None => {
            free_page_range(ks_base, ks_base + elf_loader::KERNEL_STACK_SIZE);
            tf.general.x0 = (-ENOMEM) as usize; return;
        }
    };

    // ── 8. Create kernel thread in the child's address space ────────────
    let child_tid = match unsafe {
        sche::create_thread(128, ks_base, ctx_addr, elf_loader::KERNEL_STACK_SIZE, child_ttbr0, child_asid.0)
    } {
        Ok(id) => id,
        Err(_) => {
            bmm::unregister_address_space(child_asid);
            free_page_range(ks_base, ks_base + elf_loader::KERNEL_STACK_SIZE);
            tf.general.x0 = (-ENOMEM) as usize; return;
        }
    };

    // ── 9. Build TaskContext on child's kernel stack ────────────────────
    let tcb_addr = unsafe { sche::tcb_ptr(child_tid) } as usize;
    unsafe {
        write_volatile((ctx_addr + 0x00) as *mut usize, thread_trampoline_addr());
        write_volatile((ctx_addr + 0x08) as *mut usize, tcb_addr);
        write_volatile((ctx_addr + 0x10) as *mut usize, tf_addr);      // x20 = tf_addr
        write_volatile((ctx_addr + 0x70) as *mut usize, 0usize);       // ttbr1 = 0
        write_volatile((ctx_addr + 0x78) as *mut usize, child_ttbr0);  // isolated PT
    }

    // ── 10. Set child's parent + Linux handler info ──
    let _ = sche::with_thread_mut(child_tid, |t| {
        t.parent_tid = Some(tid);
        t.linux_handler_pc = Some(handler_pc);
        t.linux_save_area  = Some(save_area_va);
    });

    // ── 11. CLONE_CHILD_SETTID — write child TID into child's memory ───
    if flags & CLONE_CHILD_SETTID != 0 && child_tid_ptr != 0 {
        if let Some(tid_pa) = bmm::with_page_table_mut(child_asid, |pt| {
            pt.translate_va_to_pa(VirtAddr::from(child_tid_ptr))
        }).flatten() {
            unsafe { write_volatile(tid_pa as *mut u32, child_tid.0); }
        }
    }

    // ── 12. Enqueue child thread ────────────────────────────────────────
    let prio = sche::with_thread(child_tid, |t| t.effective_priority()).unwrap_or(128);
    sche::enqueue_ready(child_tid, prio).ok();

    // ── 13. Full TLB flush so the new AS is usable ──────────────────────
    unsafe { core::arch::asm!("dsb ish; tlbi vmalle1is; dsb ish; isb"); }

    // ── 14. Write child TID into parent's memory if requested ───────────
    if parent_tid_ptr != 0 {
        unsafe { write_volatile(parent_tid_ptr as *mut u32, child_tid.0); }
    }

    // ── 15. Return child TID to parent ──────────────────────────────────
    tf.general.x0 = child_tid.0 as usize;
}

// ---------------------------------------------------------------------------
// 16. sys_exec — replace current address space with a new ELF (execve)
// ---------------------------------------------------------------------------
// args: x0 = elf_data_ptr (user-space ELF buffer)
//       x1 = elf_len       (length in bytes)
//       x2 = stack_top     (top of user stack for new process)
//       x3 = bootinfo_ptr  (user-space BootInfo for the new program)
// returns: does not return on success (TrapFrame is overwritten)
fn sys_exec(tf: &mut TrapFrame) {
    use crate::kernel::bmm;
    use crate::usr::proc::elf_loader;
    use alloc::vec;

    const BOOTINFO_VA: usize = 0x208110; // must match elf_loader::BOOTINFO_VA

    let elf_ptr      = tf.general.x0;
    let elf_len      = tf.general.x1;
    let stack_top    = tf.general.x2;
    let bootinfo_ptr = tf.general.x3;

    // crate::print_uart("[exec] sys_exec called ptr=");
    // crate::print_uart_hex(elf_ptr as u64);
    // crate::print_uart(" len=");
    // crate::print_uart_hex(elf_len as u64);
    // crate::print_uart("\n");

    // Validate
    if elf_len == 0 || elf_len > 1024 * 1024 * 16 {
        // crate::print_uart("[exec] FAIL: invalid elf_len\n");
        tf.general.x0 = (-EINVAL) as usize;
        return;
    }
    if stack_top & 0xF != 0 || stack_top >= 0x0000_8000_0000_0000 {
        // crate::print_uart("[exec] FAIL: invalid stack_top\n");
        tf.general.x0 = (-EINVAL) as usize;
        return;
    }

    // Copy ELF data from user space
    let mut elf_buf = vec![0u8; elf_len as usize];
    unsafe {
        core::ptr::copy_nonoverlapping(
            elf_ptr as *const u8,
            elf_buf.as_mut_ptr(),
            elf_len as usize,
        );
    }

    // Copy BootInfo from user space
    #[repr(C)]
    struct BootInfo { program_entry: u64, stack_top: u64, brk: u64,
                      phdr_addr: u64, phent_size: u64, phnum: u64, }
    let bootinfo = unsafe { core::ptr::read_volatile(bootinfo_ptr as *const BootInfo) };

    // Create a new page table with kernel identity mappings
    let mut pt = match bmm::create_kernel_mapped_page_table() {
        Ok(pt) => pt,
        Err(_) => {
            // crate::print_uart("[exec] FAIL: create_kernel_mapped_page_table\n");
            tf.general.x0 = (-ENOMEM) as usize;
            return;
        }
    };

    // Clone only the liblinux image into the new AS — the old program's
    // pages (code, stack, mmap regions) must NOT survive an exec.
    let lib_entry = *LIBLINUX_ENTRY.lock();
    let lib_start = *LIBLINUX_VA_START.lock();
    let lib_end   = *LIBLINUX_VA_END.lock();
    if lib_entry == 0 || lib_start >= lib_end {
        // crate::print_uart("[exec] FAIL: invalid liblinux range\n");
        tf.general.x0 = (-ENOMEM) as usize;
        return;
    }

    // crate::print_uart("[exec] liblinux range [");
    // crate::print_uart_hex(lib_start as u64);
    // crate::print_uart(",");
    // crate::print_uart_hex(lib_end as u64);
    // crate::print_uart(") entry=");
    // crate::print_uart_hex(lib_entry as u64);
    // crate::print_uart("\n");

    let current_tid = crate::kernel::sche::current_thread();
    let old_asid_val = crate::kernel::sche::with_thread(current_tid, |t| t.asid).unwrap_or(0);
    // crate::print_uart("[exec] old_asid=");
    // crate::print_uart_hex(old_asid_val as u64);
    // crate::print_uart("\n");

    if old_asid_val != 0 {
        let old_asid = bmm::AddressSpaceId(old_asid_val);
        let _ = bmm::with_page_table_mut(old_asid, |old_pt| {
            bmm::clone_user_range(old_pt, &mut pt, lib_start, lib_end)
        });
        // crate::print_uart("[exec] liblinux cloned\n");
    }

    // Load the new Linux ELF into the page table
    // crate::print_uart("[exec] loading new ELF...\n");
    if let Err(_) = elf_loader::load_elf_bytes(&mut pt, &elf_buf) {
        // crate::print_uart("[exec] FAIL: load_elf_bytes\n");
        tf.general.x0 = (-EINVAL) as usize;
        return;
    }
    // crate::print_uart("[exec] ELF loaded OK\n");

    // Map user stack
    elf_loader::map_user_stack(&mut pt);

    // Write BootInfo into the new AS at BOOTINFO_VA
    let boot_pa = match pt.translate_va_to_pa(aarch64::base::mm::VirtAddr::from(BOOTINFO_VA)) {
        Some(pa) => pa,
        None => {
            // crate::print_uart("[exec] FAIL: BOOTINFO_VA not mapped\n");
            tf.general.x0 = (-ENOMEM) as usize;
            return;
        }
    };
    unsafe { core::ptr::write_volatile(boot_pa as *mut BootInfo, bootinfo); }
    // crate::print_uart("[exec] BootInfo written pa=");
    // crate::print_uart_hex(boot_pa as u64);
    // crate::print_uart("\n");

    // Register new AS
    let (new_asid, new_ttbr0) = match bmm::register_page_table(pt) {
        Some((id, token)) => (id, token),
        None => {
            // crate::print_uart("[exec] FAIL: register_page_table\n");
            tf.general.x0 = (-ENOMEM) as usize; return;
        }
    };
    // crate::print_uart("[exec] new AS registered asid=");
    // crate::print_uart_hex(new_asid.0 as u64);
    // crate::print_uart(" ttbr0=");
    // crate::print_uart_hex(new_ttbr0 as u64);
    // crate::print_uart("\n");

    // Update current thread's AS
    let _ = crate::kernel::sche::with_thread_mut(current_tid, |t| {
        t.asid = new_asid.0;
        t.ttbr0 = new_ttbr0;
        t.linux_handler_pc = None;
        t.linux_save_area = None;
    });

    // Overwrite TrapFrame to restart into liblinux _start
    tf.elr = lib_entry;
    tf.spsr = 0; // EL0t
    tf.sp = stack_top;
    tf.general.x0 = 0;
    tf.general.x1  = 0; tf.general.x2  = 0; tf.general.x3  = 0;
    tf.general.x4  = 0; tf.general.x5  = 0; tf.general.x6  = 0;
    tf.general.x7  = 0; tf.general.x8  = 0; tf.general.x9  = 0;
    tf.general.x10 = 0; tf.general.x11 = 0; tf.general.x12 = 0;
    tf.general.x13 = 0; tf.general.x14 = 0; tf.general.x15 = 0;
    tf.general.x16 = 0; tf.general.x17 = 0; tf.general.x18 = 0;
    tf.general.x19 = 0; tf.general.x20 = 0; tf.general.x21 = 0;
    tf.general.x22 = 0; tf.general.x23 = 0; tf.general.x24 = 0;
    tf.general.x25 = 0; tf.general.x26 = 0; tf.general.x27 = 0;
    tf.general.x28 = 0; tf.general.x29 = 0; tf.general.x30 = 0;

    // Switch to the new TTBR0 and flush TLB BEFORE freeing the old AS.
    // The old page table pages must not be walked after they are freed.
    // crate::print_uart("[exec] switching TTBR0, eret to liblinux _start=");
    // crate::print_uart_hex(lib_entry as u64);
    // crate::print_uart("\n");

    unsafe {
        core::arch::asm!(
            "msr ttbr0_el1, {ttbr}",
            "isb",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            ttbr = in(reg) new_ttbr0,
        );
    }

    // Detach liblinux PTEs from the old page table BEFORE freeing it.
    // `clone_user_range` deep-copied these pages into the new PT with
    // different physical pages.  The old PTEs are stale — clear them so
    // no stale mapping survives into any page that gets reused later.
    if old_asid_val != 0 {
        let old_asid = bmm::AddressSpaceId(old_asid_val);
        let _ = bmm::with_page_table_mut(old_asid, |old_pt| {
            use aarch64::base::mm::VirtPageNum;
            let page_size = aarch64::base::config::PAGE_SIZE;
            let mut va = lib_start & !(page_size - 1);
            let end = (lib_end + page_size - 1) & !(page_size - 1);
            while va < end {
                let vpn = VirtPageNum::from(va >> 12);
                if old_pt.find_pte(vpn).map_or(false, |pte| pte.is_valid()) {
                    old_pt.unmap(vpn);
                }
                va += page_size;
            }
        });
    }

    // Now safe to free the old AS — TTBR0 points to the new one.
    if old_asid_val != 0 {
        bmm::unregister_address_space(bmm::AddressSpaceId(old_asid_val));
    }

    // Return 0 — but the caller (trap_return) will eret to liblinux _start,
    // not back to the old binary.  This is the execve semantic.
    tf.general.x0 = 0;
}

// ---------------------------------------------------------------------------
// 17. sys_wait4 — wait for a child thread to exit
// ---------------------------------------------------------------------------
// args: none (uses current thread to find children)
// returns: x0 = child tid on success / -ECHILD if no children
fn sys_wait4(tf: &mut TrapFrame) {
    let current_tid = crate::kernel::sche::current_thread();

    // Reap any dying child.  If children exist but none has exited yet,
    // block until sys_exit_thread wakes us.
    let result = loop {
        let (child, has_children) = crate::kernel::sche::with_all_threads(|threads| {
            let mut dying = None;
            let mut any = false;
            for t in threads.iter() {
                if t.parent_tid == Some(current_tid) {
                    any = true;
                    if t.atomic_state() == crate::kernel::sche::ThreadState::Dying {
                        dying = Some((t.id, t.exit_code));
                        break;
                    }
                }
            }
            (dying, any)
        });

        if let Some(ct) = child {
            break Some(ct);
        }

        if has_children {
            // Block until a child transitions to Dying.
            unsafe {
                crate::kernel::ipc::synchronization::block_current(
                    crate::kernel::ipc::synchronization::IpcState::BlockedOnWait4,
                );
            }
            // When woken, re-scan for the dying child.
        } else {
            break None;
        }
    };

    match result {
        Some((child_tid, exit_code)) => {
            // Clean up: destroy the child thread + its AS
            let child_asid = crate::kernel::sche::with_thread(child_tid, |t| t.asid).unwrap_or(0);
            if child_asid != 0 {
                crate::kernel::bmm::unregister_address_space(
                    crate::kernel::bmm::AddressSpaceId(child_asid));
            }
            let _ = crate::kernel::sche::destroy_thread(child_tid);
            // Return pid in x0, status in x1
            // status = (exit_code & 0xFF) << 8  (WIFEXITED + WEXITSTATUS encoding)
            let status = ((exit_code & 0xFF) << 8) as usize;
            tf.general.x0 = child_tid.0 as usize;
            tf.general.x1 = status;
        }
        None => {
            const ECHILD: isize = 10;
            tf.general.x0 = (-ECHILD as i64) as usize;
        }
    }
}

// ---------------------------------------------------------------------------
// 11. sys_console_write — write bytes to UART from user mode
// ---------------------------------------------------------------------------
fn sys_console_write(tf: &mut TrapFrame) {
    let buf_ptr = tf.general.x0;
    let len = tf.general.x1;

    let n = len.min(4096);
    for i in 0..n {
        let byte = unsafe { core::ptr::read_volatile((buf_ptr as *const u8).add(i)) };
        unsafe { core::ptr::write_volatile(0x09000000 as *mut u8, byte); }
    }
    tf.general.x0 = n;
}

// ---------------------------------------------------------------------------
// 15. sys_console_read — read pending bytes from UART (non-blocking)
// ---------------------------------------------------------------------------
/// Drains up to `len` bytes from the PL011 RX FIFO into `buf_ptr`.
/// Returns the number of bytes read (0 when the FIFO is empty — the caller
/// is expected to yield and retry for blocking semantics).
fn sys_console_read(tf: &mut TrapFrame) {
    const UART_DR: *const u32 = 0x0900_0000 as *const u32;
    const UART_FR: *const u32 = 0x0900_0018 as *const u32;
    const FR_RXFE: u32 = 1 << 4;

    let buf_ptr = tf.general.x0;
    let len = tf.general.x1.min(4096);

    let mut n = 0usize;
    while n < len {
        let fr = unsafe { core::ptr::read_volatile(UART_FR) };
        if fr & FR_RXFE != 0 {
            break;
        }
        let byte = (unsafe { core::ptr::read_volatile(UART_DR) } & 0xFF) as u8;
        unsafe { core::ptr::write_volatile((buf_ptr as *mut u8).add(n), byte); }
        n += 1;
    }
    tf.general.x0 = n;
}

// ---------------------------------------------------------------------------
// 18. sys_create_notification — create a new notification object
// ---------------------------------------------------------------------------
// args: none
// returns: x0 = notification_id on success, negative errno on failure
fn sys_create_notification(tf: &mut TrapFrame) {
    match crate::kernel::ipc::notification::create_notification() {
        Ok(nid) => tf.general.x0 = nid.0 as usize,
        Err(_) => tf.general.x0 = (-ENOMEM) as usize,
    }
}

// ---------------------------------------------------------------------------
// 19. sys_notify_send — signal (post) a notification
// ---------------------------------------------------------------------------
// args: x0 = notification_id
// returns: x0 = 0 on success, negative errno on failure
fn sys_notify_send(tf: &mut TrapFrame) {
    let nid = crate::kernel::ipc::notification::NotificationId(tf.general.x0 as u32);
    match crate::kernel::ipc::notification::signal_notification(nid) {
        Ok(()) => tf.general.x0 = 0,
        Err(_) => tf.general.x0 = (-EBADF) as usize,
    }
}

// ---------------------------------------------------------------------------
// 20. sys_notify_wait — wait (pend) on a notification
// ---------------------------------------------------------------------------
// args: x0 = notification_id
// returns: x0 = 0 on success (after being signaled), negative errno on failure
fn sys_notify_wait(tf: &mut TrapFrame) {
    let nid = crate::kernel::ipc::notification::NotificationId(tf.general.x0 as u32);
    match crate::kernel::ipc::notification::wait_on_notification(nid) {
        Ok(()) => tf.general.x0 = 0,
        Err(_) => tf.general.x0 = (-EBADF) as usize,
    }
}

// ---------------------------------------------------------------------------
// 21. sys_irq_register — register an IRQ line and create a notification
// ---------------------------------------------------------------------------
// args: x0 = irq_num
// returns: x0 = notification_id on success, negative errno on failure
fn sys_irq_register(tf: &mut TrapFrame) {
    let irq_num = tf.general.x0 as u32;
    match crate::kernel::irq::register_irq(irq_num) {
        Ok(nid) => tf.general.x0 = nid.0 as usize,
        Err(e) => tf.general.x0 = e as usize,
    }
}

// ---------------------------------------------------------------------------
// 22. sys_irq_ack — acknowledge (EOI) an IRQ line
// ---------------------------------------------------------------------------
// args: x0 = irq_num
// returns: x0 = 0 on success, negative errno on failure
fn sys_irq_ack(tf: &mut TrapFrame) {
    let irq_num = tf.general.x0 as u32;
    match crate::kernel::irq::ack_irq(irq_num) {
        Ok(()) => tf.general.x0 = 0,
        Err(e) => tf.general.x0 = e as usize,
    }
}

// ---------------------------------------------------------------------------
// 23. sys_ipc_recv_timeout — receive with timeout
// ---------------------------------------------------------------------------
// args: x0 = channel_id, x1 = buf_ptr, x2 = buf_len, x3 = timeout_ms
// returns: x0 = bytes read on success, negative errno on failure
fn sys_ipc_recv_timeout(tf: &mut TrapFrame) {
    let ch_raw = tf.general.x0 as u32;
    let buf_ptr = tf.general.x1;
    let buf_len = tf.general.x2;
    let timeout_ms = tf.general.x3 as u32;

    const ETIMEDOUT: isize = 110;

    let channel_id = crate::kernel::ipc::channel::ChannelId(ch_raw);
    let tid = crate::kernel::sche::current_thread();
    use crate::kernel::ipc::channel::{with_channel, RecvMatch};
    use crate::kernel::ipc::{deliver, wake, get_ipc_buffer};
    use crate::kernel::sche::{block_current, IpcState};
    use crate::kernel::ipc::message::Message;

    let action = match with_channel(channel_id, |inner| inner.match_receiver(tid)) {
        Ok(a) => a,
        Err(_) => { tf.general.x0 = (-EBADF) as usize; return; }
    };

    match action {
        RecvMatch::Matched(sender_entry) => {
            let msg = sender_entry.msg.unwrap_or_else(|| {
                Message::new_short(
                    0,
                    crate::kernel::ipc::message::ShortPayload { words: [0; 32], len: 0 }
                )
            });
            let sender_tid = sender_entry.thread_id;
            let _ = deliver(&msg, tid, None, None);
            wake(sender_tid);

            if let Message::Short(_, ref payload) = msg {
                tf.general.x0 = unpack_short(payload, buf_ptr, buf_len);
            } else {
                tf.general.x0 = 0;
            }
        }
        RecvMatch::Parked => {
            if timeout_ms > 0 {
                crate::kernel::timer::set_ipc_timeout(tid, timeout_ms);
            }
            unsafe { block_current(IpcState::BlockedOnReceive(channel_id)); }

            let timed_out = crate::kernel::sche::with_thread(tid, |t| {
                matches!(t.ipc_state, IpcState::TimedOut)
            }).unwrap_or(false);

            if timed_out {
                let _ = with_channel(channel_id, |inner| {
                    inner.cancel_receive(tid);
                    Ok(())
                });
                tf.general.x0 = (-ETIMEDOUT) as usize;
                return;
            }

            crate::kernel::timer::cancel_ipc_timeout(tid);

            let buf = match get_ipc_buffer(tid) {
                Ok(b) => b,
                Err(_) => { tf.general.x0 = 0; return; }
            };
            if let Some(payload) = buf.read_short() {
                let n = unpack_short(&payload, buf_ptr, buf_len);
                tf.general.x0 = n;
            } else {
                tf.general.x0 = 0;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 24. sys_ipc_call_timeout — synchronous IPC (send + recv) with timeout
// ---------------------------------------------------------------------------
// args: x0 = channel_id, x1 = send_ptr, x2 = send_len, x3 = recv_buf,
//       x4 = recv_len, x5 = timeout_ms
#[inline(never)]
fn sys_ipc_call_timeout(tf: &mut TrapFrame) {
    let ch_raw = tf.general.x0 as u32;
    let send_ptr = tf.general.x1;
    let send_len = tf.general.x2;
    let recv_buf = tf.general.x3;
    let recv_len = tf.general.x4;
    let timeout_ms = tf.general.x5 as u32;

    // Send phase
    let saved_x0 = tf.general.x0; let saved_x1 = tf.general.x1; let saved_x2 = tf.general.x2;
    let saved_x3 = tf.general.x3; let saved_x4 = tf.general.x4;
    tf.general.x0 = ch_raw as usize; tf.general.x1 = send_ptr; tf.general.x2 = send_len;
    sys_ipc_send(tf);
    if tf.general.x0 != 0 { return; }

    // Receive phase with timeout
    tf.general.x0 = ch_raw as usize; tf.general.x1 = recv_buf; tf.general.x2 = recv_len;
    tf.general.x3 = timeout_ms as usize;
    sys_ipc_recv_timeout(tf);

    // Restore original registers
    tf.general.x1 = saved_x1; tf.general.x2 = saved_x2;
    tf.general.x3 = saved_x3; tf.general.x4 = saved_x4;
}

// ---------------------------------------------------------------------------
// 25. sys_cspace_mint — create a root capability and insert into CSpace
// ---------------------------------------------------------------------------
// args: x0 = obj_id, x1 = cap_type (u8), x2 = rights (u16)
// returns: x0 = cptr on success, negative errno on failure
fn sys_cspace_mint(tf: &mut TrapFrame) {
    let obj_id = tf.general.x0;
    let cap_type_raw = tf.general.x1 as u8;
    let rights_raw = tf.general.x2 as u16;

    let cap_type = match cap_type_from_u8(cap_type_raw) {
        Some(ct) => ct,
        None => { tf.general.x0 = (-EINVAL) as usize; return; }
    };

    let pid: crate::kernel::ipc::message::ProcessId = crate::kernel::sche::current_thread().0;

    let cap = match crate::kernel::cap::mint_cap(
        obj_id,
        cap_type,
        crate::kernel::cap::CapRights(rights_raw),
        pid,
    ) {
        Ok(c) => c,
        Err(e) => { tf.general.x0 = cap_errno(e) as usize; return; }
    };

    match crate::kernel::cap::insert_cap(pid, cap) {
        Ok(cptr) => tf.general.x0 = cptr.0,
        Err(e) => tf.general.x0 = cap_errno(e) as usize,
    }
}

// ---------------------------------------------------------------------------
// 26. sys_cspace_derive — derive a child capability with reduced rights
// ---------------------------------------------------------------------------
// args: x0 = src_cptr, x1 = new_rights (u16)
// returns: x0 = dest_cptr on success, negative errno on failure
fn sys_cspace_derive(tf: &mut TrapFrame) {
    let src_cptr = crate::kernel::cap::CPtr(tf.general.x0);
    let new_rights = crate::kernel::cap::CapRights(tf.general.x1 as u16);

    let pid: crate::kernel::ipc::message::ProcessId = crate::kernel::sche::current_thread().0;

    let parent_cap = match crate::kernel::cap::lookup_cap(pid, src_cptr) {
        Ok(c) => c,
        Err(e) => { tf.general.x0 = cap_errno(e) as usize; return; }
    };

    let derived = match crate::kernel::cap::derive_cap(&parent_cap, new_rights, pid) {
        Ok(c) => c,
        Err(e) => { tf.general.x0 = derive_errno(e) as usize; return; }
    };

    match crate::kernel::cap::insert_cap(pid, derived) {
        Ok(cptr) => tf.general.x0 = cptr.0,
        Err(e) => tf.general.x0 = cap_errno(e) as usize,
    }
}

// ---------------------------------------------------------------------------
// 27. sys_cspace_revoke — revoke a capability and all descendants
// ---------------------------------------------------------------------------
// args: x0 = cptr
// returns: x0 = 0 on success, negative errno on failure
fn sys_cspace_revoke(tf: &mut TrapFrame) {
    let cptr = crate::kernel::cap::CPtr(tf.general.x0);
    let pid: crate::kernel::ipc::message::ProcessId = crate::kernel::sche::current_thread().0;

    let cap = match crate::kernel::cap::lookup_cap(pid, cptr) {
        Ok(c) => c,
        Err(e) => { tf.general.x0 = cap_errno(e) as usize; return; }
    };

    // Revoke in the derivation tree first
    if let Err(e) = crate::kernel::cap::revoke(&cap) {
        tf.general.x0 = cap_errno(e) as usize;
        return;
    }

    // Then remove from CSpace
    match crate::kernel::cap::remove_cap(pid, cptr) {
        Ok(_) => tf.general.x0 = 0,
        Err(e) => tf.general.x0 = cap_errno(e) as usize,
    }
}

// ---------------------------------------------------------------------------
// 28. sys_cspace_move — move a capability from one slot to another
// ---------------------------------------------------------------------------
// args: x0 = src_cptr, x1 = dest_cptr
// returns: x0 = 0 on success, negative errno on failure
fn sys_cspace_move(tf: &mut TrapFrame) {
    let src_cptr = crate::kernel::cap::CPtr(tf.general.x0);
    let dest_cptr = crate::kernel::cap::CPtr(tf.general.x1);
    let pid: crate::kernel::ipc::message::ProcessId = crate::kernel::sche::current_thread().0;

    let cap = match crate::kernel::cap::remove_cap(pid, src_cptr) {
        Ok(c) => c,
        Err(e) => { tf.general.x0 = cap_errno(e) as usize; return; }
    };

    match crate::kernel::cap::insert_cap_at(pid, dest_cptr, cap) {
        Ok(()) => tf.general.x0 = 0,
        Err(e) => tf.general.x0 = cap_errno(e) as usize,
    }
}

// ---------------------------------------------------------------------------
// 29. sys_cspace_delete — delete a capability from a slot
// ---------------------------------------------------------------------------
// args: x0 = cptr
// returns: x0 = 0 on success, negative errno on failure
fn sys_cspace_delete(tf: &mut TrapFrame) {
    let cptr = crate::kernel::cap::CPtr(tf.general.x0);
    let pid: crate::kernel::ipc::message::ProcessId = crate::kernel::sche::current_thread().0;

    match crate::kernel::cap::remove_cap(pid, cptr) {
        Ok(_) => tf.general.x0 = 0,
        Err(e) => tf.general.x0 = cap_errno(e) as usize,
    }
}

// ---------------------------------------------------------------------------
// Error mapping helpers
// ---------------------------------------------------------------------------

fn cap_type_from_u8(v: u8) -> Option<crate::kernel::cap::CapType> {
    match v {
        0 => Some(crate::kernel::cap::CapType::Untyped),
        1 => Some(crate::kernel::cap::CapType::Endpoint),
        2 => Some(crate::kernel::cap::CapType::Thread),
        3 => Some(crate::kernel::cap::CapType::PageTable),
        4 => Some(crate::kernel::cap::CapType::Frame),
        5 => Some(crate::kernel::cap::CapType::Notification),
        6 => Some(crate::kernel::cap::CapType::CNode),
        _ => None,
    }
}

fn cap_errno(e: crate::kernel::cap::CapError) -> isize {
    use crate::kernel::cap::CapError;
    match e {
        CapError::InvalidCPtr | CapError::EmptySlot => EBADF,
        CapError::RightsEscalation | CapError::Revoked | CapError::GrantChainBroken => EACCES,
        CapError::WrongCapType | CapError::InvalidProcess | CapError::InvalidArgument => EINVAL,
        CapError::CSpaceFull | CapError::CNodeFull
        | CapError::UntypedTooSmall | CapError::UntypedExhausted => ENOMEM,
        CapError::NotImplemented => ENOSYS,
    }
}

fn derive_errno(e: crate::kernel::cap::DeriveError) -> isize {
    use crate::kernel::cap::DeriveError;
    match e {
        DeriveError::RightsEscalation | DeriveError::ParentRevoked => EACCES,
        DeriveError::TableFull => ENOMEM,
    }
}
