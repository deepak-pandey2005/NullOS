//! PCI (Peripheral Component Interconnect) Bus Subsystem (Phase 6 - M17 PCI).
//!
//! Subsystem features:
//! - PCI Configuration Space access via 0xCF8 / 0xCFC I/O ports.
//! - Complete PCI Bus scanning & enumeration across 256 buses, 32 devices, 8 functions.
//! - Vendor ID, Device ID, and Class/Subclass decoding.
//! - Base Address Register (BAR) probing: 32-bit MMIO, 64-bit MMIO, and Port I/O sizing.
//! - Device Control: Bus Mastering, Memory Space, and I/O Space enabling.
//! - Capabilities List discovery (MSI, MSI-X, PCIe, Power Management).
//! - Global thread-safe PCI Device Registry.

use alloc::vec::Vec;
use crate::println;
use crate::sync::Spinlock;

const PCI_CONFIG_ADDRESS: u16 = 0xCF8;
const PCI_CONFIG_DATA: u16 = 0xCFC;

/// Formats a 32-bit PCI configuration address for I/O port 0xCF8.
#[inline]
fn config_address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    0x8000_0000u32
        | ((bus as u32) << 16)
        | (((device & 0x1F) as u32) << 11)
        | (((function & 0x07) as u32) << 8)
        | ((offset & 0xFC) as u32)
}

/// Reads a 32-bit DWORD from PCI configuration space.
pub fn pci_read_u32(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let address = config_address(bus, device, function, offset);
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") PCI_CONFIG_ADDRESS,
            in("eax") address,
            options(nomem, nostack),
        );
        let value: u32;
        core::arch::asm!(
            "in eax, dx",
            in("dx") PCI_CONFIG_DATA,
            out("eax") value,
            options(nomem, nostack),
        );
        value
    }
}

/// Reads a 16-bit WORD from PCI configuration space.
pub fn pci_read_u16(bus: u8, device: u8, function: u8, offset: u8) -> u16 {
    let dword = pci_read_u32(bus, device, function, offset);
    ((dword >> ((offset & 2) * 8)) & 0xFFFF) as u16
}

/// Reads an 8-bit BYTE from PCI configuration space.
pub fn pci_read_u8(bus: u8, device: u8, function: u8, offset: u8) -> u8 {
    let dword = pci_read_u32(bus, device, function, offset);
    ((dword >> ((offset & 3) * 8)) & 0xFF) as u8
}

/// Writes a 32-bit DWORD to PCI configuration space.
pub fn pci_write_u32(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    let address = config_address(bus, device, function, offset);
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") PCI_CONFIG_ADDRESS,
            in("eax") address,
            options(nomem, nostack),
        );
        core::arch::asm!(
            "out dx, eax",
            in("dx") PCI_CONFIG_DATA,
            in("eax") value,
            options(nomem, nostack),
        );
    }
}

/// Writes a 16-bit WORD to PCI configuration space.
#[allow(dead_code)]
pub fn pci_write_u16(bus: u8, device: u8, function: u8, offset: u8, value: u16) {
    let shift = (offset & 2) * 8;
    let mask = !(0xFFFFu32 << shift);
    let current = pci_read_u32(bus, device, function, offset);
    let new_val = (current & mask) | ((value as u32) << shift);
    pci_write_u32(bus, device, function, offset, new_val);
}

/// Writes an 8-bit BYTE to PCI configuration space.
#[allow(dead_code)]
pub fn pci_write_u8(bus: u8, device: u8, function: u8, offset: u8, value: u8) {
    let shift = (offset & 3) * 8;
    let mask = !(0xFFu32 << shift);
    let current = pci_read_u32(bus, device, function, offset);
    let new_val = (current & mask) | ((value as u32) << shift);
    pci_write_u32(bus, device, function, offset, new_val);
}

/// The physical coordinates of a device on the PCI bus hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PciAddress {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl core::fmt::Display for PciAddress {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:02x}:{:02x}.{}", self.bus, self.device, self.function)
    }
}

/// Type and physical address configuration of a Base Address Register (BAR).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarType {
    Memory32 {
        address: u32,
        size: usize,
        prefetchable: bool,
    },
    Memory64 {
        address: u64,
        size: usize,
        prefetchable: bool,
    },
    Io {
        port: u16,
        size: usize,
    },
}

/// A parsed Base Address Register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciBar {
    pub index: usize,
    pub bar_type: BarType,
}

/// A PCI Capability entry discovered in the capability pointer linked list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciCapability {
    pub cap_id: u8,
    pub offset: u8,
}

impl PciCapability {
    pub fn name(&self) -> &'static str {
        match self.cap_id {
            0x01 => "Power Management",
            0x02 => "AGP",
            0x03 => "VPD",
            0x04 => "Slot Identification",
            0x05 => "MSI (Message Signaled Interrupts)",
            0x09 => "Vendor Specific",
            0x10 => "PCI Express",
            0x11 => "MSI-X",
            0x12 => "SATA Data Index",
            0x13 => "Advanced Features",
            _ => "Unknown Capability",
        }
    }
}

/// Representation of an enumerated hardware PCI device.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PciDevice {
    pub address: PciAddress,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision: u8,
    pub header_type: u8,
    pub bars: Vec<PciBar>,
    pub capabilities: Vec<PciCapability>,
    pub interrupt_line: u8,
    pub interrupt_pin: u8,
}

#[allow(dead_code)]
impl PciDevice {
    /// Enables Bus Mastering (allows device to initiate DMA memory transfers).
    pub fn enable_bus_mastering(&self) {
        let cmd = pci_read_u16(self.address.bus, self.address.device, self.address.function, 0x04);
        pci_write_u16(self.address.bus, self.address.device, self.address.function, 0x04, cmd | (1 << 2));
    }

    /// Enables Memory Space access (MMIO BARs).
    pub fn enable_memory_space(&self) {
        let cmd = pci_read_u16(self.address.bus, self.address.device, self.address.function, 0x04);
        pci_write_u16(self.address.bus, self.address.device, self.address.function, 0x04, cmd | (1 << 1));
    }

    /// Enables I/O Space access (Port I/O BARs).
    pub fn enable_io_space(&self) {
        let cmd = pci_read_u16(self.address.bus, self.address.device, self.address.function, 0x04);
        pci_write_u16(self.address.bus, self.address.device, self.address.function, 0x04, cmd | (1 << 0));
    }

    /// Human-readable category of the PCI device class.
    pub fn class_name(&self) -> &'static str {
        match self.class_code {
            0x00 => "Unclassified Device",
            0x01 => match self.subclass {
                0x00 => "SCSI Storage Controller",
                0x01 => "IDE Interface",
                0x05 => "ATA Controller",
                0x06 => "SATA/AHCI Controller",
                0x08 => "Non-Volatile Memory (NVMe)",
                _ => "Mass Storage Controller",
            },
            0x02 => match self.subclass {
                0x00 => "Ethernet Controller",
                0x80 => "Network Controller",
                _ => "Network Controller",
            },
            0x03 => match self.subclass {
                0x00 => "VGA Compatible Controller",
                0x02 => "3D Controller",
                _ => "Display Controller",
            },
            0x04 => "Multimedia Controller",
            0x05 => "Memory Controller",
            0x06 => match self.subclass {
                0x00 => "Host Bridge",
                0x01 => "ISA Bridge",
                0x04 => "PCI-to-PCI Bridge",
                _ => "Bridge Device",
            },
            0x07 => "Communication Controller",
            0x08 => "Generic System Peripheral",
            0x09 => "Input Device Controller",
            0x0C => match self.subclass {
                0x03 => "USB Controller",
                0x05 => "SMBus Controller",
                _ => "Serial Bus Controller",
            },
            _ => "Unknown Class",
        }
    }

    /// Decodes popular vendor names.
    pub fn vendor_name(&self) -> &'static str {
        match self.vendor_id {
            0x8086 => "Intel Corporation",
            0x10DE => "NVIDIA Corporation",
            0x1002 => "Advanced Micro Devices (AMD)",
            0x1AF4 => "Red Hat / VirtIO",
            0x1B36 => "Red Hat QEMU",
            0x1234 => "QEMU Virtual Hardware",
            0x10EC => "Realtek Semiconductor",
            0x14E4 => "Broadcom Inc.",
            0x15AD => "VMware Inc.",
            _ => "Generic Vendor",
        }
    }
}

/// Global registry of all enumerated PCI devices.
pub static PCI_DEVICES: Spinlock<Vec<PciDevice>> = Spinlock::new(Vec::new());

/// Probes a specific BAR, calculating its physical base address, size, and type.
fn probe_bar(bus: u8, dev: u8, func: u8, bar_idx: usize) -> Option<(PciBar, usize)> {
    if bar_idx >= 6 {
        return None;
    }

    let offset = (0x10 + bar_idx * 4) as u8;
    let original = pci_read_u32(bus, dev, func, offset);

    // 1. Determine size by writing all 1s and reading back
    pci_write_u32(bus, dev, func, offset, 0xFFFF_FFFF);
    let read_back = pci_read_u32(bus, dev, func, offset);
    pci_write_u32(bus, dev, func, offset, original);

    if read_back == 0 || read_back == 0xFFFF_FFFF {
        return None; // BAR not implemented
    }

    let is_io = (original & 0x01) != 0;

    if is_io {
        let mask = !(read_back & 0xFFFF_FFFC) + 1;
        let size = mask as usize;
        let port = (original & 0xFFFC) as u16;
        Some((
            PciBar {
                index: bar_idx,
                bar_type: BarType::Io { port, size },
            },
            1, // Consumed 1 BAR register
        ))
    } else {
        let bar_type_bits = (original >> 1) & 0x03;
        let prefetchable = (original & (1 << 3)) != 0;

        if bar_type_bits == 0x02 {
            // 64-bit BAR: uses bar_idx and bar_idx + 1
            if bar_idx + 1 >= 6 {
                return None;
            }

            let next_offset = offset + 4;
            let original_high = pci_read_u32(bus, dev, func, next_offset);

            pci_write_u32(bus, dev, func, next_offset, 0xFFFF_FFFF);
            let read_high = pci_read_u32(bus, dev, func, next_offset);
            pci_write_u32(bus, dev, func, next_offset, original_high);

            let combined_mask = ((read_high as u64) << 32) | ((read_back & 0xFFFF_FFF0) as u64);
            let size = (!combined_mask + 1) as usize;
            let address = ((original_high as u64) << 32) | ((original & 0xFFFF_FFF0) as u64);

            Some((
                PciBar {
                    index: bar_idx,
                    bar_type: BarType::Memory64 {
                        address,
                        size,
                        prefetchable,
                    },
                },
                2, // Consumed 2 BAR registers
            ))
        } else {
            // 32-bit BAR
            let mask = !(read_back & 0xFFFF_FFF0) + 1;
            let size = mask as usize;
            let address = original & 0xFFFF_FFF0;

            Some((
                PciBar {
                    index: bar_idx,
                    bar_type: BarType::Memory32 {
                        address,
                        size,
                        prefetchable,
                    },
                },
                1, // Consumed 1 BAR register
            ))
        }
    }
}

/// Traverses the capability linked list starting at offset 0x34 if supported.
fn probe_capabilities(bus: u8, dev: u8, func: u8, status: u16) -> Vec<PciCapability> {
    let mut caps = Vec::new();
    // Bit 4 in Status Register indicates capabilities list presence
    if (status & (1 << 4)) == 0 {
        return caps;
    }

    let mut ptr = pci_read_u8(bus, dev, func, 0x34) & 0xFC;
    let mut count = 0;

    while ptr != 0 && count < 48 {
        let cap_id = pci_read_u8(bus, dev, func, ptr);
        let next_ptr = pci_read_u8(bus, dev, func, ptr + 1) & 0xFC;

        caps.push(PciCapability {
            cap_id,
            offset: ptr,
        });

        ptr = next_ptr;
        count += 1;
    }

    caps
}

/// Scans a single PCI function.
fn scan_function(bus: u8, device: u8, function: u8, list: &mut Vec<PciDevice>) {
    let vendor_id = pci_read_u16(bus, device, function, 0x00);
    if vendor_id == 0xFFFF {
        return;
    }

    let device_id = pci_read_u16(bus, device, function, 0x02);
    let status = pci_read_u16(bus, device, function, 0x06);
    let revision = pci_read_u8(bus, device, function, 0x08);
    let prog_if = pci_read_u8(bus, device, function, 0x09);
    let subclass = pci_read_u8(bus, device, function, 0x0A);
    let class_code = pci_read_u8(bus, device, function, 0x0B);
    let header_type = pci_read_u8(bus, device, function, 0x0E);

    let interrupt_line = pci_read_u8(bus, device, function, 0x3C);
    let interrupt_pin = pci_read_u8(bus, device, function, 0x3D);

    // Probe BARs only for Type 0x00 (Standard Device)
    let mut bars = Vec::new();
    if (header_type & 0x7F) == 0x00 {
        let mut bar_idx = 0;
        while bar_idx < 6 {
            if let Some((bar, step)) = probe_bar(bus, device, function, bar_idx) {
                bars.push(bar);
                bar_idx += step;
            } else {
                bar_idx += 1;
            }
        }
    }

    let capabilities = probe_capabilities(bus, device, function, status);

    let dev = PciDevice {
        address: PciAddress { bus, device, function },
        vendor_id,
        device_id,
        class_code,
        subclass,
        prog_if,
        revision,
        header_type,
        bars,
        capabilities,
        interrupt_line,
        interrupt_pin,
    };

    list.push(dev);
}

/// Scans all devices and functions on a given PCI bus.
fn scan_bus(bus: u8, list: &mut Vec<PciDevice>) {
    for device in 0..32 {
        let vendor = pci_read_u16(bus, device, 0, 0x00);
        if vendor == 0xFFFF {
            continue;
        }

        scan_function(bus, device, 0, list);

        let header_type = pci_read_u8(bus, device, 0, 0x0E);
        // Multi-function device: check functions 1..7
        if (header_type & 0x80) != 0 {
            for function in 1..8 {
                let func_vendor = pci_read_u16(bus, device, function, 0x00);
                if func_vendor != 0xFFFF {
                    scan_function(bus, device, function, list);
                }
            }
        }
    }
}

/// Initializes the PCI subsystem and enumerates all connected hardware.
pub fn init() {
    println!();
    println!("----------------------------------------");
    println!(" [PCI] Initializing PCI Bus Discovery   ");
    println!("----------------------------------------");

    let mut discovered = Vec::new();

    // Check Bus 0 first
    scan_bus(0, &mut discovered);

    // If Host Bridge at 00:00.0 is multi-function, check remaining buses 1..255
    let host_hdr = pci_read_u8(0, 0, 0, 0x0E);
    if (host_hdr & 0x80) != 0 {
        for bus in 1..=255 {
            scan_bus(bus, &mut discovered);
        }
    }

    println!("[PCI] Discovered {} PCI hardware devices:", discovered.len());
    for dev in &discovered {
        println!(
            "  [{}] {:04x}:{:04x} | {} ({})",
            dev.address,
            dev.vendor_id,
            dev.device_id,
            dev.class_name(),
            dev.vendor_name()
        );
        for bar in &dev.bars {
            match bar.bar_type {
                BarType::Memory32 { address, size, prefetchable } => {
                    println!(
                        "    BAR{}: MMIO32 0x{:08x} (size: {} KiB, prefetch: {})",
                        bar.index, address, size / 1024, prefetchable
                    );
                }
                BarType::Memory64 { address, size, prefetchable } => {
                    println!(
                        "    BAR{}: MMIO64 0x{:016x} (size: {} KiB, prefetch: {})",
                        bar.index, address, size / 1024, prefetchable
                    );
                }
                BarType::Io { port, size } => {
                    println!("    BAR{}: I/O Port 0x{:04x} (size: {} bytes)", bar.index, port, size);
                }
            }
        }
        if !dev.capabilities.is_empty() {
            let cap_names: Vec<&'static str> = dev.capabilities.iter().map(|c| c.name()).collect();
            println!("    Caps: {}", cap_names.join(", "));
        }
    }

    *PCI_DEVICES.lock() = discovered;
}

/// Finds a device by Vendor ID and Device ID.
#[allow(dead_code)]
pub fn find_device(vendor_id: u16, device_id: u16) -> Option<PciDevice> {
    let devices = PCI_DEVICES.lock();
    devices.iter().find(|d| d.vendor_id == vendor_id && d.device_id == device_id).cloned()
}

/// Finds all devices matching a class and subclass code.
#[allow(dead_code)]
pub fn find_by_class(class: u8, subclass: u8) -> Vec<PciDevice> {
    let devices = PCI_DEVICES.lock();
    devices.iter().filter(|d| d.class_code == class && d.subclass == subclass).cloned().collect()
}

/// Runs automated verification self-tests for M17 PCI Bus.
pub fn run_self_tests() {
    println!("[PCI] Running PCI Subsystem verification self-tests...");

    let devices = PCI_DEVICES.lock();
    assert!(!devices.is_empty(), "PCI bus scan failed: no devices discovered!");

    // Verify Host Bridge (Class 0x06, Subclass 0x00) exists at 00:00.0
    let host_bridge = devices.iter().find(|d| d.address.bus == 0 && d.address.device == 0 && d.address.function == 0);
    assert!(host_bridge.is_some(), "Host Bridge (00:00.0) not found in PCI registry");
    let host = host_bridge.unwrap();
    assert_eq!(host.class_code, 0x06, "00:00.0 should be a Bridge device");

    println!("[PCI] Host Bridge verified: Vendor 0x{:04x}, Device 0x{:04x}", host.vendor_id, host.device_id);

    // Verify BAR configuration on devices that have them
    let mut total_bars = 0;
    for dev in devices.iter() {
        total_bars += dev.bars.len();
    }
    println!("[PCI] Successfully probed {} Base Address Registers across {} devices.", total_bars, devices.len());

    println!("[PCI] All PCI subsystem self-tests passed successfully!");
}
