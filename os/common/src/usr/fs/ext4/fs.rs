use alloc::vec;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::usr::fs::dev::block_dev::BlockDevice;
use super::block_group::{self, Ext4GroupDescDisk, GroupDesc};
use super::super_block::{Ext4SuperBlock, Ext4SuperBlockDisk};
use super::block_op::EXT4_BLOCK_SIZE;
use super::inode::read_block;

// ---------------------------------------------------------------------------
// Ext4FileSystem
// ---------------------------------------------------------------------------

pub struct Ext4FileSystem {
    pub super_block: Arc<Ext4SuperBlock>,
    pub block_groups: Vec<Arc<GroupDesc>>,
    pub block_device: Arc<dyn BlockDevice>,
}

const EXT4_SUPERBLOCK_OFFSET: usize = 1024;

impl Ext4FileSystem {
    /// Opens and loads an Ext4 filesystem from the `block_device`.
    /// Returns the filesystem handle.
    pub fn open(block_device: Arc<dyn BlockDevice>) -> Arc<Self> {
        // Superblock is at byte offset 1024 within block 0
        let data = read_block(&block_device, 0, EXT4_BLOCK_SIZE);

        let super_block = {
            let ext4_super_block_disk: &Ext4SuperBlockDisk =
                unsafe { &*(data.as_ptr().add(EXT4_SUPERBLOCK_OFFSET) as *const Ext4SuperBlockDisk) };
            log::info!(
                "[Ext4FileSystem::open()] super_block: {:?}",
                ext4_super_block_disk
            );
            debug_assert!(
                ext4_super_block_disk.is_valid(),
                "[Ext4FileSystem::open()] Error loading super_block!"
            );
            Arc::new(Ext4SuperBlock::new(ext4_super_block_disk))
        };

        // Read block group descriptors from block 1
        log::info!(
            "size of GroupDesc: {}",
            core::mem::size_of::<block_group::GroupDesc>()
        );
        let mut block_groups: Vec<Arc<GroupDesc>> = Vec::new();
        let block_group_count = super_block.block_group_count as usize;

        let block_size = super_block.block_size as usize;
        let gdt_block = (EXT4_SUPERBLOCK_OFFSET / block_size) + 1;
        debug_assert!(block_group_count * core::mem::size_of::<Ext4GroupDescDisk>() <= block_size);
        let bg_data = read_block(&block_device, gdt_block, block_size);
        for i in 0..block_group_count {
            let offset = i * core::mem::size_of::<Ext4GroupDescDisk>();
            let group_desc: &Ext4GroupDescDisk =
                unsafe { &*(bg_data.as_ptr().add(offset) as *const Ext4GroupDescDisk) };
            block_groups.push(Arc::new(GroupDesc::new(group_desc)));
        }
        log::info!("Group 0 inode_table: {}", block_groups[0].inode_table());

        Arc::new(Self {
            super_block,
            block_groups,
            block_device,
        })
    }

    pub fn alloc_inode(&self, block_device: Arc<dyn BlockDevice>, is_dir: bool) -> usize {
        let inode_bitmap_size = self.super_block.inodes_per_group as usize / 8;
        for (i, group) in self.block_groups.iter().enumerate() {
            if let Some(local_inode_num) = group.alloc_inode(
                block_device.clone(),
                self.block_size(),
                inode_bitmap_size,
                is_dir,
            ) {
                self.super_block.inner.write().free_inodes_count -= 1;
                let global_inode_num =
                    local_inode_num + self.super_block.inodes_per_group as usize * i + 1;
                return global_inode_num;
            }
        }
        panic!("No available inode!");
    }

    pub fn dealloc_inode(
        &self,
        block_device: Arc<dyn BlockDevice>,
        global_inode_num: usize,
        is_dir: bool,
    ) {
        let inode_index = global_inode_num - 1;
        let group_id = inode_index / self.super_block.inodes_per_group as usize;
        let local_inode_num = inode_index % self.super_block.inodes_per_group as usize;
        let block_bitmap_size = self.super_block.inodes_per_group as usize / 8;
        self.block_groups[group_id].dealloc_inode(
            block_device.clone(),
            local_inode_num,
            is_dir,
            self.super_block.inode_size as usize,
            self.block_size(),
            block_bitmap_size,
        );
    }

    pub fn add_orphan_inode(&self, inode_num: usize) {
        self.super_block.orphan_inodes.write().push(inode_num);
    }

    pub fn alloc_one_block(&self, block_device: Arc<dyn BlockDevice>) -> usize {
        let block_bitmap_size = self.super_block.blocks_per_group as usize / 8;
        let bs = self.block_size();
        for (i, group) in self.block_groups.iter().enumerate() {
            if let Some(local_start) = group.alloc_one_block(
                block_device.clone(),
                bs,
                block_bitmap_size,
            ) {
                self.super_block.inner.write().free_blocks_count -= 1;
                let global_start =
                    local_start + self.super_block.blocks_per_group as usize * i;
                let zeroes = vec![0u8; bs];
                self.block_device
                    .write(global_start * bs, &zeroes);
                return global_start;
            }
        }
        panic!("No available block in any block group!");
    }

    pub fn alloc_block(
        &self,
        block_device: Arc<dyn BlockDevice>,
        block_count: usize,
    ) -> Vec<(usize, u32)> {
        let block_bitmap_size = self.super_block.blocks_per_group as usize / 8;
        let bs = self.block_size();
        let mut result = Vec::new();
        let mut remaining = block_count;

        for (i, group) in self.block_groups.iter().enumerate() {
            if remaining == 0 {
                break;
            }
            let allocated = group.alloc_block(
                block_device.clone(),
                bs,
                block_bitmap_size,
                remaining,
            );
            for (local_start, count) in allocated {
                let global_start =
                    local_start + self.super_block.blocks_per_group as usize * i;
                result.push((global_start, count));
                remaining -= count as usize;
                self.super_block.inner.write().free_blocks_count -= count as u64;
                let zeroes = vec![0u8; bs];
                for off in 0..count as usize {
                    self.block_device
                        .write((global_start + off) * bs, &zeroes);
                }
                if remaining == 0 {
                    break;
                }
            }
        }
        result
    }

    pub fn dealloc_block(
        &self,
        block_device: Arc<dyn BlockDevice>,
        block_num: usize,
        block_count: usize,
    ) {
        let group_id = block_num / self.super_block.blocks_per_group as usize;
        let local_block_num = block_num % self.super_block.blocks_per_group as usize;
        let block_bitmap_size = self.super_block.blocks_per_group as usize / 8;
        self.block_groups[group_id].dealloc_block(
            block_device.clone(),
            local_block_num,
            block_count,
            self.super_block.block_size as usize,
            block_bitmap_size,
        );
        self.super_block.inner.write().free_blocks_count += block_count as u64;
    }
}

impl Ext4FileSystem {
    pub fn block_size(&self) -> usize {
        self.super_block.block_size as usize
    }
}
