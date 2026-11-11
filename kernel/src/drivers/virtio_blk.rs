//! # VirtIO Block Device Driver
//!
//! Implements the VirtIO 0.9.5 / 1.0 paravirtualized block device specification over PCI:
//! - PCI bus device discovery (Vendor `0x1AF4`, Device `0x1001` / `0x1042`).
//! - Legacy PCI I/O port register access.
//! - Split Virtqueue memory management (Descriptor Table, Available Ring, Used Ring).
//! - 512-byte sector read and write DMA transfers.
//! - Integration with the generic [`BlockDevice`] trait and global block registry.
//!
//! ### VirtIO Split Virtqueue Architecture
//!
//! A Virtqueue is composed of three contiguous memory structures aligned per the VirtIO specification:
//! ```text
//! +-------------------------------------------------------+
//! | Descriptor Table: [VirtqDesc; queue_size]             |
//! | (queue_size * 16 bytes)                               |
//! +-------------------------------------------------------+
//! | Available Ring: flags, idx, ring[queue_size]          |
//! | (4 + queue_size * 2 bytes)                            |
//! +-------------------------------------------------------+
//! | PAD to 4096-byte boundary                             |
//! +-------------------------------------------------------+
//! | Used Ring: flags, idx, ring[queue_size] (flags, id)   |
//! | (4 + queue_size * 8 bytes)                            |
//! +-------------------------------------------------------+
//! ```
//!
//! ### Request Descriptor Chaining
//!
//! Each 512-byte sector I/O operation consumes a 3-descriptor chain:
//! 1. **Descriptor 0 (Header)**: Contains [`VirtIoBlkReqHeader`] (request type and 64-bit sector). Device reads (`NEXT`).
//! 2. **Descriptor 1 (Data Buffer)**: 512-byte physical buffer. Device reads on write, device writes on read (`NEXT` | `WRITE`).
//! 3. **Descriptor 2 (Status Byte)**: 1-byte return status (`0 = OK`, `1 = IOERR`, `2 = UNSUPP`). Device writes (`WRITE`).
//!
//! # Real Hardware vs. Virtual Machine Note:
//! VirtIO-Blk is a paravirtualized hypervisor standard (QEMU/KVM/VirtualBox).
//! On real physical computers (bare metal USB boot), VirtIO PCI devices (0x1AF4:0x1001)
//! DO NOT exist. Physical computers attach drives via:
//! 1. AHCI / SATA controllers (PCI Class 0x01, Subclass 0x06) for SATA SSDs/HDDs.
//! 2. NVMe controllers (PCI Class 0x01, Subclass 0x08) for modern M.2 PCIe SSDs.
//! 3. USB Mass Storage (BOT/UAS) via xHCI (PCI Class 0x0C, Subclass 0x03).
//!
//! // TODO(real-hardware-storage): Implement AHCI/SATA (`ahci.rs`) and NVMe (`nvme.rs`)
//! // drivers implementing the `BlockDevice` trait and register them with `register_block_device()`.
//! // Without these drivers, real hardware boots using the in-memory RamFS (`/`) only.
#![allow(dead_code)]

use alloc::sync::Arc;
use core::sync::atomic::{fence, Ordering};
use crate::println;
use crate::sync::Spinlock;
use crate::memory::{self, PhysFrame, PAGE_SIZE_4K};
use crate::drivers::block::{BlockDevice, BlockError, SECTOR_SIZE, register_block_device};
use crate::drivers::pci::{self, BarType, PciDevice};

/// PCI Vendor ID for Red Hat / VirtIO paravirtualized hardware.
const VIRTIO_VENDOR_ID: u16 = 0x1AF4;
/// PCI Device ID for Transitional / Legacy VirtIO Block Device.
const VIRTIO_DEV_BLOCK_LEGACY: u16 = 0x1001;
/// PCI Device ID for Modern VirtIO 1.0 Block Device.
const VIRTIO_DEV_BLOCK_MODERN: u16 = 0x1042;

// VirtIO Legacy PCI I/O Register Offsets (relative to BAR0 I/O base)
/// 32-bit R: Host feature bits.
const VIRTIO_REG_DEVICE_FEATURES: u16 = 0x00;
/// 32-bit W: Guest negotiated feature bits.
const VIRTIO_REG_GUEST_FEATURES: u16  = 0x04;
/// 32-bit RW: Queue Page Frame Number (physical frame >> 12).
const VIRTIO_REG_QUEUE_ADDRESS: u16   = 0x08;
/// 16-bit R: Number of descriptors in currently selected queue.
const VIRTIO_REG_QUEUE_SIZE: u16      = 0x0C;
/// 16-bit W: Queue index selector (0 for block request queue).
const VIRTIO_REG_QUEUE_SELECT: u16    = 0x0E;
/// 16-bit W: Queue notify port (write queue index to kick hypervisor).
const VIRTIO_REG_QUEUE_NOTIFY: u16    = 0x10;
/// 8-bit RW: Device status register bits.
const VIRTIO_REG_DEVICE_STATUS: u16   = 0x12;
/// 8-bit R: Interrupt status register (cleared on read).
const VIRTIO_REG_ISR_STATUS: u16      = 0x13;
/// Start of device-specific configuration space.
const VIRTIO_REG_DEVICE_CONFIG: u16   = 0x14;

// VirtIO Device Status Flags
/// Guest OS has acknowledged the device presence.
const VIRTIO_STATUS_ACKNOWLEDGE: u8 = 1;
/// Guest OS recognizes the device and loaded a driver.
const VIRTIO_STATUS_DRIVER: u8      = 2;
/// Driver setup is complete and device is live.
const VIRTIO_STATUS_DRIVER_OK: u8   = 4;
/// Feature negotiation completed successfully.
const VIRTIO_STATUS_FEATURES_OK: u8 = 8;
/// Driver encountered an unrecoverable failure.
const VIRTIO_STATUS_FAILED: u8      = 128;

// VirtIO Block Request Types
/// Read sector(s) from block storage into host memory.
const VIRTIO_BLK_T_IN: u32  = 0;
/// Write sector(s) from host memory to block storage.
const VIRTIO_BLK_T_OUT: u32 = 1;
/// Flush volatile hardware write caches.
const VIRTIO_BLK_T_FLUSH: u32 = 4;

// VirtIO Block Response Status
/// Request completed successfully.
const VIRTIO_BLK_S_OK: u8     = 0;
/// Device reported an unrecoverable I/O error.
const VIRTIO_BLK_S_IOERR: u8  = 1;
/// Request command is unsupported by the controller.
const VIRTIO_BLK_S_UNSUPP: u8 = 2;

// Virtqueue Descriptor Flags
/// Buffer continues via the `next` descriptor index field.
const VIRTQ_DESC_F_NEXT: u16  = 1;
/// Buffer is write-only for the device (device writes to memory).
const VIRTQ_DESC_F_WRITE: u16 = 2;

/// Standard VirtIO 16-byte memory buffer descriptor.
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, Default)]
pub struct VirtqDesc {
    /// 64-bit physical guest address of the buffer.
    pub addr: u64,
    /// 32-bit length of the buffer in bytes.
    pub len: u32,
    /// Descriptor flags (`VIRTQ_DESC_F_NEXT`, `VIRTQ_DESC_F_WRITE`).
    pub flags: u16,
    /// Next descriptor index in the chain if `VIRTQ_DESC_F_NEXT` is set.
    pub next: u16,
}

/// Virtqueue Available Ring structure updated by the driver to pass requests to the device.
#[repr(C)]
pub struct VirtqAvail {
    /// Driver flags (e.g. interrupt suppression).
    pub flags: u16,
    /// Head index where the driver will place the next request descriptor index.
    pub idx: u16,
    /// Ring array containing descriptor indices.
    pub ring: [u16; 256],
    /// Event index for interrupt suppression (VirtIO 1.0).
    pub used_event: u16,
}

/// Single completed request item in the Virtqueue Used Ring.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VirtqUsedElem {
    /// Index of the head descriptor of the completed chain.
    pub id: u32,
    /// Total number of bytes written into buffer by the device.
    pub len: u32,
}

/// Virtqueue Used Ring structure updated by the device to notify completions.
#[repr(C)]
pub struct VirtqUsed {
    /// Device flags (e.g. notification suppression).
    pub flags: u16,
    /// Head index where the device will place the next completed request element.
    pub idx: u16,
    /// Ring array containing completed request items.
    pub ring: [VirtqUsedElem; 256],
    /// Event index for available ring notifications.
    pub avail_event: u16,
}

/// VirtIO Block Request Header placed in the first descriptor of an I/O chain.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VirtIoBlkReqHeader {
    /// Command type (`VIRTIO_BLK_T_IN`, `VIRTIO_BLK_T_OUT`, `VIRTIO_BLK_T_FLUSH`).
    pub req_type: u32,
    /// Reserved field (must be 0).
    pub reserved: u32,
    /// Logical Block Address (sector number) on disk.
    pub sector: u64,
}

/// Helper Port I/O functions.
#[inline]
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    unsafe {
        core::arch::asm!("in al, dx", in("dx") port, out("al") val, options(nomem, nostack));
    }
    val
}

#[inline]
unsafe fn inw(port: u16) -> u16 {
    let val: u16;
    unsafe {
        core::arch::asm!("in ax, dx", in("dx") port, out("ax") val, options(nomem, nostack));
    }
    val
}

#[inline]
unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    unsafe {
        core::arch::asm!("in eax, dx", in("dx") port, out("eax") val, options(nomem, nostack));
    }
    val
}

#[inline]
unsafe fn outb(port: u16, val: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
    }
}

#[inline]
unsafe fn outw(port: u16, val: u16) {
    unsafe {
        core::arch::asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack));
    }
}

#[inline]
unsafe fn outl(port: u16, val: u32) {
    unsafe {
        core::arch::asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack));
    }
}

/// Driver instance managing a single VirtIO Block PCI device.
pub struct VirtIoBlockDevice {
    /// Base I/O port address decoded from BAR0.
    io_base: u16,
    /// Total disk capacity in 512-byte sectors.
    capacity_sectors: u64,
    /// Number of descriptors in the active virtqueue (e.g. 128 or 256).
    queue_size: u16,
    /// Allocated physical frame backing the virtqueue memory.
    _base_frame: PhysFrame,
    /// Number of contiguous physical frames allocated.
    _num_frames: usize,
    /// Dedicated identity-mapped 4 KiB physical bounce page for header, status, and 512-byte sector DMA.
    dma_frame: PhysFrame,
    /// Pointer to the descriptor table.
    desc_table: *mut VirtqDesc,
    /// Pointer to the available ring.
    avail_ring: *mut VirtqAvail,
    /// Pointer to the used ring.
    used_ring: *mut VirtqUsed,
    /// Synchronized tracker for the last processed used ring index.
    last_used_idx: Spinlock<u16>,
}

// VirtIoBlockDevice is safe to share across threads because port I/O and virtqueue
// operations are synchronized via internal spinlocks.
unsafe impl Send for VirtIoBlockDevice {}
unsafe impl Sync for VirtIoBlockDevice {}

impl VirtIoBlockDevice {
    /// Attempts to probe and initialize a VirtIO Block device from a discovered PCI device.
    ///
    /// Performs feature handshake, allocates contiguous physical memory for the split virtqueue,
    /// configures queue registers, enables the device, and reads capacity geometry.
    pub fn probe(pci_dev: &PciDevice) -> Result<Self, BlockError> {
        // Extract BAR0 I/O port address
        let io_base = match pci_dev.bars.first() {
            Some(bar) => match bar.bar_type {
                BarType::Io { port, .. } => port,
                _ => return Err(BlockError::Unsupported),
            },
            _ => return Err(BlockError::Unsupported),
        };

        // Enable PCI Bus Mastering and I/O port decoding
        pci_dev.enable_bus_mastering();
        pci_dev.enable_io_space();

        unsafe {
            // 1. Reset device
            outb(io_base + VIRTIO_REG_DEVICE_STATUS, 0);

            // 2. Set ACKNOWLEDGE status bit
            let mut status = VIRTIO_STATUS_ACKNOWLEDGE;
            outb(io_base + VIRTIO_REG_DEVICE_STATUS, status);

            // 3. Set DRIVER status bit
            status |= VIRTIO_STATUS_DRIVER;
            outb(io_base + VIRTIO_REG_DEVICE_STATUS, status);

            // 4. Read host features and acknowledge supported subset
            let _host_features = inl(io_base + VIRTIO_REG_DEVICE_FEATURES);
            // Write guest features (we don't require fancy features for basic raw block I/O)
            outl(io_base + VIRTIO_REG_GUEST_FEATURES, 0);

            // 5. Select queue 0 (Block Request Queue)
            outw(io_base + VIRTIO_REG_QUEUE_SELECT, 0);

            // Read queue size
            let q_size = inw(io_base + VIRTIO_REG_QUEUE_SIZE);
            if q_size == 0 || q_size > 256 {
                println!("[VIRTIO-BLK] Unsupported queue size: {}", q_size);
                return Err(BlockError::Unsupported);
            }

            // 6. Allocate contiguous physical frames for split virtqueue per legacy VirtIO spec
            // Descriptors: q_size * 16 bytes
            // Available ring: 4 + q_size * 2 bytes
            // Used ring: aligned to 4096-byte boundary; size = 4 + q_size * 8 bytes
            let desc_bytes = (q_size as usize) * 16;
            let avail_bytes = 4 + (q_size as usize) * 2;
            let used_offset = (desc_bytes + avail_bytes + 4095) & !4095;
            let used_bytes = 4 + (q_size as usize) * 8;
            let total_bytes = (used_offset + used_bytes + 4095) & !4095;
            let num_frames = total_bytes / PAGE_SIZE_4K as usize;

            let base_frame = memory::alloc_contiguous_frames(num_frames).ok_or(BlockError::DeviceBusy)?;
            let queue_ptr = base_frame.start_address().as_u64() as *mut u8;
            core::ptr::write_bytes(queue_ptr, 0, total_bytes);

            let desc_table = queue_ptr as *mut VirtqDesc;
            let avail_ring = queue_ptr.add(desc_bytes) as *mut VirtqAvail;
            let used_ring = queue_ptr.add(used_offset) as *mut VirtqUsed;

            // 7. Inform device of Virtqueue physical address (Page Frame Number)
            let queue_pfn = (base_frame.start_address().as_u64() / 4096) as u32;
            outl(io_base + VIRTIO_REG_QUEUE_ADDRESS, queue_pfn);

            // 8. Set DRIVER_OK status bit
            status |= VIRTIO_STATUS_DRIVER_OK;
            outb(io_base + VIRTIO_REG_DEVICE_STATUS, status);

            // 9. Read disk capacity in 512-byte sectors from device configuration space (offset 0x14)
            let cap_low = inl(io_base + VIRTIO_REG_DEVICE_CONFIG);
            let cap_high = inl(io_base + VIRTIO_REG_DEVICE_CONFIG + 4);
            let capacity_sectors = ((cap_high as u64) << 32) | (cap_low as u64);

            let dma_frame = memory::alloc_frame().ok_or(BlockError::DeviceBusy)?;
            core::ptr::write_bytes(dma_frame.start_address().as_u64() as *mut u8, 0, PAGE_SIZE_4K as usize);

            Ok(Self {
                io_base,
                capacity_sectors,
                queue_size: q_size,
                _base_frame: base_frame,
                _num_frames: num_frames,
                dma_frame,
                desc_table,
                avail_ring,
                used_ring,
                last_used_idx: Spinlock::new(0),
            })
        }
    }

    /// Submits a block request (Read or Write) through the Virtqueue ring buffer and polls for completion.
    ///
    /// Uses the device's dedicated identity-mapped physical `dma_frame` bounce page so callers can
    /// safely pass buffers living in non-identity kernel heap memory (`Vec`) or arbitrary thread stacks.
    fn submit_request(
        &self,
        req_type: u32,
        sector: u64,
        write_data: Option<&[u8]>,
        read_out: Option<&mut [u8]>,
    ) -> Result<(), BlockError> {
        let mut last_used = self.last_used_idx.lock();

        let dma_base = self.dma_frame.start_address().as_u64();
        let req_header_phys = dma_base;
        let status_byte_phys = dma_base + 32;
        let data_buf_phys = dma_base + 64;
        let is_write = write_data.is_some();

        unsafe {
            let hdr_ptr = req_header_phys as *mut VirtIoBlkReqHeader;
            hdr_ptr.write(VirtIoBlkReqHeader {
                req_type,
                reserved: 0,
                sector,
            });
            let status_ptr = status_byte_phys as *mut u8;
            status_ptr.write_volatile(0xFF);
            if let Some(src) = write_data {
                core::ptr::copy_nonoverlapping(src.as_ptr(), data_buf_phys as *mut u8, SECTOR_SIZE);
            }

            let desc = self.desc_table;
            let avail = &mut *self.avail_ring;

            // Descriptor 0: Request Header (Device Reads)
            (*desc.add(0)) = VirtqDesc {
                addr: req_header_phys,
                len: core::mem::size_of::<VirtIoBlkReqHeader>() as u32,
                flags: VIRTQ_DESC_F_NEXT,
                next: 1,
            };

            // Descriptor 1: Data Buffer (512 bytes) in physical bounce page
            let buf_flags = if is_write {
                VIRTQ_DESC_F_NEXT
            } else {
                VIRTQ_DESC_F_NEXT | VIRTQ_DESC_F_WRITE
            };

            (*desc.add(1)) = VirtqDesc {
                addr: data_buf_phys,
                len: SECTOR_SIZE as u32,
                flags: buf_flags,
                next: 2,
            };

            // Descriptor 2: Status Byte (Device Writes)
            (*desc.add(2)) = VirtqDesc {
                addr: status_byte_phys,
                len: 1,
                flags: VIRTQ_DESC_F_WRITE,
                next: 0,
            };

            // Memory fence before updating available ring
            fence(Ordering::SeqCst);

            // Put Descriptor 0 index onto Available Ring
            let avail_slot = (avail.idx % self.queue_size) as usize;
            avail.ring[avail_slot] = 0;

            fence(Ordering::SeqCst);

            // Increment available index
            avail.idx = avail.idx.wrapping_add(1);

            fence(Ordering::SeqCst);

            // Kick the device by writing queue index 0 to QUEUE_NOTIFY
            outw(self.io_base + VIRTIO_REG_QUEUE_NOTIFY, 0);

            // Poll Used Ring for request completion
            let used = &*self.used_ring;
            let expected_used_idx = last_used.wrapping_add(1);

            let mut timeout = 20_000_000;
            while used.idx != expected_used_idx {
                core::hint::spin_loop();
                timeout -= 1;
                if timeout == 0 {
                    return Err(BlockError::Timeout);
                }
            }

            *last_used = expected_used_idx;

            // Acknowledge interrupt in ISR status register
            let _ = inb(self.io_base + VIRTIO_REG_ISR_STATUS);

            fence(Ordering::SeqCst);

            // Copy DMA read result out of physical bounce page if request succeeded
            let status_val = status_ptr.read_volatile();
            match status_val {
                VIRTIO_BLK_S_OK => {
                    if let Some(dst) = read_out {
                        core::ptr::copy_nonoverlapping(
                            data_buf_phys as *const u8,
                            dst.as_mut_ptr(),
                            SECTOR_SIZE,
                        );
                    }
                    Ok(())
                }
                VIRTIO_BLK_S_IOERR => Err(BlockError::IoError),
                VIRTIO_BLK_S_UNSUPP => Err(BlockError::Unsupported),
                _ => Err(BlockError::IoError),
            }
        }
    }
}

impl BlockDevice for VirtIoBlockDevice {
    fn name(&self) -> &str {
        "virtio0"
    }

    fn sector_count(&self) -> u64 {
        self.capacity_sectors
    }

    fn read_sector(&self, sector: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if sector >= self.capacity_sectors {
            return Err(BlockError::InvalidSector);
        }
        if buf.len() < SECTOR_SIZE {
            return Err(BlockError::BufferTooSmall);
        }

        self.submit_request(VIRTIO_BLK_T_IN, sector, None, Some(buf))
    }

    fn write_sector(&self, sector: u64, buf: &[u8]) -> Result<(), BlockError> {
        if sector >= self.capacity_sectors {
            return Err(BlockError::InvalidSector);
        }
        if buf.len() < SECTOR_SIZE {
            return Err(BlockError::BufferTooSmall);
        }

        self.submit_request(VIRTIO_BLK_T_OUT, sector, Some(buf), None)
    }
}

/// Discovers VirtIO Block devices on the PCI bus and registers them in the block subsystem.
pub fn init() {
    println!("----------------------------------------");
    println!(" [VIRTIO-BLK] Probing VirtIO Storage    ");
    println!("----------------------------------------");

    let devices = pci::PCI_DEVICES.lock();
    let mut found_count = 0;

    for dev in devices.iter() {
        if dev.vendor_id == VIRTIO_VENDOR_ID
            && (dev.device_id == VIRTIO_DEV_BLOCK_LEGACY || dev.device_id == VIRTIO_DEV_BLOCK_MODERN)
        {
            println!(
                "[VIRTIO-BLK] Found VirtIO Block Device at PCI {} (Vendor: {:#06x}, Device: {:#06x})",
                dev.address, dev.vendor_id, dev.device_id
            );

            match VirtIoBlockDevice::probe(dev) {
                Ok(blk_dev) => {
                    let sectors = blk_dev.sector_count();
                    let mib = (sectors * 512) / (1024 * 1024);
                    println!(
                        "[VIRTIO-BLK] Initialized '{}': {} sectors ({} MiB capacity)",
                        blk_dev.name(), sectors, mib
                    );
                    register_block_device(Arc::new(blk_dev));
                    found_count += 1;
                }
                Err(e) => {
                    println!("[VIRTIO-BLK] Failed to initialize VirtIO device: {:?}", e);
                }
            }
        }
    }

    if found_count == 0 {
        println!("[VIRTIO-BLK] Note: No VirtIO block device detected on PCI bus.");
        println!("[VIRTIO-BLK] (Hypervisor-only: Physical PCs require AHCI/SATA or NVMe drivers).");
    }
}

/// Executes automated verification self-tests on the VirtIO block device.
///
/// Writes a deterministic 512-byte test pattern to sector 10, reads it back,
/// and asserts that the written and read bytes match exactly.
pub fn run_self_tests() {
    println!("[VIRTIO-BLK] Running VirtIO block verification self-tests...");

    if let Some(device) = crate::drivers::block::primary_device() {
        println!(
            "[VIRTIO-BLK] Testing read/write sector on primary device '{}' ({} sectors)...",
            device.name(), device.sector_count()
        );

        let test_sector = 10u64.min(device.sector_count().saturating_sub(1));

        // 1. Prepare 512-byte test pattern
        let mut write_buf = [0u8; SECTOR_SIZE];
        for (i, b) in write_buf.iter_mut().enumerate() {
            *b = ((i * 7 + 13) & 0xFF) as u8;
        }

        // 2. Write test sector
        match device.write_sector(test_sector, &write_buf) {
            Ok(()) => println!("[VIRTIO-BLK] Successfully wrote 512 bytes to sector {}!", test_sector),
            Err(e) => panic!("[VIRTIO-BLK FATAL] write_sector failed: {:?}", e),
        }

        // 3. Read back sector
        let mut read_buf = [0u8; SECTOR_SIZE];
        match device.read_sector(test_sector, &mut read_buf) {
            Ok(()) => println!("[VIRTIO-BLK] Successfully read 512 bytes from sector {}!", test_sector),
            Err(e) => panic!("[VIRTIO-BLK FATAL] read_sector failed: {:?}", e),
        }

        // 4. Assert data integrity
        assert_eq!(write_buf, read_buf, "VirtIO Block read/write data mismatch!");
        println!("[VIRTIO-BLK] Sector data integrity verified: write pattern matched read pattern 100%!");
        println!("[VIRTIO-BLK] All VirtIO Block self-tests passed successfully!");
    } else {
        println!("[VIRTIO-BLK] Self-test skipped: No block device attached.");
    }
}
