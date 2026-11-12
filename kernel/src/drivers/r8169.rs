//! # Realtek r8169 / RTL8168 / RTL8111 Gigabit Ethernet Driver
//!
//! ## Overview
//! Bare-metal PCIe Gigabit Ethernet driver for Realtek RTL8168/RTL8111/RTL8169/RTL8125
//! controllers commonly found on physical laptops and motherboards.

use core::ptr::{read_volatile, write_volatile};

use crate::drivers::pci::{BarType, PciDevice, PCI_DEVICES};
use crate::drivers::virtio_net::{MacAddress, NetStats};
use crate::memory::{self, PageTableFlags, PhysAddr};
use crate::println;
use crate::sync::Spinlock;

const REG_IDR0: usize = 0x00;
const REG_TX_DESC_LOW: usize = 0x20;
const REG_TX_DESC_HIGH: usize = 0x24;
const REG_CHIP_CMD: usize = 0x37;
const REG_TX_POLL: usize = 0x38;
const REG_IMR: usize = 0x3C;
const REG_ISR: usize = 0x3E;
const REG_TX_CONFIG: usize = 0x40;
const REG_RX_CONFIG: usize = 0x44;
const REG_CFG9346: usize = 0x50;
const REG_RX_MAX_SIZE: usize = 0xDA;
const REG_RX_DESC_LOW: usize = 0xE4;
const REG_RX_DESC_HIGH: usize = 0xE8;
const REG_TX_MAX_SIZE: usize = 0xEC;

const CMD_TX_ENABLE: u8 = 1 << 2;
const CMD_RX_ENABLE: u8 = 1 << 3;
const CMD_RESET: u8 = 1 << 4;

const DESC_OWN: u32 = 1 << 31;
const DESC_EOR: u32 = 1 << 30;
const DESC_FS: u32 = 1 << 29;
const DESC_LS: u32 = 1 << 28;

const RX_DESC_COUNT: usize = 32;
const TX_DESC_COUNT: usize = 32;
const PKT_BUF_SIZE: usize = 2048;

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct R8169Desc {
    opts1: u32,
    opts2: u32,
    addr: u64,
}

pub struct R8169Device {
    mmio_base: usize,
    mac: MacAddress,
    rx_ring_phys: u64,
    tx_ring_phys: u64,
    rx_bufs: [u64; RX_DESC_COUNT],
    tx_bufs: [u64; TX_DESC_COUNT],
    rx_cur: usize,
    tx_cur: usize,
    pub stats: NetStats,
}

unsafe impl Send for R8169Device {}
unsafe impl Sync for R8169Device {}

pub static R8169_DEVICE: Spinlock<Option<R8169Device>> = Spinlock::new(None);

#[inline]
fn mmio_read8(base: usize, reg: usize) -> u8 {
    unsafe { read_volatile((base + reg) as *const u8) }
}

#[inline]
fn mmio_write8(base: usize, reg: usize, val: u8) {
    unsafe { write_volatile((base + reg) as *mut u8, val) }
}

#[inline]
fn mmio_write16(base: usize, reg: usize, val: u16) {
    unsafe { write_volatile((base + reg) as *mut u16, val) }
}

#[inline]
fn mmio_write32(base: usize, reg: usize, val: u32) {
    unsafe { write_volatile((base + reg) as *mut u32, val) }
}

fn alloc_packet_buffers<const N: usize>() -> Result<[u64; N], &'static str> {
    let mut bufs = [0u64; N];
    let mut i = 0;
    while i < N {
        let frame = memory::alloc_frame().ok_or("Out of memory for r8169 packet buffers")?;
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

impl R8169Device {
    fn probe(pci_dev: &PciDevice) -> Result<Self, &'static str> {
        let mut mmio_phys = 0u64;
        let mut mmio_size = 0usize;
        for bar in &pci_dev.bars {
            match bar.bar_type {
                BarType::Memory32 { address, size, .. } if address != 0 => {
                    mmio_phys = address as u64;
                    mmio_size = size;
                    break;
                }
                BarType::Memory64 { address, size, .. } if address != 0 => {
                    mmio_phys = address;
                    mmio_size = size;
                    break;
                }
                _ => {}
            }
        }
        if mmio_phys == 0 {
            return Err("No MMIO BAR found on Realtek NIC");
        }

        let map_size = core::cmp::max(mmio_size, 8192) as u64;
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

        // Software reset
        mmio_write8(base, REG_CHIP_CMD, CMD_RESET);
        for _ in 0..1_000_000 {
            if (mmio_read8(base, REG_CHIP_CMD) & CMD_RESET) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        let mut mac_bytes = [0u8; 6];
        for i in 0..6 {
            mac_bytes[i] = mmio_read8(base, REG_IDR0 + i);
        }

        // Mask interrupts
        mmio_write16(base, REG_IMR, 0);
        mmio_write16(base, REG_ISR, 0xFFFF);

        let rx_ring_frame = memory::alloc_frame().ok_or("Out of memory for r8169 RX ring")?;
        let tx_ring_frame = memory::alloc_frame().ok_or("Out of memory for r8169 TX ring")?;
        let rx_ring_phys = rx_ring_frame.start_address().as_u64();
        let tx_ring_phys = tx_ring_frame.start_address().as_u64();

        unsafe {
            core::ptr::write_bytes(rx_ring_phys as *mut u8, 0, 4096);
            core::ptr::write_bytes(tx_ring_phys as *mut u8, 0, 4096);
        }

        let rx_bufs: [u64; RX_DESC_COUNT] = alloc_packet_buffers()?;
        let tx_bufs: [u64; TX_DESC_COUNT] = alloc_packet_buffers()?;

        let rx_descs = unsafe {
            core::slice::from_raw_parts_mut(rx_ring_phys as *mut R8169Desc, RX_DESC_COUNT)
        };
        for i in 0..RX_DESC_COUNT {
            let eor = if i + 1 == RX_DESC_COUNT { DESC_EOR } else { 0 };
            rx_descs[i] = R8169Desc {
                opts1: DESC_OWN | eor | (PKT_BUF_SIZE as u32),
                opts2: 0,
                addr: rx_bufs[i],
            };
        }

        // Unlock configuration registers
        mmio_write8(base, REG_CFG9346, 0xC0);
        mmio_write16(base, REG_RX_MAX_SIZE, 1536);
        mmio_write8(base, REG_TX_MAX_SIZE, 0x3F);

        mmio_write32(base, REG_RX_DESC_LOW, rx_ring_phys as u32);
        mmio_write32(base, REG_RX_DESC_HIGH, (rx_ring_phys >> 32) as u32);
        mmio_write32(base, REG_TX_DESC_LOW, tx_ring_phys as u32);
        mmio_write32(base, REG_TX_DESC_HIGH, (tx_ring_phys >> 32) as u32);

        mmio_write8(base, REG_CHIP_CMD, CMD_TX_ENABLE | CMD_RX_ENABLE);
        // Accept broadcast, multicast, physical match; unlimited DMA burst
        mmio_write32(base, REG_RX_CONFIG, 0x0000_070E);
        mmio_write32(base, REG_TX_CONFIG, 0x0300_0700);
        mmio_write8(base, REG_CFG9346, 0x00);

        Ok(Self {
            mmio_base: base,
            mac: MacAddress(mac_bytes),
            rx_ring_phys,
            tx_ring_phys,
            rx_bufs,
            tx_bufs,
            rx_cur: 0,
            tx_cur: 0,
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

        let idx = self.tx_cur;
        let buf_phys = self.tx_bufs[idx];
        unsafe {
            core::ptr::copy_nonoverlapping(packet.as_ptr(), buf_phys as *mut u8, packet.len());
        }

        let tx_descs = unsafe {
            core::slice::from_raw_parts_mut(self.tx_ring_phys as *mut R8169Desc, TX_DESC_COUNT)
        };
        let eor = if idx + 1 == TX_DESC_COUNT { DESC_EOR } else { 0 };
        tx_descs[idx] = R8169Desc {
            opts1: DESC_OWN | eor | DESC_FS | DESC_LS | (packet.len() as u32 & 0xFFFF),
            opts2: 0,
            addr: buf_phys,
        };

        self.tx_cur = (idx + 1) % TX_DESC_COUNT;
        mmio_write8(self.mmio_base, REG_TX_POLL, 0x40); // NPQ

        for _ in 0..1_000_000 {
            let opts1 = unsafe { read_volatile(&tx_descs[idx].opts1) };
            if (opts1 & DESC_OWN) == 0 {
                self.stats.tx_packets += 1;
                self.stats.tx_bytes += packet.len();
                return Ok(());
            }
            core::hint::spin_loop();
        }

        Err("r8169 TX timeout")
    }

    pub fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, &'static str> {
        let rx_descs = unsafe {
            core::slice::from_raw_parts_mut(self.rx_ring_phys as *mut R8169Desc, RX_DESC_COUNT)
        };
        let idx = self.rx_cur;
        let opts1 = unsafe { read_volatile(&rx_descs[idx].opts1) };
        if (opts1 & DESC_OWN) != 0 {
            return Ok(None);
        }

        let raw_len = (opts1 & 0x3FFF) as usize;
        let len = raw_len.saturating_sub(4).min(buf.len()); // strip 4-byte FCS
        let pkt_phys = self.rx_bufs[idx];
        unsafe {
            core::ptr::copy_nonoverlapping(pkt_phys as *const u8, buf.as_mut_ptr(), len);
        }

        let eor = if idx + 1 == RX_DESC_COUNT { DESC_EOR } else { 0 };
        rx_descs[idx].opts1 = DESC_OWN | eor | (PKT_BUF_SIZE as u32);
        self.rx_cur = (idx + 1) % RX_DESC_COUNT;

        self.stats.rx_packets += 1;
        self.stats.rx_bytes += len;

        Ok(Some(len))
    }
}

/// Probes PCI bus for Realtek RTL8168/RTL8111/RTL8169 Gigabit Ethernet controllers.
pub fn init() {
    let devices = PCI_DEVICES.lock().clone();
    for dev in devices {
        if dev.vendor_id == 0x10EC
            && dev.class_code == 0x02
            && dev.subclass == 0x00
            && matches!(dev.device_id, 0x8168 | 0x8169 | 0x8136 | 0x8125 | 0x8161 | 0x8162)
        {
            println!(
                "[R8169] Found Realtek Ethernet at PCI {:02x}:{:02x}.{} ({:04x}:{:04x})",
                dev.address.bus,
                dev.address.device,
                dev.address.function,
                dev.vendor_id,
                dev.device_id
            );
            match R8169Device::probe(&dev) {
                Ok(nic) => {
                    println!("[R8169] Initialized 'eth0': Hardware MAC: {}", nic.mac());
                    *R8169_DEVICE.lock() = Some(nic);
                    return;
                }
                Err(e) => {
                    println!("[R8169] Probe failed: {}", e);
                }
            }
        }
    }
}
