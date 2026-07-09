use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;

use crate::usr::fs::dev::block_dev::BlockDevice;
use super::block_op::Ext4Bitmap;
use super::inode::{read_ext4_block, write_ext4_block, Ext4InodeDisk};
use super::block_op::EXT4_BLOCK_SIZE;

#[derive(Debug, Clone)]
#[repr(C)]
pub struct Ext4GroupDescDisk {
    block_bitmap_lo: u32,      // block位图的起始块号(低32位)
    inode_bitmap_lo: u32,      // inode位图的起始块号(低32位)
    inode_table_lo: u32,       // inode表的起始块号(低32位)
    free_blocks_count_lo: u16, // 空闲的block总数(低16位)
    free_inodes_count_lo: u16, // 空闲的inode总数(低16位)
    used_dirs_count_lo: u16,   // 使用的目录总数(低16位)
    pub flags: u16,            // 块组标志, EXT$_BG_flags(INODE_UNINIT, etc)
    exclude_bitmap_lo: u32,    // 快照排除位图
    block_bitmap_csum_lo: u16, // block位图校验和(低16位, crc32c(s_uuid+grp_num+bitmap)) LE
    inode_bitmap_csum_lo: u16, // inode位图校验和(低16位, crc32c(s_uuid+grp_num+bitmap)) LE
    itable_unused_lo: u16,     // 未使用的inode 数量(低16位)
    checksum: u16,             // crc16(sb_uuid+group_num+desc)
    block_bitmap_hi: u32,      // block位图的起始块号(高32位)
    inode_bitmap_hi: u32,      // inode位图的起始块号(高32位)
    inode_table_hi: u32,       // inode表的起始块号(高32位)
    free_blocks_count_hi: u16, // 空闲的block总数(高16位)
    free_inodes_count_hi: u16, // 空闲的inode总数(高16位)
    used_dirs_count_hi: u16,   // 使用的目录总数(高16位)
    itable_unused_hi: u16,     // (已分配但未被初始化)未使用的inode 数量(高16位)
    exclude_bitmap_hi: u32,    // 快照排除位图
    block_bitmap_csum_hi: u16, // crc32c(s_uuid+grp_num+bitmap)的高16位
    inode_bitmap_csum_hi: u16, // crc32c(s_uuid+grp_num+bitmap)的高16位
    reserved: u32,             // 保留字段, 填充
}

impl Ext4GroupDescDisk {
    pub fn is_inode_uninit(&self) -> bool {
        self.flags & 0x1 == 0x1
    }
    pub fn inode_table(&self) -> u64 {
        (self.inode_table_hi as u64) << 32 | self.inode_table_lo as u64
    }
    pub fn block_bitmap(&self) -> u64 {
        (self.block_bitmap_hi as u64) << 32 | self.block_bitmap_lo as u64
    }
    pub fn inode_bitmap(&self) -> u64 {
        (self.inode_bitmap_hi as u64) << 32 | self.inode_bitmap_lo as u64
    }
    pub fn exclude_bitmap(&self) -> u64 {
        (self.exclude_bitmap_hi as u64) << 32 | self.exclude_bitmap_lo as u64
    }
    pub fn free_blocks_count(&self) -> u32 {
        (self.free_blocks_count_hi as u32) << 16 | self.free_blocks_count_lo as u32
    }
    pub fn free_inodes_count(&self) -> u32 {
        (self.free_inodes_count_hi as u32) << 16 | self.free_inodes_count_lo as u32
    }
    pub fn used_dirs_count(&self) -> u32 {
        (self.used_dirs_count_hi as u32) << 16 | self.used_dirs_count_lo as u32
    }
    pub fn itable_unused(&self) -> u32 {
        (self.itable_unused_hi as u32) << 16 | self.itable_unused_lo as u32
    }
}

#[allow(dead_code)]
pub struct GroupDesc {
    pub inode_table: u64,
    pub block_bitmap: u64,
    pub inode_bitmap: u64,
    pub exclude_bitmap: u64,

    inner: RwLock<GroupDescInner>,
}

impl GroupDesc {
    pub fn inode_table(&self) -> u64 {
        self.inode_table
    }
}

impl GroupDesc {
    pub fn new(group_desc_disk: &Ext4GroupDescDisk) -> Self {
        Self {
            inode_table: group_desc_disk.inode_table(),
            block_bitmap: group_desc_disk.block_bitmap(),
            inode_bitmap: group_desc_disk.inode_bitmap(),
            exclude_bitmap: (group_desc_disk.exclude_bitmap_hi as u64) << 32
                | group_desc_disk.exclude_bitmap_lo as u64,
            inner: RwLock::new(GroupDescInner::new(
                group_desc_disk.free_blocks_count(),
                group_desc_disk.free_inodes_count(),
                group_desc_disk.used_dirs_count(),
                group_desc_disk.itable_unused(),
            )),
        }
    }

    /// 在块组的inode_bitmap中分配一个inode
    /// 注意这个inode_num是相对于块组的inode_table的inode_num
    /// 调用者需要将inode_num转换为全局的inode_num(加上inodes_per_group * group_num)
    /// 认为inode_bitmap的大小不会超过一个块大小, 通过assert检测
    pub fn alloc_inode(
        &self,
        block_device: Arc<dyn BlockDevice>,
        ext4_block_size: usize,
        inode_bitmap_size: usize,
        is_dir: bool,
    ) -> Option<usize> {
        debug_assert!(inode_bitmap_size <= ext4_block_size);
        let mut inner = self.inner.write();
        if inner.free_inodes_count > 0 {
            let num_blocks = (inode_bitmap_size + ext4_block_size - 1) / ext4_block_size;
            for i in 0..num_blocks {
                let block_id = self.inode_bitmap as usize + i;
                let mut block_data = read_ext4_block(&block_device, block_id);
                let result = Ext4Bitmap::new(&mut block_data).alloc(inode_bitmap_size);
                if result.is_some() {
                    write_ext4_block(&block_device, block_id, &block_data);
                    inner.free_inodes_count -= 1;
                    if is_dir {
                        inner.used_dirs_count += 1;
                    }
                    return result.map(|n| n + (i * ext4_block_size * 8));
                }
            }
        }
        None
    }

    pub fn dealloc_inode(
        &self,
        block_device: Arc<dyn BlockDevice>,
        local_inode_num: usize,
        is_dir: bool,
        inode_size: usize,
        ext4_block_size: usize,
        block_bitmap_size: usize,
    ) {
        let mut inner = self.inner.write();
        // Free inode table entry
        let table_block_id =
            self.inode_table as usize + local_inode_num * inode_size / ext4_block_size;
        let table_offset = local_inode_num * inode_size % ext4_block_size;
        let mut table_data = read_ext4_block(&block_device, table_block_id);
        unsafe {
            let inode_ptr =
                table_data.as_mut_ptr().add(table_offset) as *mut Ext4InodeDisk;
            (*inode_ptr).set_size(0);
            (*inode_ptr).set_dtime(66666666);
            (*inode_ptr).set_mode(0);
            (*inode_ptr).clear_block();
        }
        write_ext4_block(&block_device, table_block_id, &table_data);

        // Free inode bitmap
        let bitmap_block_id =
            self.inode_bitmap as usize + local_inode_num / (ext4_block_size * 8);
        let bitmap_offset = local_inode_num % (ext4_block_size * 8);
        let mut bitmap_data = read_ext4_block(&block_device, bitmap_block_id);
        Ext4Bitmap::new(&mut bitmap_data).dealloc(bitmap_offset, block_bitmap_size);
        write_ext4_block(&block_device, bitmap_block_id, &bitmap_data);

        inner.free_inodes_count += 1;
        if is_dir {
            inner.used_dirs_count -= 1;
        }
    }

    pub fn alloc_one_block(
        &self,
        block_device: Arc<dyn BlockDevice>,
        ext4_block_size: usize,
        block_bitmap_size: usize,
    ) -> Option<usize> {
        let mut inner = self.inner.write();
        let num_blocks = block_bitmap_size / ext4_block_size;
        if inner.free_blocks_count < 1 {
            return None;
        }
        for i in 0..num_blocks {
            let block_id = self.block_bitmap as usize + i;
            let mut block_data = read_ext4_block(&block_device, block_id);
            let result = Ext4Bitmap::new(&mut block_data).alloc(block_bitmap_size);
            if result.is_some() {
                write_ext4_block(&block_device, block_id, &block_data);
                inner.free_blocks_count -= 1;
                return result.map(|n| n + (i * ext4_block_size * 8));
            }
        }
        None
    }

    pub fn alloc_block(
        &self,
        block_device: Arc<dyn BlockDevice>,
        ext4_block_size: usize,
        block_bitmap_size: usize,
        mut block_count: usize,
    ) -> Vec<(usize, u32)> {
        let mut inner = self.inner.write();
        let mut result = Vec::new();
        let num_blocks = block_bitmap_size / ext4_block_size;

        for i in 0..num_blocks {
            if block_count == 0 {
                break;
            }
            let block_id = self.block_bitmap as usize + i;
            let mut block_data = read_ext4_block(&block_device, block_id);
            let mut bitmap = Ext4Bitmap::new(&mut block_data);
            let mut modified = false;

            while block_count > 0 {
                if let Some((local_start, allocated)) =
                    bitmap.alloc_contiguous(block_bitmap_size, block_count)
                {
                    inner.free_blocks_count -= allocated as u32;
                    let global_start = local_start + (i * ext4_block_size * 8);
                    result.push((global_start, allocated));
                    block_count -= allocated as usize;
                    modified = true;
                } else {
                    break;
                }
            }
            if modified {
                write_ext4_block(&block_device, block_id, &block_data);
            }
        }
        result
    }

    pub fn dealloc_block(
        &self,
        block_device: Arc<dyn BlockDevice>,
        local_block_num: usize,
        block_count: usize,
        ext4_block_size: usize,
        block_bitmap_size: usize,
    ) {
        let mut inner = self.inner.write();
        let block_id =
            self.block_bitmap as usize + local_block_num / (ext4_block_size * 8);
        let block_offset = local_block_num % (ext4_block_size * 8);
        let mut block_data = read_ext4_block(&block_device, block_id);
        Ext4Bitmap::new(&mut block_data)
            .dealloc_contiguous(block_offset, block_count, block_bitmap_size);
        write_ext4_block(&block_device, block_id, &block_data);
        inner.free_blocks_count += block_count as u32;
    }
}

#[allow(dead_code)]
pub struct GroupDescInner {
    free_blocks_count: u32,
    free_inodes_count: u32,
    used_dirs_count: u32,
    #[allow(unused)]
    itable_unused: u32,
}

impl GroupDescInner {
    pub fn new(
        free_blocks_count: u32,
        free_inodes_count: u32,
        used_dirs_count: u32,
        itable_unused: u32,
    ) -> Self {
        Self {
            free_blocks_count,
            free_inodes_count,
            used_dirs_count,
            itable_unused,
        }
    }
}
