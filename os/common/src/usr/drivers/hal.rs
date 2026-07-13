//! Hardware Abstraction Layer for the virtio-drivers crate.
//!
//! Since the kernel uses identity mapping (virt == phys for all kernel addresses),
//! the HAL is straightforward: DMA allocations return identity-mapped memory,
//! and virt↔phys conversions are just identity casts.

use virtio_drivers::{Hal, PhysAddr, BufferDirection};
use core::ptr::NonNull;
use aarch64::base::mm::{alloc_page, free_page};

pub struct QuackHal;

unsafe impl Hal for QuackHal {
    /// Allocate `pages` physically-contiguous pages for DMA.
    ///
    /// With identity mapping, `virt == phys`, so returned virtual address
    /// equals the physical address.
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        if pages == 0 {
            panic!("dma_alloc: pages must be > 0");
        }
        // Allocate the first page.
        let first_pa = alloc_page().expect("dma_alloc: out of memory");
        // Check contiguity for multi-page allocations.
        let mut prev_pa = first_pa;
        for _ in 1..pages {
            let pa = alloc_page().expect("dma_alloc: out of memory");
            if pa != prev_pa + 4096 && pa + 4096 != prev_pa {
                panic!("dma_alloc: non-contiguous allocation across {} pages", pages);
            }
            prev_pa = pa;
        }
        let ptr = NonNull::new(first_pa as *mut u8).expect("dma_alloc: null pointer");
        (first_pa, ptr)
    }

    /// Free DMA memory previously allocated by `dma_alloc`.
    unsafe fn dma_dealloc(paddr: PhysAddr, _vaddr: NonNull<u8>, pages: usize) -> i32 {
        for i in 0..pages {
            free_page(paddr + i * 4096);
        }
        0
    }

    /// Convert an MMIO physical address to a kernel virtual address.
    ///
    /// With identity mapping, virt == phys.
    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, _size: usize) -> NonNull<u8> {
        NonNull::new(paddr as *mut u8).expect("Invalid MMIO physical address")
    }

    /// Get the physical address of a buffer for DMA.
    ///
    /// With identity mapping, phys == virt.
    unsafe fn share(buffer: NonNull<[u8]>, _direction: BufferDirection) -> PhysAddr {
        buffer.as_ptr() as *mut u8 as usize
    }

    /// Unshare a buffer (no-op for identity-mapped kernel).
    unsafe fn unshare(_paddr: PhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {}
}
