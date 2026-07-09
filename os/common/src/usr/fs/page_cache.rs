//! Page cache — caches file data in memory pages.  Maps (inode, page_index)
//! → Page so that repeated reads/writes hit memory instead of disk.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use aarch64::base::config::PAGE_SIZE;

// ---------------------------------------------------------------------------
// Page — a cached page of file data
// ---------------------------------------------------------------------------

pub struct Page {
    pub page_index: usize,
    pub data: [u8; PAGE_SIZE],
    pub dirty: bool,
}

impl Page {
    pub fn new(page_index: usize) -> Self {
        Self {
            page_index,
            data: [0u8; PAGE_SIZE],
            dirty: false,
        }
    }

    /// Get a reference to data at `offset` within this page.
    pub fn get_ref<T>(&self, offset: usize) -> &T {
        assert!(offset + core::mem::size_of::<T>() <= PAGE_SIZE);
        unsafe { &*(self.data.as_ptr().add(offset) as *const T) }
    }

    /// Get a mutable reference to data at `offset`.
    pub fn get_mut<T>(&mut self, offset: usize) -> &mut T {
        assert!(offset + core::mem::size_of::<T>() <= PAGE_SIZE);
        unsafe { &mut *(self.data.as_mut_ptr().add(offset) as *mut T) }
    }

    /// Copy bytes into the page starting at `offset`.
    pub fn write_at(&mut self, offset: usize, buf: &[u8]) -> usize {
        let len = core::cmp::min(buf.len(), PAGE_SIZE - offset);
        self.data[offset..offset + len].copy_from_slice(&buf[..len]);
        self.dirty = true;
        len
    }

    /// Copy bytes out of the page starting at `offset`.
    pub fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let len = core::cmp::min(buf.len(), PAGE_SIZE - offset);
        buf[..len].copy_from_slice(&self.data[offset..offset + len]);
        len
    }
}

// ---------------------------------------------------------------------------
// AddressSpace — per-inode page cache
// ---------------------------------------------------------------------------

pub struct AddressSpace {
    pages: BTreeMap<usize, Arc<Mutex<Page>>>,
}

impl AddressSpace {
    pub fn new() -> Self {
        Self {
            pages: BTreeMap::new(),
        }
    }

    pub fn get_page_cache(&self, page_index: usize) -> Option<Arc<Mutex<Page>>> {
        self.pages.get(&page_index).cloned()
    }

    pub fn get_page_caches(&self, page_index: usize, count: usize) -> Vec<Arc<Mutex<Page>>> {
        let mut result = Vec::new();
        for i in 0..count {
            if let Some(page) = self.get_page_cache(page_index + i) {
                result.push(page);
            }
        }
        result
    }

    pub fn insert_page(&mut self, page: Arc<Mutex<Page>>) {
        let idx = page.lock().page_index;
        self.pages.insert(idx, page);
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn remove_page_cache(&mut self, page_index: usize) -> Option<Arc<Mutex<Page>>> {
        self.pages.remove(&page_index)
    }

    pub fn i_pages(&self) -> &BTreeMap<usize, Arc<Mutex<Page>>> {
        &self.pages
    }
}
