//! Linux process management syscall implementations.

use crate::task::TaskStruct;
use crate::native;

/// exit_group(status) — syscall 94
pub fn sys_exit_group(_task: &mut TaskStruct, status: usize) -> ! {
    unsafe { native::exit_thread(status); }
}

/// exit(status) — syscall 93
pub fn sys_exit(task: &mut TaskStruct, status: usize) -> ! {
    task.exit_code = status as i32;
    // If clear_child_tid is set, zero it before exit (Linux ABI).
    if task.clear_child_tid != 0 {
        unsafe { *(task.clear_child_tid as *mut u32) = 0; }
    }
    unsafe { native::exit_thread(status); }
}

/// set_tid_address(tidptr) — syscall 96
pub fn sys_set_tid_address(task: &mut TaskStruct, tidptr: usize) -> u64 {
    task.clear_child_tid = tidptr;
    task.pid
}

/// getpid() — syscall 172
pub fn sys_getpid(task: &TaskStruct) -> u64 {
    task.pid
}

/// getuid() — syscall 174
pub fn sys_getuid(_task: &TaskStruct) -> u64 {
    0 // root
}

/// geteuid() — syscall 175
pub fn sys_geteuid(_task: &TaskStruct) -> u64 {
    0 // root
}

/// getgid() — syscall 176
pub fn sys_getgid(_task: &TaskStruct) -> u64 {
    0 // root
}

/// getegid() — syscall 177
pub fn sys_getegid(_task: &TaskStruct) -> u64 {
    0 // root
}

/// gettid() — syscall 178
pub fn sys_gettid(task: &TaskStruct) -> u64 {
    task.pid
}

/// sched_yield() — syscall 124
pub fn sys_sched_yield() -> u64 {
    unsafe { crate::native::yield_cpu(); }
    0
}

/// prctl(option, arg2, arg3, arg4, arg5) — syscall 167
///
/// Minimal implementation covering common operations.
#[allow(non_upper_case_globals)]
pub fn sys_prctl(task: &mut TaskStruct, option: usize, arg2: usize, _arg3: usize, _arg4: usize, _arg5: usize) -> u64 {
    // PR_SET_NAME (15) — set process name
    const PR_SET_NAME: usize = 15;
    // PR_GET_NAME (16) — get process name
    const PR_GET_NAME: usize = 16;
    // PR_SET_SECCOMP (22) — set seccomp mode
    const PR_SET_SECCOMP: usize = 22;
    // PR_CAPBSET_DROP (24) — drop capability
    const PR_CAPBSET_DROP: usize = 24;
    // PR_SET_NO_NEW_PRIVS (36) — set no_new_privs
    const PR_SET_NO_NEW_PRIVS: usize = 36;
    // PR_GET_NO_NEW_PRIVS (39) — get no_new_privs
    const PR_GET_NO_NEW_PRIVS: usize = 39;
    // PR_SET_VMA (0x53564d41) — set VMA properties
    const PR_SET_VMA: usize = 0x53564d41;

    match option {
        PR_SET_NAME => 0,
        PR_GET_NAME => {
            // Write placeholder process name to *arg2
            if arg2 != 0 {
                unsafe {
                    let name = b"quackos\0";
                    core::ptr::copy_nonoverlapping(name.as_ptr(), arg2 as *mut u8, name.len());
                }
            }
            0
        }
        PR_SET_NO_NEW_PRIVS => {
            task.no_new_privs = true;
            0
        }
        PR_GET_NO_NEW_PRIVS => task.no_new_privs as u64,
        PR_SET_SECCOMP => 0, // accept but ignore
        PR_CAPBSET_DROP => 0,
        PR_SET_VMA => 0,
        _ => (-crate::errno::EINVAL as u64),
    }
}

/// clone(flags, child_sp, parent_tid, child_tid, tls) — syscall 220
///
/// Creates a child process that shares a copy of the parent's address space.
/// The kernel handles page table cloning and thread creation.
/// Returns: child tid in parent, 0 in child.
pub fn sys_clone(_task: &mut TaskStruct, flags: usize, child_sp: usize,
                 parent_tid: usize, child_tid: usize, tls: usize) -> u64 {
    let ret = unsafe { native::clone(flags, child_sp, parent_tid, child_tid, tls) };
    // ret is already -errno on failure; Linux ABI returns it as-is.
    ret as u64
}

/// execve(path, argv, envp) — syscall 221
///
/// Replaces the current process image with a new ELF loaded from `path`.
/// Reads the ELF via IPC, then calls the kernel's sys_exec to perform the
/// address-space replacement.
pub fn sys_execve(task: &TaskStruct, path_ptr: usize, _argv_ptr: usize, _envp_ptr: usize) -> u64 {
    let mut path = [0u8; 256];
    let len = unsafe {
        let mut l = 0;
        while l < 256 {
            let b = *((path_ptr as *const u8).add(l));
            if b == 0 { break; }
            path[l] = b;
            l += 1;
        }
        l
    };
    let path_str = core::str::from_utf8(&path[..len]).unwrap_or("/");
    if path_str.is_empty() {
        return (-crate::errno::ENOENT as u64);
    }

    let page_size = 4096;

    // Open the file
    unsafe { crate::native::console_write(b"\n[ex1]\0".as_ptr(), 6); }
    let fid = match crate::ipc::fs_open(path_str) {
        Ok(f) => f,
        Err(e) => {
            unsafe { crate::native::console_write(b"[ex1e]\0".as_ptr(), 6); }
            return (-e as u64);
        }
    };
    unsafe { crate::native::console_write(b"[ex2]\0".as_ptr(), 5); }

    // Get file size
    let file_size = match crate::ipc::fs_fstat(fid) {
        Ok(sz) => sz as usize,
        Err(e) => {
            unsafe { crate::native::console_write(b"[ex2e]\0".as_ptr(), 6); }
            crate::ipc::fs_close(fid).ok(); return (-e as u64);
        }
    };
    unsafe { crate::native::console_write(b"[ex3]\0".as_ptr(), 5); }
    if file_size == 0 || file_size > 16 * 1024 * 1024 {
        crate::ipc::fs_close(fid).ok();
        return (-crate::errno::ENOEXEC as u64);
    }

    // Map pages for the ELF buffer + one extra for BootInfo
    let buf_addr = task.mmap_base;
    let buf_size = (file_size + page_size - 1) & !(page_size - 1);
    let total_size = buf_size + page_size; // extra page for BootInfo
    unsafe {
        for va in (buf_addr..buf_addr + total_size).step_by(page_size) {
            let ret = crate::native::map_page(va, 1 | 2);
            if ret < 0 {
                let mut c = buf_addr;
                while c < va {
                    crate::native::unmap_page(c);
                    c += page_size;
                }
                crate::ipc::fs_close(fid).ok();
                unsafe { crate::native::console_write(b"[ex3e]\0".as_ptr(), 6); }
                return (-crate::errno::ENOMEM as u64);
            }
        }
    }
    unsafe { crate::native::console_write(b"[ex4]\0".as_ptr(), 5); }

    // Read the entire ELF — loop since IPC payload is limited
    let mut total = 0usize;
    while total < file_size {
        let buf_slice = unsafe {
            core::slice::from_raw_parts_mut((buf_addr + total) as *mut u8, file_size - total)
        };
        match crate::ipc::fs_read(fid, buf_slice) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) => {
                unsafe { crate::native::console_write(b"[ex4e]\0".as_ptr(), 6); }
                crate::ipc::fs_close(fid).ok();
                for va in (buf_addr..buf_addr + total_size).step_by(page_size) {
                    unsafe { crate::native::unmap_page(va); }
                }
                return (-e as u64);
            }
        }
    }
    crate::ipc::fs_close(fid).ok();
    unsafe { crate::native::console_write(b"[ex5]\0".as_ptr(), 5); }

    // Parse ELF header
    let elf_slice = unsafe { core::slice::from_raw_parts(buf_addr as *const u8, total) };
    let (entry, brk, phdr_addr, phent, phnum) = match xmas_elf::ElfFile::new(elf_slice) {
        Ok(elf) => {
            let entry = elf.header.pt2.entry_point() as usize;
            let mut data_end = 0usize;
            let mut first_load_va = 0usize;
            for ph in elf.program_iter() {
                if ph.get_type() == Ok(xmas_elf::program::Type::Load) {
                    let vaddr = ph.virtual_addr() as usize;
                    let memsz = ph.mem_size() as usize;
                    if first_load_va == 0 { first_load_va = vaddr; }
                    let seg_end = vaddr + memsz;
                    if seg_end > data_end { data_end = seg_end; }
                }
            }
            let brk = (data_end + page_size - 1) & !(page_size - 1);
            let phdr = first_load_va + elf.header.pt2.ph_offset() as usize;
            let phent = elf.header.pt2.ph_entry_size();
            let phnum = elf.header.pt2.ph_count();
            (entry, brk, phdr, phent, phnum)
        }
        Err(_) => {
            unsafe { crate::native::console_write(b"[ex6e]\0".as_ptr(), 6); }
            for va in (buf_addr..buf_addr + total_size).step_by(page_size) {
                unsafe { crate::native::unmap_page(va); }
            }
            return (-crate::errno::ENOEXEC as u64);
        }
    };
    unsafe { crate::native::console_write(b"[ex6]\0".as_ptr(), 5); }

    // Write BootInfo to the extra page after the ELF buffer
    #[repr(C)]
    struct BootInfo {
        program_entry: u64,
        stack_top: u64,
        brk: u64,
        phdr_addr: u64,
        phent_size: u64,
        phnum: u64,
    }
    let stack_top = 0x7FFF_FFF1_0000usize;
    let bootinfo_addr = buf_addr + buf_size;
    unsafe {
        core::ptr::write_volatile(bootinfo_addr as *mut BootInfo, BootInfo {
            program_entry: entry as u64,
            stack_top: stack_top as u64,
            brk: brk as u64,
            phdr_addr: phdr_addr as u64,
            phent_size: phent as u64,
            phnum: phnum as u64,
        });
    }

    // Call native exec — on success this never returns
    unsafe { crate::native::console_write(b"[ex7]\0".as_ptr(), 5); }
    let ret = unsafe { crate::native::exec(buf_addr, total, stack_top, bootinfo_addr) };

    // If exec returns, it failed — clean up
    unsafe { crate::native::console_write(b"[ex7e]\0".as_ptr(), 6); }
    for va in (buf_addr..buf_addr + total_size).step_by(page_size) {
        unsafe { crate::native::unmap_page(va); }
    }
    if ret < 0 { ret as u64 } else { 0 }
}

/// wait4(pid, wstatus, options, rusage) — syscall 260
///
/// Waits for a child process to exit.  The kernel returns -EAGAIN while
/// children are alive but none has exited; we yield and retry to get
/// blocking semantics, unless WNOHANG was requested.
pub fn sys_wait4(_task: &TaskStruct, _pid: usize, wstatus: usize, options: usize, _rusage: usize) -> u64 {
    const WNOHANG: usize = 1;
    const EAGAIN: isize = 11;

    loop {
        let (ret, status) = unsafe { native::wait4() };
        if ret == -EAGAIN {
            if options & WNOHANG != 0 {
                return 0; // children exist but none exited yet
            }
            unsafe { native::yield_cpu(); }
            continue;
        }
        if ret < 0 {
            return ret as u64; // -errno (e.g. -ECHILD), Linux ABI returns it as-is
        }
        // Write the exit status to user-space
        if wstatus != 0 {
            unsafe { core::ptr::write_volatile(wstatus as *mut i32, status as i32); }
        }
        return ret as u64;
    }
}
