//! Block device abstraction — the interface between filesystems and the
//! underlying storage hardware (e.g. virtio-blk).

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

// ---------------------------------------------------------------------------
// BlockDevice trait
// ---------------------------------------------------------------------------

/// A fixed-size block storage device.
pub trait BlockDevice: Send + Sync {
    /// Read `len` bytes from byte-offset `offset` into `buf`.
    fn read(&self, offset: usize, buf: &mut [u8]);

    /// Write `len` bytes from `buf` to byte-offset `offset`.
    fn write(&self, offset: usize, buf: &[u8]);

    /// Total size of the device in bytes.
    fn size(&self) -> usize;

    /// Sector size in bytes (typically 512 for virtio-blk).
    fn sector_size(&self) -> usize;
}

// ---------------------------------------------------------------------------
// BlockBuffer — a cached block
// ---------------------------------------------------------------------------

pub struct BlockBuffer {
    pub data: Vec<u8>,
    pub dirty: bool,
}

impl BlockBuffer {
    pub fn new(block_size: usize) -> Self {
        Self {
            data: vec![0u8; block_size],
            dirty: false,
        }
    }

    /// Read a struct from this block at `offset` by calling a closure with a
    /// reference to the struct.
    pub fn read<T, R>(&self, offset: usize, f: impl FnOnce(&T) -> R) -> R {
        let ptr = self.data.as_ptr().wrapping_add(offset) as *const T;
        unsafe { f(&*ptr) }
    }
}

// ---------------------------------------------------------------------------
// BlockCache — LRU-style cache for filesystem blocks
// ---------------------------------------------------------------------------

const BLOCK_CACHE_CAP: usize = 64;

pub struct BlockCache {
    device: Arc<dyn BlockDevice>,
    block_size: usize,
    blocks: BTreeMap<usize, Arc<Mutex<BlockBuffer>>>,
    lru: Vec<usize>,
}

impl BlockCache {
    pub fn new(device: Arc<dyn BlockDevice>, block_size: usize) -> Self {
        Self {
            device,
            block_size,
            blocks: BTreeMap::new(),
            lru: Vec::new(),
        }
    }

    pub fn block_size(&self) -> usize {
        self.block_size
    }

    pub fn device(&self) -> &Arc<dyn BlockDevice> {
        &self.device
    }

    /// Get a cached block, reading it from the device if necessary.
    pub fn get_block(&mut self, block_num: usize) -> Arc<Mutex<BlockBuffer>> {
        let cached = self.blocks.get(&block_num).cloned();
        if let Some(buf) = cached {
            self.touch_lru(block_num);
            return buf;
        }
        // Read from device.
        let mut buf = BlockBuffer::new(self.block_size);
        self.device
            .read(block_num * self.block_size, &mut buf.data);
        let buf = Arc::new(Mutex::new(buf));
        self.blocks.insert(block_num, buf.clone());
        self.lru.push(block_num);
        self.evict_if_needed();
        buf
    }

    /// Write a specific block back to the device.
    pub fn write_block(&self, block_num: usize) {
        if let Some(buf) = self.blocks.get(&block_num) {
            let b = buf.lock();
            self.device
                .write(block_num * self.block_size, &b.data);
        }
    }

    fn touch_lru(&mut self, block_num: usize) {
        self.lru.retain(|&n| n != block_num);
        self.lru.push(block_num);
    }

    fn evict_if_needed(&mut self) {
        while self.lru.len() > BLOCK_CACHE_CAP {
            if let Some(victim) = self.lru.first().copied() {
                self.lru.remove(0);
                self.blocks.remove(&victim);
            } else {
                break;
            }
        }
    }
}
