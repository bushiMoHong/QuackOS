use alloc::{sync::Arc, vec::Vec};

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

        debug_assert!(block_group_count * core::mem::size_of::<GroupDesc>() < EXT4_BLOCK_SIZE);
        let bg_data = read_block(&block_device, 1, EXT4_BLOCK_SIZE);
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

    // TODO: uncomment when GroupDesc allocation methods are ported
    pub fn alloc_inode(&self, _block_device: Arc<dyn BlockDevice>, _is_dir: bool) -> usize {
        // TODO: implement using GroupDesc::alloc_inode
        panic!("[Ext4FileSystem::alloc_inode] not yet implemented");
    }

    pub fn dealloc_inode(
        &self,
        _block_device: Arc<dyn BlockDevice>,
        _global_inode_num: usize,
        _is_dir: bool,
    ) {
        // TODO: implement using GroupDesc::dealloc_inode
        panic!("[Ext4FileSystem::dealloc_inode] not yet implemented");
    }

    pub fn add_orphan_inode(&self, inode_num: usize) {
        self.super_block.orphan_inodes.write().push(inode_num);
    }

    // TODO: uncomment when GroupDesc allocation methods are ported
    pub fn alloc_one_block(&self, _block_device: Arc<dyn BlockDevice>) -> usize {
        // TODO: implement using GroupDesc::alloc_one_block
        panic!("[Ext4FileSystem::alloc_one_block] not yet implemented");
    }

    pub fn alloc_block(
        &self,
        _block_device: Arc<dyn BlockDevice>,
        _block_count: usize,
    ) -> Vec<(usize, u32)> {
        // TODO: implement using GroupDesc::alloc_block
        panic!("[Ext4FileSystem::alloc_block] not yet implemented");
    }

    pub fn dealloc_block(
        &self,
        _block_device: Arc<dyn BlockDevice>,
        _block_num: usize,
        _block_count: usize,
    ) {
        // TODO: implement using GroupDesc::dealloc_block
        panic!("[Ext4FileSystem::dealloc_block] not yet implemented");
    }
}

impl Ext4FileSystem {
    pub fn block_size(&self) -> usize {
        self.super_block.block_size as usize
    }
}
