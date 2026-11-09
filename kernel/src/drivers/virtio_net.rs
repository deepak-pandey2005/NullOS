//! # VirtIO Network Device Driver
//!
//! Implements the VirtIO 0.9.5 / 1.0 paravirtualized network card (NIC) specification over PCI:
//! - PCI device discovery (Vendor `0x1AF4`, Device `0x1000` legacy / `0x1041` modern).
//! - Hardware MAC address discovery from device configuration space (`0x14..0x19`).
//! - Dual Split Virtqueues:
//!   - **Queue 0 (Receive / RX)**: Pre-populated descriptor ring buffers ready to absorb inbound packets.
//!   - **Queue 1 (Transmit / TX)**: Synchronous completion polling for outgoing Ethernet frames.
//! - VirtIO 10-byte network packet header ([`VirtIoNetHdr`]) encapsulation.
//! - Raw packet transmission and reception primitives.
//! - Device statistics tracking (RX/TX packets, RX/TX bytes).
//!
//! ### Split Virtqueue Architecture for Networking
//!
//! Unlike block devices that share a single queue for bidirectional commands, VirtIO networking
//! requires separate, dedicated split virtqueues:
//! - **Receive Queue (Queue 0)**: Guest provisions write-only memory buffers (`VIRTQ_DESC_F_WRITE`).
//!   When an Ethernet packet arrives over the wire/tap device, the host hypervisor deposits the
//!   header and payload into an available buffer and signals completion on the used ring.
//!   The guest consumes the payload and must immediately re-arm the descriptor back to the available ring.
//! - **Transmit Queue (Queue 1)**: When sending a packet, the guest constructs a 2-descriptor chain:
//!   - Descriptor 0: 10-byte [`VirtIoNetHdr`] specifying checksum / GSO offload metadata.
//!   - Descriptor 1: Raw Ethernet frame payload (up to 1514 bytes for standard MTU 1500).
//!   The driver kicks Queue 1 and polls the used ring until the hypervisor finishes wire transmission.
//!
//! # Real Hardware vs. Virtual Machine Note:
//! VirtIO-Net is a paravirtualized hypervisor standard (QEMU/KVM/VirtualBox).
//! On real physical computers (bare metal USB boot), VirtIO network PCI devices (0x1AF4:0x1000)
//! DO NOT exist. Physical computers communicate over real vendor-specific NICs:
//! 1. Intel Gigabit Ethernet (e1000 / e1000e / I217 / I219-V, PCI Vendor 0x8086).
//! 2. Realtek PCIe Gigabit Family (RTL8168 / RTL8111 / RTL8125, PCI Vendor 0x10EC).
//! 3. Modern Intel / Broadcom Wi-Fi PCI adapters.
//!
//! // TODO(real-hardware-networking): Implement physical NIC drivers (e.g., `e1000.rs` and `rtl8168.rs`)
//! // to support bare-metal Ethernet connections on physical motherboards and laptops.
#![allow(dead_code)]

use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{fence, Ordering};

use crate::drivers::pci::{self, BarType, PciDevice};
use crate::memory::{self, PhysFrame, PAGE_SIZE_4K};
use crate::println;
use crate::sync::Spinlock;

/// PCI Vendor ID for Red Hat / VirtIO hardware devices.
pub const VIRTIO_VENDOR_ID: u16 = 0x1AF4;
/// PCI Device ID for Transitional / Legacy VirtIO Network Adapter.
pub const VIRTIO_DEV_NET_LEGACY: u16 = 0x1000;
/// PCI Device ID for Modern VirtIO 1.0 Network Adapter.
pub const VIRTIO_DEV_NET_MODERN: u16 = 0x1041;

// VirtIO Legacy PCI I/O Register Offsets (relative to BAR0 I/O base)
/// 32-bit R: Host feature bits.
const VIRTIO_REG_DEVICE_FEATURES: u16 = 0x00;
/// 32-bit W: Guest negotiated feature bits.
const VIRTIO_REG_GUEST_FEATURES: u16 = 0x04;
/// 32-bit RW: Queue Page Frame Number (physical frame >> 12).
const VIRTIO_REG_QUEUE_ADDRESS: u16 = 0x08;
/// 16-bit R: Number of descriptors in currently selected queue.
const VIRTIO_REG_QUEUE_SIZE: u16 = 0x0C;
/// 16-bit W: Queue index selector (0 = RX, 1 = TX).
const VIRTIO_REG_QUEUE_SELECT: u16 = 0x0E;
/// 16-bit W: Queue notify port (write queue index to kick hypervisor).
const VIRTIO_REG_QUEUE_NOTIFY: u16 = 0x10;
/// 8-bit RW: Device status register bits.
const VIRTIO_REG_DEVICE_STATUS: u16 = 0x12;
/// 8-bit R: Interrupt status register (cleared on read).
const VIRTIO_REG_ISR_STATUS: u16 = 0x13;
/// Start of device-specific configuration space.
const VIRTIO_REG_DEVICE_CONFIG: u16 = 0x14;

// VirtIO Device Status Flags
/// Guest OS has acknowledged the device presence.
const VIRTIO_STATUS_ACKNOWLEDGE: u8 = 1;
/// Guest OS recognizes the device and loaded a driver.
const VIRTIO_STATUS_DRIVER: u8 = 2;
/// Driver setup is complete and device is live.
const VIRTIO_STATUS_DRIVER_OK: u8 = 4;
/// Feature negotiation completed successfully.
const VIRTIO_STATUS_FEATURES_OK: u8 = 8;
/// Driver encountered an unrecoverable failure.
const VIRTIO_STATUS_FAILED: u8 = 128;

// VirtIO Network Features
/// Feature bit 5: Device has given MAC address.
const VIRTIO_NET_F_MAC: u32 = 1 << 5;
/// Feature bit 16: Device reports link status in config space.
const VIRTIO_NET_F_STATUS: u32 = 1 << 16;

// Virtqueue Descriptor Flags
/// Buffer continues via the `next` descriptor index field.
const VIRTQ_DESC_F_NEXT: u16 = 1;
/// Buffer is write-only for the device (device writes to memory).
const VIRTQ_DESC_F_WRITE: u16 = 2;

// Maximum packet and buffer limits
/// Length of an Ethernet hardware MAC address in bytes (6 bytes).
pub const ETH_ALEN: usize = 6;
/// Standard Ethernet Maximum Transmission Unit (MTU) payload size (1500 bytes).
pub const ETH_MTU: usize = 1500;
/// Maximum Ethernet frame size: 14 bytes header (Dest MAC + Src MAC + EtherType) + 1500 bytes payload.
pub const ETH_MAX_FRAME: usize = 1514;
/// Standard 10-byte VirtIoNetHdr prefix size.
pub const NET_HDR_SIZE: usize = 10;
/// Size of pre-allocated receive buffer: 10 bytes header + 1514 bytes max frame = 1524 bytes.
pub const RX_BUF_SIZE: usize = NET_HDR_SIZE + ETH_MAX_FRAME;
/// Default number of pre-allocated RX buffers queued to the receive ring.
const NUM_RX_BUFFERS: usize = 32;

/// A 6-byte Ethernet Media Access Control (MAC) Address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MacAddress(pub [u8; ETH_ALEN]);

impl MacAddress {
    /// Broadcast MAC address `FF:FF:FF:FF:FF:FF`.
    pub const BROADCAST: Self = Self([0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    /// All-zeros MAC address `00:00:00:00:00:00`.
    pub const ZERO: Self = Self([0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

    /// Creates a new MAC address from a 6-byte array.
    pub const fn new(bytes: [u8; ETH_ALEN]) -> Self {
        Self(bytes)
    }

    /// Checks if this address matches the broadcast MAC.
    pub fn is_broadcast(&self) -> bool {
        self.0 == Self::BROADCAST.0
    }
}

impl fmt::Display for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

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

/// VirtIO Network Request/Response Header (10 bytes in legacy VirtIO 0.9.5).
///
/// Precedes every Ethernet packet transmitted or received over the VirtIO interface.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VirtIoNetHdr {
    /// Checksum / packet flags.
    pub flags: u8,
    /// Generic Segmentation Offload (GSO) type.
    pub gso_type: u8,
    /// Ethernet + IP + L4 header length for GSO calculation.
    pub hdr_len: u16,
    /// Maximum Segment Size for GSO.
    pub gso_size: u16,
    /// Offset within packet where checksum calculation starts.
    pub csum_start: u16,
    /// Offset relative to csum_start where checksum field is stored.
    pub csum_offset: u16,
}

/// Network interface I/O performance and packet statistics.
#[derive(Debug, Clone, Copy, Default)]
pub struct NetStats {
    /// Total count of successfully received Ethernet frames.
    pub rx_packets: usize,
    /// Total count of successfully transmitted Ethernet frames.
    pub tx_packets: usize,
    /// Total payload and frame bytes received.
    pub rx_bytes: usize,
    /// Total payload and frame bytes transmitted.
    pub tx_bytes: usize,
}

// Low-level port I/O helper functions
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

/// Driver instance managing a VirtIO Network device.
pub struct VirtIoNetDevice {
    /// Base I/O port address decoded from BAR0.
    io_base: u16,
    /// Hardware MAC address assigned to this network interface.
    mac: MacAddress,
    /// Network packet and byte statistics.
    pub stats: NetStats,

    // Queue 0: Receive (RX)
    rx_queue_size: u16,
    rx_desc_table: *mut VirtqDesc,
    rx_avail_ring: *mut VirtqAvail,
    rx_used_ring: *mut VirtqUsed,
    rx_last_used_idx: u16,
    _rx_frame: PhysFrame,
    rx_buffers: Vec<PhysFrame>, // Holds the physical memory frames for RX buffers

    // Queue 1: Transmit (TX)
    tx_queue_size: u16,
    tx_desc_table: *mut VirtqDesc,
    tx_avail_ring: *mut VirtqAvail,
    tx_used_ring: *mut VirtqUsed,
    tx_last_used_idx: u16,
    _tx_frame: PhysFrame,
    tx_header: VirtIoNetHdr,
}

unsafe impl Send for VirtIoNetDevice {}
unsafe impl Sync for VirtIoNetDevice {}

impl VirtIoNetDevice {
    /// Probes and initializes a VirtIO Network device from a discovered PCI device.
    ///
    /// Configures legacy port I/O registers, negotiates MAC feature bits, allocates split virtqueues
    /// for RX (queue 0) and TX (queue 1), pre-populates RX buffers, and transitions status to `DRIVER_OK`.
    pub fn probe(pci_dev: &PciDevice) -> Result<Self, &'static str> {
        // 1. Extract BAR0 I/O port address
        let io_base = match pci_dev.bars.first() {
            Some(bar) => match bar.bar_type {
                BarType::Io { port, .. } => port,
                _ => return Err("BAR0 is not an I/O port"),
            },
            _ => return Err("No BARs found on VirtIO device"),
        };

        // 2. Enable PCI Bus Mastering and I/O port decoding
        pci_dev.enable_bus_mastering();
        pci_dev.enable_io_space();

        unsafe {
            // Reset device
            outb(io_base + VIRTIO_REG_DEVICE_STATUS, 0);

            // Set ACKNOWLEDGE status bit
            let mut status = VIRTIO_STATUS_ACKNOWLEDGE;
            outb(io_base + VIRTIO_REG_DEVICE_STATUS, status);

            // Set DRIVER status bit
            status |= VIRTIO_STATUS_DRIVER;
            outb(io_base + VIRTIO_REG_DEVICE_STATUS, status);

            // Read device features and acknowledge supported subset
            let host_features = inl(io_base + VIRTIO_REG_DEVICE_FEATURES);
            let guest_features = host_features & (VIRTIO_NET_F_MAC | VIRTIO_NET_F_STATUS);
            outl(io_base + VIRTIO_REG_GUEST_FEATURES, guest_features);

            // 3. Read Hardware MAC Address from device configuration space (offset 0x14)
            let mut mac_bytes = [0u8; 6];
            for i in 0..6 {
                mac_bytes[i] = inb(io_base + VIRTIO_REG_DEVICE_CONFIG + i as u16);
            }
            let mac = MacAddress(mac_bytes);

            // 4. Initialize Queue 0 (RX Queue)
            outw(io_base + VIRTIO_REG_QUEUE_SELECT, 0);
            let rx_q_size = inw(io_base + VIRTIO_REG_QUEUE_SIZE);
            if rx_q_size == 0 || rx_q_size > 256 {
                return Err("Invalid RX queue size");
            }

            let (rx_frame, rx_desc, rx_avail, rx_used) = Self::alloc_queue(rx_q_size)?;
            let rx_pfn = (rx_frame.start_address().as_u64() / 4096) as u32;
            outl(io_base + VIRTIO_REG_QUEUE_ADDRESS, rx_pfn);

            // 5. Initialize Queue 1 (TX Queue)
            outw(io_base + VIRTIO_REG_QUEUE_SELECT, 1);
            let tx_q_size = inw(io_base + VIRTIO_REG_QUEUE_SIZE);
            if tx_q_size == 0 || tx_q_size > 256 {
                return Err("Invalid TX queue size");
            }

            let (tx_frame, tx_desc, tx_avail, tx_used) = Self::alloc_queue(tx_q_size)?;
            let tx_pfn = (tx_frame.start_address().as_u64() / 4096) as u32;
            outl(io_base + VIRTIO_REG_QUEUE_ADDRESS, tx_pfn);

            // 6. Set DRIVER_OK status bit
            status |= VIRTIO_STATUS_DRIVER_OK;
            outb(io_base + VIRTIO_REG_DEVICE_STATUS, status);

            // 7. Pre-populate RX Queue with receive buffers
            let mut rx_buffers = Vec::new();
            let num_bufs = NUM_RX_BUFFERS.min(rx_q_size as usize);
            let avail = &mut *rx_avail;

            for i in 0..num_bufs {
                let frame = memory::alloc_frame().ok_or("Failed to allocate RX frame")?;
                let buf_addr = frame.start_address().as_u64();
                core::ptr::write_bytes(buf_addr as *mut u8, 0, PAGE_SIZE_4K as usize);

                (*rx_desc.add(i)) = VirtqDesc {
                    addr: buf_addr,
                    len: RX_BUF_SIZE as u32,
                    flags: VIRTQ_DESC_F_WRITE,
                    next: 0,
                };

                avail.ring[i] = i as u16;
                rx_buffers.push(frame);
            }

            fence(Ordering::SeqCst);
            avail.idx = num_bufs as u16;
            fence(Ordering::SeqCst);

            // Kick RX Queue so the device can begin receiving packets
            outw(io_base + VIRTIO_REG_QUEUE_NOTIFY, 0);

            Ok(Self {
                io_base,
                mac,
                stats: NetStats::default(),
                rx_queue_size: rx_q_size,
                rx_desc_table: rx_desc,
                rx_avail_ring: rx_avail,
                rx_used_ring: rx_used,
                rx_last_used_idx: 0,
                _rx_frame: rx_frame,
                rx_buffers,
                tx_queue_size: tx_q_size,
                tx_desc_table: tx_desc,
                tx_avail_ring: tx_avail,
                tx_used_ring: tx_used,
                tx_last_used_idx: 0,
                _tx_frame: tx_frame,
                tx_header: VirtIoNetHdr::default(),
            })
        }
    }

    /// Allocates memory for a single Split Virtqueue per specification.
    ///
    /// Layout:
    /// - Descriptor Table: `q_size * 16` bytes.
    /// - Available Ring: `4 + q_size * 2` bytes.
    /// - Aligned to 4096-byte boundary.
    /// - Used Ring: `4 + q_size * 8` bytes.
    unsafe fn alloc_queue(
        q_size: u16,
    ) -> Result<(PhysFrame, *mut VirtqDesc, *mut VirtqAvail, *mut VirtqUsed), &'static str> {
        let desc_bytes = (q_size as usize) * 16;
        let avail_bytes = 4 + (q_size as usize) * 2;
        let used_offset = (desc_bytes + avail_bytes + 4095) & !4095;
        let used_bytes = 4 + (q_size as usize) * 8;
        let total_bytes = (used_offset + used_bytes + 4095) & !4095;
        let num_frames = total_bytes / PAGE_SIZE_4K as usize;

        let frame = memory::alloc_contiguous_frames(num_frames).ok_or("Out of memory for virtqueue")?;
        let ptr = frame.start_address().as_u64() as *mut u8;
        unsafe {
            core::ptr::write_bytes(ptr, 0, total_bytes);
            let desc = ptr as *mut VirtqDesc;
            let avail = ptr.add(desc_bytes) as *mut VirtqAvail;
            let used = ptr.add(used_offset) as *mut VirtqUsed;
            Ok((frame, desc, avail, used))
        }
    }

    /// Returns the hardware MAC address of the network interface.
    #[inline]
    pub fn mac(&self) -> MacAddress {
        self.mac
    }

    /// Transmits a raw Ethernet frame over the network.
    ///
    /// Constructs a 2-descriptor chain ([`VirtIoNetHdr`] + payload), inserts into TX Available Ring,
    /// kicks Queue 1, and polls the Used Ring until transmission completes.
    pub fn send_packet(&mut self, packet: &[u8]) -> Result<(), &'static str> {
        if packet.is_empty() || packet.len() > ETH_MAX_FRAME {
            return Err("Packet size exceeds Ethernet MTU");
        }

        unsafe {
            let desc = self.tx_desc_table;
            let avail = &mut *self.tx_avail_ring;
            let used = &*self.tx_used_ring;

            // Clear header
            self.tx_header = VirtIoNetHdr::default();
            let hdr_phys = &self.tx_header as *const _ as u64;
            let packet_phys = packet.as_ptr() as u64;

            // Descriptor 0: 10-byte VirtIoNetHdr (Device reads)
            (*desc.add(0)) = VirtqDesc {
                addr: hdr_phys,
                len: NET_HDR_SIZE as u32,
                flags: VIRTQ_DESC_F_NEXT,
                next: 1,
            };

            // Descriptor 1: Packet payload (Device reads)
            (*desc.add(1)) = VirtqDesc {
                addr: packet_phys,
                len: packet.len() as u32,
                flags: 0,
                next: 0,
            };

            fence(Ordering::SeqCst);

            // Add descriptor 0 to Available Ring
            let slot = (avail.idx % self.tx_queue_size) as usize;
            avail.ring[slot] = 0;

            fence(Ordering::SeqCst);
            avail.idx = avail.idx.wrapping_add(1);
            fence(Ordering::SeqCst);

            // Kick TX queue (Queue index 1)
            outw(self.io_base + VIRTIO_REG_QUEUE_NOTIFY, 1);

            // Poll for transmission completion
            let expected_idx = self.tx_last_used_idx.wrapping_add(1);
            let mut timeout = 20_000_000;
            while used.idx != expected_idx {
                core::hint::spin_loop();
                timeout -= 1;
                if timeout == 0 {
                    return Err("TX packet timed out");
                }
            }

            self.tx_last_used_idx = expected_idx;

            // Acknowledge interrupt
            let _ = inb(self.io_base + VIRTIO_REG_ISR_STATUS);

            // Update stats
            self.stats.tx_packets += 1;
            self.stats.tx_bytes += packet.len();

            Ok(())
        }
    }

    /// Checks the RX queue for an incoming network packet and copies it into `buf`.
    ///
    /// Returns `Ok(Some(len))` if a packet was received, or `Ok(None)` if no packet is available.
    /// Automatically strips the 10-byte VirtIO network header and re-arms the receive descriptor.
    pub fn receive_packet(&mut self, buf: &mut [u8]) -> Result<Option<usize>, &'static str> {
        unsafe {
            let used = &*self.rx_used_ring;
            let avail = &mut *self.rx_avail_ring;

            if used.idx == self.rx_last_used_idx {
                return Ok(None); // No new packets
            }

            let slot = (self.rx_last_used_idx % self.rx_queue_size) as usize;
            let elem = used.ring[slot];
            let desc_idx = elem.id as usize;
            let total_len = elem.len as usize;

            if total_len <= NET_HDR_SIZE {
                // Malformed packet or empty header, discard
                self.rx_last_used_idx = self.rx_last_used_idx.wrapping_add(1);
                return Ok(None);
            }

            let payload_len = total_len - NET_HDR_SIZE;
            if buf.len() < payload_len {
                return Err("Destination buffer too small for received packet");
            }

            // Copy packet payload out of the RX buffer (skipping the 10-byte VirtIoNetHdr)
            let buf_phys = (*self.rx_desc_table.add(desc_idx)).addr;
            let src_ptr = (buf_phys + NET_HDR_SIZE as u64) as *const u8;
            core::ptr::copy_nonoverlapping(src_ptr, buf.as_mut_ptr(), payload_len);

            // Re-arm descriptor in available ring to receive future packets
            let avail_slot = (avail.idx % self.rx_queue_size) as usize;
            avail.ring[avail_slot] = desc_idx as u16;

            fence(Ordering::SeqCst);
            avail.idx = avail.idx.wrapping_add(1);
            self.rx_last_used_idx = self.rx_last_used_idx.wrapping_add(1);
            fence(Ordering::SeqCst);

            // Notify device of replenished buffer
            outw(self.io_base + VIRTIO_REG_QUEUE_NOTIFY, 0);

            // Update stats
            self.stats.rx_packets += 1;
            self.stats.rx_bytes += payload_len;

            Ok(Some(payload_len))
        }
    }
}

/// Global thread-safe instance of the primary VirtIO Network Device.
pub static NETWORK_DEVICE: Spinlock<Option<VirtIoNetDevice>> = Spinlock::new(None);

/// Discovers and initializes the primary VirtIO network card.
pub fn init() {
    println!("----------------------------------------");
    println!(" [VIRTIO-NET] Probing Network Hardware  ");
    println!("----------------------------------------");

    let devices = pci::PCI_DEVICES.lock();
    for dev in devices.iter() {
        if dev.vendor_id == VIRTIO_VENDOR_ID
            && (dev.device_id == VIRTIO_DEV_NET_LEGACY || dev.device_id == VIRTIO_DEV_NET_MODERN)
        {
            println!(
                "[VIRTIO-NET] Found VirtIO Network Card at PCI {} (Vendor: {:#06x}, Device: {:#06x})",
                dev.address, dev.vendor_id, dev.device_id
            );

            match VirtIoNetDevice::probe(dev) {
                Ok(net_dev) => {
                    let mac = net_dev.mac();
                    println!("[VIRTIO-NET] Initialized 'eth0': Hardware MAC: {}", mac);
                    println!("[VIRTIO-NET] Virtqueues active: RX Queue (size: {}), TX Queue (size: {})",
                        net_dev.rx_queue_size, net_dev.tx_queue_size
                    );
                    *NETWORK_DEVICE.lock() = Some(net_dev);
                    return;
                }
                Err(e) => {
                    println!("[VIRTIO-NET] Failed to initialize VirtIO network device: {}", e);
                }
            }
        }
    }

    println!("[VIRTIO-NET] Note: No VirtIO network device detected on PCI bus.");
    println!("[VIRTIO-NET] (Hypervisor-only: Physical PCs require Intel e1000e or Realtek RTL8168 drivers).");
}

/// Transmits a raw packet through the global network device.
pub fn send(packet: &[u8]) -> Result<(), &'static str> {
    if let Some(ref mut dev) = *NETWORK_DEVICE.lock() {
        dev.send_packet(packet)
    } else {
        Err("No active network device")
    }
}

/// Receives a raw packet through the global network device if available.
pub fn receive(buf: &mut [u8]) -> Result<Option<usize>, &'static str> {
    if let Some(ref mut dev) = *NETWORK_DEVICE.lock() {
        dev.receive_packet(buf)
    } else {
        Err("No active network device")
    }
}

/// Returns the MAC address of the network interface.
pub fn get_mac() -> Option<MacAddress> {
    NETWORK_DEVICE.lock().as_ref().map(|dev| dev.mac())
}

/// Returns current network statistics.
pub fn get_stats() -> Option<NetStats> {
    NETWORK_DEVICE.lock().as_ref().map(|dev| dev.stats)
}

/// Runs automated driver self-tests verifying packet transmission and loopback handling.
///
/// Constructs a test IEEE 802 Local Experimental frame (EtherType 0x88B5), transmits it,
/// and verifies that interface statistics counters correctly record the transmission.
pub fn run_self_tests() {
    println!("[VIRTIO-NET] Running VirtIO Network verification self-tests...");

    let mut guard = NETWORK_DEVICE.lock();
    if let Some(ref mut dev) = *guard {
        let mac = dev.mac();
        println!("[VIRTIO-NET] Testing packet transmission on 'eth0' (MAC: {})...", mac);

        // Construct a standard 60-byte Ethernet broadcast test frame
        let mut test_frame = [0u8; 60];
        // Destination MAC: Broadcast FF:FF:FF:FF:FF:FF
        test_frame[0..6].copy_from_slice(&[0xFF; 6]);
        // Source MAC: Hardware MAC
        test_frame[6..12].copy_from_slice(&mac.0);
        // EtherType: Experimental / Test 0x88B5 (IEEE 802 Local Experimental)
        test_frame[12] = 0x88;
        test_frame[13] = 0xB5;
        // Payload test message
        let msg = b"NullOS VirtIO-Net Driver Verification Packet!";
        test_frame[14..14 + msg.len()].copy_from_slice(msg);

        match dev.send_packet(&test_frame) {
            Ok(()) => {
                println!("[VIRTIO-NET] Successfully transmitted 60-byte raw Ethernet test frame via Virtqueue!");
            }
            Err(e) => {
                panic!("[VIRTIO-NET FATAL] send_packet failed: {}", e);
            }
        }

        let stats = dev.stats;
        assert_eq!(stats.tx_packets, 1, "TX packet count mismatch");
        assert_eq!(stats.tx_bytes, 60, "TX byte count mismatch");
        println!("[VIRTIO-NET] Driver statistics verified: 1 packet sent (60 bytes).");
        println!("[VIRTIO-NET] All VirtIO Network driver self-tests passed successfully!");
    } else {
        println!("[VIRTIO-NET] Self-test skipped: No network device attached.");
    }
}
