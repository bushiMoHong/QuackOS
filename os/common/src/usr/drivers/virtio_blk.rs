//! VirtIO block device driver.
//!
//! Wraps the `virtio-drivers` crate's `VirtIOBlk` and exposes it via
//! the QuackOS `BlockDevice` trait so the ext4 filesystem can use it.

use virtio_drivers::{
    device::blk::VirtIOBlk,
    transport::mmio::{MmioTransport, VirtIOHeader},
    transport::Transport,
};
use core::ptr::NonNull;
use spin::Mutex;
use alloc::sync::Arc;

use super::hal::QuackHal;
use crate::usr::fs::dev::block_dev::BlockDevice;

/// A VirtIO block device, wrapped for multi-sector arbitrary-offset access
/// and safe sharing via `Arc`.
pub struct VirtIOBlockDev {
    inner: Mutex<VirtIOBlk<QuackHal, MmioTransport>>,
}

impl VirtIOBlockDev {
    /// Create a new VirtIO block device from the MMIO base address.
    pub fn new(mmio_base: usize) -> Result<Self, &'static str> {
        let header = NonNull::new(mmio_base as *mut VirtIOHeader)
            .ok_or("Invalid MMIO base address")?;

        let transport = unsafe {
            MmioTransport::new(header).map_err(|_| "Failed to create MmioTransport")?
        };

        if transport.device_type() != virtio_drivers::transport::DeviceType::Block {
            return Err("Device at MMIO base is not a Block device");
        }

        let blk = VirtIOBlk::<QuackHal, MmioTransport>::new(transport)
            .map_err(|_| "Failed to initialize VirtIOBlk")?;

        Ok(Self { inner: Mutex::new(blk) })
    }
}

impl BlockDevice for VirtIOBlockDev {
    fn read(&self, offset: usize, buf: &mut [u8]) {
        let mut dev = self.inner.lock();
        let sector_size = 512;
        let mut remaining = buf.len();
        let mut buf_offset = 0;
        let mut current_offset = offset;

        while remaining > 0 {
            let sector = current_offset / sector_size;
            let sector_off = current_offset % sector_size;
            let chunk = remaining.min(sector_size - sector_off);

            let mut sector_buf = [0u8; 512];
            dev.read_blocks(sector, &mut sector_buf)
                .expect("VirtIO read failed");

            buf[buf_offset..buf_offset + chunk]
                .copy_from_slice(&sector_buf[sector_off..sector_off + chunk]);

            remaining -= chunk;
            buf_offset += chunk;
            current_offset += chunk;
        }
    }

    fn write(&self, offset: usize, buf: &[u8]) {
        let mut dev = self.inner.lock();
        let sector_size = 512;
        let mut remaining = buf.len();
        let mut buf_offset = 0;
        let mut current_offset = offset;

        while remaining > 0 {
            let sector = current_offset / sector_size;
            let sector_off = current_offset % sector_size;
            let chunk = remaining.min(sector_size - sector_off);

            if sector_off == 0 && chunk == sector_size {
                // Full sector write — pass directly.
                let slice: &[u8; 512] = buf[buf_offset..buf_offset + 512]
                    .try_into()
                    .expect("wrong size");
                dev.write_blocks(sector, slice)
                    .expect("VirtIO write failed");
            } else {
                // Partial sector — read-modify-write.
                let mut sector_buf = [0u8; 512];
                dev.read_blocks(sector, &mut sector_buf)
                    .expect("VirtIO read-modify-write read failed");
                sector_buf[sector_off..sector_off + chunk]
                    .copy_from_slice(&buf[buf_offset..buf_offset + chunk]);
                dev.write_blocks(sector, &sector_buf)
                    .expect("VirtIO read-modify-write write failed");
            }

            remaining -= chunk;
            buf_offset += chunk;
            current_offset += chunk;
        }
    }

    fn size(&self) -> usize {
        let dev = self.inner.lock();
        dev.capacity() as usize * 512
    }

    fn sector_size(&self) -> usize {
        512
    }
}
