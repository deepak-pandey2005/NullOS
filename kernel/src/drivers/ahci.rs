//! # AHCI (Advanced Host Controller Interface) SATA Storage Driver
//!
//! ## Overview
//! Implements a bare-metal SATA storage driver conforming to the Serial ATA AHCI 1.3.1
//! specification for physical laptops, desktops, and QEMU `ich9-ahci` controllers.
//!
//! ### Features:
//! - PCI Class `0x01:0x06:0x01` (Mass Storage / SATA / AHCI 1.0) discovery via BAR5 (ABAR).
//! - HBA reset/enable (`GHC.AE`) and active port enumeration (`PI`, `PxSSTS`, `PxSIG`).
//! - 48-bit LBA DMA transfers (`READ DMA EXT`, `WRITE DMA EXT`, `FLUSH CACHE EXT`).
//! - `IDENTIFY DEVICE` (`0xEC`) capacity and model string parsing.
//! - Thread-safe [`BlockDevice`] registration (`ahci0`, `ahci1`, ...).

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};
use crate::drivers::block::{self, BlockDevice, BlockError, SECTOR_SIZE};
use crate::drivers::pci::{BarType, PCI_DEVICES};
use crate::memory::{self, PageTableFlags, PhysAddr};
use crate::println;
use crate::sync::Spinlock;

// HBA Generic Host Control Register Offsets
const HBA_CAP: usize = 0x00;
const HBA_GHC: usize = 0x04;
const HBA_IS: usize = 0x08;
const HBA_PI: usize = 0x0C;
const HBA_VS: usize = 0x10;

const GHC_AE: u32 = 1 << 31;

// Port Register Offsets (relative to ABAR + 0x100 + port * 0x80)
const PX_CLB: usize = 0x00;
const PX_CLBU: usize = 0x04;
const PX_FB: usize = 0x08;
const PX_FBU: usize = 0x0C;
const PX_IS: usize = 0x10;
const PX_IE: usize = 0x14;
const PX_CMD: usize = 0x18;
const PX_TFD: usize = 0x20;
const PX_SIG: usize = 0x24;
const PX_SSTS: usize = 0x28;
const PX_SERR: usize = 0x30;
const PX_CI: usize = 0x38;

const PX_CMD_ST: u32 = 1 << 0;
const PX_CMD_FRE: u32 = 1 << 4;
const PX_CMD_FR: u32 = 1 << 14;
const PX_CMD_CR: u32 = 1 << 15;

const PX_IS_TFES: u32 = 1 << 30;
const PX_TFD_ERR: u32 = 1 << 0;
const PX_TFD_DRQ: u32 = 1 << 3;
const PX_TFD_BSY: u32 = 1 << 7;

const SATA_SIG_ATA: u32 = 0x0000_0101;

// ATA Commands
const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
const ATA_CMD_WRITE_DMA_EXT: u8 = 0x35;
const ATA_CMD_FLUSH_CACHE_EXT: u8 = 0xEA;
const ATA_CMD_IDENTIFY: u8 = 0xEC;

const FIS_TYPE_REG_H2D: u8 = 0x27;

#[repr(C)]
#[derive(Clone, Copy)]
struct HbaCmdHeader {
    flags: u16,
    prdtl: u16,
    prdbc: u32,
    ctba: u32,
    ctbau: u32,
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct HbaPrdtEntry {
    dba: u32,
    dbau: u32,
    reserved: u32,
    dbc: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FisRegH2D {
    fis_type: u8,
    pmport_c: u8,
    command: u8,
    featurel: u8,
    lba0: u8,
    lba1: u8,
    lba2: u8,
    device: u8,
    lba3: u8,
    lba4: u8,
    lba5: u8,
    featureh: u8,
    countl: u8,
    counth: u8,
    icc: u8,
    control: u8,
    reserved: [u8; 4],
}

struct AhciPortState {
    port_base: usize,
    clb_phys: u64,
    _fb_phys: u64,
    ctba_phys: u64,
    dma_buf_phys: u64,
}

pub struct AhciDevice {
    name: String,
    model: String,
    sectors: u64,
    port: Spinlock<AhciPortState>,
}

unsafe impl Send for AhciDevice {}
unsafe impl Sync for AhciDevice {}

static AHCI_DEVICES: Spinlock<Vec<Arc<AhciDevice>>> = Spinlock::new(Vec::new());

#[inline]
fn mmio_read32(addr: usize) -> u32 {
    unsafe { read_volatile(addr as *const u32) }
}

#[inline]
fn mmio_write32(addr: usize, val: u32) {
    unsafe { write_volatile(addr as *mut u32, val) }
}

fn stop_port_cmd(port_base: usize) -> bool {
    let mut cmd = mmio_read32(port_base + PX_CMD);
    cmd &= !(PX_CMD_ST | PX_CMD_FRE);
    mmio_write32(port_base + PX_CMD, cmd);

    for _ in 0..1_000_000 {
        let cur = mmio_read32(port_base + PX_CMD);
        if (cur & (PX_CMD_FR | PX_CMD_CR)) == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

fn start_port_cmd(port_base: usize) {
    for _ in 0..1_000_000 {
        if (mmio_read32(port_base + PX_CMD) & PX_CMD_CR) == 0 {
            break;
        }
        core::hint::spin_loop();
    }
    let cmd = mmio_read32(port_base + PX_CMD);
    mmio_write32(port_base + PX_CMD, cmd | PX_CMD_FRE);
    mmio_write32(port_base + PX_CMD, cmd | PX_CMD_FRE | PX_CMD_ST);
}

impl AhciPortState {
    fn wait_ready(&self) -> Result<(), BlockError> {
        for _ in 0..2_000_000 {
            let tfd = mmio_read32(self.port_base + PX_TFD);
            if (tfd & (PX_TFD_BSY | PX_TFD_DRQ)) == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(BlockError::Timeout)
    }

    fn issue_ata_cmd(
        &mut self,
        command: u8,
        lba: u64,
        sector_count: u16,
        is_write: bool,
        use_dma_buf: bool,
        device_reg: u8,
    ) -> Result<(), BlockError> {
        self.wait_ready()?;

        mmio_write32(self.port_base + PX_IS, 0xFFFF_FFFF);

        // Configure Command Header slot 0
        let hdr_ptr = self.clb_phys as *mut HbaCmdHeader;
        let cfl_dwords = (core::mem::size_of::<FisRegH2D>() / 4) as u16;
        let write_flag = if is_write { 1u16 << 6 } else { 0 };
        let prdtl = if use_dma_buf { 1u16 } else { 0u16 };

        unsafe {
            write_volatile(
                hdr_ptr,
                HbaCmdHeader {
                    flags: cfl_dwords | write_flag,
                    prdtl,
                    prdbc: 0,
                    ctba: self.ctba_phys as u32,
                    ctbau: (self.ctba_phys >> 32) as u32,
                    reserved: [0; 4],
                },
            );
        }

        // Zero Command Table (header + PRDT)
        unsafe {
            core::ptr::write_bytes(self.ctba_phys as *mut u8, 0, 256);
        }

        // Build H2D Register FIS at offset 0 of Command Table
        let fis_ptr = self.ctba_phys as *mut FisRegH2D;
        unsafe {
            write_volatile(
                fis_ptr,
                FisRegH2D {
                    fis_type: FIS_TYPE_REG_H2D,
                    pmport_c: 0x80,
                    command,
                    featurel: 0,
                    lba0: (lba & 0xFF) as u8,
                    lba1: ((lba >> 8) & 0xFF) as u8,
                    lba2: ((lba >> 16) & 0xFF) as u8,
                    device: device_reg,
                    lba3: ((lba >> 24) & 0xFF) as u8,
                    lba4: ((lba >> 32) & 0xFF) as u8,
                    lba5: ((lba >> 40) & 0xFF) as u8,
                    featureh: 0,
                    countl: (sector_count & 0xFF) as u8,
                    counth: ((sector_count >> 8) & 0xFF) as u8,
                    icc: 0,
                    control: 0,
                    reserved: [0; 4],
                },
            );
        }

        if use_dma_buf {
            let byte_count = (sector_count as u32).max(1) * (SECTOR_SIZE as u32);
            let prdt_ptr = (self.ctba_phys + 0x80) as *mut HbaPrdtEntry;
            unsafe {
                write_volatile(
                    prdt_ptr,
                    HbaPrdtEntry {
                        dba: self.dma_buf_phys as u32,
                        dbau: (self.dma_buf_phys >> 32) as u32,
                        reserved: 0,
                        dbc: byte_count - 1,
                    },
                );
            }
        }

        // Issue command in slot 0
        mmio_write32(self.port_base + PX_CI, 1);

        // Poll for completion
        for _ in 0..10_000_000 {
            let ci = mmio_read32(self.port_base + PX_CI);
            let is = mmio_read32(self.port_base + PX_IS);
            if (is & PX_IS_TFES) != 0 {
                return Err(BlockError::IoError);
            }
            if (ci & 1) == 0 {
                let tfd = mmio_read32(self.port_base + PX_TFD);
                if (tfd & PX_TFD_ERR) != 0 {
                    return Err(BlockError::IoError);
                }
                return Ok(());
            }
            core::hint::spin_loop();
        }

        Err(BlockError::Timeout)
    }

    fn identify(&mut self) -> Result<(u64, String), BlockError> {
        unsafe {
            core::ptr::write_bytes(self.dma_buf_phys as *mut u8, 0, SECTOR_SIZE);
        }
        self.issue_ata_cmd(ATA_CMD_IDENTIFY, 0, 1, false, true, 0)?;

        let words = unsafe { core::slice::from_raw_parts(self.dma_buf_phys as *const u16, 256) };

        // LBA48 sector count in words 100..103
        let lba48 = (words[100] as u64)
            | ((words[101] as u64) << 16)
            | ((words[102] as u64) << 32)
            | ((words[103] as u64) << 48);

        // Fallback to LBA28 sector count in words 60..61
        let lba28 = (words[60] as u64) | ((words[61] as u64) << 16);
        let sectors = if lba48 != 0 { lba48 } else { lba28 };

        // Model string in words 27..46 (byte-swapped ASCII pairs)
        let mut model_bytes = [0u8; 40];
        for i in 0..20 {
            let w = words[27 + i];
            model_bytes[i * 2] = (w >> 8) as u8;
            model_bytes[i * 2 + 1] = (w & 0xFF) as u8;
        }
        let model = core::str::from_utf8(&model_bytes)
            .unwrap_or("SATA Drive")
            .trim()
            .into();

        Ok((sectors, model))
    }
}

impl BlockDevice for AhciDevice {
    fn name(&self) -> &str {
        &self.name
    }

    fn sector_count(&self) -> u64 {
        self.sectors
    }

    fn read_sector(&self, sector: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if buf.len() < SECTOR_SIZE {
            return Err(BlockError::BufferTooSmall);
        }
        if sector >= self.sectors {
            return Err(BlockError::InvalidSector);
        }

        let mut port = self.port.lock();
        port.issue_ata_cmd(ATA_CMD_READ_DMA_EXT, sector, 1, false, true, 0x40)?;
        unsafe {
            core::ptr::copy_nonoverlapping(
                port.dma_buf_phys as *const u8,
                buf.as_mut_ptr(),
                SECTOR_SIZE,
            );
        }
        Ok(())
    }

    fn write_sector(&self, sector: u64, buf: &[u8]) -> Result<(), BlockError> {
        if buf.len() < SECTOR_SIZE {
            return Err(BlockError::BufferTooSmall);
        }
        if sector >= self.sectors {
            return Err(BlockError::InvalidSector);
        }

        let mut port = self.port.lock();
        unsafe {
            core::ptr::copy_nonoverlapping(
                buf.as_ptr(),
                port.dma_buf_phys as *mut u8,
                SECTOR_SIZE,
            );
        }
        port.issue_ata_cmd(ATA_CMD_WRITE_DMA_EXT, sector, 1, true, true, 0x40)
    }

    fn flush(&self) -> Result<(), BlockError> {
        let mut port = self.port.lock();
        port.issue_ata_cmd(ATA_CMD_FLUSH_CACHE_EXT, 0, 0, false, false, 0x40)
    }
}

/// Discovers PCI AHCI SATA controllers and registers attached SATA drives.
pub fn init() {
    println!("----------------------------------------");
    println!(" [AHCI] Probing SATA/AHCI Controllers   ");
    println!("----------------------------------------");

    let devices = PCI_DEVICES.lock().clone();
    let mut drive_index = 0usize;

    for dev in devices {
        // Class 0x01 = Mass Storage, Subclass 0x06 = SATA, ProgIF 0x01 = AHCI 1.0
        if dev.class_code != 0x01 || dev.subclass != 0x06 || dev.prog_if != 0x01 {
            continue;
        }

        let bar5 = match dev.bars.iter().find(|b| b.index == 5) {
            Some(b) => b,
            None => continue,
        };

        let (abar_phys, abar_size) = match bar5.bar_type {
            BarType::Memory32 { address, size, .. } => (address as u64, size),
            BarType::Memory64 { address, size, .. } => (address, size),
            BarType::Io { .. } => continue,
        };

        if abar_phys == 0 {
            continue;
        }

        let map_size = core::cmp::max(abar_size, 8192) as u64;
        dev.enable_bus_mastering();
        dev.enable_memory_space();

        if let Err(e) = memory::identity_map_range(
            PhysAddr::new(abar_phys),
            map_size,
            PageTableFlags::PRESENT
                | PageTableFlags::WRITABLE
                | PageTableFlags::NO_CACHE
                | PageTableFlags::NO_EXECUTE,
        ) {
            println!("[AHCI] Failed to map ABAR MMIO: {}", e);
            continue;
        }

        let abar = abar_phys as usize;
        let ghc = mmio_read32(abar + HBA_GHC);
        mmio_write32(abar + HBA_GHC, ghc | GHC_AE);
        mmio_write32(abar + HBA_IS, 0xFFFF_FFFF);

        let vs = mmio_read32(abar + HBA_VS);
        let cap = mmio_read32(abar + HBA_CAP);
        let pi = mmio_read32(abar + HBA_PI);
        let ncs = ((cap >> 8) & 0x1F) + 1;

        println!(
            "[AHCI] Controller at PCI {:02x}:{:02x}.{} ({:04x}:{:04x}) ABAR={:#x} AHCI={}.{} Slots={} PI={:#x}",
            dev.address.bus,
            dev.address.device,
            dev.address.function,
            dev.vendor_id,
            dev.device_id,
            abar_phys,
            (vs >> 16) & 0xFFFF,
            vs & 0xFFFF,
            ncs,
            pi
        );

        for port_num in 0..32u32 {
            if (pi & (1 << port_num)) == 0 {
                continue;
            }

            let port_base = abar + 0x100 + (port_num as usize) * 0x80;
            let ssts = mmio_read32(port_base + PX_SSTS);
            let det = ssts & 0x0F;
            let ipm = (ssts >> 8) & 0x0F;
            if det != 3 || ipm != 1 {
                continue;
            }

            let sig = mmio_read32(port_base + PX_SIG);
            if sig != SATA_SIG_ATA {
                continue;
            }

            if !stop_port_cmd(port_base) {
                println!("[AHCI] Port {} failed to stop command engine.", port_num);
                continue;
            }

            let clb_frame = match memory::alloc_frame() {
                Some(f) => f.start_address().as_u64(),
                None => continue,
            };
            let fb_frame = match memory::alloc_frame() {
                Some(f) => f.start_address().as_u64(),
                None => continue,
            };
            let ctba_frame = match memory::alloc_frame() {
                Some(f) => f.start_address().as_u64(),
                None => continue,
            };
            let dma_frame = match memory::alloc_frame() {
                Some(f) => f.start_address().as_u64(),
                None => continue,
            };

            unsafe {
                core::ptr::write_bytes(clb_frame as *mut u8, 0, 4096);
                core::ptr::write_bytes(fb_frame as *mut u8, 0, 4096);
                core::ptr::write_bytes(ctba_frame as *mut u8, 0, 4096);
                core::ptr::write_bytes(dma_frame as *mut u8, 0, 4096);
            }

            mmio_write32(port_base + PX_CLB, clb_frame as u32);
            mmio_write32(port_base + PX_CLBU, (clb_frame >> 32) as u32);
            mmio_write32(port_base + PX_FB, fb_frame as u32);
            mmio_write32(port_base + PX_FBU, (fb_frame >> 32) as u32);
            mmio_write32(port_base + PX_IE, 0);
            mmio_write32(port_base + PX_SERR, 0xFFFF_FFFF);
            mmio_write32(port_base + PX_IS, 0xFFFF_FFFF);

            start_port_cmd(port_base);

            let mut port_state = AhciPortState {
                port_base,
                clb_phys: clb_frame,
                _fb_phys: fb_frame,
                ctba_phys: ctba_frame,
                dma_buf_phys: dma_frame,
            };

            match port_state.identify() {
                Ok((sectors, model)) if sectors > 0 => {
                    let name = format!("ahci{}", drive_index);
                    let mib = (sectors * (SECTOR_SIZE as u64)) / (1024 * 1024);
                    println!(
                        "[AHCI] Port {}: Registered '{}' (Model=\"{}\", {} sectors = {} MiB).",
                        port_num, name, model, sectors, mib
                    );
                    let dev_arc = Arc::new(AhciDevice {
                        name,
                        model,
                        sectors,
                        port: Spinlock::new(port_state),
                    });
                    AHCI_DEVICES.lock().push(dev_arc.clone());
                    block::register_block_device(dev_arc);
                    drive_index += 1;
                }
                Ok(_) => {
                    println!("[AHCI] Port {}: SATA device reported 0 sectors.", port_num);
                }
                Err(e) => {
                    println!("[AHCI] Port {}: IDENTIFY failed ({:?}).", port_num, e);
                }
            }
        }
    }

    if drive_index == 0 {
        println!("[AHCI] No active SATA drives attached.");
    }
}

/// Verifies non-destructive sector 0 read on any discovered AHCI SATA drives.
pub fn run_self_tests() {
    let devs = AHCI_DEVICES.lock().clone();
    for dev in devs {
        let mut buf = [0u8; SECTOR_SIZE];
        match dev.read_sector(0, &mut buf) {
            Ok(()) => {
                println!(
                    "[AHCI] Self-test passed for '{}' (Model=\"{}\", LBA 0 read 512B OK).",
                    dev.name(),
                    dev.model
                );
            }
            Err(e) => {
                println!("[AHCI] Self-test failed for '{}': {:?}", dev.name(), e);
            }
        }
    }
}
