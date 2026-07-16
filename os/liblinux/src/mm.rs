//! Linux memory management syscall implementations.

use crate::errno;
use crate::native;
use crate::task::TaskStruct;

/// brk(new_brk) — syscall 214
///
/// Sets the program break. If new_brk is 0, returns the current brk.
pub fn sys_brk(task: &mut TaskStruct, new_brk: usize) -> u64 {
    if new_brk == 0 {
        return task.brk as u64;
    }
    match task.do_brk(new_brk) {
        Ok(brk) => brk as u64,
        Err(e) => (-e as u64),
    }
}

/// mmap(addr, length, prot, flags, fd, offset) — syscall 222
///
/// Supports anonymous private mappings (MAP_ANONYMOUS | MAP_PRIVATE).
/// Uses a simple bump allocator for virtual addresses.
pub fn sys_mmap(task: &mut TaskStruct, addr: usize, length: usize, prot: usize,
                flags: usize, _fd: usize, _off: usize) -> u64 {
    // Linux mmap flags
    const MAP_ANONYMOUS: usize = 0x20;
    const MAP_PRIVATE:   usize = 0x02;
    const MAP_FIXED:     usize = 0x10;

    // Only support anonymous private mappings for now
    if flags & MAP_ANONYMOUS == 0 {
        return (-errno::ENOSYS as u64);
    }

    let page_size = 4096;
    let len = (length + page_size - 1) & !(page_size - 1);

    let map_addr = if flags & MAP_FIXED != 0 && addr != 0 {
        addr
    } else if addr != 0 {
        // Hint — but our bump allocator ignores hints
        task.mmap_base
    } else {
        task.mmap_base
    };

    // Allocate and map pages
    let mut va = map_addr;
    for _ in (0..len).step_by(page_size) {
        let ret = unsafe { native::map_page(va, prot) };
        if ret < 0 {
            // Cleanup already-mapped pages on failure
            let mut cleanup_va = map_addr;
            while cleanup_va < va {
                unsafe { native::unmap_page(cleanup_va); }
                cleanup_va += page_size;
            }
            return (-errno::ENOMEM as u64);
        }
        va += page_size;
    }

    // Update mmap_base for next allocation
    task.mmap_base = va;

    map_addr as u64
}

/// munmap(addr, length) — syscall 215
pub fn sys_munmap(_task: &mut TaskStruct, addr: usize, length: usize) -> u64 {
    if addr == 0 || length == 0 {
        return 0;
    }
    let page_size = 4096;
    let aligned_addr = addr & !(page_size - 1);
    let len = ((addr + length + page_size - 1) & !(page_size - 1)) - aligned_addr;

    for va in (aligned_addr..aligned_addr + len).step_by(page_size) {
        unsafe { native::unmap_page(va); }
    }
    0
}

/// mprotect(addr, len, prot) — syscall 226
///
/// Changes page permissions for the given address range.
/// prot bits: 1=READ, 2=WRITE, 4=EXEC (same as native syscall convention).
pub fn sys_mprotect(_task: &mut TaskStruct, addr: usize, len: usize, prot: usize) -> u64 {
    if addr == 0 || len == 0 {
        return 0; // nothing to do
    }
    let page_size = 4096;
    let start = addr & !(page_size - 1);
    let end = (addr + len + page_size - 1) & !(page_size - 1);
    // Map prot bits: Linux PROT_READ=1, PROT_WRITE=2, PROT_EXEC=4 → same as native
    for va in (start..end).step_by(page_size) {
        let ret = unsafe { native::mprotect_page(va, prot) };
        if ret < 0 {
            return (-errno::ENOMEM as u64);
        }
    }
    0
}

/// madvise(addr, len, advice) — syscall 233
///
/// Give advice about use of memory — always succeeds (no-op).
pub fn sys_madvise(_task: &mut TaskStruct, _addr: usize, _len: usize, _advice: usize) -> u64 {
    0
}
