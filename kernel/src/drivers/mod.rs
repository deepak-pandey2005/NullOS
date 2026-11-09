//! # NullOS Device Driver Subsystem
//!
//! ## Overview
//! This subsystem encapsulates hardware device drivers and bus abstractions for NullOS:
//! - **[`serial`]**: 16550 UART serial driver for COM1 debug logging and kernel panic output.
//! - **[`console`]**: VGA graphical console with text rendering, scrolling, and ANSI escape sequences.
//! - **[`keyboard`]**: PS/2 keyboard controller driver with scancode set 1 decoding and modifier tracking.
//! - **[`pci`]**: PCI configuration space mechanism #1 and device bus enumeration.
//! - **[`block`]**: Abstract [`block::BlockDevice`] trait interface for persistent storage.
//! - **[`virtio_blk`]**: VirtIO modern PCI block storage driver implementing the BlockDevice trait.
//! - **[`virtio_net`]**: VirtIO modern PCI network adapter driver with transmit/receive virtqueues.
//! - **[`usb`]**: Extensible Host Controller Interface (USB 3.0 xHCI) and USB HID mouse/keyboard drivers.
//! - **[`audio`]**: Intel High Definition Audio (HDA) controller and PCM audio generation.

pub mod audio;
pub mod block;
pub mod console;
pub mod keyboard;
pub mod pci;
pub mod serial;
pub mod usb;
pub mod virtio_blk;
pub mod virtio_net;


