//! User-space ELF loader — loads a statically-linked Linux binary.
//!
//! Uses `xmas_elf` for parsing (same as the kernel's `elf_loader`) and
//! `native::map_page` / IPC for mapping and file I/O from user space.
//!
//! The flow mirrors `load_elf_bytes` from the kernel: parse PT_LOAD segments,
//! map pages with the correct permissions, copy file data, and zero BSS.

use crate::ipc;
use crate::native;
use xmas_elf::program::{Flags, Type};
use xmas_elf::ElfFile;

const PAGE_SIZE: usize = 4096;

/// Map flags from ELF program header to native prot bits.
/// bit0=READ, bit1=WRITE, bit2=EXEC
fn flags_to_prot(ph_flags: Flags) -> usize {
    let mut prot = 0usize;
    if ph_flags.is_read()    { prot |= 1; }
    if ph_flags.is_write()   { prot |= 2; }
    if ph_flags.is_execute() { prot |= 4; }
    prot
}

/// Load a statically-linked ELF from the given path.
///
/// Returns (entry_point, initial_brk) on success.
pub fn load_elf(path: &str) -> Result<(usize, usize), isize> {
    // 1. Open the file
    let fid = ipc::fs_open(path)?;

    // 2. Read ELF header + program headers into a contiguous buffer.
    //    For a typical ELF (< 10 PH entries × 56 bytes), 2 KiB is plenty.
    let mut buf = [0u8; 2048];
    let n = ipc::fs_read(fid, &mut buf[..])?;
    if n < 64 {
        ipc::fs_close(fid).ok();
        return Err(-1);
    }

    let elf = ElfFile::new(&buf[..n]).map_err(|_| -1_isize)?;
    let entry = elf.header.pt2.entry_point() as usize;

    // 3. Map each PT_LOAD segment
    let mut data_end: usize = 0;

    for ph in elf.program_iter() {
        if ph.get_type() != Ok(Type::Load) {
            continue;
        }

        let vaddr  = ph.virtual_addr() as usize;
        let memsz  = ph.mem_size() as usize;
        let filesz = ph.file_size() as usize;
        let offset = ph.offset() as usize;
        let prot   = flags_to_prot(ph.flags());

        let seg_end = vaddr + memsz;
        if seg_end > data_end {
            data_end = seg_end;
        }

        let start_page = vaddr & !(PAGE_SIZE - 1);
        let end_page = (seg_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        for page_va in (start_page..end_page).step_by(PAGE_SIZE) {
            // Map the page via kernel syscall
            let ret = unsafe { native::map_page(page_va, prot) };
            if ret < 0 {
                ipc::fs_close(fid).ok();
                return Err(ret);
            }

            let page_end = page_va + PAGE_SIZE;

            // Copy file data into this page
            let copy_start = page_va.max(vaddr);
            let copy_end = page_end.min(vaddr + filesz);
            if copy_start < copy_end {
                let file_pos = offset + (copy_start - vaddr);
                let len = copy_end - copy_start;
                ipc::fs_lseek(fid, file_pos as isize, 0 /* SEEK_SET */)?;
                let dst = unsafe {
                    core::slice::from_raw_parts_mut(copy_start as *mut u8, len)
                };
                ipc::fs_read(fid, dst)?;
            }

            // Zero BSS portion
            let bss_start = page_va.max(vaddr + filesz);
            let bss_end = page_end.min(seg_end);
            if bss_start < bss_end {
                unsafe {
                    core::ptr::write_bytes(
                        bss_start as *mut u8,
                        0,
                        bss_end - bss_start,
                    );
                }
            }
        }
    }

    ipc::fs_close(fid).ok();

    let initial_brk = (data_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    Ok((entry, initial_brk))
}
