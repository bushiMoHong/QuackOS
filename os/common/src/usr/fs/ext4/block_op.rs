//! ext4 block operations — directory entry parsing, bitmap manipulation,
//! and extent tree block management.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use super::super::dev::block_dev::BlockDevice;
use super::super::types::Errno;
use super::dentry::Ext4DirEntry;
use super::extent_tree::{Ext4Extent, Ext4ExtentHeader, Ext4ExtentIdx};
use super::inode::read_ext4_block;

use super::dentry::EXT4_DT_DIR;

pub const EXT4_BLOCK_SIZE: usize = 4096;

// ---------------------------------------------------------------------------
// LinuxDirent64 — getdents64 syscall return format
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct LinuxDirent64 {
    pub d_ino: u64,
    pub d_off: u64,
    pub d_reclen: u16,
    pub d_type: u8,
    pub d_name: Vec<u8>,
}

impl LinuxDirent64 {
    pub fn write_to_mem(&self, buf: &mut [u8]) {
        buf[0..8].copy_from_slice(&self.d_ino.to_le_bytes());
        buf[8..16].copy_from_slice(&self.d_off.to_le_bytes());
        buf[16..18].copy_from_slice(&self.d_reclen.to_le_bytes());
        buf[18] = self.d_type;
        let name_bytes = &self.d_name;
        let name_len = name_bytes.len();
        buf[19..19 + name_len].copy_from_slice(name_bytes);
        if 19 + name_len < buf.len() {
            buf[19 + name_len] = 0; // null terminator
        }
    }
}

// ---------------------------------------------------------------------------
// Directory content readers/writers
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct Ext4DirContentRO<'a> {
    content: &'a [u8],
}

#[repr(C)]
pub struct Ext4DirContentWE<'a> {
    content: &'a mut [u8],
}

impl<'a> Ext4DirContentRO<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { content: data }
    }

    pub fn getdents(&self, buf: &mut [u8]) -> Result<(usize, usize), Errno> {
        const NAME_OFFSET: usize = 19;
        let mut buf_offset = 0;
        let mut file_offset = 0;
        let buf_len = buf.len();
        let content_len = self.content.len();
        while file_offset + 5 < content_len {
            let rec_len = u16::from_le_bytes([
                self.content[file_offset + 4],
                self.content[file_offset + 5],
            ]);
            if rec_len == 0 || file_offset + rec_len as usize > content_len {
                break;
            }
            let dentry = Ext4DirEntry::try_from(
                &self.content[file_offset..file_offset + rec_len as usize],
            )
            .expect("DirEntry::try_from failed");
            file_offset += rec_len as usize;
            if dentry.inode_num == 0 {
                continue;
            }
            let null_term_name_len = dentry.name.len() + 1;
            let d_reclen: usize = (NAME_OFFSET + null_term_name_len + 7) & !0x7;
            let dirent = LinuxDirent64 {
                d_ino: dentry.inode_num as u64,
                d_off: file_offset as u64,
                d_reclen: d_reclen as u16,
                d_type: dentry.file_type,
                d_name: dentry.name.clone(),
            };
            if buf_offset + d_reclen as usize > buf_len {
                break;
            }
            dirent.write_to_mem(&mut buf[buf_offset..buf_offset + d_reclen]);
            buf_offset += d_reclen as usize;
        }
        Ok((file_offset, buf_offset))
    }

    pub fn find(&self, name: &str) -> Option<Ext4DirEntry> {
        let mut rec_len_total = 0;
        let content_len = self.content.len();
        while rec_len_total < content_len {
            let rec_len = u16::from_le_bytes([
                self.content[rec_len_total + 4],
                self.content[rec_len_total + 5],
            ]);
            if rec_len_total + rec_len as usize > content_len {
                break;
            }
            let dentry = Ext4DirEntry::try_from(
                &self.content[rec_len_total..rec_len_total + rec_len as usize],
            )
            .unwrap();
            let dentry_name = String::from_utf8_lossy(&dentry.name[..dentry.name_len as usize]);
            if dentry_name == name {
                return Some(dentry);
            }
            rec_len_total += rec_len as usize;
        }
        None
    }
}

impl<'a> Ext4DirContentWE<'a> {
    pub fn new(data: &'a mut [u8]) -> Self {
        Self { content: data }
    }

    pub fn add_entry(
        &mut self,
        name: &str,
        inode_num: u32,
        file_type: u8,
    ) -> Result<(), &'static str> {
        let name_len = name.len();
        let needed_len = ((name_len + 8 + 3) & !3) as u16;
        let mut offset = 0;
        let content_len = self.content.len();

        while offset < content_len {
            if offset + 8 > content_len {
                return Err("Invalid directory entry");
            }
            let rec_len = u16::from_le_bytes([
                self.content[offset + 4],
                self.content[offset + 5],
            ]);
            if rec_len < 8 || offset + rec_len as usize > content_len {
                return Err("Invalid rec_len");
            }

            let dentry = match Ext4DirEntry::try_from(
                &self.content[offset..offset + rec_len as usize],
            ) {
                Ok(d) => d,
                Err(_) => return Err("Corrupted directory entry"),
            };

            // Case 1: free directory entry (inode_num == 0)
            if dentry.inode_num == 0 {
                if rec_len >= needed_len {
                    let new_dentry = Ext4DirEntry {
                        inode_num,
                        rec_len,
                        name_len: name_len as u8,
                        file_type,
                        name: name.as_bytes().to_vec(),
                    };
                    new_dentry.write_to_mem(&mut self.content[offset..]);
                    return Ok(());
                }
            }
            // Case 2: split existing entry
            else {
                let current_len = ((dentry.name_len as usize + 8 + 3) & !3) as u16;
                if rec_len >= current_len + needed_len {
                    let mut updated_dentry = dentry;
                    updated_dentry.rec_len = current_len;
                    updated_dentry.write_to_mem(&mut self.content[offset..]);

                    let new_dentry = Ext4DirEntry {
                        inode_num,
                        rec_len: rec_len - current_len,
                        name_len: name_len as u8,
                        file_type,
                        name: name.as_bytes().to_vec(),
                    };
                    new_dentry
                        .write_to_mem(&mut self.content[offset + current_len as usize..]);
                    return Ok(());
                }
            }

            offset += rec_len as usize;
        }

        Err("No space left in directory block")
    }

    pub fn delete_entry(&mut self, name: &str, _inode_num: u32) -> Result<(), Errno> {
        let mut rec_len_total = 0;
        let mut prev_len_total = 0;
        let content_len = self.content.len();
        while rec_len_total < content_len {
            let rec_len = u16::from_le_bytes([
                self.content[rec_len_total + 4],
                self.content[rec_len_total + 5],
            ]);
            let mut dentry = Ext4DirEntry::try_from(
                &self.content[rec_len_total..rec_len_total + rec_len as usize],
            )
            .expect("DirEntry::try_from failed");
            let dentry_name =
                String::from_utf8_lossy(&dentry.name[..dentry.name_len as usize]);
            if dentry_name == name {
                if rec_len_total == 0 {
                    dentry.inode_num = 0;
                    dentry.write_to_mem(
                        &mut self.content[rec_len_total..rec_len_total + rec_len as usize],
                    );
                    return Ok(());
                } else {
                    let mut prev_dentry = Ext4DirEntry::try_from(
                        &self.content[prev_len_total..rec_len_total as usize],
                    )
                    .expect("merge into previous dentry failed");
                    prev_dentry.rec_len += rec_len;
                    prev_dentry
                        .write_to_mem(&mut self.content[prev_len_total..rec_len_total as usize]);
                }
                return Ok(());
            }
            prev_len_total = rec_len_total;
            rec_len_total += rec_len as usize;
        }
        Err(Errno::ENOENT)
    }

    pub fn set_entry(
        &mut self,
        old_name: &str,
        new_inode_num: u32,
        new_file_type: u8,
    ) -> Result<(), &'static str> {
        let mut rec_len_total = 0;
        let content_len = self.content.len();
        while rec_len_total < content_len {
            let rec_len = u16::from_le_bytes([
                self.content[rec_len_total + 4],
                self.content[rec_len_total + 5],
            ]);
            let mut dentry = Ext4DirEntry::try_from(
                &self.content[rec_len_total..rec_len_total + rec_len as usize],
            )
            .map_err(|_| "DirEntry::try_from failed")?;
            let dentry_name =
                String::from_utf8_lossy(&dentry.name[..dentry.name_len as usize]);
            if dentry_name == old_name {
                dentry.inode_num = new_inode_num;
                dentry.file_type = new_file_type;
                dentry.write_to_mem(
                    &mut self.content[rec_len_total..rec_len_total + rec_len as usize],
                );
                return Ok(());
            }
            rec_len_total += rec_len as usize;
        }
        Err("Entry not found")
    }

    pub fn init_dot_dotdot(
        &mut self,
        parent_inode_num: u32,
        self_inode_num: u32,
        ext4_block_size: usize,
    ) {
        let mut dentry = Ext4DirEntry::default();
        // `.` entry
        dentry.inode_num = self_inode_num;
        dentry.rec_len = 12;
        dentry.name_len = 1;
        dentry.file_type = EXT4_DT_DIR;
        dentry.name = vec![b'.'];
        dentry.write_to_mem(&mut self.content[0..9]);

        // `..` entry
        dentry.inode_num = parent_inode_num;
        dentry.rec_len = ext4_block_size as u16 - 12;
        dentry.name_len = 2;
        dentry.name = vec![b'.', b'.'];
        dentry.write_to_mem(&mut self.content[12..22]);
    }
}

// ---------------------------------------------------------------------------
// Bitmap operations
// ---------------------------------------------------------------------------

pub struct Ext4Bitmap<'a> {
    bitmap: &'a mut [u8; EXT4_BLOCK_SIZE],
}

impl<'a> Ext4Bitmap<'a> {
    pub fn new(bitmap: &'a mut [u8; EXT4_BLOCK_SIZE]) -> Self {
        Self { bitmap }
    }

    pub fn alloc(&mut self, inode_bitmap_size: usize) -> Option<usize> {
        for (i, byte_ptr) in self.bitmap.iter_mut().enumerate() {
            let byte = *byte_ptr;
            if byte != 0xff {
                for j in 0..8 {
                    if (byte & (1 << j)) == 0 {
                        *byte_ptr |= 1 << j;
                        if i <= inode_bitmap_size {
                            return Some(i * 8 + j);
                        } else {
                            return None;
                        }
                    }
                }
            }
        }
        None
    }

    pub fn alloc_contiguous(
        &mut self,
        bitmap_size: usize,
        max_count: usize,
    ) -> Option<(usize, u32)> {
        let total_bits = bitmap_size * 8;
        let mut current_run = 0;
        let mut start_bit = 0;
        let mut longest_run = 0;
        let mut longest_start = 0;

        for bit in 0..total_bits {
            let byte_index = bit / 8;
            let bit_index = bit % 8;
            if byte_index >= self.bitmap.len() {
                break;
            }
            if self.bitmap[byte_index] & (1 << bit_index) == 0 {
                if current_run == 0 {
                    start_bit = bit;
                }
                current_run += 1;
                if current_run > longest_run {
                    longest_run = current_run;
                    longest_start = start_bit;
                }
                if current_run == max_count {
                    for b in start_bit..(start_bit + max_count) {
                        let bi = b / 8;
                        let bj = b % 8;
                        self.bitmap[bi] |= 1 << bj;
                    }
                    return Some((start_bit, max_count as u32));
                }
            } else {
                current_run = 0;
            }
        }

        if longest_run > 0 {
            for b in longest_start..(longest_start + longest_run) {
                let bi = b / 8;
                let bj = b % 8;
                self.bitmap[bi] |= 1 << bj;
            }
            return Some((longest_start, longest_run as u32));
        }
        None
    }

    pub fn dealloc(&mut self, block_offset: usize, bitmap_size: usize) {
        let byte_index = block_offset / 8;
        let bit_index = block_offset % 8;
        if byte_index < self.bitmap.len() && byte_index < bitmap_size {
            self.bitmap[byte_index] &= !(1 << bit_index);
        }
    }

    pub fn dealloc_contiguous(
        &mut self,
        start_block: usize,
        block_count: usize,
        bitmap_size: usize,
    ) {
        let mut byte_index = start_block / 8;
        let mut bit_index = start_block % 8;
        if byte_index < self.bitmap.len() && byte_index + block_count < bitmap_size {
            for _ in 0..block_count {
                self.bitmap[byte_index] &= !(1 << bit_index);
                if bit_index == 7 {
                    byte_index += 1;
                    bit_index = 0;
                } else {
                    bit_index += 1;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Extent tree block operations
// ---------------------------------------------------------------------------

pub const EXTENT_BLOCK_MAX_ENTRIES: usize = 340;

pub struct Ext4ExtentBlock<'a> {
    block: &'a mut [u8; EXT4_BLOCK_SIZE],
}

impl<'a> Ext4ExtentBlock<'a> {
    pub fn new(block: &'a mut [u8; EXT4_BLOCK_SIZE]) -> Self {
        Self { block }
    }

    fn extent_header(&self) -> &mut Ext4ExtentHeader {
        unsafe { &mut *(self.block.as_ptr() as *mut Ext4ExtentHeader) }
    }

    /// Look up an extent by logical block number.
    /// Recursively traverses the extent tree if needed.
    pub fn lookup_extent(
        &self,
        logical_block: u32,
        block_device: Arc<dyn BlockDevice>,
        ext4_block_size: usize,
    ) -> Option<Ext4Extent> {
        let header = self.extent_header();
        if header.depth == 0 {
            // Leaf node — scan extents
            let extents = unsafe {
                core::slice::from_raw_parts(
                    self.block.as_ptr().add(12) as *const Ext4Extent,
                    header.entries as usize,
                )
            };
            for extent in extents {
                if logical_block >= extent.logical_block
                    && logical_block < extent.logical_block + extent.len as u32
                {
                    return Some(*extent);
                }
            }
            None
        } else {
            // Index node — find child block and recurse
            let idxs = unsafe {
                core::slice::from_raw_parts(
                    self.block.as_ptr().add(12) as *const Ext4ExtentIdx,
                    header.entries as usize,
                )
            };
            // Find the last index whose block <= logical_block
            if let Some(idx) = idxs.iter()
                .filter(|i| i.block <= logical_block)
                .max_by_key(|i| i.block)
            {
                let child_block_num = idx.physical_leaf_block();
                let mut child_data =
                    read_ext4_block(&block_device, child_block_num, ext4_block_size);
                let child_ext4_block = Ext4ExtentBlock::new(&mut child_data);
                child_ext4_block.lookup_extent(
                    logical_block,
                    block_device,
                    ext4_block_size,
                )
            } else {
                None
            }
        }
    }

    /// Collect all extents from this node, recursively descending into
    /// index nodes.
    pub fn iter_all_extents(
        &mut self,
        block_device: Arc<dyn BlockDevice>,
        ext4_block_size: usize,
        result: &mut Vec<Ext4Extent>,
    ) {
        let header = self.extent_header();
        if header.depth > 0 {
            // Index node — recurse into children
            let idxs = unsafe {
                core::slice::from_raw_parts(
                    self.block.as_ptr().add(12) as *const Ext4ExtentIdx,
                    header.entries as usize,
                )
            };
            for idx in idxs {
                let child_block_num = idx.physical_leaf_block();
                let mut child_data =
                    read_ext4_block(&block_device, child_block_num, ext4_block_size);
                let mut child_block = Ext4ExtentBlock::new(&mut child_data);
                child_block.iter_all_extents(block_device.clone(), ext4_block_size, result);
            }
        } else {
            let extents = unsafe {
                core::slice::from_raw_parts(
                    self.block.as_ptr().add(12) as *const Ext4Extent,
                    header.entries as usize,
                )
            };
            result.extend(extents);
        }
    }

    pub fn insert_extent(
        &mut self,
        logical_block_num: u32,
        physical_block_num: u64,
        blocks_count: u32,
    ) -> Result<(), &'static str> {
        let header = self.extent_header();
        if header.depth == 0 {
            let extents = unsafe {
                core::slice::from_raw_parts_mut(
                    self.block.as_ptr().add(12) as *mut Ext4Extent,
                    header.entries as usize,
                )
            };
            for (i, extent) in extents.iter().enumerate() {
                let lend_block = extent.logical_block + extent.len as u32;
                let pend_block = extent.physical_start_block() as u32 + extent.len as u32;
                if logical_block_num == lend_block
                    && physical_block_num as u32 == pend_block
                    && extent.len < 32768
                {
                    unsafe {
                        let extent_ptr =
                            self.block.as_ptr().add(12 + i * 12) as *mut Ext4Extent;
                        (*extent_ptr).len += blocks_count as u16;
                        return Ok(());
                    }
                }
            }
            if header.entries as usize >= EXTENT_BLOCK_MAX_ENTRIES {
                panic!("Extent block is full, split not implemented");
            }
            let new_extent = Ext4Extent::new(
                logical_block_num,
                blocks_count as u16,
                physical_block_num as usize,
            );
            let insert_pos = extents
                .iter()
                .position(|e| e.logical_block > logical_block_num)
                .unwrap_or(extents.len());
            unsafe {
                let extents_ptr =
                    self.block.as_ptr().add(12) as *mut Ext4Extent;
                core::ptr::copy(
                    extents_ptr.add(insert_pos),
                    extents_ptr.add(insert_pos + 1),
                    (header.entries as usize) - insert_pos,
                );
                core::ptr::write(extents_ptr.add(insert_pos), new_extent);
            }
            header.entries += 1;
            Ok(())
        } else {
            Ok(())
        }
    }

    pub fn init_as_leaf(&mut self, extents: &[Ext4Extent]) {
        self.block.fill(0);
        let header =
            unsafe { &mut *(self.block.as_mut_ptr() as *mut Ext4ExtentHeader) };
        header.magic = 0xf30a;
        header.entries = extents.len() as u16;
        header.max = EXTENT_BLOCK_MAX_ENTRIES as u16;
        header.depth = 0;
        for (i, extent) in extents.iter().enumerate() {
            unsafe {
                let dst_ptr = self
                    .block
                    .as_mut_ptr()
                    .add(12 + i * core::mem::size_of::<Ext4Extent>())
                    as *mut Ext4Extent;
                dst_ptr.write(*extent);
            }
        }
    }
}
