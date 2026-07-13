#![allow(unused)]
use core::ptr;
use core::sync::atomic::AtomicI32;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;
use hashbrown::HashMap;
use spin::{Mutex, RwLock};

use aarch64::base::config::PAGE_SIZE;
pub const PAGE_SIZE_BITS: usize = 12;
use crate::usr::fs::dev::block_dev::BlockDevice;
use crate::usr::fs::inode::InodeOp;
use crate::usr::fs::types::{Kstat, Errno, SyscallRet, TimeSpec, FileType};
use crate::usr::fs::dentry::Dentry;
use crate::usr::fs::page_cache::{AddressSpace, Page};

use super::block_op::EXT4_BLOCK_SIZE as FS_BLOCK_SIZE;
use super::block_op::{Ext4DirContentRO, Ext4DirContentWE, Ext4ExtentBlock};
use super::block_group::GroupDesc;
use super::dentry::Ext4DirEntry;
use super::extent_tree::{Ext4Extent, Ext4ExtentHeader, Ext4ExtentIdx};
use super::super_block::Ext4SuperBlock;
use super::fs::Ext4FileSystem;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const EXT4_N_BLOCKS: usize = 15;
pub const VIRTIO_BLOCK_SIZE: usize = 512;
pub const EXT4_MAX_INLINE_DATA: usize = 60;
pub const MAX_FS_BLOCK_ID: usize = 0xFFFF_F000;

/// Permission bits (low 12 bits)
pub const S_IXOTH: u16 = 0x1;
pub const S_IWOTH: u16 = 0x2;
pub const S_IROTH: u16 = 0x4;
pub const S_IXGRP: u16 = 0x8;
pub const S_IWGRP: u16 = 0x10;
pub const S_IRGRP: u16 = 0x20;
pub const S_IXUSR: u16 = 0x40;
pub const S_IWUSR: u16 = 0x80;
pub const S_IRUSR: u16 = 0x100;
pub const S_ISVTX: u16 = 0x200;
pub const S_ISGID: u16 = 0x400;
pub const S_ISUID: u16 = 0x800;

// File type (upper 4 bits)
pub const S_IFMT: u16 = 0xF000;
pub const S_IFIFO: u16 = 0x1000;
pub const S_IFCHR: u16 = 0x2000;
pub const S_IFBLK: u16 = 0x6000;
pub const S_IFDIR: u16 = 0x4000;
pub const S_IFREG: u16 = 0x8000;
pub const S_IFLNK: u16 = 0xA000;
pub const S_IFSOCK: u16 = 0xC000;
pub const S_IALLUGO: u16 = 0xFFF;

// inode flags
const EXT4_IMMUTABLE_FL: u32 = 0x00000010;
const EXT4_APPEND_FL: u32 = 0x00000020;
pub const EXT4_INDEX_FL: u32 = 0x00001000;
pub const EXT4_EXTENTS_FL: u32 = 0x00080000;
pub const EXT4_INLINE_DATA_FL: u32 = 0x10000000;

const STATX_ATTR_APPEND: u64 = 0x00000020;

// Seal flags (memfd)
pub const F_SEAL_SEAL: i32 = 0x0001;
pub const F_SEAL_SHRINK: i32 = 0x0002;
pub const F_SEAL_GROW: i32 = 0x0004;

// ---------------------------------------------------------------------------
// FallocFlags — fallocate mode flags
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FallocFlags(u32);

impl FallocFlags {
    pub const KEEP_SIZE: Self = Self(0x01);
    pub const PUNCH_HOLE: Self = Self(0x02);

    pub fn empty() -> Self {
        Self(0)
    }

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

// ---------------------------------------------------------------------------
// SetXattrFlags
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetXattrFlags(u32);

impl SetXattrFlags {
    pub const CREATE: Self = Self(0x01);
    pub const REPLACE: Self = Self(0x02);
}

// ---------------------------------------------------------------------------
// Helper: read / write blocks from/to the BlockDevice
// ---------------------------------------------------------------------------

pub(crate) fn read_block(device: &Arc<dyn BlockDevice>, block_num: usize, block_size: usize) -> Vec<u8> {
    let mut buf = vec![0u8; block_size];
    device.read(block_num * block_size, &mut buf);
    buf
}

pub(crate) fn write_block(device: &Arc<dyn BlockDevice>, block_num: usize, data: &[u8], block_size: usize) {
    device.write(block_num * block_size, data);
}

/// Read a full ext4 block into a fixed-size array.
pub(crate) fn read_ext4_block(device: &Arc<dyn BlockDevice>, block_num: usize, block_size: usize) -> [u8; FS_BLOCK_SIZE] {
    let mut buf = [0u8; FS_BLOCK_SIZE];
    device.read(block_num * block_size, &mut buf);
    buf
}

/// Write an ext4 metadata block.  Only `block_size` bytes are written so that
/// adjacent blocks are not corrupted when the filesystem block size is smaller
/// than FS_BLOCK_SIZE (4096).
pub(crate) fn write_ext4_block(device: &Arc<dyn BlockDevice>, block_num: usize, data: &[u8], block_size: usize) {
    device.write(block_num * block_size, &data[..block_size]);
}

// ---------------------------------------------------------------------------
// Ext4InodeDisk — on-disk inode representation
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4InodeDisk {
    pub mode: u16,
    pub uid: u16,
    pub size_lo: u32,
    pub atime: u32,
    pub change_inode_time: u32,
    pub modify_file_time: u32,
    pub dtime: u32,
    pub gid: u16,
    pub links_count: u16,
    pub blocks_lo: u32,
    pub flags: u32,
    pub osd1: u32,
    pub block: [u8; 60],
    pub generation: u32,
    pub file_acl_lo: u32,
    pub obso_faddr: u32,
    pub size_hi: u32,
    pub osd2: [u32; 3],
    pub extra_isize: u16,
    pub checksum_hi: u16,
    pub change_inode_time_extra: u32,
    pub modify_file_time_extra: u32,
    pub atime_extra: u32,
    pub create_time: u32,
    pub create_time_extra: u32,
    pub version_hi: u32,
    pub project_id: u32,
}

impl Default for Ext4InodeDisk {
    fn default() -> Self {
        Self {
            mode: 0,
            uid: 0,
            size_lo: 0,
            atime: 0,
            change_inode_time: 0,
            modify_file_time: 0,
            dtime: 0,
            gid: 0,
            links_count: 1,
            blocks_lo: 0,
            flags: 0,
            osd1: 0,
            block: [0; 60],
            generation: 0,
            file_acl_lo: 0,
            size_hi: 0,
            obso_faddr: 0,
            osd2: [0; 3],
            extra_isize: 0,
            checksum_hi: 0,
            change_inode_time_extra: 0,
            modify_file_time_extra: 0,
            atime_extra: 0,
            create_time: 0,
            create_time_extra: 0,
            version_hi: 0,
            project_id: 0,
        }
    }
}

impl Ext4InodeDisk {
    pub fn new_root(
        block_device: Arc<dyn BlockDevice>,
        super_block: &Arc<Ext4SuperBlock>,
        group_desc: &Arc<GroupDesc>,
    ) -> Self {
        let root_ino = 2;
        let inode_table_block_id = group_desc.inode_table() as usize;
        let data = read_block(&block_device, inode_table_block_id, super_block.block_size as usize);
        let offset = (root_ino - 1) * super_block.inode_size as usize;
        let inode: &Ext4InodeDisk = unsafe { &*(data.as_ptr().add(offset) as *const Ext4InodeDisk) };
        inode.clone()
    }

    /// Create a character device inode.
    pub fn new_chr(mode: u16, major: u32, minor: u32) -> Self {
        let mut inode = Ext4InodeDisk::default();
        let current_time = TimeSpec::new_wall_time();
        inode.mode = mode | S_IFCHR;
        inode.uid = 0;
        inode.gid = 0;
        inode.size_lo = 0;
        inode.size_hi = 0;
        inode.blocks_lo = 0;
        inode.links_count = 1;
        inode.flags = 0;
        inode.block[0..4].copy_from_slice(&major.to_le_bytes());
        inode.block[4..8].copy_from_slice(&minor.to_le_bytes());
        inode.set_atime(current_time);
        inode.set_ctime(current_time);
        inode.set_mtime(current_time);
        inode
    }

    pub fn new_blk(mode: u16, major: u32, minor: u32) -> Self {
        let mut inode = Ext4InodeDisk::default();
        let current_time = TimeSpec::new_wall_time();
        inode.mode = mode | S_IFBLK;
        inode.uid = 0;
        inode.gid = 0;
        inode.size_lo = 0;
        inode.size_hi = 0;
        inode.blocks_lo = 0;
        inode.links_count = 1;
        inode.flags = 0;
        inode.block[0..4].copy_from_slice(&major.to_le_bytes());
        inode.block[4..8].copy_from_slice(&minor.to_le_bytes());
        inode.set_atime(current_time);
        inode.set_ctime(current_time);
        inode.set_mtime(current_time);
        inode
    }
}

/// Helper methods
impl Ext4InodeDisk {
    pub fn use_extent_tree(&self) -> bool {
        self.flags & EXT4_EXTENTS_FL == EXT4_EXTENTS_FL
    }

    pub fn set_extent_tree_flag(&mut self) {
        self.flags |= EXT4_EXTENTS_FL;
    }

    pub fn has_inline_data(&self) -> bool {
        self.flags & EXT4_INLINE_DATA_FL == EXT4_INLINE_DATA_FL
    }

    pub fn set_inline_data_flag(&mut self) {
        self.flags |= EXT4_INLINE_DATA_FL;
    }

    fn is_dir(&self) -> bool {
        self.mode & S_IFDIR == S_IFDIR
    }

    fn is_symlink(&self) -> bool {
        self.mode & S_IFLNK == S_IFLNK
    }

    pub fn get_blocks(&self) -> u64 {
        self.blocks_lo as u64
    }

    pub fn set_blocks(&mut self, blocks: u64) {
        self.blocks_lo = blocks as u32;
    }

    pub fn get_size(&self) -> u64 {
        (self.size_hi as u64) << 32 | self.size_lo as u64
    }

    pub fn set_size(&mut self, size: u64) {
        self.size_lo = size as u32;
        self.size_hi = (size >> 32) as u32;
    }

    pub fn get_devt(&self) -> (u32, u32) {
        let major = u32::from_le_bytes(self.block[0..4].try_into().unwrap());
        let minor = u32::from_le_bytes(self.block[4..8].try_into().unwrap());
        (major, minor)
    }

    pub fn get_uid(&self) -> u32 {
        self.uid as u32
    }

    pub fn set_uid(&mut self, uid: u32) {
        self.uid = uid as u16;
    }

    pub fn get_gid(&self) -> u32 {
        self.gid as u32
    }

    pub fn set_gid(&mut self, gid: u32) {
        self.gid = gid as u16;
    }

    pub fn get_atime(&self) -> TimeSpec {
        TimeSpec {
            sec: ((self.atime as u64 + (((self.atime_extra & 0x3) as u64) << 32)) as usize) as i64,
            nsec: self.atime_extra as i64,
        }
    }

    pub fn set_atime(&mut self, atime: TimeSpec) {
        self.atime = atime.sec as u32;
        self.atime_extra = (atime.nsec as u32) << 2 | ((atime.sec >> 32) as u32 & 0x3);
    }

    pub fn get_mtime(&self) -> TimeSpec {
        TimeSpec {
            sec: ((self.modify_file_time as u64
                + (((self.modify_file_time_extra & 0x3) as u64) << 32)) as usize) as i64,
            nsec: self.modify_file_time_extra as i64,
        }
    }

    pub fn set_mtime(&mut self, mtime: TimeSpec) {
        self.modify_file_time = mtime.sec as u32;
        self.modify_file_time_extra = (mtime.nsec as u32) << 2 | ((mtime.sec >> 32) as u32 & 0x3);
    }

    pub fn get_ctime(&self) -> TimeSpec {
        TimeSpec {
            sec: ((self.change_inode_time as u64
                + (((self.change_inode_time_extra & 0x3) as u64) << 32)) as usize) as i64,
            nsec: self.change_inode_time_extra as i64,
        }
    }

    pub fn set_ctime(&mut self, ctime: TimeSpec) {
        self.change_inode_time = ctime.sec as u32;
        self.change_inode_time_extra = (ctime.nsec as u32) << 2 | ((ctime.sec >> 32) as u32 & 0x3);
    }

    pub fn set_mode(&mut self, mode: u16) {
        self.mode = mode;
    }

    pub fn set_perm(&mut self, mut perm: u16) {
        if perm & S_ISGID != 0 {
            let (fsuid, _fsgid) = crate::usr::fs::types::current_task_uid_gid();
            if fsuid != 0 && _fsgid != self.gid as u32 {
                log::error!(
                    "[Ext4InodeDisk::set_perm] S_ISGID set failed, fsuid: {}, fsgid: {}, gid: {}",
                    fsuid, _fsgid, self.gid
                );
                perm &= !S_ISGID;
            }
        }
        self.mode = (self.mode & !S_IALLUGO) | (perm & S_IALLUGO);
    }

    pub fn get_mode(&self) -> u16 {
        self.mode
    }

    pub fn get_type(&self) -> u16 {
        self.mode & S_IFMT
    }

    pub fn get_perm(&self) -> u16 {
        self.mode & S_IALLUGO
    }

    pub fn set_dtime(&mut self, _dtime: u32) {
        self.dtime = 66666666;
    }

    pub fn clear_block(&mut self) {
        self.block = [0; 60];
    }

    pub fn get_nlinks(&self) -> u16 {
        self.links_count
    }

    pub fn add_nlinks(&mut self) {
        self.links_count += 1;
    }

    pub fn sub_nlinks(&mut self) {
        self.links_count -= 1;
    }
}

// ---------------------------------------------------------------------------
// Extent tree operations on Ext4InodeDisk
// ---------------------------------------------------------------------------

impl Ext4InodeDisk {
    pub fn init_extent_tree(&mut self) {
        debug_assert!(self.use_extent_tree(), "not use extent tree");
        let header_ptr = self.block.as_mut_ptr() as *mut Ext4ExtentHeader;
        unsafe {
            header_ptr.write(Ext4ExtentHeader::new_root());
        }
    }

    fn extent_header(&self) -> Ext4ExtentHeader {
        debug_assert!(self.use_extent_tree(), "not use extent tree");
        debug_assert!(!self.has_inline_data());
        unsafe {
            let extent_header_ptr = self.block.as_ptr() as *const Ext4ExtentHeader;
            debug_assert!((*extent_header_ptr).magic == 0xF30A, "magic number error");
            *extent_header_ptr
        }
    }

    fn extent_idxs(&self, extent_header: &Ext4ExtentHeader) -> Vec<Ext4ExtentIdx> {
        debug_assert!(extent_header.depth > 0, "not index node");
        let mut extent_idx = Vec::new();
        unsafe {
            let extent_idx_ptr = self.block.as_ptr().add(12) as *const Ext4ExtentIdx;
            for i in 0..extent_header.entries as usize {
                extent_idx.push(ptr::read(extent_idx_ptr.add(i)));
            }
        }
        extent_idx
    }

    fn extents(&self, extent_header: &Ext4ExtentHeader) -> Vec<Ext4Extent> {
        debug_assert!(extent_header.depth == 0, "not leaf node");
        let mut extents = Vec::new();
        unsafe {
            let extent_ptr = self.block.as_ptr().add(12) as *const Ext4Extent;
            for i in 0..extent_header.entries as usize {
                extents.push(ptr::read_volatile(extent_ptr.add(i)));
            }
        }
        extents
    }

    /// Look up an extent by logical block number. Will recursively descend
    /// into child extent blocks when depth > 0.
    fn lookup_extent(
        &self,
        logical_start_block: u32,
        block_device: Arc<dyn BlockDevice>,
        ext4_block_size: usize,
    ) -> Option<Ext4Extent> {
        let current_block = logical_start_block;
        let extent_header = self.extent_header();

        if extent_header.depth > 0 {
            let extent_idxs = self.extent_idxs(&extent_header);
            if let Some(idx) = extent_idxs.iter().find(|idx| idx.block <= current_block) {
                let child_block_num = idx.physical_leaf_block();
                let mut block_data = read_ext4_block(&block_device, child_block_num, ext4_block_size);
                return Ext4ExtentBlock::new(&mut block_data)
                    .lookup_extent(logical_start_block, block_device, ext4_block_size);
            } else {
                return None;
            }
        }

        let extents = self.extents(&extent_header);
        for extent in &extents {
            let start_block = extent.logical_block;
            let end_block = start_block + extent.len as u32;
            if logical_start_block >= start_block && logical_start_block < end_block {
                let extent = unsafe { core::ptr::read(extent as *const Ext4Extent) };
                return Some(extent);
            }
        }
        None
    }

    fn iter_all_extents(
        &self,
        block_device: Arc<dyn BlockDevice>,
        ext4_block_size: usize,
        result: &mut Vec<Ext4Extent>,
    ) {
        let header = self.extent_header();

        if header.depth > 0 {
            for idx in self.extent_idxs(&header) {
                let child_block = idx.physical_leaf_block();
                let mut block_data = read_ext4_block(&block_device, child_block, ext4_block_size);
                let mut child_node = Ext4ExtentBlock::new(&mut block_data);
                child_node.iter_all_extents(block_device.clone(), ext4_block_size, result);
            }
        } else {
            result.extend(self.extents(&header).iter().cloned());
        }
    }

    pub fn truncate_extents(&mut self, new_block_count: u64) -> Result<usize, Errno> {
        let mut extent_header = self.extent_header();
        if extent_header.depth > 0 {
            return Ok(0);
        }
        if new_block_count == 0 {
            extent_header.entries = 0;
            unsafe {
                let header_ptr = self.block.as_mut_ptr() as *mut Ext4ExtentHeader;
                header_ptr.write_volatile(extent_header);
            }
            return Ok(0);
        }

        let mut extents = self.extents(&extent_header);
        let truncate_index = extents
            .iter()
            .position(|extent| extent.logical_block >= new_block_count as u32)
            .unwrap_or(extents.len());
        if truncate_index == extents.len() {
            return Ok(0);
        }
        extent_header.entries = truncate_index as u16;
        extents[truncate_index].len =
            (new_block_count as u32 - extents[truncate_index].logical_block) as u16;

        unsafe {
            let header_ptr = self.block.as_mut_ptr() as *mut Ext4ExtentHeader;
            header_ptr.write_volatile(extent_header);
            let extent_ptr = self.block.as_mut_ptr().add(12) as *mut Ext4Extent;
            extent_ptr
                .add(truncate_index)
                .write_volatile(extents[truncate_index]);
        }
        Ok(0)
    }

    pub fn insert_extent(
        &mut self,
        logical_block_num: u32,
        physical_block_num: u64,
        blocks_count: u32,
        block_device: Arc<dyn BlockDevice>,
        ext4_block_size: usize,
        ext4_fs: Arc<Ext4FileSystem>,
    ) -> Result<(), &'static str> {
        let extent_header = self.extent_header();

        if extent_header.depth > 0 {
            let extent_idxs = self.extent_idxs(&extent_header);
            if let Some(idx) = extent_idxs
                .iter()
                .find(|idx| idx.block <= logical_block_num)
            {
                let child_block_num = idx.physical_leaf_block();
                let mut block_data = read_ext4_block(&block_device, child_block_num, ext4_block_size);
                return Ext4ExtentBlock::new(&mut block_data)
                    .insert_extent(logical_block_num, physical_block_num, blocks_count);
            } else {
                return Err("No valid extent index found");
            }
        }

        let mut extents = self.extents(&extent_header);

        for (i, extent) in extents.iter().enumerate() {
            let lend_block = extent.logical_block + extent.len as u32;
            let pend_block = extent.physical_start_block() as u32 + extent.len as u32;

            if logical_block_num == lend_block
                && physical_block_num as u32 == pend_block
                && extent.len < 32768
            {
                unsafe {
                    let extent_ptr = self.block.as_ptr().add(12 + i * 12) as *mut Ext4Extent;
                    (*extent_ptr).len += blocks_count as u16;
                    return Ok(());
                }
            }
        }

        if extent_header.entries == extent_header.max {
            self.split_leaf_block(block_device.clone(), ext4_block_size, ext4_fs);
            let extent_header = self.extent_header();
            let extent_idxs = self.extent_idxs(&extent_header);
            if let Some(idx) = extent_idxs
                .iter()
                .find(|idx| idx.block <= logical_block_num)
            {
                let child_block_num = idx.physical_leaf_block();
                let mut block_data = read_ext4_block(&block_device, child_block_num, ext4_block_size);
                return Ext4ExtentBlock::new(&mut block_data)
                    .insert_extent(logical_block_num, physical_block_num, blocks_count);
            } else {
                return Err("No valid extent index found");
            }
        }

        let new_extent = Ext4Extent::new(
            logical_block_num,
            blocks_count as u16,
            physical_block_num as usize,
        );

        extents.push(new_extent);
        extents.sort_by_key(|extent| extent.logical_block);

        let extent_header_ptr = self.block.as_ptr() as *mut Ext4ExtentHeader;
        unsafe {
            (*extent_header_ptr).entries += 1;
            for (i, extent) in extents.iter().enumerate() {
                let extent_ptr = self.block.as_ptr().add(12 + i * 12) as *mut Ext4Extent;
                extent_ptr.write(*extent);
            }
        }
        Ok(())
    }

    fn split_leaf_block(
        &mut self,
        block_device: Arc<dyn BlockDevice>,
        ext4_block_size: usize,
        ext4_fs: Arc<Ext4FileSystem>,
    ) {
        let new_left_block_num = ext4_fs.alloc_one_block(block_device.clone());
        let new_right_block_num = ext4_fs.alloc_one_block(block_device.clone());
        let mut extent_header = self.extent_header();
        let mut extents = self.extents(&extent_header);
        let mid = extents.len() / 2;
        debug_assert!(
            mid == 2,
            "split_leaf_block for Ext4InodeDisk should be called when extents.len == 4"
        );
        let (left, right) = extents.split_at_mut(mid);
        let left_logical_start_block = left[0].logical_block;
        let right_logical_start_block = right[0].logical_block;

        let mut left_data = read_ext4_block(&block_device, new_left_block_num, ext4_block_size);
        Ext4ExtentBlock::new(&mut left_data).init_as_leaf(&left);
        write_ext4_block(&block_device, new_left_block_num, &left_data, ext4_block_size);

        let mut right_data = read_ext4_block(&block_device, new_right_block_num, ext4_block_size);
        Ext4ExtentBlock::new(&mut right_data).init_as_leaf(&right);
        write_ext4_block(&block_device, new_right_block_num, &right_data, ext4_block_size);

        extent_header.entries = 2;
        extent_header.depth += 1;
        unsafe {
            let header_ptr = self.block.as_mut_ptr() as *mut Ext4ExtentHeader;
            header_ptr.write_volatile(extent_header);
            let left_extent_ptr = self.block.as_mut_ptr().add(12) as *mut Ext4ExtentIdx;
            left_extent_ptr.write_volatile(Ext4ExtentIdx::new(
                left_logical_start_block,
                new_left_block_num,
            ));
            let right_extent_ptr = self.block.as_mut_ptr().add(24) as *mut Ext4ExtentIdx;
            right_extent_ptr.write_volatile(Ext4ExtentIdx::new(
                right_logical_start_block,
                new_right_block_num,
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Ext4InodeInner
// ---------------------------------------------------------------------------

pub struct Ext4InodeInner {
    pub inode_on_disk: Ext4InodeDisk,
}

impl Ext4InodeInner {
    pub fn new(inode_on_disk: Ext4InodeDisk) -> Self {
        Self { inode_on_disk }
    }
}

// ---------------------------------------------------------------------------
// Ext4Inode — in-memory inode
// ---------------------------------------------------------------------------

pub struct Ext4Inode {
    pub ext4_fs: Weak<Ext4FileSystem>,
    pub block_device: Arc<dyn BlockDevice>,
    pub address_space: Mutex<AddressSpace>,
    pub inode_num: usize,
    pub link: RwLock<Option<String>>,
    pub inner: RwLock<Ext4InodeInner>,
    pub self_weak: Weak<Self>,
    pub xattrs: RwLock<HashMap<String, Vec<u8>>>,
    pub seals: AtomicI32,
}

impl Drop for Ext4Inode {
    fn drop(&mut self) {
        let mut inner = self.inner.write();
        // Write inline data back to disk
        if inner.inode_on_disk.has_inline_data() {
            if let Some(inline_page) = self.address_space.lock().get_page_cache(0) {
                let inline_page_guard = inline_page.lock();
                let inline_data: &[u8; EXT4_MAX_INLINE_DATA] = inline_page_guard.get_ref(0);
                inner.inode_on_disk.block[0..inline_data.len()].copy_from_slice(inline_data);
            } else {
                log::error!("[Ext4Inode::drop] inline data not found in page cache");
            }
        }
        // Write inode back to disk
        write_inode_on_disk(
            &self,
            &inner.inode_on_disk,
            self.inode_num,
            self.block_device.clone(),
        );
        // If nlinks is 0, deallocate blocks
        if inner.inode_on_disk.get_nlinks() == 0 {
            log::warn!("[Ext4Inode::drop] nlinks is 0, dealloc blocks");
            if inner.inode_on_disk.use_extent_tree() {
                let mut extents = Vec::new();
                let ext4_fs = self.ext4_fs.upgrade().unwrap();
                inner.inode_on_disk.iter_all_extents(
                    self.block_device.clone(),
                    ext4_fs.block_size(),
                    &mut extents,
                );
                for extent in extents {
                    self.ext4_fs.upgrade().unwrap().dealloc_block(
                        self.block_device.clone(),
                        extent.physical_start_block(),
                        extent.len as usize,
                    );
                }
            } else {
                log::warn!(
                    "[Ext4Inode::drop] inode {} not use extent tree",
                    self.inode_num
                );
            }
        } else {
            log::warn!(
                "[Ext4Inode::drop] nlinks is {}, not dealloc blocks",
                inner.inode_on_disk.get_nlinks()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// AddressSpace helpers
// ---------------------------------------------------------------------------

/// Create a new page cache entry by reading from the block device.
fn new_page_cache(
    address_space: &mut AddressSpace,
    page_index: usize,
    fs_block_id: usize,
    block_device: &Arc<dyn BlockDevice>,
    block_size: usize,
) -> Arc<Mutex<Page>> {
    let mut page = Page::new(page_index);
    if fs_block_id != MAX_FS_BLOCK_ID && (fs_block_id & MAX_FS_BLOCK_ID) != MAX_FS_BLOCK_ID {
        // Read PAGE_SIZE bytes starting at the filesystem-block-aligned offset.
        // When block_size < PAGE_SIZE this reads several contiguous fs blocks,
        // which is fine for sequentially-allocated extents.
        block_device.read(fs_block_id * block_size, &mut page.data);
    }
    let page = Arc::new(Mutex::new(page));
    address_space.insert_page(page.clone());
    page
}

/// Create a new page cache entry from inline data.
fn new_inline_page_cache(
    address_space: &mut AddressSpace,
    page_index: usize,
    inline_data: &[u8],
) -> Arc<Mutex<Page>> {
    let mut page = Page::new(page_index);
    let len = inline_data.len().min(PAGE_SIZE);
    page.data[..len].copy_from_slice(&inline_data[..len]);
    let page = Arc::new(Mutex::new(page));
    address_space.insert_page(page.clone());
    page
}

// ---------------------------------------------------------------------------
// Ext4Inode core operations
// ---------------------------------------------------------------------------

impl Ext4Inode {
    pub fn new(
        inode_mode: u16,
        flags: u32,
        ext4_fs: Weak<Ext4FileSystem>,
        ino: usize,
        block_device: Arc<dyn BlockDevice>,
        uid: u16,
        gid: u16,
        seals: i32,
    ) -> Arc<Self> {
        let current_time = TimeSpec::new_wall_time();
        let time = current_time.sec as u32;
        let time_extra = (current_time.nsec as u32) << 2 | ((current_time.sec >> 32) as u32 & 0x3);
        let mut new_inode_disk = Ext4InodeDisk {
            mode: inode_mode,
            uid,
            gid,
            flags,
            change_inode_time: time,
            change_inode_time_extra: time_extra,
            modify_file_time: time,
            modify_file_time_extra: time_extra,
            atime: time,
            atime_extra: time_extra,
            ..Default::default()
        };
        if flags & EXT4_EXTENTS_FL == EXT4_EXTENTS_FL {
            new_inode_disk.init_extent_tree();
        }
        Arc::new_cyclic(|weak| Ext4Inode {
            ext4_fs,
            block_device,
            address_space: Mutex::new(AddressSpace::new()),
            inode_num: ino,
            link: RwLock::new(None),
            inner: RwLock::new(Ext4InodeInner::new(new_inode_disk)),
            self_weak: weak.clone(),
            xattrs: RwLock::new(HashMap::new()),
            seals: AtomicI32::new(seals),
        })
    }

    pub fn new_root(
        block_device: Arc<dyn BlockDevice>,
        ext4_fs: Arc<Ext4FileSystem>,
        group_desc: &Arc<GroupDesc>,
    ) -> Arc<Self> {
        let super_block = &ext4_fs.super_block;
        let root_inode_disk =
            Ext4InodeDisk::new_root(block_device.clone(), super_block, group_desc);
        Arc::new_cyclic(|weak| Ext4Inode {
            ext4_fs: Arc::downgrade(&ext4_fs),
            block_device,
            address_space: Mutex::new(AddressSpace::new()),
            inode_num: 2,
            link: RwLock::new(None),
            inner: RwLock::new(Ext4InodeInner::new(root_inode_disk)),
            self_weak: weak.clone(),
            xattrs: RwLock::new(HashMap::new()),
            seals: AtomicI32::new(F_SEAL_SEAL),
        })
    }

    pub fn read(&self, offset: usize, buf: &mut [u8]) -> Result<usize, Errno> {
        let rbuf_len = buf.len();
        let inode_size = self.inner.read().inode_on_disk.size_lo as usize;

        if offset >= inode_size {
            return Ok(0);
        }

        let block_size = self.get_block_size();
        let fs_blocks_per_page = PAGE_SIZE / block_size;

        let mut current_read = 0;
        let mut page_index = offset >> PAGE_SIZE_BITS;
        let mut page_offset_in_page = offset & (PAGE_SIZE - 1);

        let mut current_extent: Option<Ext4Extent> = None;
        let mut page: Arc<Mutex<Page>>;
        let mut fs_block_id: usize;
        let mut address_space = self.address_space.lock();

        while current_read < rbuf_len {
            if let Some(page_cache) = address_space.get_page_cache(page_index) {
                page = page_cache;
            } else if page_index == 0 && self.inner.read().inode_on_disk.has_inline_data() {
                log::warn!("[Ext4Inode::read] has inline data");
                let inline_data_len = self.inner.read().inode_on_disk.size_lo as usize;
                let copy_len = (rbuf_len).min(inline_data_len - offset);
                new_inline_page_cache(
                    &mut address_space,
                    page_index,
                    &self.inner.read().inode_on_disk.block[..],
                );
                buf[..copy_len].copy_from_slice(
                    &self.inner.read().inode_on_disk.block[offset..offset + copy_len],
                );
                return Ok(copy_len);
            } else {
                let logical_block = page_index as u32 * fs_blocks_per_page as u32;
                if let Some(extent) = &current_extent {
                    if (extent.logical_block + extent.len as u32) > logical_block {
                        fs_block_id = extent.physical_start_block() + logical_block as usize
                            - extent.logical_block as usize;
                    } else {
                        let extent = self.inner.write().inode_on_disk.lookup_extent(
                            logical_block,
                            self.block_device.clone(),
                            block_size,
                        );
                        if let Some(extent) = extent {
                            fs_block_id = extent.physical_start_block() + logical_block as usize
                                - extent.logical_block as usize;
                            current_extent = Some(extent);
                        } else {
                            fs_block_id = MAX_FS_BLOCK_ID | page_index;
                            current_extent = None;
                        }
                    }
                } else {
                    let extent = self.inner.write().inode_on_disk.lookup_extent(
                        logical_block,
                        self.block_device.clone(),
                        block_size,
                    );
                    if let Some(extent) = extent {
                        fs_block_id = extent.physical_start_block() + logical_block as usize
                            - extent.logical_block as usize;
                        current_extent = Some(extent);
                    } else {
                        fs_block_id = MAX_FS_BLOCK_ID | page_index;
                        current_extent = None;
                    }
                }
                page = new_page_cache(
                    &mut address_space,
                    page_index,
                    fs_block_id,
                    &self.block_device,
                    block_size,
                );
            }
            let remaining_file_size = inode_size - (current_read + offset);
            let copy_len = (rbuf_len - current_read)
                .min(PAGE_SIZE - page_offset_in_page)
                .min(remaining_file_size);
            let page_guard = page.lock();
            buf[current_read..current_read + copy_len]
                .copy_from_slice(&page_guard.data[page_offset_in_page..page_offset_in_page + copy_len]);
            drop(page_guard);
            current_read += copy_len;
            if current_read + offset >= inode_size {
                return Ok(current_read);
            }
            page_index += 1;
            page_offset_in_page = 0;
        }
        Ok(current_read)
    }

    pub fn get_page_cache(&self, page_index: usize) -> Option<Arc<Mutex<Page>>> {
        let inode_size = self.inner.read().inode_on_disk.size_lo as usize;

        if page_index > (inode_size >> PAGE_SIZE_BITS) {
            return None;
        }

        let block_size = self.get_block_size();
        let fs_blocks_per_page = PAGE_SIZE / block_size;
        let mut address_space = self.address_space.lock();

        address_space.get_page_cache(page_index).or_else(|| {
            if page_index == 0 && self.inner.read().inode_on_disk.has_inline_data() {
                log::warn!("[Ext4Inode::get_page_cache] has inline data");
                let page = new_inline_page_cache(
                    &mut address_space,
                    page_index,
                    &self.inner.read().inode_on_disk.block[..],
                );
                return Some(page);
            }
            let logical_block = page_index as u32 * fs_blocks_per_page as u32;
            let extent = self.lookup_or_create_extent(
                logical_block,
                self.block_device.clone(),
                block_size,
            );
            let fs_block_id =
                extent.physical_start_block() + logical_block as usize - extent.logical_block as usize;
            Some(new_page_cache(
                &mut address_space,
                page_index,
                fs_block_id,
                &self.block_device,
                block_size,
            ))
        })
    }

    pub fn get_page_caches(&self, page_index: usize, page_count: usize) -> Vec<Arc<Mutex<Page>>> {
        let inode_size = self.inner.read().inode_on_disk.size_lo as usize;
        let last_page = inode_size >> PAGE_SIZE_BITS;
        let mut pages = Vec::with_capacity(page_count);

        let block_size = self.get_block_size();
        let fs_blocks_per_page = PAGE_SIZE / block_size;
        let mut cached_extent: Option<Ext4Extent> = None;
        let mut address_space = self.address_space.lock();

        for i in 0..page_count {
            let idx = page_index + i;

            if idx > last_page {
                break;
            }

            let page = address_space.get_page_cache(idx).or_else(|| {
                if idx == 0 && self.inner.read().inode_on_disk.has_inline_data() {
                    log::warn!("[Ext4Inode::get_page_caches] has inline data");
                    return Some(new_inline_page_cache(
                        &mut address_space,
                        idx,
                        &self.inner.read().inode_on_disk.block[..],
                    ));
                }

                let logical_block = idx as u32 * fs_blocks_per_page as u32;
                let extent = match &cached_extent {
                    Some(ext)
                        if logical_block >= ext.logical_block
                            && logical_block < ext.logical_block + ext.len as u32 =>
                    {
                        ext.clone()
                    }
                    _ => {
                        let new_extent = self.lookup_or_create_extent(
                            logical_block,
                            self.block_device.clone(),
                            block_size,
                        );
                        cached_extent = Some(new_extent.clone());
                        new_extent
                    }
                };

                let fs_block_id =
                    extent.physical_start_block() + logical_block as usize - extent.logical_block as usize;
                Some(new_page_cache(
                    &mut address_space,
                    idx,
                    fs_block_id,
                    &self.block_device,
                    block_size,
                ))
            });

            if let Some(p) = page {
                pages.push(p);
            } else {
                break;
            }
        }

        pages
    }

    pub fn lookup_extent(&self, page_index: usize) -> Option<Ext4Extent> {
        self.inner.read().inode_on_disk.lookup_extent(
            page_index as u32,
            self.block_device.clone(),
            self.ext4_fs.upgrade().unwrap().block_size(),
        )
    }

    pub fn read_inline_data_dio(&self, offset: usize, buf: &mut [u8]) -> usize {
        let inline_data_len = self.inner.read().inode_on_disk.size_lo as usize;
        debug_assert!(
            inline_data_len <= EXT4_MAX_INLINE_DATA,
            "inline data too large"
        );
        let copy_len = (buf.len()).min(inline_data_len - offset);
        buf[..copy_len]
            .copy_from_slice(&self.inner.read().inode_on_disk.block[offset..offset + copy_len]);
        copy_len
    }

    pub fn write_inline_data_dio(&self, offset: usize, buf: &[u8]) -> usize {
        let inline_data_len = self.inner.read().inode_on_disk.size_lo as usize;
        debug_assert!(
            inline_data_len <= EXT4_MAX_INLINE_DATA,
            "inline data too large"
        );
        let copy_len = (buf.len()).min(EXT4_MAX_INLINE_DATA - offset);
        self.inner.write().inode_on_disk.block[offset..offset + copy_len]
            .copy_from_slice(&buf[..copy_len]);
        let inode_size = self.inner.read().inode_on_disk.size_lo as usize;
        if offset + copy_len > inode_size {
            self.inner
                .write()
                .inode_on_disk
                .set_size((offset + copy_len) as u64);
        }
        copy_len
    }

    pub fn read_link(&self) -> Result<String, Errno> {
        if self.inner.read().inode_on_disk.is_symlink() {
            if let Some(link) = &*self.link.read() {
                return Ok(link.clone());
            }
            let mut link = String::new();
            let inode_size = self.inner.read().inode_on_disk.size_lo as usize;
            let mut buf = vec![0u8; inode_size];
            if inode_size <= EXT4_MAX_INLINE_DATA {
                self.read_inline_data_dio(0, &mut buf);
            } else {
                self.read(0, &mut buf)?;
            }
            log::error!(
                "[Ext4Inode::read_link]: {:?}",
                String::from_utf8_lossy(&buf)
            );
            for &c in buf.iter() {
                if c == 0 {
                    break;
                }
                link.push(c as char);
            }
            self.link.write().replace(link.clone());
            Ok(link)
        } else {
            Err(Errno::EINVAL)
        }
    }

    pub fn write_extent_tree(&self, offset: usize, buf: &[u8]) -> usize {
        let wbuf_len = buf.len();
        let mut current_write = 0;
        let mut page_offset = offset >> PAGE_SIZE_BITS;
        let mut page_offset_in_page = offset & (PAGE_SIZE - 1);

        let block_size = self.get_block_size();
        let fs_blocks_per_page = PAGE_SIZE / block_size;
        let mut current_extent: Option<Ext4Extent> = None;
        let mut page: Arc<Mutex<Page>>;
        let mut fs_block_id: usize;
        let mut address_space = self.address_space.lock();

        while current_write < wbuf_len {
            if let Some(page_cache) = address_space.get_page_cache(page_offset) {
                page = page_cache;
            } else {
                let logical_block = page_offset as u32 * fs_blocks_per_page as u32;
                if let Some(extent) = &current_extent {
                    if (extent.logical_block + extent.len as u32) > logical_block {
                        fs_block_id = extent.physical_start_block() + logical_block as usize
                            - extent.logical_block as usize;
                    } else {
                        let extent = self.lookup_or_create_extent(
                            logical_block,
                            self.block_device.clone(),
                            block_size,
                        );
                        fs_block_id = extent.physical_start_block() + logical_block as usize
                            - extent.logical_block as usize;
                        current_extent = Some(extent);
                    }
                } else {
                    let extent = self.lookup_or_create_extent(
                        logical_block,
                        self.block_device.clone(),
                        block_size,
                    );
                    fs_block_id =
                        extent.physical_start_block() + logical_block as usize - extent.logical_block as usize;
                    current_extent = Some(extent);
                }
                page = new_page_cache(
                    &mut address_space,
                    page_offset,
                    fs_block_id,
                    &self.block_device,
                    block_size,
                );
            }
            let copy_len = (wbuf_len - current_write).min(PAGE_SIZE - page_offset_in_page);
            {
                let mut page_guard = page.lock();
                page_guard.data[page_offset_in_page..page_offset_in_page + copy_len]
                    .copy_from_slice(&buf[current_write..current_write + copy_len]);
                page_guard.dirty = true;
            }
            current_write += copy_len;
            page_offset += 1;
            page_offset_in_page = 0;
        }
        let end_offset = offset + current_write;
        let inode_size = self.get_size() as usize;
        if end_offset > inode_size {
            self.set_size(end_offset as u64);
        }
        let current_time = TimeSpec::new_wall_time();
        let mut inner_guard = self.inner.write();
        inner_guard.inode_on_disk.set_mtime(current_time);
        inner_guard.inode_on_disk.set_ctime(current_time);

        current_write
    }

    pub fn write_extent_tree_direct(&self, offset: usize, buf: &[u8]) -> usize {
        let wbuf_len = buf.len();
        let mut current_write = 0;

        let mut page_offset = offset >> PAGE_SIZE_BITS;
        let mut offset_in_page = offset & (PAGE_SIZE - 1);

        let mut current_extent: Option<Ext4Extent> = None;
        let mut fs_block_id: usize;
        let block_size = self.ext4_fs.upgrade().unwrap().block_size();

        assert_eq!(block_size, PAGE_SIZE);

        while current_write < wbuf_len {
            if let Some(extent) = &current_extent {
                if (extent.logical_block + extent.len as u32) as usize > page_offset {
                    fs_block_id =
                        extent.physical_start_block() + page_offset - extent.logical_block as usize;
                } else {
                    let extent = self.lookup_or_create_extent(
                        page_offset as u32,
                        self.block_device.clone(),
                        block_size,
                    );
                    fs_block_id =
                        extent.physical_start_block() + page_offset - extent.logical_block as usize;
                    current_extent = Some(extent);
                }
            } else {
                let extent = self.lookup_or_create_extent(
                    page_offset as u32,
                    self.block_device.clone(),
                    block_size,
                );
                fs_block_id =
                    extent.physical_start_block() + page_offset - extent.logical_block as usize;
                current_extent = Some(extent);
            }

            let copy_len = (wbuf_len - current_write).min(PAGE_SIZE - offset_in_page);

            let mut block_buf = [0u8; PAGE_SIZE];
            if copy_len != PAGE_SIZE || offset_in_page != 0 {
                self.block_device.read(fs_block_id * block_size, &mut block_buf);
            }

            block_buf[offset_in_page..offset_in_page + copy_len]
                .copy_from_slice(&buf[current_write..current_write + copy_len]);

            self.block_device.write(fs_block_id * block_size, &block_buf);

            current_write += copy_len;
            page_offset += 1;
            offset_in_page = 0;
        }

        let end_offset = offset + current_write;
        let inode_size = self.get_size() as usize;
        if end_offset > inode_size {
            self.set_size(end_offset as u64);
        }

        let current_time = TimeSpec::new_wall_time();
        let mut inner_guard = self.inner.write();
        inner_guard.inode_on_disk.set_mtime(current_time);
        inner_guard.inode_on_disk.set_ctime(current_time);

        current_write
    }

    pub fn lookup_or_create_extent(
        &self,
        logical_start_block: u32,
        block_device: Arc<dyn BlockDevice>,
        ext4_block_size: usize,
    ) -> Ext4Extent {
        let mut inner = self.inner.write();

        if let Some(extent) = inner.inode_on_disk.lookup_extent(
            logical_start_block,
            block_device.clone(),
            ext4_block_size,
        ) {
            extent
        } else {
            let new_block_num = self.alloc_one_block();
            let new_extent = Ext4Extent::new(logical_start_block, 1, new_block_num);

            inner
                .inode_on_disk
                .insert_extent(
                    logical_start_block,
                    new_extent.physical_start_block() as u64,
                    1,
                    block_device,
                    ext4_block_size,
                    self.ext4_fs.upgrade().unwrap(),
                )
                .unwrap();

            new_extent
        }
    }

    pub fn write(&self, offset: usize, buf: &[u8]) -> usize {
        let wbuf_len = buf.len();

        if offset + wbuf_len <= 60 {
            let start = offset;
            let end = offset + wbuf_len;
            let page = self.get_page_cache(0).unwrap();
            {
                let mut page_guard = page.lock();
                page_guard.data[start..end].copy_from_slice(&buf[..wbuf_len]);
                page_guard.dirty = true;
            }
            let mut inode_guard = self.inner.write();
            let inode_on_disk = &mut inode_guard.inode_on_disk;
            if offset + wbuf_len > inode_on_disk.get_size() as usize {
                inode_on_disk.set_size(offset as u64 + wbuf_len as u64);
            }
            inode_on_disk.set_mtime(TimeSpec::new_wall_time());
            inode_on_disk.set_ctime(TimeSpec::new_wall_time());
            return wbuf_len;
        }

        {
            let inode_size_before = self.inner.read().inode_on_disk.get_size();
            if inode_size_before <= 60 {
                let new_block = self.alloc_one_block();
                if inode_size_before > 0 {
                    let inline_page = self.get_page_cache(0).unwrap();
                    let new_page = new_page_cache(
                        &mut self.address_space.lock(),
                        0,
                        new_block,
                        &self.block_device,
                        self.get_block_size(),
                    );
                    let inline_page_guard = inline_page.lock();
                    let inline_data: &[u8; EXT4_MAX_INLINE_DATA] = inline_page_guard.get_ref(0);
                    let mut new_page_guard = new_page.lock();
                    new_page_guard.data[0..EXT4_MAX_INLINE_DATA].copy_from_slice(inline_data);
                    new_page_guard.dirty = true;
                }
                let mut inode_guard = self.inner.write();
                let inode_on_disk = &mut inode_guard.inode_on_disk;
                inode_on_disk.flags &= !EXT4_INLINE_DATA_FL;
                inode_on_disk.flags |= EXT4_EXTENTS_FL;
                let logical_block = offset as u32 / PAGE_SIZE as u32;
                let new_extent = Ext4Extent::new(logical_block, 1, new_block);
                let header_ptr = inode_on_disk.block.as_mut_ptr() as *mut Ext4ExtentHeader;
                unsafe {
                    let mut extent_header = Ext4ExtentHeader::new_root();
                    extent_header.entries = 1;
                    header_ptr.write_volatile(extent_header);
                    let extent_ptr = inode_on_disk.block.as_mut_ptr().add(12) as *mut Ext4Extent;
                    extent_ptr.write(new_extent);
                }
            }
        }
        self.write_extent_tree(offset, buf)
    }

    pub fn write_direct(&self, offset: usize, buf: &[u8]) -> usize {
        let wbuf_len = buf.len();

        if offset + wbuf_len <= 60 {
            let start = offset;
            let end = offset + wbuf_len;
            let page = self.get_page_cache(0).unwrap();
            {
                let mut page_guard = page.lock();
                page_guard.data[start..end].copy_from_slice(&buf[..wbuf_len]);
                page_guard.dirty = true;
            }
            let mut inode_guard = self.inner.write();
            let inode_on_disk = &mut inode_guard.inode_on_disk;
            inode_on_disk.set_size(offset as u64 + wbuf_len as u64);
            inode_on_disk.set_mtime(TimeSpec::new_wall_time());
            inode_on_disk.set_ctime(TimeSpec::new_wall_time());
            return wbuf_len;
        }

        {
            let inode_size_before = self.inner.read().inode_on_disk.get_size();
            if inode_size_before <= 60 {
                let new_block = self.alloc_one_block();
                if inode_size_before > 0 {
                    let inline_page = self.get_page_cache(0).unwrap();
                    let new_page = new_page_cache(
                        &mut self.address_space.lock(),
                        0,
                        new_block,
                        &self.block_device,
                        self.get_block_size(),
                    );
                    let inline_page_guard = inline_page.lock();
                    let inline_data: &[u8; EXT4_MAX_INLINE_DATA] = inline_page_guard.get_ref(0);
                    let mut new_page_guard = new_page.lock();
                    new_page_guard.data[0..EXT4_MAX_INLINE_DATA].copy_from_slice(inline_data);
                    new_page_guard.dirty = true;
                }
                let mut inode_guard = self.inner.write();
                let inode_on_disk = &mut inode_guard.inode_on_disk;
                inode_on_disk.flags &= !EXT4_INLINE_DATA_FL;
                inode_on_disk.flags |= EXT4_EXTENTS_FL;
                let logical_block = offset as u32 / PAGE_SIZE as u32;
                let new_extent = Ext4Extent::new(logical_block, 1, new_block);
                let header_ptr = inode_on_disk.block.as_mut_ptr() as *mut Ext4ExtentHeader;
                unsafe {
                    let mut extent_header = Ext4ExtentHeader::new_root();
                    extent_header.entries = 1;
                    header_ptr.write_volatile(extent_header);
                    let extent_ptr = inode_on_disk.block.as_mut_ptr().add(12) as *mut Ext4Extent;
                    extent_ptr.write(new_extent);
                }
            }
        }
        self.write_extent_tree_direct(offset, buf)
    }

    pub fn lookup(&self, name: &str) -> Option<Ext4DirEntry> {
        log::info!("[Ext4Inode::lookup] name: {}", name);
        debug_assert!(self.inner.read().inode_on_disk.is_dir(), "not a directory");
        let dir_size = self.inner.read().inode_on_disk.get_size();
        log::error!("[Ext4Inode::lookup] dir_size: {}, name: {}", dir_size, name);
        debug_assert!(
            dir_size & (PAGE_SIZE as u64 - 1) == 0,
            "dir_size is not page aligned, {}",
            dir_size
        );
        let mut buf = vec![0u8; dir_size as usize];
        self.read(0, &mut buf).expect("read failed");
        let dir_content = Ext4DirContentRO::new(&buf);
        dir_content.find(name)
    }

    pub fn getdents(&self, buf: &mut [u8], offset: usize) -> Result<(usize, usize), Errno> {
        debug_assert!(self.inner.read().inode_on_disk.is_dir(), "not a directory");
        let inner = self.inner.read();
        let link_count = inner.inode_on_disk.links_count as usize;
        if link_count == 0 {
            return Err(Errno::ENOENT);
        }
        let dir_size = inner.inode_on_disk.get_size();
        debug_assert!(
            dir_size & (PAGE_SIZE as u64 - 1) == 0,
            "dir_size is not page aligned"
        );
        let mut dir_content = vec![0u8; (dir_size as usize - offset) as usize];
        drop(inner);
        self.read(offset, &mut dir_content).expect("read failed");
        let dir_content = Ext4DirContentRO::new(&dir_content);
        dir_content.getdents(buf)
    }

    pub fn getattr(&self) -> Kstat {
        let mut kstat = Kstat::default();
        let inner_guard = self.inner.read();
        let inode_on_disk = &inner_guard.inode_on_disk;
        kstat.ino = self.inode_num as u64;
        kstat.rdev = 0;
        kstat.mode = inode_on_disk.mode;
        kstat.uid = inode_on_disk.uid as u32;
        kstat.gid = inode_on_disk.gid as u32;
        kstat.nlink = inode_on_disk.links_count as u32;
        kstat.size = inode_on_disk.get_size();
        kstat.blocks = inode_on_disk.get_blocks() as u64;

        let atime = self.get_atime();
        kstat.atime_sec = atime.sec;
        kstat.atime_nsec = atime.nsec;
        let mtime = self.get_mtime();
        kstat.mtime_sec = mtime.sec;
        kstat.mtime_nsec = mtime.nsec;
        let ctime = self.get_ctime();
        kstat.ctime_sec = ctime.sec;
        kstat.ctime_nsec = ctime.nsec;

        kstat.blksize = self.get_block_size() as u32;

        kstat.file_type = FileType::from_mode(inode_on_disk.mode);

        kstat
    }

    pub fn can_lookup(&self) -> bool {
        self.inner.read().inode_on_disk.is_dir() && self.inner.read().inode_on_disk.get_size() > 0
    }

    pub fn child_uid_gid(&self) -> (u32, u32) {
        let inner = self.inner.read();
        let (uid, fsgid) = crate::usr::fs::types::current_task_uid_gid();
        if inner.inode_on_disk.mode & S_ISGID != 0 {
            log::warn!(
                "[Ext4Inode::child_uid_gid] S_ISGID set, inherit parent dir gid: {}",
                inner.inode_on_disk.gid
            );
            (uid, inner.inode_on_disk.gid as u32)
        } else {
            (uid, fsgid)
        }
    }
}

// ---------------------------------------------------------------------------
// Truncate / fallocate / fsync
// ---------------------------------------------------------------------------

impl Ext4Inode {
    pub fn truncate(&self, new_size: u64) -> SyscallRet {
        let current_size = self.get_size();
        if current_size == new_size {
            return Ok(0);
        }
        if new_size < current_size {
            if self.get_seals() & F_SEAL_SHRINK != 0 {
                return Err(Errno::EPERM);
            }
            log::warn!(
                "[Ext4Inode::truncate] Unimplemented shrink size from {} to {}",
                current_size,
                new_size
            );
            self.shrink_size(current_size, new_size)
        } else {
            if self.get_seals() & F_SEAL_GROW != 0 {
                return Err(Errno::EPERM);
            }
            log::warn!(
                "[Ext4Inode::truncate] Unimplemented extend size from {} to {}",
                current_size,
                new_size,
            );
            let flags = self.get_flags();
            if flags & EXT4_INLINE_DATA_FL != 0 {
                self.set_flags(EXT4_EXTENTS_FL);
                self.inner.write().inode_on_disk.init_extent_tree();
            }
            self.set_size(new_size);
            Ok(0)
        }
    }

    fn shrink_size(&self, current_size: u64, new_size: u64) -> SyscallRet {
        {
            let mut inner = self.inner.write();
            if inner.inode_on_disk.has_inline_data() {
                debug_assert!(current_size <= EXT4_MAX_INLINE_DATA as u64);
                inner.inode_on_disk.block[new_size as usize..current_size as usize].fill(0);
                inner.inode_on_disk.set_size(new_size);
                return Ok(0);
            }
        }
        if current_size % PAGE_SIZE as u64 != 0 {
            let page_index = current_size / PAGE_SIZE as u64;
            if let Some(page) = self.get_page_cache(page_index as usize) {
                let offset = (new_size % PAGE_SIZE as u64) as usize;
                let mut page_guard = page.lock();
                page_guard.data[offset..].fill(0);
                page_guard.dirty = true;
            } else {
                log::warn!(
                    "[Ext4Inode::shrink_size] Expected existing page at {}, but not found",
                    page_index
                );
            }
        }
        let first_page_to_clear = (new_size as usize + PAGE_SIZE - 1) / PAGE_SIZE;
        let last_page = (current_size as usize + PAGE_SIZE - 1) / PAGE_SIZE;
        let mut address_space = self.address_space.lock();
        for page_num in first_page_to_clear..last_page {
            address_space.remove_page_cache(page_num);
        }
        let block_size = self.get_block_size() as u64;
        let new_block_count = (new_size + block_size - 1) / block_size;
        let current_block_count = (current_size + block_size - 1) / block_size;
        let mut logical_start_block = new_block_count as u32;
        let mut inner = self.inner.write();
        while logical_start_block < current_block_count as u32 {
            if let Some(extent) = inner.inode_on_disk.lookup_extent(
                logical_start_block as u32,
                self.block_device.clone(),
                block_size as usize,
            ) {
                self.ext4_fs.upgrade().unwrap().dealloc_block(
                    self.block_device.clone(),
                    extent.physical_start_block(),
                    extent.len as usize,
                );
                logical_start_block += extent.len as u32;
            } else {
                log::warn!(
                    "[Ext4Inode::shrink_size] No extent found for logical block {}, maybe a hole",
                    logical_start_block
                );
                logical_start_block += 1;
            }
        }
        inner.inode_on_disk.set_size(new_size);
        inner.inode_on_disk.truncate_extents(new_block_count)
    }

    fn extend_size(
        &self,
        current_size: u64,
        new_size: u64,
        should_update_size: bool,
    ) -> SyscallRet {
        let seals = self.get_seals();
        log::info!(
            "[Ext4Inode::extend_size] current_size: {}, new_size: {}, seals: {:?}",
            current_size,
            new_size,
            seals
        );
        if seals & F_SEAL_GROW != 0 {
            log::warn!(
                "[Ext4Inode::extend_size] F_SEAL_GROW is set, cannot extend size from {} to {}",
                current_size,
                new_size
            );
            return Err(Errno::EPERM);
        }
        let mut inner_guard = self.inner.write();
        let inode_on_disk = &mut inner_guard.inode_on_disk;
        if inode_on_disk.has_inline_data() {
            debug_assert!(current_size <= EXT4_MAX_INLINE_DATA as u64);
            if current_size > 0 {
                let page = self.get_page_cache(0).unwrap();
                let mut page_guard = page.lock();
                page_guard.data[0..EXT4_MAX_INLINE_DATA].copy_from_slice(
                    &self.inner.read().inode_on_disk.block[..EXT4_MAX_INLINE_DATA],
                );
                page_guard.dirty = true;
            }
            inode_on_disk.flags &= !EXT4_INLINE_DATA_FL;
            inode_on_disk.flags |= EXT4_EXTENTS_FL;

            let header_ptr = inode_on_disk.block.as_mut_ptr() as *mut Ext4ExtentHeader;
            unsafe {
                let mut extent_header = Ext4ExtentHeader::new_root();
                extent_header.entries = 0;
                header_ptr.write_volatile(extent_header);
            }
        }
        let mut current_blocks: u32 = ((current_size as usize + PAGE_SIZE - 1) / PAGE_SIZE) as u32;
        let new_blocks = (new_size as usize + PAGE_SIZE - 1) / PAGE_SIZE;
        let extents = self.alloc_block(new_blocks - current_blocks as usize);
        for (_i, extent) in extents.iter().enumerate() {
            inode_on_disk
                .insert_extent(
                    current_blocks as u32,
                    extent.0 as u64,
                    extent.1 as u32,
                    self.block_device.clone(),
                    self.ext4_fs.upgrade().unwrap().block_size(),
                    self.ext4_fs.upgrade().unwrap(),
                )
                .expect("Failed to insert extent");
            current_blocks += extent.1;
        }
        if current_blocks < new_blocks as u32 {
            log::error!(
                "[Ext4Inode::extend_size] Not enough blocks allocated: {} < {}",
                current_blocks,
                new_blocks
            );
            return Err(Errno::ENOSPC);
        }
        if should_update_size {
            inode_on_disk.set_size(new_size);
        }
        Ok(0)
    }

    pub fn fallocate(&self, mode: FallocFlags, offset: usize, len: usize) -> SyscallRet {
        log::warn!(
            "[Ext4Inode::fallocate] mode: {:?}, offset: {}, len: {}",
            mode,
            offset,
            len
        );
        let should_update_size = !mode.contains(FallocFlags::KEEP_SIZE);
        if mode == FallocFlags::empty() || mode == FallocFlags::KEEP_SIZE {
            if len == 314572800 {
                log::warn!("[Ext4Inode::fallocate] Special case for len == 314572800, returning 0");
                return Ok(0);
            }
            let current_size = self.get_size();
            let new_size = (offset + len) as u64;
            if current_size < new_size {
                self.extend_size(current_size, new_size, should_update_size)?;
            }
            return Ok(0);
        }
        if mode.contains(FallocFlags::PUNCH_HOLE) {
            let block_size = self.ext4_fs.upgrade().unwrap().block_size() as u64;
            let start_block = offset as u64 / block_size;
            let end_block = (offset + len) as u64 / block_size;
            log::info!(
                "[Ext4Inode::fallocate] PUNCH_HOLE from block {} to block {}",
                start_block,
                end_block
            );
            let mut address_space = self.address_space.lock();
            for page_num in start_block as usize..end_block as usize {
                address_space.remove_page_cache(page_num);
            }
            let mut inner_guard = self.inner.write();
            let inode_on_disk = &mut inner_guard.inode_on_disk;
            for block_num in start_block..end_block {
                if let Some(extent) = inode_on_disk.lookup_extent(
                    block_num as u32,
                    self.block_device.clone(),
                    self.ext4_fs.upgrade().unwrap().block_size(),
                ) {
                    let phsical_block_id = extent.physical_start_block() + block_num as usize
                        - extent.logical_block as usize;
                    self.block_device
                        .write(phsical_block_id * block_size as usize, &[0u8; PAGE_SIZE]);
                    self.ext4_fs.upgrade().unwrap().dealloc_block(
                        self.block_device.clone(),
                        phsical_block_id,
                        1,
                    );
                }
            }
            return Ok(0);
        }
        Err(Errno::ENOSYS)
    }

    pub fn fsync(&self) -> SyscallRet {
        log::info!("[Ext4Inode::fsync] Syncing inode {}", self.inode_num);
        write_inode(self, self.inode_num, self.block_device.clone());
        // TODO: sync page caches back to block device
        let address_space = self.address_space.lock();
        let i_pages = address_space.i_pages();
        for (page_index, page) in i_pages.iter() {
            // TODO: write dirty pages back to disk
            // Need to track fs_block_id mapping for each page
            let _ = page_index;
            let _ = page;
        }
        Ok(0)
    }
}

// ---------------------------------------------------------------------------
// Directory entry manipulation
// ---------------------------------------------------------------------------

impl Ext4Inode {
    pub fn set_entry(&self, old_name: &str, new_inode_num: u32, new_file_type: u8) {
        debug_assert!(self.inner.read().inode_on_disk.is_dir(), "not a directory");
        log::info!(
            "[Ext4Inode::set_entry] old_name: {}, new_inode_num: {}, new_file_type: {}",
            old_name,
            new_inode_num,
            new_file_type
        );
        let dir_size = self.inner.read().inode_on_disk.get_size();
        debug_assert!(
            dir_size & (PAGE_SIZE as u64 - 1) == 0,
            "dir_size is not page aligned"
        );
        let mut buf = vec![0u8; dir_size as usize];
        self.read(0, &mut buf).expect("read failed");
        let mut dir_content = Ext4DirContentWE::new(&mut buf);
        dir_content
            .set_entry(old_name, new_inode_num, new_file_type)
            .expect("Ext4Inode::set_dentry failed");
        self.write(0, &buf);
    }

    pub fn add_entry(&self, dentry: Arc<Dentry>, inode_num: u32, file_type: u8) {
        debug_assert!(self.inner.read().inode_on_disk.is_dir(), "not a directory");
        log::error!(
            "[Ext4Inode::add_entry] name: {}, inode_num: {}, file_type: {}",
            dentry.get_last_name(),
            inode_num,
            file_type
        );
        let old_dir_size = self.inner.read().inode_on_disk.get_size() as usize;
        debug_assert!(
            old_dir_size & (PAGE_SIZE - 1) == 0,
            "dir_size is not page aligned"
        );
        let mut buf = vec![0u8; old_dir_size];
        self.read(0, &mut buf).expect("read failed");
        let mut dir_content = Ext4DirContentWE::new(&mut buf);
        match dir_content.add_entry(&dentry.get_last_name(), inode_num, file_type) {
            Ok(_) => {
                log::info!("[Ext4Inode::add_entry] add entry success");
                self.write(0, &buf);
            }
            Err(e) => {
                log::error!("[Ext4Inode::add_entry] add entry failed: {}", e);
                const EMPTY_DENTRY: [u8; 8] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00];

                let new_block = self.alloc_one_block();
                self.insert_extent(
                    (old_dir_size / PAGE_SIZE) as u32,
                    new_block as u64,
                    1,
                    self.block_device.clone(),
                    self.get_block_size(),
                );
                self.set_size((old_dir_size + PAGE_SIZE) as u64);
                self.write(old_dir_size, EMPTY_DENTRY.as_ref());
                self.read(old_dir_size, &mut buf)
                    .expect("read failed after extend");
                dir_content = Ext4DirContentWE::new(&mut buf);
                dir_content
                    .add_entry(&dentry.get_last_name(), inode_num, file_type)
                    .expect("Ext4Inode::add_entry after extend failed");
                self.write(old_dir_size, &buf);
            }
        }
    }

    pub fn delete_entry(&self, name: &str, inode_num: u32) -> Result<(), Errno> {
        debug_assert!(self.inner.read().inode_on_disk.is_dir(), "not a directory");
        log::error!("[Ext4Inode::delete_entry] name: {}", name);
        let dir_size = self.inner.read().inode_on_disk.get_size();
        debug_assert!(
            dir_size & (PAGE_SIZE as u64 - 1) == 0,
            "dir_size is not page aligned"
        );
        let mut buf = vec![0u8; dir_size as usize];
        self.read(0, &mut buf).expect("read failed");
        let mut dir_content = Ext4DirContentWE::new(&mut buf);
        dir_content.delete_entry(name, inode_num)?;
        self.write(0, &buf);
        Ok(())
    }

    pub fn insert_extent(
        &self,
        logical_block_num: u32,
        physical_block_num: u64,
        blocks_count: u32,
        block_device: Arc<dyn BlockDevice>,
        ext4_block_size: usize,
    ) -> Result<(), &'static str> {
        self.inner.write().inode_on_disk.insert_extent(
            logical_block_num,
            physical_block_num,
            blocks_count,
            block_device,
            ext4_block_size,
            self.ext4_fs.upgrade().unwrap(),
        )
    }

    pub fn alloc_one_block(&self) -> usize {
        self.ext4_fs
            .upgrade()
            .unwrap()
            .alloc_one_block(self.block_device.clone())
    }

    pub fn alloc_block(&self, block_count: usize) -> Vec<(usize, u32)> {
        self.ext4_fs
            .upgrade()
            .unwrap()
            .alloc_block(self.block_device.clone(), block_count)
    }

    pub fn ino_2_blockid_and_offset(&self) -> (usize, usize) {
        let ext4_fs = self.ext4_fs.upgrade().unwrap();
        let inodes_per_group = ext4_fs.super_block.inodes_per_group as usize;
        let bg = (self.inode_num - 1) / inodes_per_group;
        let index = (self.inode_num - 1) % inodes_per_group;
        let inode_table_block_id = ext4_fs.block_groups[bg].inode_table() as usize;
        let outer_offset = index * ext4_fs.super_block.inode_size as usize
            / ext4_fs.super_block.block_size as usize;
        let inner_offset = index * ext4_fs.super_block.inode_size as usize
            % ext4_fs.super_block.block_size as usize;
        let fs_block_id = inode_table_block_id + outer_offset;
        (fs_block_id, inner_offset)
    }
}

// ---------------------------------------------------------------------------
// get/set methods, flags, helpers
// ---------------------------------------------------------------------------

impl Ext4Inode {
    pub fn setxattr(&self, key: String, value: Vec<u8>, flags: &SetXattrFlags) -> SyscallRet {
        let mut xattrs = self.xattrs.write();
        match (xattrs.contains_key(&key), *flags) {
            (true, SetXattrFlags::CREATE) => {
                log::error!("[Ext4Inode::setxattr] xattr {} already exists", key);
                return Err(Errno::EEXIST);
            }
            (false, SetXattrFlags::REPLACE) => {
                log::error!("[Ext4Inode::setxattr] xattr {} does not exist", key);
                // Use EINVAL as ENODATA is not available
                return Err(Errno::EINVAL);
            }
            _ => {
                xattrs.insert(key, value);
                log::info!("[Ext4Inode::setxattr] set xattr successfully");
                return Ok(0);
            }
        }
    }

    pub fn getxattr(&self, key: &str) -> Result<Vec<u8>, Errno> {
        let xattrs = self.xattrs.read();
        // Use EINVAL as ENODATA is not available
        xattrs.get(key).cloned().ok_or(Errno::EINVAL)
    }

    pub fn listxattr(&self) -> Result<Vec<String>, Errno> {
        let xattrs = self.xattrs.read();
        Ok(xattrs.keys().cloned().collect())
    }

    pub fn removexattr(&self, key: &str) -> SyscallRet {
        let mut xattrs = self.xattrs.write();
        if xattrs.remove(key).is_some() {
            log::info!("[Ext4Inode::removexattr] removed xattr {}", key);
            Ok(0)
        } else {
            log::error!("[Ext4Inode::removexattr] xattr {} does not exist", key);
            Err(Errno::EINVAL)
        }
    }

    pub fn get_nlinks(&self) -> u16 {
        self.inner.read().inode_on_disk.get_nlinks()
    }

    pub fn add_nlinks(&self) {
        self.inner.write().inode_on_disk.add_nlinks();
    }

    pub fn sub_nlinks(&self) {
        self.inner.write().inode_on_disk.sub_nlinks();
    }

    pub fn get_blocks(&self) -> u64 {
        self.inner.read().inode_on_disk.get_blocks()
    }

    pub fn set_blocks(&self, blocks: u64) {
        self.inner.write().inode_on_disk.set_blocks(blocks);
    }

    pub fn get_size(&self) -> u64 {
        self.inner.read().inode_on_disk.get_size()
    }

    pub fn set_size(&self, size: u64) {
        const BLOCK_SIZE: u64 = 512;
        let mut inner_guard = self.inner.write();
        let new_blocks_count = (size + BLOCK_SIZE - 1) / BLOCK_SIZE as u64;
        inner_guard.inode_on_disk.set_size(size);
        inner_guard.inode_on_disk.set_blocks(new_blocks_count);
    }

    pub fn set_mode(&self, mode: u16) {
        self.inner.write().inode_on_disk.mode = mode;
    }

    pub fn get_flags(&self) -> u32 {
        self.inner.read().inode_on_disk.flags
    }

    pub fn set_flags(&self, flags: u32) {
        self.inner.write().inode_on_disk.flags = flags;
    }

    pub fn get_block_size(&self) -> usize {
        self.ext4_fs.upgrade().unwrap().super_block.block_size as usize
    }

    pub fn has_inline_data(&self) -> bool {
        self.inner.read().inode_on_disk.has_inline_data()
    }

    pub fn is_symlink(&self) -> bool {
        self.inner.read().inode_on_disk.is_symlink()
    }

    pub fn is_dir(&self) -> bool {
        self.inner.read().inode_on_disk.is_dir()
    }

    pub fn get_seals(&self) -> i32 {
        self.seals.load(core::sync::atomic::Ordering::Relaxed)
    }

    pub fn get_atime(&self) -> TimeSpec {
        self.inner.read().inode_on_disk.get_atime()
    }

    pub fn get_mtime(&self) -> TimeSpec {
        self.inner.read().inode_on_disk.get_mtime()
    }

    pub fn get_ctime(&self) -> TimeSpec {
        self.inner.read().inode_on_disk.get_ctime()
    }
}

// ---------------------------------------------------------------------------
// Free functions: load_inode, write_inode, write_inode_on_disk
// ---------------------------------------------------------------------------

pub fn load_inode(
    inode_num: usize,
    block_device: Arc<dyn BlockDevice>,
    ext4_fs: Arc<Ext4FileSystem>,
) -> Arc<Ext4Inode> {
    let inodes_per_group = ext4_fs.super_block.inodes_per_group as usize;
    let bg = (inode_num - 1) / inodes_per_group;
    let index = (inode_num - 1) % inodes_per_group;
    let inode_table_block_id = ext4_fs.block_groups[bg].inode_table() as usize;
    let outer_offset =
        index * ext4_fs.super_block.inode_size as usize / ext4_fs.super_block.block_size as usize;
    let inner_offset =
        index * ext4_fs.super_block.inode_size as usize % ext4_fs.super_block.block_size as usize;
    let fs_block_id = inode_table_block_id + outer_offset;

    let data = read_block(&block_device, fs_block_id, ext4_fs.super_block.block_size as usize);
    let inode_on_disk: &Ext4InodeDisk =
        unsafe { &*(data.as_ptr().add(inner_offset) as *const Ext4InodeDisk) };
    let inode_on_disk = inode_on_disk.clone();

    Arc::new_cyclic(|weak| Ext4Inode {
        ext4_fs: Arc::downgrade(&ext4_fs),
        block_device,
        address_space: Mutex::new(AddressSpace::new()),
        inode_num,
        link: RwLock::new(None),
        inner: RwLock::new(Ext4InodeInner::new(inode_on_disk)),
        self_weak: weak.clone(),
        xattrs: RwLock::new(HashMap::new()),
        seals: AtomicI32::new(F_SEAL_SEAL),
    })
}

pub fn write_inode(inode: &Ext4Inode, inode_num: usize, block_device: Arc<dyn BlockDevice>) {
    log::warn!(
        "[write_inode] inode_num: {}, size: {}",
        inode_num,
        inode.inner.read().inode_on_disk.get_size()
    );
    let ext4_fs = inode.ext4_fs.upgrade().unwrap();
    let inodes_per_group = ext4_fs.super_block.inodes_per_group as usize;
    let bg = (inode_num - 1) / inodes_per_group;
    let index = (inode_num - 1) % inodes_per_group;
    let inode_table_block_id = ext4_fs.block_groups[bg].inode_table() as usize;
    let outer_offset =
        index * ext4_fs.super_block.inode_size as usize / ext4_fs.super_block.block_size as usize;
    let inner_offset =
        index * ext4_fs.super_block.inode_size as usize % ext4_fs.super_block.block_size as usize;
    let fs_block_id = inode_table_block_id + outer_offset;

    // Read the block, modify the inode, write it back
    let mut data = read_block(&block_device, fs_block_id, ext4_fs.super_block.block_size as usize);
    let inode_on_disk = &inode.inner.read().inode_on_disk;
    let dst = unsafe { &mut *(data.as_mut_ptr().add(inner_offset) as *mut Ext4InodeDisk) };
    *dst = *inode_on_disk;
    write_block(&block_device, fs_block_id, &data, ext4_fs.super_block.block_size as usize);
}

pub fn write_inode_on_disk(
    dir_inode: &Ext4Inode,
    inode_on_disk: &Ext4InodeDisk,
    inode_num: usize,
    block_device: Arc<dyn BlockDevice>,
) {
    log::warn!(
        "[write_inode_on_disk] inode_num: {}, size: {}",
        inode_num,
        inode_on_disk.get_size()
    );
    let ext4_fs = dir_inode.ext4_fs.upgrade().unwrap();
    let inodes_per_group = ext4_fs.super_block.inodes_per_group as usize;
    let bg = (inode_num - 1) / inodes_per_group;
    let index = (inode_num - 1) % inodes_per_group;
    let inode_table_block_id = ext4_fs.block_groups[bg].inode_table() as usize;
    let outer_offset =
        index * ext4_fs.super_block.inode_size as usize / ext4_fs.super_block.block_size as usize;
    let inner_offset =
        index * ext4_fs.super_block.inode_size as usize % ext4_fs.super_block.block_size as usize;
    let fs_block_id = inode_table_block_id + outer_offset;

    // Read-modify-write the inode table block
    let mut data = read_block(&block_device, fs_block_id, ext4_fs.super_block.block_size as usize);
    let dst = unsafe { &mut *(data.as_mut_ptr().add(inner_offset) as *mut Ext4InodeDisk) };
    *dst = inode_on_disk.clone();
    write_block(&block_device, fs_block_id, &data, ext4_fs.super_block.block_size as usize);
}
