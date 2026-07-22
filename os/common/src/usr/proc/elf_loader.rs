//! ELF executable loader.
//!
//! Two-layer design:
//!
//! **Low-level** — `load_elf_bytes()` parses ELF from a byte slice, allocates
//! physical pages for every PT_LOAD segment, copies file data, and maps them
//! with user-accessible permissions.  Also sets up a user stack.  This is the
//! direct replacement for the ad-hoc loader in `init.rs`.
//!
//! **High-level** — `spawn_process()` reads the ELF from the filesystem,
//! loads segments, registers VMAs with `MmServer` (when available), creates
//! a process record, and spawns the initial thread.  When `MmServer` is
//! `None` the loader falls back to direct page-table manipulation.

use aarch64::base::config::PAGE_SIZE;
use aarch64::base::mm::page_table::PageTable;
use aarch64::base::mm::{alloc_page, alloc_pages_contig, PhysPageNum, VirtPageNum};
use core::ptr::write_volatile;
use xmas_elf::program::Type;
use xmas_elf::ElfFile;

use crate::kernel::bmm::MapFlags;
use crate::kernel::sche;
use crate::kernel::trap::{thread_trampoline_addr, GeneralRegs, TrapFrame};

use crate::usr::fs::server::FsServer;
use crate::usr::fs::types::OpenFlags;
use crate::usr::mm::server::MmServer;
use crate::usr::proc::proc_table::{ProcessInfo, ProcessTable};
use crate::usr::proc::types::{ProcessId, ProcessPriority};
use crate::usr::task::{TaskId, TaskManager};

// ---------------------------------------------------------------------------
// User-space layout constants
// ---------------------------------------------------------------------------

/// Default top of the user stack (grows down from here).
pub const USER_STACK_TOP: usize = 0x7FFF_FFF1_0000;
/// Default number of 4 KiB pages for the initial user stack.
pub const USER_STACK_PAGES: usize = 32; // 128 KiB
/// Default bottom of the user stack.
pub const USER_STACK_BOTTOM: usize = USER_STACK_TOP - USER_STACK_PAGES * PAGE_SIZE;

/// Fixed user-space address where the kernel writes BootInfo.
/// Must match liblinux's `BOOTINFO_ADDR`.
/// Placed right after liblinux's SAVE_AREA (34×u64 = 272 bytes) in the
/// same ELF-mapped page — no extra mapping needed.
pub const BOOTINFO_VA: usize = 0x208110;

// ---------------------------------------------------------------------------
// Page-table helpers
// ---------------------------------------------------------------------------

fn page_align_down(v: usize) -> usize {
    v & !(PAGE_SIZE - 1)
}

fn page_align_up(v: usize) -> usize {
    (v + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

fn map_user_page(pt: &mut PageTable, vaddr: usize, paddr: usize, mapf: MapFlags) {
    let vpn = VirtPageNum::from(vaddr >> 12);
    let ppn = PhysPageNum::from(paddr >> 12);
    let ptef = mapf.to_pte_flags();
    pt.map(vpn, ppn, ptef);
}

fn map_user_pages_anon(pt: &mut PageTable, vaddr_start: usize, count: usize, mapf: MapFlags) {
    for i in 0..count {
        let va = vaddr_start + i * PAGE_SIZE;
        let pa = alloc_page().expect("OOM mapping user anon pages");
        unsafe { core::ptr::write_bytes(pa as *mut u8, 0, PAGE_SIZE); }
        map_user_page(pt, va, pa, mapf);
    }
}

fn elf_flags_to_mapf(ph_flags: xmas_elf::program::Flags) -> MapFlags {
    let mut mf = MapFlags::empty();
    mf.0 |= MapFlags::USER;
    if ph_flags.is_read() {
        mf.0 |= MapFlags::READ;
    }
    // Always allow writes — static PIE executables (like bash built with
    // musl-gcc) often place .got / .data.rel.ro inside the single text
    // segment.  The startup code must write to these pages during
    // self-relocation.  On Linux the dynamic linker would mprotect these
    // pages temporarily, but our kernel lacks a full dynamic linker.
    mf.0 |= MapFlags::WRITE;
    if ph_flags.is_execute() {
        mf.0 |= MapFlags::EXEC;
    }
    mf
}

/// Copy file data into a newly-allocated page.
///
/// `pa`        — physical address of the destination page
/// `page_va`   — start VA of this page
/// `seg_va`    — start VA of the segment
/// `seg_filesz`— file size of the segment
/// `seg_offset`— file offset of the segment
/// `elf_bytes` — the raw ELF bytes
fn copy_segment_data(
    pa: usize,
    page_va: usize,
    seg_va: usize,
    seg_filesz: usize,
    seg_offset: usize,
    elf_bytes: &[u8],
) {
    let page_end = page_va + PAGE_SIZE;
    let seg_data_end = seg_va + seg_filesz;

    let copy_start = page_va.max(seg_va);
    let copy_end = page_end.min(seg_data_end);
    if copy_start >= copy_end {
        return;
    }

    let offset_in_page = copy_start - page_va;
    let copy_len = copy_end - copy_start;
    let file_off = seg_offset + (copy_start - seg_va);

    if file_off + copy_len <= elf_bytes.len() {
        unsafe {
            core::ptr::copy_nonoverlapping(
                elf_bytes.as_ptr().add(file_off),
                (pa + offset_in_page) as *mut u8,
                copy_len,
            );
        }
    }
}

/// Zero the BSS portion of a page.
fn zero_bss(pa: usize, page_va: usize, data_end: usize, seg_end: usize) {
    let page_end = page_va + PAGE_SIZE;
    let bss_start = page_va.max(data_end);
    let bss_finish = page_end.min(seg_end);
    if bss_start < bss_finish {
        let offset = bss_start - page_va;
        let len = bss_finish - bss_start;
        unsafe {
            core::ptr::write_bytes((pa + offset) as *mut u8, 0, len);
        }
    }
}

// ---------------------------------------------------------------------------
// Low-level API — load an ELF from a byte slice
// ---------------------------------------------------------------------------

/// Result of loading an ELF binary into an address space.
pub struct LoadedElf {
    /// Entry-point virtual address.
    pub entry: usize,
    /// First byte past the highest-mapped data VA (candidate for `brk`).
    pub brk: usize,
    /// Virtual address of the ELF program headers in memory (AT_PHDR).
    pub phdr_addr: usize,
    /// Size of each program header entry (AT_PHENT).
    pub phent_size: u16,
    /// Number of program header entries (AT_PHNUM).
    pub phnum: u16,
}

/// Parse an ELF binary and map its PT_LOAD segments into `pt`.
///
/// Each segment page gets a freshly-allocated physical page, segment data is
/// copied from `elf_bytes`, BSS is zeroed, and the page is mapped with
/// user-mode permissions derived from the program header flags.
///
/// Any gaps between consecutive PT_LOAD segments (e.g. alignment padding
/// inserted by the linker) are filled with anonymous zero pages so that the
/// entire VA range is covered.  This prevents translation faults when musl's
/// startup code accesses globals whose page falls in such a gap.
///
/// Returns the entry point and suggested `brk`.
pub fn load_elf_bytes(pt: &mut PageTable, elf_bytes: &[u8]) -> Result<LoadedElf, &'static str> {
    // use crate::print_uart;
    // use crate::print_uart_hex;

    let elf = ElfFile::new(elf_bytes).map_err(|_| "Invalid ELF magic")?;
    let entry = elf.header.pt2.entry_point() as usize;

    // ── Phase 1: collect segment page ranges ──────────────────────────
    const MAX_SEGMENTS: usize = 8;
    let mut seg_starts: [usize; MAX_SEGMENTS] = [0; MAX_SEGMENTS];
    let mut seg_ends:   [usize; MAX_SEGMENTS] = [0; MAX_SEGMENTS];
    let mut seg_fileszs: [usize; MAX_SEGMENTS] = [0; MAX_SEGMENTS];
    let mut seg_offsets: [usize; MAX_SEGMENTS] = [0; MAX_SEGMENTS];
    let mut seg_vaddrs:  [usize; MAX_SEGMENTS] = [0; MAX_SEGMENTS];
    let mut seg_mapfs:   [MapFlags; MAX_SEGMENTS] = [MapFlags::empty(); MAX_SEGMENTS];
    let mut nsegs: usize = 0;

    let mut data_end: usize = 0;
    let mut tls_memsz: usize = 0;

    for ph in elf.program_iter() {
        let ph_type = ph.get_type().unwrap_or(Type::Null);

        // Track PT_TLS size for brk guard pages
        if ph_type == Type::Tls {
            tls_memsz = ph.mem_size() as usize;
            // print_uart("[load_elf] PT_TLS memsz=");
            // print_uart_hex(tls_memsz as u64);
            // print_uart("\n");
            continue;
        }

        if ph_type != Type::Load {
            continue;
        }

        let vaddr  = ph.virtual_addr() as usize;
        let memsz  = ph.mem_size() as usize;
        let filesz = ph.file_size() as usize;
        let offset = ph.offset() as usize;
        let mapf   = elf_flags_to_mapf(ph.flags());

        let seg_end = vaddr + memsz;
        if seg_end > data_end {
            data_end = seg_end;
        }

        let start_page = page_align_down(vaddr);
        let end_page   = page_align_up(seg_end);

        // print_uart("[load_elf] PT_LOAD va=");
        // print_uart_hex(vaddr as u64);
        // print_uart(" memsz=");
        // print_uart_hex(memsz as u64);
        // print_uart(" filesz=");
        // print_uart_hex(filesz as u64);
        // print_uart(" pages=[");
        // print_uart_hex(start_page as u64);
        // print_uart(",");
        // print_uart_hex(end_page as u64);
        // print_uart(") flags=");
        // let w = mapf.contains(MapFlags::WRITE);
        // let x = mapf.contains(MapFlags::EXEC);
        // if x && w { print_uart("RWX"); }
        // else if x  { print_uart("R-X"); }
        // else if w  { print_uart("RW-"); }
        // else       { print_uart("R--"); }
        // print_uart("\n");

        if nsegs < MAX_SEGMENTS {
            seg_starts[nsegs]  = start_page;
            seg_ends[nsegs]    = end_page;
            seg_fileszs[nsegs] = filesz;
            seg_offsets[nsegs] = offset;
            seg_vaddrs[nsegs]  = vaddr;
            seg_mapfs[nsegs]   = mapf;
            nsegs += 1;
        }
    }

    // ── Phase 2: sort segments by start_page (bubble sort, small n) ──
    for i in 0..nsegs {
        for j in i+1..nsegs {
            if seg_starts[i] > seg_starts[j] {
                seg_starts.swap(i, j);
                seg_ends.swap(i, j);
                seg_fileszs.swap(i, j);
                seg_offsets.swap(i, j);
                seg_vaddrs.swap(i, j);
                let tmp = seg_mapfs[i];
                seg_mapfs[i] = seg_mapfs[j];
                seg_mapfs[j] = tmp;
            }
        }
    }

    // ── Phase 3: map segments, filling gaps with anon zero pages ─────
    let anon_flags = MapFlags(MapFlags::READ | MapFlags::WRITE | MapFlags::USER);
    let mut last_end: usize = 0;

    for i in 0..nsegs {
        let start_page = seg_starts[i];
        let end_page   = seg_ends[i];

        // Fill gap between previous segment and this one
        if last_end > 0 && start_page > last_end {
            // print_uart("[load_elf] gap fill [");
            // print_uart_hex(last_end as u64);
            // print_uart(",");
            // print_uart_hex(start_page as u64);
            // print_uart(") ");
            // print_uart_hex(((start_page - last_end) >> 12) as u64);
            // print_uart(" pages\n");

            for page_va in (last_end..start_page).step_by(PAGE_SIZE) {
                let pa = alloc_page().ok_or("OOM filling segment gap")?;
                unsafe { core::ptr::write_bytes(pa as *mut u8, 0, PAGE_SIZE); }
                map_user_page(pt, page_va, pa, anon_flags);
            }
        }

        // Map this segment's pages
        let filesz = seg_fileszs[i];
        let offset = seg_offsets[i];
        let vaddr  = seg_vaddrs[i];
        let mapf   = seg_mapfs[i];

        for page_va in (start_page..end_page).step_by(PAGE_SIZE) {
            let pa = alloc_page().ok_or("OOM loading ELF segment")?;
            copy_segment_data(pa, page_va, vaddr, filesz, offset, elf_bytes);
            // Zero BSS portion: bytes from (vaddr + filesz) to (vaddr + memsz).
            // seg_ends[i] is page_align_up(vaddr + memsz), which is safe to
            // pass as the upper bound — at most one partial page is over-zeroed.
            zero_bss(pa, page_va, vaddr + filesz, seg_ends[i]);
            map_user_page(pt, page_va, pa, mapf);
        }

        last_end = end_page;
    }

    // ── Phase 4: map pages at brk for musl's main-thread TLS area ──────
    // musl __init_tls copies the TLS initialisation image to the region
    // starting at brk.  The area must be large enough for:
    //   struct pthread (~0x200 bytes) + PT_TLS.memsz
    // We map this eagerly because the kernel has no demand-paging fallback
    // for page faults in the brk region.
    let brk = page_align_up(data_end);
    let brk_pages = if tls_memsz > 0 {
        // Round up: (pthread overhead + TLS) in pages, minimum 1
        (0x200 + tls_memsz + PAGE_SIZE - 1) / PAGE_SIZE
    } else {
        4 // generous default when PT_TLS is absent
    };

    if brk >= last_end {
        let brk_start = page_align_down(brk);
        // print_uart("[load_elf] brk area [");
        // print_uart_hex(brk_start as u64);
        // print_uart(",");
        // print_uart_hex((brk_start + brk_pages * PAGE_SIZE) as u64);
        // print_uart(") tls_memsz=");
        // print_uart_hex(tls_memsz as u64);
        // print_uart(" pages=");
        // print_uart_hex(brk_pages as u64);
        // print_uart("\n");

        for page_va in (brk_start..brk_start + brk_pages * PAGE_SIZE).step_by(PAGE_SIZE) {
            let pa = alloc_page().ok_or("OOM mapping brk area")?;
            unsafe { core::ptr::write_bytes(pa as *mut u8, 0, PAGE_SIZE); }
            map_user_page(pt, page_va, pa, anon_flags);
        }
    }

    // Compute PHDR virtual address (first PT_LOAD vaddr + ph_offset)
    let mut first_load_vaddr: usize = 0;
    for ph in elf.program_iter() {
        if ph.get_type() == Ok(Type::Load) {
            first_load_vaddr = ph.virtual_addr() as usize;
            break;
        }
    }
    let phdr_addr = first_load_vaddr + elf.header.pt2.ph_offset() as usize;
    let phent_size = elf.header.pt2.ph_entry_size();
    let phnum = elf.header.pt2.ph_count();
    let brk = page_align_up(data_end);
    // print_uart("[load_elf] brk=");
    // print_uart_hex(brk as u64);
    // print_uart("\n");
    Ok(LoadedElf { entry, brk, phdr_addr, phent_size, phnum })
}

/// Map anonymous user stack pages at the default location.
pub fn map_user_stack(pt: &mut PageTable) {
    let flags = MapFlags(MapFlags::READ | MapFlags::WRITE | MapFlags::USER);
    map_user_pages_anon(pt, USER_STACK_BOTTOM, USER_STACK_PAGES, flags);
}
/// Map anonymous user stack pages at a custom range.
pub fn map_user_stack_at(pt: &mut PageTable, stack_bottom: usize, pages: usize) {
    let flags = MapFlags(MapFlags::READ | MapFlags::WRITE | MapFlags::USER);
    map_user_pages_anon(pt, stack_bottom, pages, flags);
}

// ---------------------------------------------------------------------------
// Thread spawning
// ---------------------------------------------------------------------------

/// Kernel stack size in bytes (8 × 4 KiB pages = 32 KiB).
pub const KERNEL_STACK_SIZE: usize = 8 * PAGE_SIZE;

/// Spawn a user-mode thread at `entry` with the given user `stack_top`.
///
/// Allocates a kernel stack, builds the initial `TrapFrame` and `TaskContext`,
/// creates a kernel thread via `sche::create_thread`, and enqueues it.
///
/// Returns the new `ThreadId` on success.
pub fn spawn_user_thread(entry: usize, stack_top: usize) -> Result<sche::ThreadId, &'static str> {
    // Allocate kernel stack (2 physically contiguous pages, zeroed)
    let ks_base = aarch64::base::mm::alloc_pages_contig(8)
        .ok_or("OOM for kernel stack")?;
    let ks_top = ks_base + KERNEL_STACK_SIZE;

    // Build TrapFrame (at top - 128 - 288)
    let ctx_addr = ks_top - 128; // TaskContext
    let tf_addr = ctx_addr - 288; // TrapFrame below TaskContext
    let initial_sp = stack_top - 16; // 16-byte alignment

    let trapframe = TrapFrame {
        trap_num: 0,
        elr: entry,
        spsr: 0, // EL0t
        sp: initial_sp,
        tpidr: 0,
        general: GeneralRegs {
            x0: 0,
            ..Default::default()
        },
    };
    unsafe { write_volatile(tf_addr as *mut TrapFrame, trapframe); }

    // Create kernel thread
    let tid = unsafe {
        sche::create_thread(
            128,        // default priority
            ks_base,
            ctx_addr,   // kernel_stack_top
            KERNEL_STACK_SIZE,
            0,          // ttbr0 (shares kernel page table)
            0,          // asid
        )
    }
    .map_err(|_| "Failed to create thread")?;

    // Build TaskContext on the kernel stack
    let tcb_addr = unsafe { sche::tcb_ptr(tid) } as usize;
    let ttbr1: usize;
    let ttbr0: usize;
    unsafe {
        core::arch::asm!(
            "mrs {t1}, ttbr1_el1",
            "mrs {t0}, ttbr0_el1",
            t1 = out(reg) ttbr1,
            t0 = out(reg) ttbr0,
        );
    }
    unsafe {
        write_volatile((ctx_addr + 0x00) as *mut usize, thread_trampoline_addr());
        write_volatile((ctx_addr + 0x08) as *mut usize, tcb_addr);
        write_volatile((ctx_addr + 0x10) as *mut usize, tf_addr); // x20 = tf_addr
        write_volatile((ctx_addr + 0x70) as *mut usize, ttbr1);
        write_volatile((ctx_addr + 0x78) as *mut usize, ttbr0);
    }

    // Enqueue
    let prio = sche::with_thread(tid, |t| t.effective_priority()).unwrap_or(128);
    sche::enqueue_ready(tid, prio).map_err(|_| "Failed to enqueue thread")?;

    Ok(tid)
}

/// Spawn a user-mode thread in a specific (isolated) address space.
///
/// Like `spawn_user_thread`, but uses the caller-supplied `ttbr0` and
/// `asid` instead of the shared kernel page table.
pub fn spawn_user_thread_in_as(
    entry: usize,
    stack_top: usize,
    ttbr0: usize,
    asid: usize,
) -> Result<sche::ThreadId, &'static str> {
    let ks_base = aarch64::base::mm::alloc_pages_contig(8)
        .ok_or("OOM for kernel stack")?;
    let ks_top = ks_base + KERNEL_STACK_SIZE;

    let ctx_addr = ks_top - 128;
    let tf_addr = ctx_addr - 288;
    let initial_sp = stack_top - 16;

    let trapframe = TrapFrame {
        trap_num: 0,
        elr: entry,
        spsr: 0,
        sp: initial_sp,
        tpidr: 0,
        general: GeneralRegs { x0: 0, ..Default::default() },
    };
    unsafe { write_volatile(tf_addr as *mut TrapFrame, trapframe); }

    let tid = unsafe {
        sche::create_thread(128, ks_base, ctx_addr, KERNEL_STACK_SIZE, ttbr0 as usize, asid)
    }
    .map_err(|_| "Failed to create thread in AS")?;

    let tcb_addr = unsafe { sche::tcb_ptr(tid) } as usize;
    unsafe {
        write_volatile((ctx_addr + 0x00) as *mut usize, thread_trampoline_addr());
        write_volatile((ctx_addr + 0x08) as *mut usize, tcb_addr);
        write_volatile((ctx_addr + 0x10) as *mut usize, tf_addr); // x20 = tf_addr
        write_volatile((ctx_addr + 0x70) as *mut usize, 0usize);  // ttbr1 = 0
        write_volatile((ctx_addr + 0x78) as *mut usize, ttbr0);   // isolated page table
    }

    let prio = sche::with_thread(tid, |t| t.effective_priority()).unwrap_or(128);
    sche::enqueue_ready(tid, prio).map_err(|_| "Failed to enqueue thread")?;

    Ok(tid)
}

// ---------------------------------------------------------------------------
// High-level API — spawn a process from the filesystem
// ---------------------------------------------------------------------------

/// Full process-spawn pipeline: read ELF from `path` via `fs`, load segments
/// into the current page table, register VMA metadata with `mm` (when
/// available), create the process record, and spawn the initial thread.
///
/// When `mm` is `None` the ELF is still loaded and mapped — only the VMA
/// bookkeeping step is skipped.
pub fn spawn_process(
    fs: &FsServer,
    mut mm: Option<&mut MmServer>,
    proc_table: &mut ProcessTable,
    _task_mgr: &TaskManager,
    parent_pid: ProcessId,
    path: &str,
) -> Result<ProcessId, &'static str> {
    // ------------------------------------------------------------------
    // 1. Read ELF from file system
    // ------------------------------------------------------------------
    let parent_u32 = parent_pid.index() as u32;
    let fd = fs
        .open(parent_u32, path, OpenFlags::O_RDONLY, 0)
        .map_err(|_| "Failed to open ELF file")?;

    let stat = fs
        .fstat(parent_u32, fd)
        .map_err(|_| "Failed to stat ELF file")?;

    let elf_data = fs
        .read(parent_u32, fd, stat.size as usize)
        .map_err(|_| "Failed to read ELF")?;

    fs.close(parent_u32, fd).ok();

    // ------------------------------------------------------------------
    // 2. Parse ELF and compute region boundaries
    // ------------------------------------------------------------------
    let elf = ElfFile::new(&elf_data).map_err(|_| "Invalid ELF format")?;
    let entry = elf.header.pt2.entry_point() as usize;

    let mut code_start = usize::MAX;
    let mut code_end = 0usize;
    let mut data_start = usize::MAX;
    let mut data_end = 0usize;

    for ph in elf.program_iter() {
        if ph.get_type() != Ok(Type::Load) {
            continue;
        }
        let vaddr = ph.virtual_addr() as usize;
        let memsz = ph.mem_size() as usize;
        let seg_end = vaddr + memsz;
        let is_exec = ph.flags().is_execute();

        if is_exec {
            code_start = code_start.min(vaddr);
            code_end = code_end.max(seg_end);
        } else {
            data_start = data_start.min(vaddr);
            data_end = data_end.max(seg_end);
        }
    }

    let code_start = if code_start == usize::MAX { 0 } else { page_align_down(code_start) };
    let code_end = page_align_up(code_end);
    let data_start = if data_start == usize::MAX { code_end } else { page_align_down(data_start) };
    let data_end = page_align_up(data_end);
    let heap_start = page_align_up(data_end);

    // ------------------------------------------------------------------
    // 3. Load ELF segments and map stack into the current page table
    // ------------------------------------------------------------------
    let l0_pa = *crate::KERNEL_L0_PA.lock();
    let mut pt = PageTable::from_token(l0_pa);

    let loaded = load_elf_bytes(&mut pt, &elf_data)?;
    map_user_stack(&mut pt);

    unsafe {
        core::arch::asm!("msr ttbr0_el1, {}", in(reg) l0_pa);
        core::arch::asm!("dsb ish; isb; tlbi vmalle1is; dsb ish; isb");
    }

    assert_eq!(loaded.entry, entry, "ELF entry mismatch");

    // ------------------------------------------------------------------
    // 4. Register VMAs with MmServer (metadata for future lazy faults)
    // ------------------------------------------------------------------
    let asid = if let Some(ref mut mm_srv) = mm {
        // Register with MmServer so it knows about this process's regions.
        // Actual pages are already loaded above; MmServer VMA metadata
        // enables lazy page-fault handling when a separate address space
        // is used in the future.
        let pid_u32: u32 = ProcessId::NULL.into();
        mm_srv
            .register_process(pid_u32, crate::kernel::bmm::AddressSpaceId(0))
            .map_err(|_| "Failed to register with MM")?;

        mm_srv
            .init_process_vma(
                pid_u32,
                code_start,
                code_end,
                data_start,
                data_end,
                USER_STACK_BOTTOM,
                USER_STACK_TOP,
                heap_start,
            )
            .map_err(|_| "Failed to init VMAs")?;

        0usize
    } else {
        0usize
    };

    // ------------------------------------------------------------------
    // 5. Register process in the process table
    // ------------------------------------------------------------------
    let proc_info = ProcessInfo::new(
        ProcessId::NULL, // placeholder, insert will assign the real one
        path.as_bytes(),
        ProcessPriority::DEFAULT,
        crate::kernel::bmm::AddressSpaceId(asid),
        parent_pid,
    );
    let child_pid = proc_table
        .insert(proc_info)
        .map_err(|_| "Process table full")?;

    // ------------------------------------------------------------------
    // 6. Spawn the initial user thread
    // ------------------------------------------------------------------
    let tid = spawn_user_thread(entry, USER_STACK_TOP)?;

    // Attach thread to process
    let proc_ref = proc_table
        .get_mut(child_pid)
        .ok_or("Process vanished after insert")?;
    proc_ref.add_thread(TaskId(tid)).map_err(|_| "Thread list full")?;

    Ok(child_pid)
}
