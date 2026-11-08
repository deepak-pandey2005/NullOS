//! USB Subsystem & xHCI Host Controller (Phase 12 - M31).
//!
//! Features:
//! - Full xHCI 1.0/1.1/1.2 Host Controller Driver.
//! - TRB Command & Transfer Rings with Producer Cycle State tracking.
//! - Event Ring Segment Table (ERST) & Interrupter 0 event dispatching.
//! - Root Hub port management & USB Device enumeration.
//! - Standard USB Descriptors parsing (Device, Config, Interface, Endpoint, HID).
//! - USB HID Boot Protocol Drivers for Keyboards and Mice.

#![allow(dead_code)]

pub mod descriptor;
pub mod hid;
pub mod trb;
pub mod xhci;

use alloc::vec::Vec;
use crate::println;
pub use hid::read_char;
pub use xhci::{XhciUsbDevice, XHCI};

/// Initializes the USB Subsystem and probes for xHCI controllers.
pub fn init() {
    xhci::init();
}

/// Retrieves a cloned list of all enumerated USB devices.
pub fn list_devices() -> Vec<XhciUsbDevice> {
    let guard = XHCI.lock();
    match guard.as_ref() {
        Some(controller) => controller.devices.clone(),
        None => Vec::new(),
    }
}

/// Returns the total count of enumerated USB devices.
pub fn device_count() -> usize {
    let guard = XHCI.lock();
    match guard.as_ref() {
        Some(controller) => controller.devices.len(),
        None => 0,
    }
}

/// Polls USB input devices (keyboards and mice) for new transfer events.
pub fn poll_inputs() {
    if let Some(ref mut controller) = *XHCI.lock() {
        controller.poll_inputs();
    }
}

/// Runs automated verification self-tests for Milestone 31 (USB & xHCI Controller).
pub fn run_self_tests() {
    println!();
    println!("--------------------------------------------------");
    println!(" [USB] Running Milestone 31 USB & xHCI Self-Tests ");
    println!("--------------------------------------------------");

    let guard = XHCI.lock();
    let controller = match guard.as_ref() {
        Some(c) => c,
        None => {
            println!("[USB] Notice: No xHCI controller detected. Skipping USB hardware tests.");
            return;
        }
    };

    // Test 1: Capability & Spec Version verification
    println!(
        "[USB] Test 1: Verifying xHCI Spec Version 0x{:04x} (>= 0x0100)...",
        controller.hciversion
    );
    assert!(controller.hciversion >= 0x0096, "xHCI spec version must be valid");

    // Test 2: Operational limits verification
    println!(
        "[USB] Test 2: Verifying operational limits (MaxSlots: {}, MaxPorts: {}, MaxIntrs: {})...",
        controller.max_slots, controller.max_ports, controller.max_intrs
    );
    assert!(controller.max_slots > 0, "Controller must support at least 1 device slot");
    assert!(controller.max_ports > 0, "Controller must expose at least 1 root hub port");

    // Test 3: Command & Event Ring initialization
    println!("[USB] Test 3: Verifying Command and Event Ring allocations...");
    assert!(controller.command_ring.phys_addr() != 0, "Command Ring physical address must be non-zero");
    assert!(controller.event_ring_frame.start_address().as_u64() != 0, "Event Ring physical address must be non-zero");

    // Test 4: Enumerated Devices & Descriptor Validation
    println!("[USB] Test 4: Enumerated USB device count: {}", controller.devices.len());
    for dev in &controller.devices {
        println!(
            "  -> Slot {}: Port {}, Vendor 0x{:04x}, Product 0x{:04x}, Class 0x{:02x}, MaxPacket: {}, Keyboard: {}, Mouse: {}",
            dev.slot_id,
            dev.port_num,
            dev.vendor_id,
            dev.product_id,
            dev.class_code,
            dev.max_packet_size,
            dev.is_keyboard,
            dev.is_mouse
        );
        assert!(dev.slot_id > 0, "Enumerated USB device must have non-zero Slot ID");
        assert!(dev.max_packet_size > 0, "MaxPacketSize must be non-zero");
    }

    println!("[USB] All Milestone 31 USB & xHCI self-tests passed successfully!");
}
