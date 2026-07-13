//! User-space ELF loader — loads a statically-linked Linux binary.
//!
//! Parses the ELF file from the filesystem (via FsServer IPC), maps
//! PT_LOAD segments into the current address space, and returns the
//! entry point.

use crate::ipc;
use crate::native;

const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;
const PAGE_SIZE: usize = 4096;

/// Load a statically-linked ELF from the given path.
///
/// Returns (entry_point, initial_brk) on success.
pub fn load_elf(path: &str) -> Result<(usize, usize), isize> {
    // 1. Open the file
    let fid = ipc::fs_open(path)?;

    // 2. Read ELF header (64 bytes)
    let mut elf_hdr = [0u8; 64];
    if ipc::fs_read(fid, &mut elf_hdr).is_err() {
        ipc::fs_close(fid).ok();
        return Err(-1);
    }

    if elf_hdr[0..4] != ELF_MAGIC {
        ipc::fs_close(fid).ok();
        return Err(-1);
    }

    let entry = u64::from_le_bytes(elf_hdr[0x18..0x20].try_into().unwrap()) as usize;
    let phoff = u64::from_le_bytes(elf_hdr[0x20..0x28].try_into().unwrap()) as usize;
    let phentsize = u16::from_le_bytes(elf_hdr[0x36..0x38].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(elf_hdr[0x38..0x3A].try_into().unwrap()) as usize;

    // 3. Read program headers
    let ph_size = phnum * phentsize;
    // Seek to phoff
    ipc::fs_lseek(fid, phoff as isize, 0 /* SEEK_SET */)?;

    let mut ph_buf = [0u8; 512];
    if ph_size > ph_buf.len() {
        ipc::fs_close(fid).ok();
        return Err(-1);
    }
    let n = ipc::fs_read(fid, &mut ph_buf[..ph_size])?;
    if n < ph_size {
        ipc::fs_close(fid).ok();
        return Err(-1);
    }

    // 4. Map each PT_LOAD segment
    let mut data_end: usize = 0;

    for i in 0..phnum {
        let off = i * phentsize;
        let p_type = u32::from_le_bytes(ph_buf[off..off+4].try_into().unwrap());
        if p_type != PT_LOAD { continue; }

        let p_flags  = u32::from_le_bytes(ph_buf[off+4..off+8].try_into().unwrap());
        let p_offset = u64::from_le_bytes(ph_buf[off+8..off+16].try_into().unwrap()) as usize;
        let p_vaddr  = u64::from_le_bytes(ph_buf[off+16..off+24].try_into().unwrap()) as usize;
        let p_filesz = u64::from_le_bytes(ph_buf[off+32..off+40].try_into().unwrap()) as usize;
        let p_memsz  = u64::from_le_bytes(ph_buf[off+40..off+48].try_into().unwrap()) as usize;

        // Build prot: bit0=READ, bit1=WRITE, bit2=EXEC
        let mut prot: usize = 0;
        if p_flags & PF_R != 0 { prot |= 1; }
        if p_flags & PF_W != 0 { prot |= 2; }
        if p_flags & PF_X != 0 { prot |= 4; }

        let seg_end = p_vaddr + p_memsz;
        if seg_end > data_end { data_end = seg_end; }

        let start_page = p_vaddr & !(PAGE_SIZE - 1);
        let end_page = (p_vaddr + p_memsz + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        for page_va in (start_page..end_page).step_by(PAGE_SIZE) {
            // Map the page
            let ret = unsafe { native::map_page(page_va, prot) };
            if ret < 0 {
                ipc::fs_close(fid).ok();
                return Err(ret);
            }

            let page_off = page_va.wrapping_sub(p_vaddr);
            let copy_start = page_off;
            let copy_end = (page_off + PAGE_SIZE).min(p_filesz);

            // Copy file data directly into the mapped page
            if copy_start < p_filesz {
                let len = copy_end - copy_start;
                if len > 0 {
                    let file_pos = p_offset + copy_start;
                    ipc::fs_lseek(fid, file_pos as isize, 0 /* SEEK_SET */)?;
                    // Read directly into the page
                    let dst = &mut unsafe { core::slice::from_raw_parts_mut(page_va as *mut u8, len) };
                    ipc::fs_read(fid, dst)?;
                }

                // Zero the BSS portion
                let zero_start = if page_off > p_filesz { 0 } else { p_filesz - page_off };
                if zero_start < PAGE_SIZE {
                    unsafe {
                        core::ptr::write_bytes(
                            (page_va + zero_start) as *mut u8,
                            0,
                            PAGE_SIZE - zero_start,
                        );
                    }
                }
            } else {
                // Entire page is BSS
                unsafe { core::ptr::write_bytes(page_va as *mut u8, 0, PAGE_SIZE); }
            }
        }
    }

    ipc::fs_close(fid).ok();

    let initial_brk = (data_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    Ok((entry, initial_brk))
}
