//! Generic Block Device Abstraction (Phase 6 - M18 Block Devices).
//!
//! Defines the standard interface for block-oriented storage devices (VirtIO Block,
//! NVMe, AHCI/SATA, and RAM disks).

use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::sync::Spinlock;

/// Standard sector size in bytes for traditional and paravirtualized block storage.
pub const SECTOR_SIZE: usize = 512;

/// Errors that can occur during block device operations.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    DeviceNotFound,
    InvalidSector,
    BufferTooSmall,
    IoError,
    Unsupported,
    DeviceBusy,
    Timeout,
}

/// Common trait implemented by all block-level storage devices.
#[allow(dead_code)]
pub trait BlockDevice: Send + Sync {
    /// Human-readable identifier for the device (e.g. "virtio0", "sda").
    fn name(&self) -> &str;

    /// Sector size in bytes (typically 512 bytes).
    fn sector_size(&self) -> usize {
        SECTOR_SIZE
    }

    /// Total capacity of the storage device in 512-byte sectors.
    fn sector_count(&self) -> u64;

    /// Total storage capacity in bytes.
    fn capacity_bytes(&self) -> u64 {
        self.sector_count() * self.sector_size() as u64
    }

    /// Reads a single 512-byte sector from the device.
    fn read_sector(&self, sector: u64, buf: &mut [u8]) -> Result<(), BlockError>;

    /// Writes a single 512-byte sector to the device.
    fn write_sector(&self, sector: u64, buf: &[u8]) -> Result<(), BlockError>;

    /// Flushes any cached dirty blocks to persistent media.
    fn flush(&self) -> Result<(), BlockError> {
        Ok(())
    }
}

/// Global registry of discovered block devices.
pub static BLOCK_DEVICES: Spinlock<Vec<Arc<dyn BlockDevice>>> = Spinlock::new(Vec::new());

/// Registers a discovered block device in the global registry.
pub fn register_block_device(device: Arc<dyn BlockDevice>) {
    BLOCK_DEVICES.lock().push(device);
}

/// Returns the primary registered block device, if any.
pub fn primary_device() -> Option<Arc<dyn BlockDevice>> {
    BLOCK_DEVICES.lock().first().cloned()
}
