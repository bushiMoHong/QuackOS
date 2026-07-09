//! ext4 filesystem implementation.

pub mod block_group;
pub mod block_op;
pub mod dentry;
pub mod extent_tree;
pub mod fs;
pub mod inode;
pub mod ops;
pub mod super_block;

pub const MAX_FS_BLOCK_ID: usize = 0x100000000;
