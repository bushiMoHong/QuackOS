//! Virtual File System layer — traits, types, and abstractions for filesystem
//! implementations.  The FsServer (server.rs) uses these to dispatch IPC
//! requests to concrete filesystems.

pub mod types;
pub mod inode;
pub mod dentry;
pub mod page_cache;
pub mod dev;
pub mod ext4;
pub mod file;
pub mod server;
