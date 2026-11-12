//! # Intel e1000 / e1000e Gigabit Ethernet Driver
//!
//! ## Overview
//! Bare-metal PCI/PCIe Gigabit Ethernet driver supporting Intel 8254x (`e1000`) and
//! 8257x/I21x (`e1000e`) controllers found on physical laptops/desktops and QEMU.

use core::ptr::{read_volatile, write_volatile};

use crate::drivers::pci::{BarType, PciDevice, PCI_DEVICES};
use crate::drivers::virtio_net::{MacAddress, NetStats};
use crate::memory::{self, PageTableFlags, PhysAddr};
use crate::println;
use crate::sync::Spinlock;

const REG_CTRL: usize = 0x0000;
const REG_STATUS: usize = 0x0008;
const REG_EERD: usize = 0x0014;
const REG_IMC: usize = 0x00D8;
const REG_RCTL: usize = 0x0100;
const REG_TCTL: usize = 0x0400;
const REG_RDBAL: usize = 0x2800;
const REG_RDBAH: usize = 0x2804;
const REG_RDLEN: usize = 0x2808;
const REG_RDH: usize = 0x2810;
const REG_RDT: usize = 0x2818;
const REG_TDBAL: usize = 0x3800;
const REG_TDBAH: usize = 0x3804;
const REG_TDLEN: usize = 0x3808;
const REG_TDH: usize = 0x3810;
const REG_TDT: usize = 0x3818;
const REG_MTA: usize = 0x5200;
const REG_RAL0: usize = 0x5400;
const REG_RAH0: usize = 0x5404;

const CTRL_ASDE: u32 = 1 << 5;
const CTRL_SLU: u32 = 1 << 6;
const CTRL_RST: u32 = 1 << 26;

const RCTL_EN: u32 = 1 << 1;
const RCTL_BAM: u32 = 1 << 15;
const RCTL_SECRC: u32 = 1 << 26;

const TCTL_EN: u32 = 1 << 1;
const TCTL_PSP: u32 = 1 << 3;

const RX_DESC_COUNT: usize = 32;
const TX_DESC_COUNT: usize = 32;
const PKT_BUF_SIZE: usize = 2048;

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct E1000RxDesc {
    addr: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct E1000TxDesc {
    addr: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

pub struct E1000Device {
    mmio_base: usize,
    mac: MacAddress,
    rx_ring_phys: u64,
    tx_ring_phys: u64,
    rx_bufs: [u64; RX_DESC_COUNT],
    tx_bufs: [u64; TX_DESC_COUNT],
    rx_cur: usize,
    tx_tail: usize,
    pub stats: NetStats,
}

unsafe impl Send for E1000Device {}
unsafe impl Sync for E1000Device {}

pub static E1000_DEVICE: Spinlock<Option<E1000Device>> = Spinlock::new(None);

#[inline]
fn mmio_read32(base: usize, reg: usize) -> u32 {
    unsafe { read_volatile((base + reg) as *const u32) }
}

#[inline]
fn mmio_write32(base: usize, reg: usize, val: u32) {
    unsafe { write_volatile((base + reg) as *mut u32, val) }
}

fn read_eeprom(base: usize, addr: u8) -> Option<u16> {
    mmio_write32(base, REG_EERD, ((addr as u32) << 8) | 1);
    for _ in 0..100_000 {
        let val = mmio_read32(base, REG_EERD);
        if (val & (1 << 4)) != 0 || (val & (1 << 1)) != 0 {
            return Some((val >> 16) as u16);
        }
        core::hint::spin_loop();
    }
    None
}

fn alloc_packet_buffers<const N: usize>() -> Result<[u64; N], &'static str> {
    let mut bufs = [0u64; N];
    let mut i = 0;
    while i < N {
        let frame = memory::alloc_frame().ok_or("Out of memory for e1000 packet buffers")?;
        let phys = frame.start_address().as_u64();
        unsafe {
            core::ptr::write_bytes(phys as *mut u8, 0, 4096);
        }
        bufs[i] = phys;
        if i + 1 < N {
            bufs[i + 1] = phys + (PKT_BUF_SIZE as u64);
        }
        i += 2;
    }
    Ok(bufs)
}

impl E1000Device {
    fn probe(pci_dev: &PciDevice) -> Result<Self, &'static str> {
        let bar0 = pci_dev
            .bars
            .iter()
            .find(|b| b.index == 0)
            .ok_or("Missing BAR0 on Intel NIC")?;

        let (mmio_phys, mmio_size) = match bar0.bar_type {
            BarType::Memory32 { address, size, .. } => (address as u64, size),
            BarType::Memory64 { address, size, .. } => (address, size),
            BarType::Io { .. } => return Err("Intel NIC BAR0 is I/O space"),
        };

        if mmio_phys == 0 {
            return Err("Intel NIC BAR0 physical address is 0");
        }

        let map_size = core::cmp::max(mmio_size, 128 * 1024) as u64;
        pci_dev.enable_bus_mastering();
        pci_dev.enable_memory_space();

        memory::identity_map_range(
            PhysAddr::new(mmio_phys),
            map_size,
            PageTableFlags::PRESENT
                | PageTableFlags::WRITABLE
                | PageTableFlags::NO_CACHE
                | PageTableFlags::NO_EXECUTE,
        )?;

        let base = mmio_phys as usize;

        // Disable interrupts and reset controller
        mmio_write32(base, REG_IMC, 0xFFFF_FFFF);
        let ctrl = mmio_read32(base, REG_CTRL);
        mmio_write32(base, REG_CTRL, ctrl | CTRL_RST);
        for _ in 0..1_000_000 {
            if (mmio_read32(base, REG_CTRL) & CTRL_RST) == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        mmio_write32(base, REG_IMC, 0xFFFF_FFFF);
        mmio_write32(base, REG_CTRL, CTRL_SLU | CTRL_ASDE);

        // Read MAC address from RAL0/RAH0 or fallback to EEPROM
        let ral = mmio_read32(base, REG_RAL0);
        let rah = mmio_read32(base, REG_RAH0);
        let mac_bytes = if (ral != 0 || (rah & 0xFFFF) != 0) && (rah & (1 << 31)) != 0 {
            [
                (ral & 0xFF) as u8,
                ((ral >> 8) & 0xFF) as u8,
                ((ral >> 16) & 0xFF) as u8,
                ((ral >> 24) & 0xFF) as u8,
                (rah & 0xFF) as u8,
                ((rah >> 8) & 0xFF) as u8,
            ]
        } else {
            let w0 = read_eeprom(base, 0).unwrap_or(0x5254);
            let w1 = read_eeprom(base, 1).unwrap_or(0x0012);
            let w2 = read_eeprom(base, 2).unwrap_or(0x3456);
            [
                (w0 & 0xFF) as u8,
                (w0 >> 8) as u8,
                (w1 & 0xFF) as u8,
                (w1 >> 8) as u8,
                (w2 & 0xFF) as u8,
                (w2 >> 8) as u8,
            ]
        };

        let ral_val = (mac_bytes[0] as u32)
            | ((mac_bytes[1] as u32) << 8)
            | ((mac_bytes[2] as u32) << 16)
            | ((mac_bytes[3] as u32) << 24);
        let rah_val = (mac_bytes[4] as u32) | ((mac_bytes[5] as u32) << 8) | (1 << 31);
        mmio_write32(base, REG_RAL0, ral_val);
        mmio_write32(base, REG_RAH0, rah_val);

        for i in 0..128 {
            mmio_write32(base, REG_MTA + i * 4, 0);
        }

        // Allocate RX/TX descriptor rings and packet buffers
        let rx_ring_frame = memory::alloc_frame().ok_or("Out of memory for e1000 RX ring")?;
        let tx_ring_frame = memory::alloc_frame().ok_or("Out of memory for e1000 TX ring")?;
        let rx_ring_phys = rx_ring_frame.start_address().as_u64();
        let tx_ring_phys = tx_ring_frame.start_address().as_u64();

        unsafe {
            core::ptr::write_bytes(rx_ring_phys as *mut u8, 0, 4096);
            core::ptr::write_bytes(tx_ring_phys as *mut u8, 0, 4096);
        }

        let rx_bufs: [u64; RX_DESC_COUNT] = alloc_packet_buffers()?;
        let tx_bufs: [u64; TX_DESC_COUNT] = alloc_packet_buffers()?;

        let rx_descs = unsafe {
            core::slice::from_raw_parts_mut(rx_ring_phys as *mut E1000RxDesc, RX_DESC_COUNT)
        };
        for i in 0..RX_DESC_COUNT {
            rx_descs[i].addr = rx_bufs[i];
            rx_descs[i].status = 0;
        }

        // Program RX registers
        mmio_write32(base, REG_RDBAL, rx_ring_phys as u32);
        mmio_write32(base, REG_RDBAH, (rx_ring_phys >> 32) as u32);
        mmio_write32(base, REG_RDLEN, (RX_DESC_COUNT * 16) as u32);
        mmio_write32(base, REG_RDH, 0);
        mmio_write32(base, REG_RDT, (RX_DESC_COUNT - 1) as u32);
        mmio_write32(base, REG_RCTL, RCTL_EN | RCTL_BAM | RCTL_SECRC);

        // Program TX registers
        mmio_write32(base, REG_TDBAL, tx_ring_phys as u32);
        mmio_write32(base, REG_TDBAH, (tx_ring_phys >> 32) as u32);
        mmio_write32(base, REG_TDLEN, (TX_DESC_COUNT * 16) as u32);
        mmio_write32(base, REG_TDH, 0);
        mmio_write32(base, REG_TDT, 0);
        mmio_write32(base, REG_TCTL, TCTL_EN | TCTL_PSP | (0x10 << 4) | (0x40 << 12));

        let _link_up = (mmio_read32(base, REG_STATUS) & (1 << 1)) != 0;

        Ok(Self {
            mmio_base: base,
            mac: MacAddress(mac_bytes),
            rx_ring_phys,
            tx_ring_phys,
            rx_bufs,
            tx_bufs,
            rx_cur: 0,
            tx_tail: 0,
            stats: NetStats::default(),
        })
    }

    pub fn mac(&self) -> MacAddress {
        self.mac
    }

    pub fn send_packet(&mut self, packet: &[u8]) -> Result<(), &'static str> {
        if packet.is_empty() || packet.len() > PKT_BUF_SIZE {
            return Err("Invalid packet length");
        }

        let idx = self.tx_tail;
        let buf_phys = self.tx_bufs[idx];
        unsafe {
            core::ptr::copy_nonoverlapping(packet.as_ptr(), buf_phys as *mut u8, packet.len());
        }

        let tx_descs = unsafe {
            core::slice::from_raw_parts_mut(self.tx_ring_phys as *mut E1000TxDesc, TX_DESC_COUNT)
        };
        tx_descs[idx] = E1000TxDesc {
            addr: buf_phys,
            length: packet.len() as u16,
            cso: 0,
            cmd: 0x0B, // EOP (1) | IFCS (2) | RS (8)
            status: 0,
            css: 0,
            special: 0,
        };

        self.tx_tail = (idx + 1) % TX_DESC_COUNT;
        mmio_write32(self.mmio_base, REG_TDT, self.tx_tail as u32);

        for _ in 0..1_000_000 {
            let st = unsafe { read_volatile(&tx_descs[idx].status) };
            if (st & 0x01) != 0 {
                self.stats.tx_packets += 1;
                self.stats.tx_bytes += packet.len();
                return Ok(());
            }
            core::hint::spin_loop();
        }

        Err("e1000 TX timeout")
    }

    pub fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, &'static str> {
        let rx_descs = unsafe {
            core::slice::from_raw_parts_mut(self.rx_ring_phys as *mut E1000RxDesc, RX_DESC_COUNT)
        };
        let idx = self.rx_cur;
        let status = unsafe { read_volatile(&rx_descs[idx].status) };
        if (status & 0x01) == 0 {
            return Ok(None);
        }

        let len = (unsafe { read_volatile(&rx_descs[idx].length) } as usize).min(buf.len());
        let pkt_phys = self.rx_bufs[idx];
        unsafe {
            core::ptr::copy_nonoverlapping(pkt_phys as *const u8, buf.as_mut_ptr(), len);
        }

        rx_descs[idx].status = 0;
        self.rx_cur = (idx + 1) % RX_DESC_COUNT;
        mmio_write32(self.mmio_base, REG_RDT, idx as u32);

        self.stats.rx_packets += 1;
        self.stats.rx_bytes += len;

        Ok(Some(len))
    }
}

/// Probes PCI bus for Intel e1000 / e1000e Gigabit Ethernet controllers.
pub fn init() {
    let devices = PCI_DEVICES.lock().clone();
    for dev in devices {
        if dev.vendor_id == 0x8086 && dev.class_code == 0x02 && dev.subclass == 0x00 {
            println!(
                "[E1000] Found Intel Ethernet at PCI {:02x}:{:02x}.{} ({:04x}:{:04x})",
                dev.address.bus,
                dev.address.device,
                dev.address.function,
                dev.vendor_id,
                dev.device_id
            );
            match E1000Device::probe(&dev) {
                Ok(nic) => {
                    println!("[E1000] Initialized 'eth0': Hardware MAC: {}", nic.mac());
                    *E1000_DEVICE.lock() = Some(nic);
                    return;
                }
                Err(e) => {
                    println!("[E1000] Probe failed: {}", e);
                }
            }
        }
    }
}
