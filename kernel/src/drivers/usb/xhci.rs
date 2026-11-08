//! USB 3.0 xHCI (Extensible Host Controller Interface) Driver (Phase 12 - M31).
//!
//! Subsystem features:
//! - PCI hardware discovery for Class 0x0C (Serial), Subclass 0x03, Prog IF 0x30 (xHCI).
//! - 64-bit MMIO BAR0 mapping with cache-disable / write-through flags.
//! - Capability, Operational, Runtime, and Doorbell register parsing.
//! - BIOS-to-OS ownership handoff (xECP USB Legacy Support).
//! - Transfer Request Block (TRB) circular rings: Command Ring and Event Ring with ERST.
//! - Device Context Base Address Array (DCBAA) and Scratchpad buffer allocation.
//! - Root Hub port scanning, reset signaling, and speed negotiation via PORTSC.
//! - USB Device slot allocation, address assignment, and EP0 control pipe setup.
//! - USB Device descriptor extraction and device classification.
//! - HID Boot Protocol configuration for USB Keyboards and Mice.

#![allow(dead_code)]

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{fence, Ordering};

use crate::drivers::pci::{self, BarType};
use crate::drivers::usb::descriptor::*;
use crate::drivers::usb::hid::{USB_KEYBOARD, USB_MOUSE};
use crate::drivers::usb::trb::*;
use crate::memory::{self, PhysAddr, PhysFrame, PageTableFlags, PAGE_SIZE_4K};
use crate::println;
use crate::sync::Spinlock;

// Operational Register Offsets (relative to Operational Base)
const REG_USBCMD: usize = 0x00;
const REG_USBSTS: usize = 0x04;
const REG_PAGESIZE: usize = 0x08;
const REG_DNCTRL: usize = 0x14;
const REG_CRCR: usize = 0x18;
const REG_DCBAAP: usize = 0x30;
const REG_CONFIG: usize = 0x38;
const REG_PORTSC_BASE: usize = 0x400;

// USBCMD Flags
const USBCMD_RS: u32 = 1 << 0;       // Run/Stop
const USBCMD_HCRST: u32 = 1 << 1;    // Host Controller Reset
const USBCMD_INTE: u32 = 1 << 2;     // Interrupter Enable

// USBSTS Flags
const USBSTS_HCH: u32 = 1 << 0;      // Host Controller Halted
const USBSTS_HSE: u32 = 1 << 2;      // Host System Error
const USBSTS_EINT: u32 = 1 << 3;     // Event Interrupt
const USBSTS_PCD: u32 = 1 << 4;      // Port Change Detect
const USBSTS_CNR: u32 = 1 << 11;     // Controller Not Ready

// PORTSC Flags
const PORTSC_CCS: u32 = 1 << 0;      // Current Connect Status
const PORTSC_PED: u32 = 1 << 1;      // Port Enabled/Disabled
const PORTSC_PR: u32 = 1 << 4;       // Port Reset
const PORTSC_PP: u32 = 1 << 9;       // Port Power
const PORTSC_SPEED_MASK: u32 = 0x0F << 10;

// Port Speeds
pub const PORT_SPEED_FULL: u8 = 1;
pub const PORT_SPEED_LOW: u8 = 2;
pub const PORT_SPEED_HIGH: u8 = 3;
pub const PORT_SPEED_SUPER: u8 = 4;

const RING_TRB_COUNT: usize = 64;

/// Safe MMIO 32-bit register read.
#[inline]
unsafe fn mmio_read32(base: *mut u8, offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile(base.add(offset) as *const u32) }
}

/// Safe MMIO 32-bit register write.
#[inline]
unsafe fn mmio_write32(base: *mut u8, offset: usize, val: u32) {
    unsafe { core::ptr::write_volatile(base.add(offset) as *mut u32, val); }
}

/// Safe MMIO 64-bit register read.
#[inline]
unsafe fn mmio_read64(base: *mut u8, offset: usize) -> u64 {
    unsafe { core::ptr::read_volatile(base.add(offset) as *const u64) }
}

/// Safe MMIO 64-bit register write.
#[inline]
unsafe fn mmio_write64(base: *mut u8, offset: usize, val: u64) {
    unsafe { core::ptr::write_volatile(base.add(offset) as *mut u64, val); }
}

/// Represents a circular TRB transfer ring in DMA memory.
pub struct TrbRing {
    pub ring_frame: PhysFrame,
    pub trbs: *mut Trb,
    pub size: usize,
    pub enqueue_idx: usize,
    pub cycle_state: bool,
}

impl TrbRing {
    pub fn new() -> Option<Self> {
        let frame = memory::alloc_frame()?;
        let trbs = frame.start_address().as_u64() as *mut Trb;

        // Zero out the entire ring
        unsafe {
            core::ptr::write_bytes(trbs as *mut u8, 0, PAGE_SIZE_4K as usize);
            // Place Link TRB at the end pointing back to the beginning
            let link_idx = RING_TRB_COUNT - 1;
            let mut link_trb = Trb::link(frame.start_address().as_u64(), true);
            link_trb.set_cycle_bit(false); // Link TRB cycle bit initialized to 0
            *trbs.add(link_idx) = link_trb;
        }

        Some(Self {
            ring_frame: frame,
            trbs,
            size: RING_TRB_COUNT,
            enqueue_idx: 0,
            cycle_state: true,
        })
    }

    pub fn phys_addr(&self) -> u64 {
        self.ring_frame.start_address().as_u64()
    }

    /// Pushes a TRB into the ring, tagging it with the current Producer Cycle State (PCS).
    pub fn push(&mut self, mut trb: Trb) {
        trb.set_cycle_bit(self.cycle_state);
        unsafe {
            *self.trbs.add(self.enqueue_idx) = trb;
        }

        self.enqueue_idx += 1;
        // When reaching the Link TRB at the end:
        if self.enqueue_idx == self.size - 1 {
            // Update Link TRB's cycle bit to match current cycle state
            unsafe {
                let link_ptr = self.trbs.add(self.enqueue_idx);
                let mut link = *link_ptr;
                link.set_cycle_bit(self.cycle_state);
                *link_ptr = link;
            }
            self.enqueue_idx = 0;
            self.cycle_state = !self.cycle_state;
        }
    }
}

impl core::fmt::Debug for TrbRing {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TrbRing(phys: {:#x}, size: {}, enqueue: {})",
            self.phys_addr(),
            self.size,
            self.enqueue_idx
        )
    }
}

/// Discovered USB device on the xHCI bus.
#[derive(Clone)]
pub struct XhciUsbDevice {
    pub slot_id: u8,
    pub port_num: u8,
    pub speed: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub max_packet_size: u8,
    pub is_keyboard: bool,
    pub is_mouse: bool,
    pub ep_in_ring: Option<Arc<Spinlock<TrbRing>>>,
    pub ep_in_dci: u8,
    pub ep_in_buf: Option<PhysFrame>,
}

impl core::fmt::Debug for XhciUsbDevice {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("XhciUsbDevice")
            .field("slot_id", &self.slot_id)
            .field("port_num", &self.port_num)
            .field("speed", &self.speed)
            .field("vendor_id", &self.vendor_id)
            .field("product_id", &self.product_id)
            .field("class_code", &self.class_code)
            .field("subclass", &self.subclass)
            .field("protocol", &self.protocol)
            .field("max_packet_size", &self.max_packet_size)
            .field("is_keyboard", &self.is_keyboard)
            .field("is_mouse", &self.is_mouse)
            .finish()
    }
}

/// The NullOS xHCI Host Controller Driver instance.
pub struct XhciController {
    pub mmio_base: *mut u8,
    pub op_base: *mut u8,
    pub rt_base: *mut u8,
    pub db_base: *mut u8,
    pub caplength: u8,
    pub hciversion: u16,
    pub max_slots: u8,
    pub max_ports: u8,
    pub max_intrs: u16,
    pub command_ring: TrbRing,
    pub event_ring_frame: PhysFrame,
    pub event_ring: *mut Trb,
    pub event_dequeue_idx: usize,
    pub event_cycle_state: bool,
    pub dcbaa_frame: PhysFrame,
    pub devices: Vec<XhciUsbDevice>,
    pub input_context_frames: Vec<PhysFrame>,
    pub device_context_frames: Vec<PhysFrame>,
}

unsafe impl Send for XhciController {}
unsafe impl Sync for XhciController {}

impl XhciController {
    /// Renders Doorbell register write to signal controller or endpoint.
    #[inline]
    pub fn ring_doorbell_raw(db_base: *mut u8, target: u8, value: u32) {
        let offset = (target as usize) * 4;
        unsafe {
            mmio_write32(db_base, offset, value);
        }
    }

    /// Renders Doorbell register write to signal controller or endpoint.
    #[inline]
    pub fn ring_doorbell(&self, target: u8, value: u32) {
        Self::ring_doorbell_raw(self.db_base, target, value);
    }

    /// Submits a command to the Command Ring and rings Doorbell 0.
    pub fn send_command(&mut self, trb: Trb) {
        self.command_ring.push(trb);
        fence(Ordering::SeqCst);
        self.ring_doorbell(0, 0); // Doorbell 0, Target 0 (Host Command)
    }

    /// Checks the Event Ring for an available event posted by the hardware.
    pub fn poll_event(&mut self) -> Option<Trb> {
        let trb = unsafe { *self.event_ring.add(self.event_dequeue_idx) };
        if trb.cycle_bit() == self.event_cycle_state {
            // Valid event received!
            self.event_dequeue_idx += 1;
            if self.event_dequeue_idx >= RING_TRB_COUNT {
                self.event_dequeue_idx = 0;
                self.event_cycle_state = !self.event_cycle_state;
            }

            // Update Event Ring Dequeue Pointer (ERDP) in Interrupter 0
            // Bit 3 is Event Handler Busy (EHB) - write 1 to clear
            let current_phys = self.event_ring_frame.start_address().as_u64()
                + (self.event_dequeue_idx * core::mem::size_of::<Trb>()) as u64;
            let erdp = (current_phys & !0x0F) | (1 << 3);
            unsafe {
                mmio_write64(self.rt_base, 0x20 + 0x18, erdp);
            }

            Some(trb)
        } else {
            None
        }
    }

    /// Waits synchronously for a Command Completion Event TRB.
    pub fn wait_command_completion(&mut self, timeout_spins: usize) -> Result<Trb, &'static str> {
        for _ in 0..timeout_spins {
            if let Some(event) = self.poll_event() {
                if event.trb_type() == TRB_TYPE_COMMAND_COMPLETION {
                    if event.completion_code() == CC_SUCCESS {
                        return Ok(event);
                    } else {
                        return Err(event.completion_code_name());
                    }
                }
            }
            core::hint::spin_loop();
        }
        Err("Command completion timed out")
    }

    /// Waits synchronously for a Transfer Event TRB on a given slot and endpoint.
    pub fn wait_transfer_completion(&mut self, slot_id: u8, endpoint_id: u8, timeout_spins: usize) -> Result<Trb, &'static str> {
        for _ in 0..timeout_spins {
            if let Some(event) = self.poll_event() {
                if event.trb_type() == TRB_TYPE_TRANSFER_EVENT {
                    if event.slot_id() == slot_id && event.endpoint_id() == endpoint_id {
                        if event.completion_code() == CC_SUCCESS || event.completion_code() == CC_SHORT_PACKET {
                            return Ok(event);
                        } else {
                            return Err(event.completion_code_name());
                        }
                    }
                }
            }
            core::hint::spin_loop();
        }
        Err("Transfer event timed out")
    }

    /// Reads a Port Status and Control (PORTSC) register for port `port_num` (1-indexed).
    pub fn read_portsc(&self, port_num: u8) -> u32 {
        let offset = REG_PORTSC_BASE + (port_num as usize - 1) * 0x10;
        unsafe { mmio_read32(self.op_base, offset) }
    }

    /// Writes a Port Status and Control (PORTSC) register.
    pub fn write_portsc(&self, port_num: u8, val: u32) {
        let offset = REG_PORTSC_BASE + (port_num as usize - 1) * 0x10;
        unsafe { mmio_write32(self.op_base, offset, val); }
    }

    /// Executes a standard USB Control Transfer on Endpoint 0 of a given slot.
    pub fn control_transfer(
        &mut self,
        slot_id: u8,
        ep0_ring: &mut TrbRing,
        setup: UsbSetupPacket,
        data_buf_phys: Option<u64>,
        data_len: u32,
        is_in: bool,
    ) -> Result<(), &'static str> {
        let transfer_type = if data_len == 0 {
            0 // No data
        } else if is_in {
            3 // IN data stage
        } else {
            2 // OUT data stage
        };

        // 1. Setup Stage TRB
        let setup_u64 = setup.as_u64();
        ep0_ring.push(Trb::setup_stage(setup_u64, transfer_type, true));

        // 2. Data Stage TRB (if data requested)
        if data_len > 0 && data_buf_phys.is_some() {
            ep0_ring.push(Trb::data_stage(data_buf_phys.unwrap(), data_len, is_in));
        }

        // 3. Status Stage TRB (direction opposite to data stage)
        let status_in = if data_len == 0 { true } else { !is_in };
        ep0_ring.push(Trb::status_stage(status_in));

        fence(Ordering::SeqCst);
        // Ring Doorbell for Slot ID, Endpoint 1 (EP0 Control Pipe)
        self.ring_doorbell(slot_id, 1);

        // Wait for Transfer Event
        self.wait_transfer_completion(slot_id, 1, 500_000)?;
        Ok(())
    }

    /// Polls connected USB input endpoints (keyboards, mice) and processes new reports.
    pub fn poll_inputs(&mut self) {
        let db_base = self.db_base;
        // Poll event ring for transfer events
        while let Some(event) = self.poll_event() {
            if event.trb_type() == TRB_TYPE_TRANSFER_EVENT {
                let slot_id = event.slot_id();
                let ep_id = event.endpoint_id();

                if let Some(dev) = self.devices.iter_mut().find(|d| d.slot_id == slot_id && d.ep_in_dci == ep_id) {
                    if let Some(ref buf_frame) = dev.ep_in_buf {
                        let buf_ptr = buf_frame.start_address().as_u64() as *const u8;
                        if dev.is_keyboard {
                            let mut report = [0u8; 8];
                            unsafe {
                                core::ptr::copy_nonoverlapping(buf_ptr, report.as_mut_ptr(), 8);
                            }
                            USB_KEYBOARD.lock().process_report(&report);
                        } else if dev.is_mouse {
                            let mut report = [0u8; 4];
                            unsafe {
                                core::ptr::copy_nonoverlapping(buf_ptr, report.as_mut_ptr(), 4);
                            }
                            USB_MOUSE.lock().process_report(&report);
                        }

                        // Re-queue transfer on this endpoint
                        if let Some(ref ring_lock) = dev.ep_in_ring {
                            let mut ring = ring_lock.lock();
                            let req_len = if dev.is_keyboard { 8 } else { 4 };
                            ring.push(Trb::normal(buf_frame.start_address().as_u64(), req_len, true));
                            Self::ring_doorbell_raw(db_base, slot_id, ep_id as u32);
                        }
                    }
                }
            }
        }
    }
}

/// Global xHCI Host Controller instance.
pub static XHCI: Spinlock<Option<XhciController>> = Spinlock::new(None);

/// Probes the PCI bus for an xHCI controller, initializes the host, and enumerates devices.
pub fn init() {
    println!();
    println!("--------------------------------------------------");
    println!(" [USB] Initializing USB 3.0 xHCI Controller Driver ");
    println!("--------------------------------------------------");

    // 1. Locate xHCI Controller on PCI bus (Class 0x0C, Subclass 0x03, Prog IF 0x30)
    let usb_controllers = pci::find_by_class(0x0C, 0x03);
    let xhci_dev = usb_controllers.into_iter().find(|d| d.prog_if == 0x30);

    let dev = match xhci_dev {
        Some(d) => d,
        None => {
            println!("[USB] No xHCI USB 3.0 host controller detected on PCI bus.");
            return;
        }
    };

    println!(
        "[USB] Discovered xHCI Controller at PCI {} | Vendor: 0x{:04x}, Device: 0x{:04x} ({})",
        dev.address, dev.vendor_id, dev.device_id, dev.vendor_name()
    );

    // 2. Discover BAR0 MMIO address and size
    let (mmio_phys, mmio_size) = match dev.bars.first() {
        Some(bar) => match bar.bar_type {
            BarType::Memory32 { address, size, .. } => (address as u64, size),
            BarType::Memory64 { address, size, .. } => (address, size),
            _ => {
                println!("[USB FATAL] BAR0 is not an MMIO register range!");
                return;
            }
        },
        None => {
            println!("[USB FATAL] xHCI controller has no BAR0 configured!");
            return;
        }
    };

    println!(
        "[USB] xHCI BAR0 MMIO at physical address {:#014x} (size: {} KiB)",
        mmio_phys, mmio_size / 1024
    );

    // Enable PCI Bus Mastering and Memory Space
    dev.enable_bus_mastering();
    dev.enable_memory_space();

    // 3. Map BAR0 MMIO pages into the kernel virtual address space (No-Cache, NX)
    if let Err(e) = memory::identity_map_range(
        PhysAddr::new(mmio_phys),
        mmio_size as u64,
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::NO_CACHE
            | PageTableFlags::NO_EXECUTE,
    ) {
        println!("[USB FATAL] Failed to map xHCI MMIO memory: {}", e);
        return;
    }

    let mmio_base = mmio_phys as *mut u8;

    // 4. Read Capability Registers
    let cap_hci = unsafe { mmio_read32(mmio_base, 0x00) };
    let caplength = (cap_hci & 0xFF) as u8;
    let hciversion = ((cap_hci >> 16) & 0xFFFF) as u16;
    let hcsparams1 = unsafe { mmio_read32(mmio_base, 0x04) };
    let hcsparams2 = unsafe { mmio_read32(mmio_base, 0x08) };
    let hccparams1 = unsafe { mmio_read32(mmio_base, 0x10) };
    let dboff = unsafe { mmio_read32(mmio_base, 0x14) } as usize;
    let rtsoff = unsafe { mmio_read32(mmio_base, 0x18) } as usize;

    let max_slots = (hcsparams1 & 0xFF) as u8;
    let max_intrs = ((hcsparams1 >> 8) & 0x7FF) as u16;
    let max_ports = ((hcsparams1 >> 24) & 0xFF) as u8;

    println!(
        "[USB] xHCI Spec Version: {}.{}, MaxSlots: {}, MaxPorts: {}, MaxIntrs: {}",
        (hciversion >> 8) & 0xFF,
        hciversion & 0xFF,
        max_slots,
        max_ports,
        max_intrs
    );

    let op_base = unsafe { mmio_base.add(caplength as usize) };
    let rt_base = unsafe { mmio_base.add(rtsoff) };
    let db_base = unsafe { mmio_base.add(dboff) };

    // 5. BIOS-to-OS Hand-off via xECP (USB Legacy Support)
    let xecp = ((hccparams1 >> 16) & 0xFFFF) as usize;
    if xecp > 0 {
        let xecp_offset = xecp * 4;
        let legsup = unsafe { mmio_read32(mmio_base, xecp_offset) };
        if (legsup & 0xFF) == 1 {
            // Cap ID 1 is USB Legacy Support
            if (legsup & (1 << 24)) != 0 {
                // BIOS Owned Semaphore set: request OS ownership!
                println!("[USB] BIOS-owned xHCI detected. Initiating OS ownership handoff...");
                unsafe {
                    mmio_write32(mmio_base, xecp_offset, legsup | (1 << 24) | (1 << 16));
                }
                // Wait for BIOS to release ownership
                let mut handoff_done = false;
                for _ in 0..10_000 {
                    let cur = unsafe { mmio_read32(mmio_base, xecp_offset) };
                    if (cur & (1 << 24)) == 0 {
                        handoff_done = true;
                        break;
                    }
                    core::hint::spin_loop();
                }
                if handoff_done {
                    println!("[USB] OS ownership established successfully.");
                } else {
                    println!("[USB] Warning: BIOS handoff timed out, proceeding anyway.");
                }
            }
        }
    }

    // 6. Stop and Reset Controller
    // Stop: write USBCMD.RS = 0
    let mut usbcmd = unsafe { mmio_read32(op_base, REG_USBCMD) };
    usbcmd &= !USBCMD_RS;
    unsafe { mmio_write32(op_base, REG_USBCMD, usbcmd); }

    // Wait for USBSTS.HCH to become 1
    for _ in 0..50_000 {
        let usbsts = unsafe { mmio_read32(op_base, REG_USBSTS) };
        if (usbsts & USBSTS_HCH) != 0 {
            break;
        }
        core::hint::spin_loop();
    }

    // Reset: write USBCMD.HCRST = 1
    unsafe { mmio_write32(op_base, REG_USBCMD, USBCMD_HCRST); }

    // Wait for HCRST and CNR (Controller Not Ready) to clear to 0
    let mut reset_ok = false;
    for _ in 0..100_000 {
        let cmd = unsafe { mmio_read32(op_base, REG_USBCMD) };
        let sts = unsafe { mmio_read32(op_base, REG_USBSTS) };
        if (cmd & USBCMD_HCRST) == 0 && (sts & USBSTS_CNR) == 0 {
            reset_ok = true;
            break;
        }
        core::hint::spin_loop();
    }

    if !reset_ok {
        println!("[USB FATAL] xHCI controller reset timed out!");
        return;
    }
    println!("[USB] xHCI controller reset complete.");

    // 7. Configure MaxSlots in CONFIG register
    unsafe {
        mmio_write32(op_base, REG_CONFIG, max_slots as u32);
    }

    // 8. Allocate and Initialize DCBAA (Device Context Base Address Array)
    let dcbaa_frame = match memory::alloc_frame() {
        Some(f) => f,
        None => {
            println!("[USB FATAL] Out of physical frames for DCBAA!");
            return;
        }
    };
    let dcbaa_ptr = dcbaa_frame.start_address().as_u64() as *mut u64;
    unsafe {
        core::ptr::write_bytes(dcbaa_ptr as *mut u8, 0, PAGE_SIZE_4K as usize);
    }

    // Allocate Scratchpad Buffers if required by controller
    let max_scratchpads = (((hcsparams2 >> 21) & 0x1F) | (((hcsparams2 >> 27) & 0x1F) << 5)) as usize;
    if max_scratchpads > 0 {
        if let Some(scratch_table_frame) = memory::alloc_frame() {
            let scratch_table = scratch_table_frame.start_address().as_u64() as *mut u64;
            for i in 0..max_scratchpads.min(512) {
                if let Some(buf_frame) = memory::alloc_frame() {
                    unsafe {
                        *scratch_table.add(i) = buf_frame.start_address().as_u64();
                    }
                }
            }
            // DCBAA[0] points to scratchpad array
            unsafe {
                *dcbaa_ptr = scratch_table_frame.start_address().as_u64();
            }
        }
    }

    // Write DCBAAP
    unsafe {
        mmio_write64(op_base, REG_DCBAAP, dcbaa_frame.start_address().as_u64());
    }

    // 9. Allocate Command Ring
    let command_ring = match TrbRing::new() {
        Some(ring) => ring,
        None => {
            println!("[USB FATAL] Out of physical frames for Command Ring!");
            return;
        }
    };

    // Write CRCR (Command Ring Control Register)
    // Bit 0 = RCS (Ring Cycle State = 1)
    let crcr_val = command_ring.phys_addr() | 1;
    unsafe {
        mmio_write64(op_base, REG_CRCR, crcr_val);
    }

    // 10. Allocate Event Ring and Event Ring Segment Table (ERST)
    let event_ring_frame = match memory::alloc_frame() {
        Some(f) => f,
        None => {
            println!("[USB FATAL] Out of physical frames for Event Ring!");
            return;
        }
    };
    let event_ring_ptr = event_ring_frame.start_address().as_u64() as *mut Trb;
    unsafe {
        core::ptr::write_bytes(event_ring_ptr as *mut u8, 0, PAGE_SIZE_4K as usize);
    }

    let erst_frame = match memory::alloc_frame() {
        Some(f) => f,
        None => {
            println!("[USB FATAL] Out of physical frames for ERST!");
            return;
        }
    };
    // ERST Entry: Base Address (u64), Size (u32), Reserved (u32)
    unsafe {
        let erst_ptr = erst_frame.start_address().as_u64() as *mut u64;
        *erst_ptr = event_ring_frame.start_address().as_u64();
        let erst_size_ptr = erst_ptr.add(1) as *mut u32;
        *erst_size_ptr = RING_TRB_COUNT as u32;
        *erst_size_ptr.add(1) = 0;
    }

    // Program Interrupter 0 Registers
    // ERSTSZ = 1 segment
    unsafe {
        mmio_write32(rt_base, 0x20 + 0x08, 1);
        // ERSTBA = Physical address of ERST
        mmio_write64(rt_base, 0x20 + 0x10, erst_frame.start_address().as_u64());
        // ERDP = Physical address of Event Ring
        mmio_write64(rt_base, 0x20 + 0x18, event_ring_frame.start_address().as_u64());
        // IMAN = Interrupter Enable (bit 1)
        mmio_write32(rt_base, 0x20 + 0x00, 2);
    }

    // 11. Start Controller (USBCMD.RS = 1, INTE = 1)
    unsafe {
        let mut cmd = mmio_read32(op_base, REG_USBCMD);
        cmd |= USBCMD_RS | USBCMD_INTE;
        mmio_write32(op_base, REG_USBCMD, cmd);
    }

    // Wait for USBSTS.HCH to clear to 0 (Controller Running)
    let mut running = false;
    for _ in 0..50_000 {
        let sts = unsafe { mmio_read32(op_base, REG_USBSTS) };
        if (sts & USBSTS_HCH) == 0 {
            running = true;
            break;
        }
        core::hint::spin_loop();
    }

    if !running {
        println!("[USB FATAL] xHCI controller failed to enter running state!");
        return;
    }
    println!("[USB] xHCI Controller running. Rings active.");

    let mut controller = XhciController {
        mmio_base,
        op_base,
        rt_base,
        db_base,
        caplength,
        hciversion,
        max_slots,
        max_ports,
        max_intrs,
        command_ring,
        event_ring_frame,
        event_ring: event_ring_ptr,
        event_dequeue_idx: 0,
        event_cycle_state: true,
        dcbaa_frame,
        devices: Vec::new(),
        input_context_frames: Vec::new(),
        device_context_frames: Vec::new(),
    };

    // 12. Scan Root Hub Ports and Enumerate Connected Devices
    controller.enumerate_root_hub();

    *XHCI.lock() = Some(controller);
    println!("[USB] xHCI initialization complete.");
}

impl XhciController {
    /// Scans all root hub ports for connected devices, resets them, and enumerates USB devices.
    pub fn enumerate_root_hub(&mut self) {
        println!("[USB] Scanning {} Root Hub ports for USB devices...", self.max_ports);

        for port in 1..=self.max_ports {
            let portsc = self.read_portsc(port);

            // Check if device is connected (CCS = 1)
            if (portsc & PORTSC_CCS) != 0 {
                println!("[USB] Device detected on Port {}. Issuing port reset...", port);

                // Issue Port Reset: set PR = 1 and preserve PP = 1
                self.write_portsc(port, PORTSC_PP | PORTSC_PR);

                // Wait for Port Reset to complete (PR clears to 0 and PED becomes 1)
                let mut enabled = false;
                for _ in 0..50_000 {
                    let cur = self.read_portsc(port);
                    if (cur & PORTSC_PR) == 0 && (cur & PORTSC_PED) != 0 {
                        enabled = true;
                        break;
                    }
                    core::hint::spin_loop();
                }

                if !enabled {
                    println!("[USB] Warning: Port {} reset/enable timed out.", port);
                    continue;
                }

                let cur_portsc = self.read_portsc(port);
                let speed = ((cur_portsc & PORTSC_SPEED_MASK) >> 10) as u8;
                let speed_str = match speed {
                    PORT_SPEED_FULL => "Full-Speed (12 Mbps)",
                    PORT_SPEED_LOW => "Low-Speed (1.5 Mbps)",
                    PORT_SPEED_HIGH => "High-Speed (480 Mbps)",
                    PORT_SPEED_SUPER => "SuperSpeed (5 Gbps)",
                    _ => "Unknown Speed",
                };
                println!("[USB] Port {} active. Negotiated speed: {}", port, speed_str);

                // Enumerate device on this port
                if let Err(e) = self.enumerate_device(port, speed) {
                    println!("[USB] Failed to enumerate device on Port {}: {}", port, e);
                }
            }
        }
    }

    /// Enumerates a device connected to `port`:
    /// 1. Issues Enable Slot command.
    /// 2. Initializes Device Context and Input Context.
    /// 3. Issues Address Device command.
    /// 4. Retrieves Device Descriptor via Control Transfer.
    /// 5. Configures device and binds HID boot drivers if keyboard/mouse.
    pub fn enumerate_device(&mut self, port: u8, speed: u8) -> Result<(), &'static str> {
        // Step 1: Enable Slot Command
        self.send_command(Trb::enable_slot());
        let completion = self.wait_command_completion(100_000)?;
        let slot_id = completion.slot_id();
        if slot_id == 0 {
            return Err("Controller assigned invalid Slot ID 0");
        }
        println!("[USB] Controller assigned Slot ID: {}", slot_id);

        // Step 2: Allocate Device Context and Input Context
        let dev_ctx_frame = memory::alloc_frame().ok_or("Out of memory for Device Context")?;
        let in_ctx_frame = memory::alloc_frame().ok_or("Out of memory for Input Context")?;

        let dev_ctx_phys = dev_ctx_frame.start_address().as_u64();
        let in_ctx_phys = in_ctx_frame.start_address().as_u64();

        unsafe {
            core::ptr::write_bytes(dev_ctx_phys as *mut u8, 0, PAGE_SIZE_4K as usize);
            core::ptr::write_bytes(in_ctx_phys as *mut u8, 0, PAGE_SIZE_4K as usize);
        }

        // Set DCBAA[slot_id] = Device Context physical address
        let dcbaa = self.dcbaa_frame.start_address().as_u64() as *mut u64;
        unsafe {
            *dcbaa.add(slot_id as usize) = dev_ctx_phys;
        }

        // Allocate EP0 Transfer Ring
        let mut ep0_ring = TrbRing::new().ok_or("Out of memory for EP0 Ring")?;

        // Populate Input Context
        // In 32-byte context geometry:
        // Offset 0: Input Control Context
        // Offset 32: Slot Context
        // Offset 64: Endpoint 0 Context
        let in_ctx = in_ctx_phys as *mut u32;
        unsafe {
            // Input Control Context: Add Context Flags = bit 0 (Slot) | bit 1 (EP0)
            *in_ctx.add(1) = (1 << 0) | (1 << 1);

            // Slot Context (offset 8 dwords = 32 bytes)
            let slot_ctx = in_ctx.add(8);
            // Dword 0: Context Entries = 1, Route String = 0, Speed = speed
            *slot_ctx.add(0) = ((speed as u32) << 20) | (1 << 27);
            // Dword 1: Root Hub Port Number
            *slot_ctx.add(1) = (port as u32) << 16;

            // Endpoint 0 Context (offset 16 dwords = 64 bytes)
            let ep0_ctx = in_ctx.add(16);
            let max_packet_size: u32 = match speed {
                PORT_SPEED_SUPER => 512,
                PORT_SPEED_HIGH => 64,
                PORT_SPEED_LOW => 8,
                _ => 64,
            };
            // Dword 1: Error Count = 3, EP Type = 4 (Control), Max Packet Size
            *ep0_ctx.add(1) = (3 << 1) | (4 << 3) | (max_packet_size << 16);
            // Dword 2: TR Dequeue Pointer Low | DCS = 1
            *ep0_ctx.add(2) = (ep0_ring.phys_addr() as u32) | 1;
            // Dword 3: TR Dequeue Pointer High
            *ep0_ctx.add(3) = (ep0_ring.phys_addr() >> 32) as u32;
            // Dword 4: Average TRB Length = 8
            *ep0_ctx.add(4) = 8;
        }

        // Step 3: Address Device Command (BSR = 0)
        self.send_command(Trb::address_device(in_ctx_phys, slot_id, false));
        self.wait_command_completion(100_000)?;
        println!("[USB] Slot {} addressed successfully.", slot_id);

        // Step 4: Request Device Descriptor (18 bytes) via EP0 Control Transfer
        let desc_frame = memory::alloc_frame().ok_or("Out of memory for descriptor buffer")?;
        let desc_phys = desc_frame.start_address().as_u64();
        let desc_ptr = desc_phys as *mut u8;

        unsafe {
            core::ptr::write_bytes(desc_ptr, 0, 64);
        }

        let setup_dev_desc = UsbSetupPacket::get_descriptor(DESC_TYPE_DEVICE, 0, 18);
        self.control_transfer(slot_id, &mut ep0_ring, setup_dev_desc, Some(desc_phys), 18, true)?;

        let dev_desc = unsafe { core::ptr::read_unaligned(desc_ptr as *const UsbDeviceDescriptor) };
        let vendor_id = dev_desc.vendor_id();
        let product_id = dev_desc.product_id();
        let class_code = dev_desc.device_class;
        let subclass = dev_desc.device_subclass;
        let protocol = dev_desc.device_protocol;
        let max_packet_size0 = dev_desc.max_packet_size0;
        println!(
            "[USB] Device Descriptor: Vendor 0x{:04x}, Product 0x{:04x}, Class: 0x{:02x} ({}), MaxPacket0: {}",
            vendor_id,
            product_id,
            class_code,
            dev_desc.class_name(),
            max_packet_size0
        );

        // Step 5: Request Configuration Descriptor & Interface Hierarchy
        let setup_cfg_desc = UsbSetupPacket::get_descriptor(DESC_TYPE_CONFIGURATION, 0, 9);
        self.control_transfer(slot_id, &mut ep0_ring, setup_cfg_desc, Some(desc_phys), 9, true)?;

        let cfg_desc = unsafe { core::ptr::read_unaligned(desc_ptr as *const UsbConfigDescriptor) };
        let total_cfg_len = cfg_desc.total_length().min(256);

        let setup_full_cfg = UsbSetupPacket::get_descriptor(DESC_TYPE_CONFIGURATION, 0, total_cfg_len);
        self.control_transfer(slot_id, &mut ep0_ring, setup_full_cfg, Some(desc_phys), total_cfg_len as u32, true)?;

        let cfg_slice = unsafe { core::slice::from_raw_parts(desc_ptr, total_cfg_len as usize) };
        let parsed = parse_configuration_blob(cfg_slice);

        let mut is_keyboard = false;
        let mut is_mouse = false;
        let mut ep_in_dci = 0;
        let mut ep_in_ring: Option<Arc<Spinlock<TrbRing>>> = None;
        let mut ep_in_buf: Option<PhysFrame> = None;

        if let Some((_, interfaces)) = parsed {
            for parsed_iface in interfaces {
                let iface = &parsed_iface.interface;
                if iface.is_boot_keyboard() {
                    is_keyboard = true;
                    println!("[USB] Discovered USB HID Boot Keyboard on Slot {}!", slot_id);
                } else if iface.is_boot_mouse() {
                    is_mouse = true;
                    println!("[USB] Discovered USB HID Boot Mouse on Slot {}!", slot_id);
                }

                // If HID, find Interrupt IN endpoint
                if (is_keyboard || is_mouse) && !parsed_iface.endpoints.is_empty() {
                    for ep in &parsed_iface.endpoints {
                        if ep.is_in() && ep.is_interrupt() {
                            ep_in_dci = ep.dci();
                            let interrupt_ring = TrbRing::new().ok_or("Out of memory for Interrupt Ring")?;
                            let buf_frame = memory::alloc_frame().ok_or("Out of memory for HID report buffer")?;
                            ep_in_buf = Some(buf_frame);
                            let ring_arc = Arc::new(Spinlock::new(interrupt_ring));
                            ep_in_ring = Some(ring_arc.clone());
                            break;
                        }
                    }
                }
            }
        }

        // Step 6: Set Configuration 1
        let setup_set_cfg = UsbSetupPacket::set_configuration(1);
        self.control_transfer(slot_id, &mut ep0_ring, setup_set_cfg, None, 0, false)?;
        println!("[USB] Slot {} configured (Configuration 1 active).", slot_id);

        // Step 7: Configure HID Boot Protocol & Idle if applicable
        if is_keyboard || is_mouse {
            // Set Protocol 0 (Boot Protocol)
            let setup_proto = UsbSetupPacket::set_protocol(0, 0);
            let _ = self.control_transfer(slot_id, &mut ep0_ring, setup_proto, None, 0, false);

            if is_keyboard {
                // Set Idle duration 0
                let setup_idle = UsbSetupPacket::set_idle(0, 0);
                let _ = self.control_transfer(slot_id, &mut ep0_ring, setup_idle, None, 0, false);
            }

            // Configure Interrupt IN Endpoint if discovered
            if ep_in_dci > 0 && ep_in_ring.is_some() && ep_in_buf.is_some() {
                let ring = ep_in_ring.as_ref().unwrap();
                let ring_phys = ring.lock().phys_addr();

                // Prepare Input Context for Configure Endpoint
                unsafe {
                    core::ptr::write_bytes(in_ctx_phys as *mut u8, 0, PAGE_SIZE_4K as usize);
                    let in_ctx_ctrl = in_ctx_phys as *mut u32;
                    // Add Context Flags: Slot (bit 0) | Endpoint DCI (bit DCI)
                    *in_ctx_ctrl.add(1) = (1 << 0) | (1 << ep_in_dci);

                    // Update Slot Context: Context Entries = max(context_entries, ep_in_dci)
                    let slot_ctx = in_ctx_ctrl.add(8);
                    *slot_ctx.add(0) = ((speed as u32) << 20) | ((ep_in_dci as u32) << 27);
                    *slot_ctx.add(1) = (port as u32) << 16;

                    // Endpoint Context for ep_in_dci (offset = (ep_in_dci + 1) * 8 dwords)
                    let ep_ctx = in_ctx_ctrl.add((ep_in_dci as usize + 1) * 8);
                    // Dword 0: Interval = 3 (8ms)
                    *ep_ctx.add(0) = 3 << 16;
                    // Dword 1: Error Count = 3, EP Type = 7 (Interrupt IN), Max Packet Size = 8
                    *ep_ctx.add(1) = (3 << 1) | (7 << 3) | (8 << 16);
                    // Dword 2: TR Dequeue Pointer Low | DCS = 1
                    *ep_ctx.add(2) = (ring_phys as u32) | 1;
                    // Dword 3: TR Dequeue Pointer High
                    *ep_ctx.add(3) = (ring_phys >> 32) as u32;
                    // Dword 4: Average TRB Length = 8
                    *ep_ctx.add(4) = 8;
                }

                // Send Configure Endpoint Command
                self.send_command(Trb::configure_endpoint(in_ctx_phys, slot_id));
                self.wait_command_completion(100_000)?;
                println!("[USB] Slot {} Interrupt IN Endpoint {} configured.", slot_id, ep_in_dci);

                // Queue first input polling transfer
                let mut r = ring.lock();
                let buf = ep_in_buf.as_ref().unwrap();
                let req_len = if is_keyboard { 8 } else { 4 };
                r.push(Trb::normal(buf.start_address().as_u64(), req_len, true));
                fence(Ordering::SeqCst);
                self.ring_doorbell(slot_id, ep_in_dci as u32);
            }
        }

        self.input_context_frames.push(in_ctx_frame);
        self.device_context_frames.push(dev_ctx_frame);

        self.devices.push(XhciUsbDevice {
            slot_id,
            port_num: port,
            speed,
            vendor_id,
            product_id,
            class_code,
            subclass,
            protocol,
            max_packet_size: max_packet_size0,
            is_keyboard,
            is_mouse,
            ep_in_ring,
            ep_in_dci,
            ep_in_buf,
        });

        Ok(())
    }
}
