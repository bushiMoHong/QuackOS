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

use super::{LinuxContext, TrapFrame};

// ---------------------------------------------------------------------------
// Linux-compatible errno constants (negative return values)
// ---------------------------------------------------------------------------

const EINVAL:   isize = 22;
const ENOMEM:   isize = 12;
const ENOSYS:   isize = 38;
const EBADF:    isize = 9;
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
        _  => {
            tf.general.x0 = (-ENOSYS) as usize;
        }
    }
}

// ---------------------------------------------------------------------------
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

    // Build PTEFlags
    let mut flags = aarch64::base::mm::page_table::PTEFlags::empty();
    flags.insert(aarch64::base::mm::page_table::PTEFlags::V);
    flags.insert(aarch64::base::mm::page_table::PTEFlags::A);
    flags.insert(aarch64::base::mm::page_table::PTEFlags::D);
    flags.insert(aarch64::base::mm::page_table::PTEFlags::U);
    if prot & 1 != 0 { flags.insert(aarch64::base::mm::page_table::PTEFlags::R); }
    if prot & 2 != 0 { flags.insert(aarch64::base::mm::page_table::PTEFlags::W); }
    if prot & 4 != 0 { flags.insert(aarch64::base::mm::page_table::PTEFlags::X); }

    // Map into current address space
    use aarch64::base::mm::{VirtPageNum, PhysPageNum};
    let vpn = VirtPageNum::from(vaddr >> 12);
    let ppn = PhysPageNum::from(pa >> 12);

    // Get the kernel page table (shared with user space)
    let l0_pa = crate::KERNEL_L0_PA.lock();
    let mut pt = aarch64::base::mm::page_table::PageTable::from_token(*l0_pa);
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

    let l0_pa = crate::KERNEL_L0_PA.lock();
    let mut pt = aarch64::base::mm::page_table::PageTable::from_token(*l0_pa);
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

/// Pack raw bytes into a `ShortPayload` (max 64 bytes).
fn pack_short(msg_ptr: usize, msg_len: usize) -> crate::kernel::ipc::message::ShortPayload {
    let mut words = [0usize; 8];
    let n = msg_len.min(64);
    let mut buf = [0u8; 64];
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
    crate::kernel::ipc::message::ShortPayload { words, len: n as u8 }
}

/// Extract raw bytes from a `ShortPayload` and copy to user buffer.
fn unpack_short(payload: &crate::kernel::ipc::message::ShortPayload, buf_ptr: usize, buf_len: usize) -> usize {
    let n = (payload.len as usize).min(buf_len).min(64);
    let mut buf = [0u8; 64];
    for i in 0..8 {
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

    if msg_len > 64 {
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
                    crate::kernel::ipc::message::ShortPayload { words: [0; 8], len: 0 }
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
                tf.general.x0 = unpack_short(&payload, buf_ptr, buf_len);
            } else {
                tf.general.x0 = 0;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 5. sys_ipc_call — synchronous IPC (send + recv on same channel)
// ---------------------------------------------------------------------------
fn sys_ipc_call(tf: &mut TrapFrame) {
    let ch_raw = tf.general.x0 as u32;
    let send_ptr = tf.general.x1;
    let send_len = tf.general.x2;
    let recv_buf = tf.general.x3;
    let recv_len = tf.general.x4;

    // Send phase: set up registers and call sys_ipc_send
    let saved_x0 = tf.general.x0; let saved_x1 = tf.general.x1; let saved_x2 = tf.general.x2;
    tf.general.x0 = ch_raw as usize; tf.general.x1 = send_ptr; tf.general.x2 = send_len;
    sys_ipc_send(tf);
    if tf.general.x0 != 0 { return; }

    // Receive phase
    tf.general.x0 = ch_raw as usize; tf.general.x1 = recv_buf; tf.general.x2 = recv_len;
    sys_ipc_recv(tf);

    // Restore original registers (caller may need them)
    tf.general.x0 = tf.general.x0; // keep return value from recv
    tf.general.x1 = saved_x1; tf.general.x2 = saved_x2;
}

// ---------------------------------------------------------------------------
// 6. sys_create_thread — create a new user-space thread
// ---------------------------------------------------------------------------

extern "C" fn thread_trampoline(next_sp: usize) -> ! {
    // __switch restored this thread's context and did `ret` here.
    // x0 (= next_sp) still points to the saved TaskContext area.
    // The TrapFrame sits right below it (288 bytes).
    let tf_ptr = next_sp - 288; // size_of::<TrapFrame>()
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

    // Allocate kernel stack (2 pages = 8 KB)
    let stack_pa0 = match alloc_page() { Some(p) => p, None => { tf.general.x0 = (-ENOMEM) as usize; return; } };
    let stack_pa1 = match alloc_page() { Some(p) => p, None => { tf.general.x0 = (-ENOMEM) as usize; return; } };

    let stack_base = stack_pa0;
    let stack_top_ks = stack_pa1 + PAGE_SIZE; // top of second page
    let stack_size = 2 * PAGE_SIZE;

    // Zero the stack pages
    unsafe {
        core::ptr::write_bytes(stack_pa0 as *mut u8, 0, stack_size);
    }

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
            0,                  // ttbr0 (shares kernel page table)
            0,                  // asid
        )
    } {
        Ok(tid) => tid,
        Err(_) => {
            aarch64::base::mm::free_page(stack_pa0);
            aarch64::base::mm::free_page(stack_pa1);
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
    let _exit_code = tf.general.x0;

    let current_tid = crate::kernel::sche::current_thread();

    // Mark as Dying — scheduler will skip re-enqueuing.
    let _ = crate::kernel::sche::with_thread_mut(current_tid, |t| {
        t.set_atomic_state(crate::kernel::sche::ThreadState::Dying);
    });

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

    // Debug: dump registers AND raw memory at trap frame offsets
    use super::uart_puts;
    use super::uart_put_hex;
    let tf_base = tf as *const TrapFrame as *const u8;
    uart_puts("[REGHLR] tf.general.x0=");
    uart_put_hex(tf.general.x0 as u64);
    uart_puts(" x1=");
    uart_put_hex(tf.general.x1 as u64);
    uart_puts("\n[REGHLR] x30=");
    uart_put_hex(tf.general.x30 as u64);
    uart_puts(" x8=");
    uart_put_hex(tf.general.x8 as u64);
    uart_puts(" elr=");
    uart_put_hex(tf.elr as u64);
    // Raw memory reads to verify struct layout
    uart_puts("\n[REGHLR] RAW[272]=");
    uart_put_hex(unsafe { core::ptr::read_volatile(tf_base.add(272) as *const u64) });
    uart_puts(" RAW[280]=");
    uart_put_hex(unsafe { core::ptr::read_volatile(tf_base.add(280) as *const u64) });
    uart_puts(" RAW[40]=");
    uart_put_hex(unsafe { core::ptr::read_volatile(tf_base.add(40) as *const u64) });
    uart_puts("\n");

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
    use super::uart_puts;
    use super::uart_put_hex;
    uart_puts("[LSD] sys_linux_syscall_done entered\n");

    let return_value = tf.general.x0;

    let tid = crate::kernel::sche::current_thread();

    // Read save_area vaddr from per-thread TCB
    let save_area = crate::kernel::sche::with_thread(tid, |thread| {
        thread.linux_save_area
    });

    let save_area = match save_area {
        Ok(Some(addr)) => addr,
        _ => {
            uart_puts("[LSD] no save_area, hanging\n");
            loop { unsafe { core::arch::asm!("wfi"); } }
        }
    };

    // Read LinuxContext from user-mode save_area
    // SAFETY: save_area points to liblinux-allocated memory in user space.
    // The address is trusted because it was registered by liblinux itself.
    let ctx = unsafe {
        let ctx_ptr = save_area as *const LinuxContext;
        core::ptr::read_volatile(ctx_ptr)
    };

    uart_puts("[LSD] save_area=");
    uart_put_hex(save_area as u64);
    uart_puts(" ret_val=");
    uart_put_hex(return_value as u64);
    uart_puts("\n[LSD] ctx.elr=");
    uart_put_hex(ctx.elr);
    uart_puts(" ctx.spsr=");
    uart_put_hex(ctx.spsr);
    uart_puts(" ctx.sp=");
    uart_put_hex(ctx.sp);
    uart_puts(" ctx.x8=");
    uart_put_hex(ctx.x8);
    uart_puts("\n[LSD] tf.elr(after)=");
    uart_put_hex(tf.elr as u64);
    uart_puts(" tf.spsr(after)=");
    uart_put_hex(tf.spsr as u64);
    uart_puts(" tf.sp(after)=");
    uart_put_hex(tf.sp as u64);
    uart_puts("\n");

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

    let l0_pa = crate::KERNEL_L0_PA.lock();
    let mut pt = aarch64::base::mm::page_table::PageTable::from_token(*l0_pa);
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
