//! # NVMe (Non-Volatile Memory Express) PCIe Storage Controller Driver
//!
//! ## Overview
//! This module implements a native `#![no_std]` PCIe NVMe Host Controller driver
//! conforming to the NVM Express Base Specification (1.x / 2.0) and NVM Command Set.
//!
//! ## Hardware Architecture
//! NVMe communicates over PCI Express using memory-mapped registers (BAR0/BAR1)
//! and paired circular ring buffers in host RAM accessed via DMA:
//! 1. **Admin Queue Pair (`QID = 0`)**: Used for controller discovery, configuration,
//!    identifying controller/namespace capabilities, and creating I/O queue pairs.
//! 2. **I/O Queue Pair (`QID = 1`)**: Dedicated circular Submission Queue (64-byte SQEs)
//!    and Completion Queue (16-byte CQEs) used for high-speed block Read, Write, and Flush.
//! 3. **Doorbell Stride (`CAP.DSTRD`)**: Hardware doorbells starting at `BAR0 + 0x1000`
//!    signal new submissions (`SQyTDBL`) and acknowledge completions (`CQyHDBL`).
//! 4. **Phase Tag (`P` bit)**: Completion Queue Entries flip an alternating phase bit
//!    every wrap-around so the driver can detect completions locklessly without polling MMIO.

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};

use crate::drivers::block::{self, BlockDevice, BlockError, SECTOR_SIZE};
use crate::drivers::pci::{BarType, PCI_DEVICES};
use crate::memory::{self, PageTableFlags, PhysAddr, PhysFrame, PAGE_SIZE_4K};
use crate::println;
use crate::sync::Spinlock;

// ============================================================================
// NVMe MMIO Register Offsets & Constants
// ============================================================================

/// Controller Capabilities (64-bit)
const NVME_REG_CAP: usize = 0x0000;
/// NVMe Specification Version (32-bit)
const NVME_REG_VS: usize = 0x0008;
/// Interrupt Mask Set (32-bit)
const NVME_REG_INTMS: usize = 0x000C;
/// Controller Configuration (32-bit)
const NVME_REG_CC: usize = 0x0014;
/// Controller Status (32-bit)
const NVME_REG_CSTS: usize = 0x001C;
/// Admin Queue Attributes (32-bit)
const NVME_REG_AQA: usize = 0x0024;
/// Admin Submission Queue Base Address (64-bit)
const NVME_REG_ASQ: usize = 0x0028;
/// Admin Completion Queue Base Address (64-bit)
const NVME_REG_ACQ: usize = 0x0030;
/// Base offset of Doorbell Registers
const NVME_REG_DBS: usize = 0x1000;

/// Admin Opcodes
#[allow(dead_code)]
const NVME_ADMIN_DELETE_SQ: u8 = 0x00;
const NVME_ADMIN_CREATE_SQ: u8 = 0x01;
#[allow(dead_code)]
const NVME_ADMIN_DELETE_CQ: u8 = 0x04;
const NVME_ADMIN_CREATE_CQ: u8 = 0x05;
const NVME_ADMIN_IDENTIFY: u8 = 0x06;

/// NVM I/O Command Opcodes
const NVME_CMD_FLUSH: u8 = 0x00;
const NVME_CMD_WRITE: u8 = 0x01;
const NVME_CMD_READ: u8 = 0x02;

/// Queue depth per ring (64 entries fit comfortably in a single 4 KiB page).
const NVME_QUEUE_DEPTH: usize = 64;

// ============================================================================
// NVMe Hardware Queue Entry Structures
// ============================================================================

/// 64-byte NVMe Submission Queue Entry (SQE).
#[repr(C, align(64))]
#[derive(Debug, Clone, Copy, Default)]
pub struct NvmeSqe {
    /// Command Dword 0: Opcode (`7:0`), Fused (`9:8`), PSDT (`15:14`), Command ID (`31:16`).
    pub cdw0: u32,
    /// Namespace Identifier (`1`-based for NVM commands, `0` for controller Identify).
    pub nsid: u32,
    /// Reserved.
    pub rsvd2: u64,
    /// Metadata Pointer (unused unless metadata LBA formats are enabled).
    pub mptr: u64,
    /// Physical Region Page Entry 1 (4 KiB aligned host DMA buffer physical address).
    pub prp1: u64,
    /// Physical Region Page Entry 2 (second page or PRP list pointer).
    pub prp2: u64,
    /// Command-specific Dword 10.
    pub cdw10: u32,
    /// Command-specific Dword 11.
    pub cdw11: u32,
    /// Command-specific Dword 12.
    pub cdw12: u32,
    /// Command-specific Dword 13.
    pub cdw13: u32,
    /// Command-specific Dword 14.
    pub cdw14: u32,
    /// Command-specific Dword 15.
    pub cdw15: u32,
}

/// 16-byte NVMe Completion Queue Entry (CQE).
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, Default)]
pub struct NvmeCqe {
    /// Command-specific return value.
    pub dw0: u32,
    /// Reserved.
    pub dw1: u32,
    /// Current Submission Queue Head Pointer updated by the controller.
    pub sq_head: u16,
    /// Submission Queue Identifier where the completed command originated.
    pub sq_id: u16,
    /// Command Identifier matching the submitted SQE's CID.
    pub cid: u16,
    /// Status Field: Bit 0 = Phase Tag (`P`), Bits 15:1 = Status Code & Status Code Type.
    pub status: u16,
}

// ============================================================================
// Queue Pair Abstraction
// ============================================================================

/// Encapsulates an NVMe Submission & Completion Queue Pair in host DMA memory.
struct NvmeQueue {
    qid: u16,
    depth: u16,
    sq_frame: PhysFrame,
    cq_frame: PhysFrame,
    sq_base: *mut NvmeSqe,
    cq_base: *mut NvmeCqe,
    sq_tail: u16,
    cq_head: u16,
    cq_phase: u16,
    next_cid: u16,
    mmio_base: usize,
    doorbell_stride: usize,
}

unsafe impl Send for NvmeQueue {}
unsafe impl Sync for NvmeQueue {}

impl NvmeQueue {
    /// Allocates physical 4 KiB pages for a Submission and Completion queue pair.
    fn new(qid: u16, depth: u16, mmio_base: usize, doorbell_stride: usize) -> Result<Self, BlockError> {
        let sq_frame = memory::alloc_frame().ok_or(BlockError::IoError)?;
        let cq_frame = memory::alloc_frame().ok_or(BlockError::IoError)?;

        let sq_ptr = sq_frame.start_address().as_u64() as *mut NvmeSqe;
        let cq_ptr = cq_frame.start_address().as_u64() as *mut NvmeCqe;

        unsafe {
            core::ptr::write_bytes(sq_ptr as *mut u8, 0, PAGE_SIZE_4K as usize);
            core::ptr::write_bytes(cq_ptr as *mut u8, 0, PAGE_SIZE_4K as usize);
        }

        Ok(Self {
            qid,
            depth,
            sq_frame,
            cq_frame,
            sq_base: sq_ptr,
            cq_base: cq_ptr,
            sq_tail: 0,
            cq_head: 0,
            cq_phase: 1, // NVMe starts with expected Phase Tag = 1
            next_cid: 1,
            mmio_base,
            doorbell_stride,
        })
    }

    #[inline]
    fn sq_phys(&self) -> u64 {
        self.sq_frame.start_address().as_u64()
    }

    #[inline]
    fn cq_phys(&self) -> u64 {
        self.cq_frame.start_address().as_u64()
    }

    #[inline]
    fn ring_sq_doorbell(&self) {
        let offset = NVME_REG_DBS + (2 * (self.qid as usize)) * self.doorbell_stride;
        unsafe {
            write_volatile((self.mmio_base + offset) as *mut u32, self.sq_tail as u32);
        }
    }

    #[inline]
    fn ring_cq_doorbell(&self) {
        let offset = NVME_REG_DBS + (2 * (self.qid as usize) + 1) * self.doorbell_stride;
        unsafe {
            write_volatile((self.mmio_base + offset) as *mut u32, self.cq_head as u32);
        }
    }

    /// Submits a single NVMe command synchronously and polls the Completion Queue using Phase Tags.
    fn submit_and_wait(&mut self, mut cmd: NvmeSqe) -> Result<NvmeCqe, BlockError> {
        let cid = self.next_cid;
        self.next_cid = self.next_cid.wrapping_add(1);
        if self.next_cid == 0 {
            self.next_cid = 1;
        }
        // Encode CID into bits 31:16 of CDW0
        cmd.cdw0 = (cmd.cdw0 & 0x0000_FFFF) | ((cid as u32) << 16);

        let tail_idx = self.sq_tail as usize;
        unsafe {
            write_volatile(self.sq_base.add(tail_idx), cmd);
        }

        self.sq_tail = (self.sq_tail + 1) % self.depth;
        fence(Ordering::SeqCst);
        self.ring_sq_doorbell();

        // Poll CQ entry for expected Phase Tag flip
        let head_idx = self.cq_head as usize;
        let mut spins: u64 = 0;
        let max_spins: u64 = 50_000_000;

        loop {
            fence(Ordering::SeqCst);
            let cqe = unsafe { read_volatile(self.cq_base.add(head_idx)) };
            let phase = cqe.status & 0x0001;
            if phase == self.cq_phase {
                // Advance completion head pointer and toggle phase on wrap-around
                self.cq_head = self.cq_head + 1;
                if self.cq_head >= self.depth {
                    self.cq_head = 0;
                    self.cq_phase ^= 1;
                }
                self.ring_cq_doorbell();

                let status_code = (cqe.status >> 1) & 0x07FF;
                if status_code != 0 {
                    return Err(BlockError::IoError);
                }
                return Ok(cqe);
            }

            spins += 1;
            if spins >= max_spins {
                return Err(BlockError::Timeout);
            }
            core::hint::spin_loop();
        }
    }
}

// ============================================================================
// Controller & Namespace Metadata
// ============================================================================

/// Discovered NVMe Controller status metadata for diagnostic reporting.
#[derive(Debug, Clone)]
pub struct NvmeControllerInfo {
    pub pci_bus: u8,
    pub pci_dev: u8,
    pub pci_func: u8,
    pub version_major: u16,
    pub version_minor: u8,
    pub model: String,
    pub serial: String,
    pub firmware: String,
    pub max_queue_entries: u16,
    pub doorbell_stride: usize,
    pub namespaces: u32,
    pub ns1_sectors: u64,
    pub ns1_sector_size: usize,
}

/// Global registry of initialized NVMe controllers.
static NVME_CONTROLLERS: Spinlock<Vec<NvmeControllerInfo>> = Spinlock::new(Vec::new());

/// Shared internal state of an initialized NVMe controller used by its namespaces.
#[allow(dead_code)]
struct NvmeControllerInner {
    mmio_base: usize,
    admin_queue: Spinlock<NvmeQueue>,
    io_queue: Spinlock<NvmeQueue>,
    dma_frame: PhysFrame,
}

#[allow(dead_code)]
impl NvmeControllerInner {
    #[inline]
    fn read_u32(&self, offset: usize) -> u32 {
        unsafe { read_volatile((self.mmio_base + offset) as *const u32) }
    }

    #[inline]
    fn write_u32(&self, offset: usize, val: u32) {
        unsafe { write_volatile((self.mmio_base + offset) as *mut u32, val) }
    }

    #[inline]
    fn read_u64(&self, offset: usize) -> u64 {
        let lo = self.read_u32(offset) as u64;
        let hi = self.read_u32(offset + 4) as u64;
        (hi << 32) | lo
    }

    #[inline]
    fn write_u64(&self, offset: usize, val: u64) {
        self.write_u32(offset, val as u32);
        self.write_u32(offset + 4, (val >> 32) as u32);
    }
}

/// BlockDevice implementation backed by a specific NVMe Namespace (`NSID`).
pub struct NvmeNamespaceDevice {
    dev_name: String,
    nsid: u32,
    sector_count: u64,
    sector_size: usize,
    controller: Arc<NvmeControllerInner>,
    /// Lock protecting the shared DMA bounce page + IO queue during synchronous commands.
    io_lock: Spinlock<()>,
}

impl NvmeNamespaceDevice {
    /// Performs an asynchronous-capable sector command on the NVMe I/O queue using the DMA buffer.
    fn execute_nvm_io(&self, opcode: u8, lba: u64, data: Option<&[u8]>, out: Option<&mut [u8]>) -> Result<(), BlockError> {
        if lba >= self.sector_count {
            return Err(BlockError::InvalidSector);
        }

        let _guard = self.io_lock.lock();
        let dma_phys = self.controller.dma_frame.start_address().as_u64();
        let dma_ptr = dma_phys as *mut u8;

        if let Some(src) = data {
            if src.len() < self.sector_size {
                return Err(BlockError::BufferTooSmall);
            }
            unsafe {
                core::ptr::copy_nonoverlapping(src.as_ptr(), dma_ptr, self.sector_size);
            }
        }

        let mut cmd = NvmeSqe::default();
        cmd.cdw0 = opcode as u32;
        cmd.nsid = self.nsid;
        cmd.prp1 = dma_phys;
        cmd.prp2 = 0;
        cmd.cdw10 = lba as u32;
        cmd.cdw11 = (lba >> 32) as u32;
        // NLB is 0-based count of logical blocks (0 -> 1 block)
        cmd.cdw12 = 0;

        {
            let mut q = self.controller.io_queue.lock();
            q.submit_and_wait(cmd)?;
        }

        if let Some(dst) = out {
            if dst.len() < self.sector_size {
                return Err(BlockError::BufferTooSmall);
            }
            unsafe {
                core::ptr::copy_nonoverlapping(dma_ptr, dst.as_mut_ptr(), self.sector_size);
            }
        }

        Ok(())
    }
}

impl BlockDevice for NvmeNamespaceDevice {
    fn name(&self) -> &str {
        &self.dev_name
    }

    fn sector_size(&self) -> usize {
        self.sector_size
    }

    fn sector_count(&self) -> u64 {
        self.sector_count
    }

    fn read_sector(&self, sector: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        self.execute_nvm_io(NVME_CMD_READ, sector, None, Some(buf))
    }

    fn write_sector(&self, sector: u64, buf: &[u8]) -> Result<(), BlockError> {
        self.execute_nvm_io(NVME_CMD_WRITE, sector, Some(buf), None)
    }

    fn flush(&self) -> Result<(), BlockError> {
        let _guard = self.io_lock.lock();
        let mut cmd = NvmeSqe::default();
        cmd.cdw0 = NVME_CMD_FLUSH as u32;
        cmd.nsid = self.nsid;
        let mut q = self.controller.io_queue.lock();
        q.submit_and_wait(cmd).map(|_| ())
    }
}

// ============================================================================
// Helper String Decoder for NVMe Identify Structures
// ============================================================================

fn extract_nvme_ascii(slice: &[u8]) -> String {
    let mut s = String::new();
    for &b in slice {
        if b == 0 {
            break;
        }
        if b >= 0x20 && b < 0x7F {
            s.push(b as char);
        }
    }
    String::from(s.trim())
}

// ============================================================================
// Controller Discovery & Initialization
// ============================================================================

/// Scans the PCI bus for NVMe controllers (Class 0x01, Subclass 0x08, ProgIF 0x02),
/// initializes their Admin & I/O Queue pairs, identifies namespaces, and registers
/// `BlockDevice` instances.
pub fn init() {
    println!();
    println!("----------------------------------------");
    println!(" [NVME] Initializing NVMe PCIe Drivers  ");
    println!("----------------------------------------");

    let devices = PCI_DEVICES.lock().clone();
    let mut ctrl_index = 0usize;

    for dev in devices {
        // Class 0x01 = Mass Storage, Subclass 0x08 = Non-Volatile Memory (NVMe)
        if dev.class_code != 0x01 || dev.subclass != 0x08 {
            continue;
        }

        // Locate BAR0 MMIO window
        let bar0 = match dev.bars.iter().find(|b| b.index == 0) {
            Some(b) => b,
            None => continue,
        };

        let (mmio_phys, mmio_size) = match bar0.bar_type {
            BarType::Memory32 { address, size, .. } => (address as u64, size),
            BarType::Memory64 { address, size, .. } => (address, size),
            BarType::Io { .. } => continue,
        };

        if mmio_phys == 0 {
            continue;
        }

        let map_size = core::cmp::max(mmio_size, 16384) as u64;

        println!(
            "[NVME] Found NVMe controller at PCI {:02x}:{:02x}.{} (Vendor {:04x}:{:04x}, BAR0 {:#x})",
            dev.address.bus, dev.address.device, dev.address.function,
            dev.vendor_id, dev.device_id, mmio_phys
        );

        dev.enable_bus_mastering();
        dev.enable_memory_space();

        if let Err(e) = memory::identity_map_range(
            PhysAddr::new(mmio_phys),
            map_size,
            PageTableFlags::PRESENT
                | PageTableFlags::WRITABLE
                | PageTableFlags::NO_CACHE
                | PageTableFlags::NO_EXECUTE,
        ) {
            println!("[NVME] Failed to map NVMe BAR0 MMIO: {}", e);
            continue;
        }

        let mmio_base = mmio_phys as usize;

        // Read CAP and VS registers
        let cap_lo = unsafe { read_volatile((mmio_base + NVME_REG_CAP) as *const u32) } as u64;
        let cap_hi = unsafe { read_volatile((mmio_base + NVME_REG_CAP + 4) as *const u32) } as u64;
        let cap = (cap_hi << 32) | cap_lo;

        let mqes = ((cap & 0xFFFF) as u16).saturating_add(1);
        let dstrd = ((cap >> 32) & 0x0F) as usize;
        let doorbell_stride = 4usize << dstrd;

        let vs = unsafe { read_volatile((mmio_base + NVME_REG_VS) as *const u32) };
        let ver_major = (vs >> 16) as u16;
        let ver_minor = ((vs >> 8) & 0xFF) as u8;

        // 1. Disable controller if currently enabled (CC.EN = 0) and wait for CSTS.RDY = 0
        let cc = unsafe { read_volatile((mmio_base + NVME_REG_CC) as *const u32) };
        if (cc & 1) != 0 {
            unsafe { write_volatile((mmio_base + NVME_REG_CC) as *mut u32, cc & !1) };
        }
        let mut timeout = 5_000_000u32;
        while (unsafe { read_volatile((mmio_base + NVME_REG_CSTS) as *const u32) } & 1) != 0 && timeout > 0 {
            timeout -= 1;
            core::hint::spin_loop();
        }
        if timeout == 0 {
            println!("[NVME] Timed out waiting for controller disable (CSTS.RDY=0).");
            continue;
        }

        // Mask all legacy interrupts (driver uses Phase Tag polling)
        unsafe { write_volatile((mmio_base + NVME_REG_INTMS) as *mut u32, 0xFFFF_FFFF) };

        // 2. Allocate Admin Queue Pair (QID 0)
        let queue_depth = core::cmp::min(NVME_QUEUE_DEPTH as u16, mqes);
        let admin_queue = match NvmeQueue::new(0, queue_depth, mmio_base, doorbell_stride) {
            Ok(q) => q,
            Err(_) => {
                println!("[NVME] Out of memory allocating Admin Queue Pair.");
                continue;
            }
        };

        let aqa = (((queue_depth - 1) as u32) << 16) | ((queue_depth - 1) as u32);
        unsafe {
            write_volatile((mmio_base + NVME_REG_AQA) as *mut u32, aqa);
            let asq = admin_queue.sq_phys();
            write_volatile((mmio_base + NVME_REG_ASQ) as *mut u32, asq as u32);
            write_volatile((mmio_base + NVME_REG_ASQ + 4) as *mut u32, (asq >> 32) as u32);
            let acq = admin_queue.cq_phys();
            write_volatile((mmio_base + NVME_REG_ACQ) as *mut u32, acq as u32);
            write_volatile((mmio_base + NVME_REG_ACQ + 4) as *mut u32, (acq >> 32) as u32);
        }

        // 3. Enable controller: IOCQES=4 (16B), IOSQES=6 (64B), MPS=0 (4K), CSS=0 (NVM), EN=1
        let new_cc: u32 = (4 << 20) | (6 << 16) | 1;
        fence(Ordering::SeqCst);
        unsafe { write_volatile((mmio_base + NVME_REG_CC) as *mut u32, new_cc) };

        timeout = 10_000_000;
        while (unsafe { read_volatile((mmio_base + NVME_REG_CSTS) as *const u32) } & 1) == 0 && timeout > 0 {
            timeout -= 1;
            core::hint::spin_loop();
        }
        if timeout == 0 {
            println!("[NVME] Timed out waiting for controller enable (CSTS.RDY=1).");
            continue;
        }

        let dma_frame = match memory::alloc_frame() {
            Some(f) => f,
            None => {
                println!("[NVME] Failed to allocate DMA buffer frame.");
                continue;
            }
        };
        let dma_phys = dma_frame.start_address().as_u64();
        let dma_slice = unsafe { core::slice::from_raw_parts_mut(dma_phys as *mut u8, PAGE_SIZE_4K as usize) };

        // 4. Identify Controller (CNS = 1)
        let mut admin_q = admin_queue;
        dma_slice.fill(0);
        let mut id_ctrl = NvmeSqe::default();
        id_ctrl.cdw0 = NVME_ADMIN_IDENTIFY as u32;
        id_ctrl.nsid = 0;
        id_ctrl.prp1 = dma_phys;
        id_ctrl.cdw10 = 1; // CNS = 1 (Identify Controller)

        if admin_q.submit_and_wait(id_ctrl).is_err() {
            println!("[NVME] Identify Controller command failed.");
            continue;
        }

        let serial = extract_nvme_ascii(&dma_slice[4..24]);
        let model = extract_nvme_ascii(&dma_slice[24..64]);
        let firmware = extract_nvme_ascii(&dma_slice[64..72]);
        let num_ns = u32::from_le_bytes([dma_slice[516], dma_slice[517], dma_slice[518], dma_slice[519]]);

        println!(
            "[NVME] nvme{} Online (NVMe {}.{}): Model=\"{}\" Serial=\"{}\" FW=\"{}\" Namespaces={}",
            ctrl_index, ver_major, ver_minor, model, serial, firmware, num_ns
        );

        // 5. Create I/O Completion Queue (QID = 1)
        let io_queue = match NvmeQueue::new(1, queue_depth, mmio_base, doorbell_stride) {
            Ok(q) => q,
            Err(_) => {
                println!("[NVME] Failed to allocate I/O Queue Pair.");
                continue;
            }
        };

        let mut create_cq = NvmeSqe::default();
        create_cq.cdw0 = NVME_ADMIN_CREATE_CQ as u32;
        create_cq.prp1 = io_queue.cq_phys();
        create_cq.cdw10 = (((queue_depth - 1) as u32) << 16) | 1; // QID=1
        create_cq.cdw11 = 1; // PC=1 (Physically Contiguous), IEN=0 (polling)
        if admin_q.submit_and_wait(create_cq).is_err() {
            println!("[NVME] Failed to create I/O Completion Queue.");
            continue;
        }

        // 6. Create I/O Submission Queue (QID = 1, mapped to CQID = 1)
        let mut create_sq = NvmeSqe::default();
        create_sq.cdw0 = NVME_ADMIN_CREATE_SQ as u32;
        create_sq.prp1 = io_queue.sq_phys();
        create_sq.cdw10 = (((queue_depth - 1) as u32) << 16) | 1; // QID=1
        create_sq.cdw11 = (1 << 16) | 1; // CQID=1, PC=1
        if admin_q.submit_and_wait(create_sq).is_err() {
            println!("[NVME] Failed to create I/O Submission Queue.");
            continue;
        }

        let ctrl_inner = Arc::new(NvmeControllerInner {
            mmio_base,
            admin_queue: Spinlock::new(admin_q),
            io_queue: Spinlock::new(io_queue),
            dma_frame,
        });

        // 7. Identify Namespaces (probe NSID 1..=min(num_ns, 4))
        let mut ns1_sectors = 0u64;
        let mut ns1_sector_size = SECTOR_SIZE;
        let max_ns = core::cmp::min(num_ns, 4);

        for nsid in 1..=max_ns {
            dma_slice.fill(0);
            let mut id_ns = NvmeSqe::default();
            id_ns.cdw0 = NVME_ADMIN_IDENTIFY as u32;
            id_ns.nsid = nsid;
            id_ns.prp1 = dma_phys;
            id_ns.cdw10 = 0; // CNS = 0 (Identify Namespace)

            let ok = {
                let mut aq = ctrl_inner.admin_queue.lock();
                aq.submit_and_wait(id_ns).is_ok()
            };
            if !ok {
                continue;
            }

            let nsze = u64::from_le_bytes([
                dma_slice[0], dma_slice[1], dma_slice[2], dma_slice[3],
                dma_slice[4], dma_slice[5], dma_slice[6], dma_slice[7],
            ]);
            if nsze == 0 {
                continue;
            }

            let flbas = (dma_slice[26] & 0x0F) as usize;
            let lbaf_offset = 128 + flbas * 4;
            let lbaf = u32::from_le_bytes([
                dma_slice[lbaf_offset],
                dma_slice[lbaf_offset + 1],
                dma_slice[lbaf_offset + 2],
                dma_slice[lbaf_offset + 3],
            ]);
            let lbads = ((lbaf >> 16) & 0xFF) as u32;
            let sec_size = if lbads >= 9 && lbads <= 12 { 1usize << lbads } else { SECTOR_SIZE };

            let ns_name = format!("nvme{}n{}", ctrl_index, nsid);
            let cap_mib = (nsze * sec_size as u64) / (1024 * 1024);
            println!(
                "[NVME] Registered namespace '{}' ({} sectors x {} bytes = {} MiB).",
                ns_name, nsze, sec_size, cap_mib
            );

            if nsid == 1 {
                ns1_sectors = nsze;
                ns1_sector_size = sec_size;
            }

            let ns_dev = Arc::new(NvmeNamespaceDevice {
                dev_name: ns_name,
                nsid,
                sector_count: nsze,
                sector_size: sec_size,
                controller: ctrl_inner.clone(),
                io_lock: Spinlock::new(()),
            });

            block::register_block_device(ns_dev);
        }

        NVME_CONTROLLERS.lock().push(NvmeControllerInfo {
            pci_bus: dev.address.bus,
            pci_dev: dev.address.device,
            pci_func: dev.address.function,
            version_major: ver_major,
            version_minor: ver_minor,
            model,
            serial,
            firmware,
            max_queue_entries: queue_depth,
            doorbell_stride,
            namespaces: num_ns,
            ns1_sectors,
            ns1_sector_size,
        });

        ctrl_index += 1;
    }

    if ctrl_index == 0 {
        println!("[NVME] No NVMe PCIe storage controllers detected on bus.");
    }
}

/// Returns a snapshot of all discovered NVMe controllers.
pub fn controllers() -> Vec<NvmeControllerInfo> {
    NVME_CONTROLLERS.lock().clone()
}

/// Runs NVMe subsystem hardware verification tests if an NVMe device is present.
pub fn run_self_tests() {
    let ctrls = controllers();
    if ctrls.is_empty() {
        println!("[NVME] Skipping hardware NVMe tests (no NVMe controller attached).");
        return;
    }

    if let Some(nvme_dev) = block::get_device("nvme0n1") {
        let mut orig = [0u8; 512];
        let mut read_back = [0u8; 512];
        let test_lba = if nvme_dev.sector_count() > 16 { 15 } else { 0 };

        // Preserve original contents of test LBA
        assert!(nvme_dev.read_sector(test_lba, &mut orig).is_ok(), "NVMe read failed");

        let mut pattern = [0u8; 512];
        for (i, b) in pattern.iter_mut().enumerate() {
            *b = ((i ^ 0xA5) & 0xFF) as u8;
        }
        pattern[0..12].copy_from_slice(b"NULLOS_NVME!");

        assert!(nvme_dev.write_sector(test_lba, &pattern).is_ok(), "NVMe write failed");
        assert!(nvme_dev.flush().is_ok(), "NVMe flush failed");
        assert!(nvme_dev.read_sector(test_lba, &mut read_back).is_ok(), "NVMe verify read failed");
        assert_eq!(&read_back[..], &pattern[..], "NVMe data integrity mismatch!");

        // Restore original sector
        let _ = nvme_dev.write_sector(test_lba, &orig);
        let _ = nvme_dev.flush();

        println!(
            "[NVME] Hardware DMA Read/Write/Flush verified on nvme0n1 (LBA {}, 512B).",
            test_lba
        );
    }
}
