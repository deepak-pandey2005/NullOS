//! # NullOS Device Driver Subsystem
//!
//! ## Overview
//! This subsystem encapsulates hardware device drivers and bus abstractions for NullOS:
//! - **[`serial`]**: 16550 UART serial driver for COM1 debug logging and kernel panic output.
//! - **[`console`]**: VGA graphical console with text rendering, scrolling, and ANSI escape sequences.
//! - **[`keyboard`]**: PS/2 keyboard controller driver with scancode set 1 decoding and modifier tracking.
//! - **[`pci`]**: PCI configuration space mechanism #1 and device bus enumeration.
//! - **[`block`]**: Abstract [`block::BlockDevice`] trait interface for persistent storage.
//! - **[`ahci`]**: Serial ATA AHCI 1.3.1 storage controller driver.
//! - **[`nvme`]**: PCIe Non-Volatile Memory Express (NVMe) storage controller driver.
//! - **[`virtio_blk`]**: VirtIO modern PCI block storage driver implementing the BlockDevice trait.
//! - **[`virtio_net`]**: VirtIO modern PCI network adapter driver with transmit/receive virtqueues.
//! - **[`e1000`]**: Intel e1000 / e1000e bare-metal PCIe Gigabit Ethernet driver.
//! - **[`r8169`]**: Realtek RTL8168/RTL8111/RTL8169 bare-metal PCIe Gigabit Ethernet driver.
//! - **[`usb`]**: Extensible Host Controller Interface (USB 3.0 xHCI) and USB HID mouse/keyboard drivers.
//! - **[`audio`]**: Intel High Definition Audio (HDA) controller and PCM audio generation.

pub mod ahci;
pub mod audio;
pub mod block;
pub mod console;
pub mod e1000;
pub mod keyboard;
pub mod nvme;
pub mod pci;
pub mod r8169;
pub mod serial;
pub mod usb;
pub mod virtio_blk;
pub mod virtio_net;


