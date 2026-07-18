//! InodeOp trait implementation for Ext4Inode — bridges the VFS to ext4.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use core::any::Any;

use super::dentry::{EXT4_DT_DIR, EXT4_DT_LNK, EXT4_DT_REG};
use super::inode::{
    load_inode, write_inode, Ext4Inode, Ext4InodeDisk, EXT4_EXTENTS_FL, EXT4_INLINE_DATA_FL,
    S_IALLUGO, S_IFBLK, S_IFCHR, S_IFDIR, S_IFLNK, S_IFMT, S_IFREG, S_ISGID,
};
use super::MAX_FS_BLOCK_ID;

use crate::usr::fs::dentry::{Dentry, DentryFlags};
use crate::usr::fs::inode::InodeOp;
use crate::usr::fs::types::{current_task_uid_gid, Errno, Kstat, SyscallRet, TimeSpec};

// ---------------------------------------------------------------------------
// InodeOp trait implementation for Ext4Inode
// ---------------------------------------------------------------------------

impl InodeOp for Ext4Inode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read(&self, offset: usize, buf: &mut [u8]) -> usize {
        self.read(offset, buf).unwrap_or(0)
    }

    fn write(&self, offset: usize, buf: &[u8]) -> usize {
        self.write(offset, buf)
    }

    fn truncate(&self, size: usize) -> SyscallRet {
        self.truncate(size as u64)
    }

    fn fsync(&self) -> SyscallRet {
        self.fsync()
    }

    fn get_stat(&self) -> Kstat {
        self.getattr()
    }

    fn get_size(&self) -> usize {
        let inner = self.inner.read();
        inner.inode_on_disk.get_size() as usize
    }

    fn getdents(&self, buf: &mut [u8]) -> (usize, usize) {
        // Call the internal getdents starting from offset 0
        match self.getdents(buf, 0) {
            Ok((file_off, buf_off)) => (file_off, buf_off),
            Err(_) => (0, 0),
        }
    }

    fn read_link(&self) -> Result<String, Errno> {
        self.read_link()
    }

    fn lookup(&self, name: &str, parent_entry: Arc<Dentry>) -> Arc<Dentry> {
        let mut dentry = Dentry::negative(
            format!("{}/{}", parent_entry.absolute_path, name),
            Some(Arc::downgrade(&parent_entry)),
        );

        if let Some(child) = parent_entry.get_child(name) {
            return child;
        }

        if let Some(ext4_dentry) = self.lookup(name) {
            let absolute_path = format!("{}/{}", parent_entry.absolute_path, name);
            let inode_num = ext4_dentry.inode_num as usize;

            let ext4_fs = match self.ext4_fs.upgrade() {
                Some(fs) => fs,
                None => return dentry,
            };

            let inode = load_inode(inode_num, self.block_device.clone(), ext4_fs);

            let inode_mode = inode.inner.read().inode_on_disk.get_mode();
            let flags = match inode_mode & S_IFMT {
                S_IFREG => DentryFlags::REGULAR,
                S_IFDIR => DentryFlags::DIRECTORY,
                S_IFCHR | S_IFBLK => DentryFlags::SPECIAL,
                S_IFLNK => DentryFlags::SYMLINK,
                _ => DentryFlags::MISS,
            };

            dentry = Dentry::new(
                absolute_path,
                Some(Arc::downgrade(&parent_entry)),
                flags,
                Some(inode),
            );
        }

        parent_entry
            .children
            .write()
            .insert(name.to_string(), Arc::downgrade(&dentry));
        dentry
    }

    fn create(&self, dentry: Arc<Dentry>, mode: u16) {
        let ext4_fs = self
            .ext4_fs
            .upgrade()
            .expect("Ext4FileSystem dropped");

        let new_inode_num = ext4_fs.alloc_inode(self.block_device.clone(), false);

        let (child_uid, child_gid) = current_task_uid_gid();
        let mode = mode & S_IALLUGO | S_IFREG;

        let new_inode = Ext4Inode::new(
            mode,
            EXT4_INLINE_DATA_FL,
            self.ext4_fs.clone(),
            new_inode_num,
            self.block_device.clone(),
            child_uid as u16,
            child_gid as u16,
            0,
        );

        write_inode(&new_inode, new_inode_num, self.block_device.clone());
        self.add_entry(dentry.clone(), new_inode_num as u32, EXT4_DT_REG);

        *dentry.inode.write() = Some(new_inode);
        dentry
            .flags
            .write()
            .update_type_from_negative(DentryFlags::REGULAR);
    }

    fn mkdir(&self, dentry: Arc<Dentry>, mode: u16) {
        let ext4_fs = self
            .ext4_fs
            .upgrade()
            .expect("Ext4FileSystem dropped");

        let new_inode_num = ext4_fs.alloc_inode(self.block_device.clone(), true);

        let (child_uid, child_gid) = current_task_uid_gid();
        let mode = mode & S_IALLUGO | S_IFDIR;

        let new_inode = Ext4Inode::new(
            mode,
            0,
            self.ext4_fs.clone(),
            new_inode_num,
            self.block_device.clone(),
            child_uid as u16,
            child_gid as u16,
            0,
        );

        write_inode(&new_inode, new_inode_num, self.block_device.clone());
        self.add_entry(dentry.clone(), new_inode_num as u32, EXT4_DT_DIR);

        // TODO: initialise . and .. entries in the new directory
        // new_inode.init_directory(...)

        *dentry.inode.write() = Some(new_inode);
        dentry
            .flags
            .write()
            .update_type_from_negative(DentryFlags::DIRECTORY);
    }

    fn symlink(&self, dentry: Arc<Dentry>, target: &str) {
        let ext4_fs = self
            .ext4_fs
            .upgrade()
            .expect("Ext4FileSystem dropped");

        let new_inode_num = ext4_fs.alloc_inode(self.block_device.clone(), false);

        let (child_uid, child_gid) = current_task_uid_gid();
        let mode = 0o777 | S_IFLNK;

        let new_inode = Ext4Inode::new(
            mode,
            EXT4_INLINE_DATA_FL,
            self.ext4_fs.clone(),
            new_inode_num,
            self.block_device.clone(),
            child_uid as u16,
            child_gid as u16,
            0,
        );

        // Write symlink target as inline data into the inode block area
        let target_bytes = target.as_bytes();
        if target_bytes.len() < 60 {
            // Fast symlink: store in inode blocks field
            let mut inner = new_inode.inner.write();
            let block = &mut inner.inode_on_disk.block;
            block[..target_bytes.len()].copy_from_slice(target_bytes);
            inner.inode_on_disk.set_size(target_bytes.len() as u64);
        } else {
            // Slow symlink: write to data blocks
            new_inode.write(0, target_bytes);
        }

        write_inode(&new_inode, new_inode_num, self.block_device.clone());
        self.add_entry(dentry.clone(), new_inode_num as u32, EXT4_DT_LNK);

        *dentry.inode.write() = Some(new_inode);
        dentry
            .flags
            .write()
            .update_type_from_negative(DentryFlags::SYMLINK);
    }

    fn mknod(&self, dentry: Arc<Dentry>, mode: u16, rdev: u64) {
        let ext4_fs = self
            .ext4_fs
            .upgrade()
            .expect("Ext4FileSystem dropped");

        let is_blk = mode & S_IFBLK == S_IFBLK;
        let new_inode_num = ext4_fs.alloc_inode(self.block_device.clone(), false);

        let major = ((rdev >> 32) & 0xFFFF_FFFF) as u32;
        let minor = (rdev & 0xFFFF_FFFF) as u32;

        // Create device inode using Ext4InodeDisk helper
        let inode_disk = if is_blk {
            Ext4InodeDisk::new_blk(mode, major, minor)
        } else {
            Ext4InodeDisk::new_chr(mode, major, minor)
        };

        // Build an Ext4Inode from the disk inode
        use alloc::sync::Weak;
        use hashbrown::HashMap;
        use core::sync::atomic::AtomicI32;
        use spin::Mutex;
        use crate::usr::fs::page_cache::AddressSpace;
        use super::inode::Ext4InodeInner;

        let new_inode = Arc::new_cyclic(|weak| Ext4Inode {
            ext4_fs: self.ext4_fs.clone(),
            block_device: self.block_device.clone(),
            address_space: Mutex::new(AddressSpace::new()),
            inode_num: new_inode_num,
            link: spin::RwLock::new(None),
            inner: spin::RwLock::new(Ext4InodeInner::new(inode_disk)),
            self_weak: weak.clone(),
            xattrs: spin::RwLock::new(HashMap::new()),
            seals: AtomicI32::new(0),
        });

        write_inode(&new_inode, new_inode_num, self.block_device.clone());
        self.add_entry(dentry.clone(), new_inode_num as u32, EXT4_DT_REG);

        *dentry.inode.write() = Some(new_inode);
        dentry
            .flags
            .write()
            .update_type_from_negative(DentryFlags::SPECIAL);
    }

    fn unlink(&self, dentry: Arc<Dentry>) -> SyscallRet {
        let name = dentry.get_last_name();
        let inode_num = {
            let inode_guard = dentry.inode.read();
            match &*inode_guard {
                Some(inode) => {
                    // Get inode number from the stored inode
                    // We need to extract the inode number — use the inner read
                    if let Some(ext4) = inode.as_any().downcast_ref::<Ext4Inode>() {
                        ext4.inode_num
                    } else {
                        return Err(Errno::EINVAL);
                    }
                }
                None => return Err(Errno::ENOENT),
            }
        };

        match self.delete_entry(&name, inode_num as u32) {
            Ok(()) => Ok(0),
            Err(e) => Err(e),
        }
    }
}
