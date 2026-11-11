//! # Generic Block Device Abstraction
//!
//! This module defines the core trait and shared types for all block-oriented storage devices
//! in NullOS (such as VirtIO Block devices, NVMe controllers, AHCI/SATA drives, and RAM disks).
//! It provides:
//! - Standardized sector-based random-access I/O primitives ([`read_sector`], [`write_sector`]).
//! - Storage capacity queries and sector size geometry reporting.
//! - Cache flush synchronization operations.
//! - A global, thread-safe device registry ([`BLOCK_DEVICES`]) with primary device lookup.
//!
//! ### Block Device Contract
//!
//! Implementors of [`BlockDevice`] must guarantee:
//! 1. **Thread Safety**: The driver must be `Send + Sync`, allowing concurrent sector requests across multiple CPU cores.
//! 2. **Buffer Bounds**: The buffer passed to [`read_sector`] or [`write_sector`] must be at least [`sector_size`](BlockDevice::sector_size) bytes long.
//! 3. **Error Reporting**: Invalid sector addresses, bus timeouts, or transfer failures must map cleanly into [`BlockError`].

use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::sync::Spinlock;

/// Standard sector size in bytes for traditional and paravirtualized block storage (512 bytes).
pub const SECTOR_SIZE: usize = 512;

/// Errors that can occur during block device operations.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// The specified storage device could not be located in the registry.
    DeviceNotFound,
    /// Requested Logical Block Address (LBA) is beyond the device's capacity.
    InvalidSector,
    /// Provided destination or source buffer is smaller than the required sector size.
    BufferTooSmall,
    /// Low-level hardware communication or parity transfer error.
    IoError,
    /// The requested operation (e.g. write on read-only media) is unsupported.
    Unsupported,
    /// Device queue is full or hardware controller is currently busy.
    DeviceBusy,
    /// Command execution timed out without a hardware completion notification.
    Timeout,
}

/// Common trait implemented by all block-level storage devices.
///
/// Provides a unified interface consumed by filesystem drivers (e.g., FAT32, ext2)
/// and raw disk partition managers.
#[allow(dead_code)]
pub trait BlockDevice: Send + Sync {
    /// Human-readable identifier for the device (e.g. "virtio0", "sda", "ramdisk0").
    fn name(&self) -> &str;

    /// Sector size in bytes (typically 512 bytes).
    fn sector_size(&self) -> usize {
        SECTOR_SIZE
    }

    /// Total capacity of the storage device expressed as the count of 512-byte sectors.
    fn sector_count(&self) -> u64;

    /// Total storage capacity in bytes (`sector_count * sector_size`).
    fn capacity_bytes(&self) -> u64 {
        self.sector_count() * self.sector_size() as u64
    }

    /// Reads a single 512-byte sector at Logical Block Address (LBA) `sector` into `buf`.
    ///
    /// # Errors
    /// Returns [`BlockError::BufferTooSmall`] if `buf.len() < sector_size()`,
    /// or [`BlockError::InvalidSector`] if `sector >= sector_count()`.
    fn read_sector(&self, sector: u64, buf: &mut [u8]) -> Result<(), BlockError>;

    /// Writes a single 512-byte sector at Logical Block Address (LBA) `sector` from `buf`.
    ///
    /// # Errors
    /// Returns [`BlockError::BufferTooSmall`] if `buf.len() < sector_size()`,
    /// or [`BlockError::InvalidSector`] if `sector >= sector_count()`.
    fn write_sector(&self, sector: u64, buf: &[u8]) -> Result<(), BlockError>;

    /// Flushes any volatile write-back caches to persistent non-volatile media.
    fn flush(&self) -> Result<(), BlockError> {
        Ok(())
    }

    /// Reads multiple contiguous sectors starting at `start_sector` into `buf`.
    ///
    /// Default implementation loops sector-by-sector; drivers may override this
    /// for multi-block DMA batching.
    fn read_sectors(&self, start_sector: u64, count: usize, buf: &mut [u8]) -> Result<(), BlockError> {
        let sec_size = self.sector_size();
        if buf.len() < count * sec_size {
            return Err(BlockError::BufferTooSmall);
        }
        for i in 0..count {
            let offset = i * sec_size;
            self.read_sector(start_sector + i as u64, &mut buf[offset..offset + sec_size])?;
        }
        Ok(())
    }

    /// Writes multiple contiguous sectors starting at `start_sector` from `buf`.
    ///
    /// Default implementation loops sector-by-sector; drivers may override this
    /// for multi-block DMA batching.
    fn write_sectors(&self, start_sector: u64, count: usize, buf: &[u8]) -> Result<(), BlockError> {
        let sec_size = self.sector_size();
        if buf.len() < count * sec_size {
            return Err(BlockError::BufferTooSmall);
        }
        for i in 0..count {
            let offset = i * sec_size;
            self.write_sector(start_sector + i as u64, &buf[offset..offset + sec_size])?;
        }
        Ok(())
    }
}

/// Global thread-safe registry of discovered and initialized block devices.
pub static BLOCK_DEVICES: Spinlock<Vec<Arc<dyn BlockDevice>>> = Spinlock::new(Vec::new());

/// Registers a discovered block device in the global registry.
///
/// The first device registered typically becomes the primary boot/storage device.
pub fn register_block_device(device: Arc<dyn BlockDevice>) {
    BLOCK_DEVICES.lock().push(device);
}

/// Returns the primary registered block device, if any is available.
pub fn primary_device() -> Option<Arc<dyn BlockDevice>> {
    BLOCK_DEVICES.lock().first().cloned()
}

/// Looks up a registered block device by its name (e.g., `"virtio0"`, `"nvme0n1"`).
pub fn get_device(name: &str) -> Option<Arc<dyn BlockDevice>> {
    BLOCK_DEVICES
        .lock()
        .iter()
        .find(|d| d.name() == name)
        .cloned()
}

/// Returns a snapshot vector of all registered block devices.
pub fn all_devices() -> Vec<Arc<dyn BlockDevice>> {
    BLOCK_DEVICES.lock().clone()
}

/// Returns the total number of registered block devices.
#[allow(dead_code)]
pub fn device_count() -> usize {
    BLOCK_DEVICES.lock().len()
}
