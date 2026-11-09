//! # USB Standard Descriptors & Setup Packets
//!
//! Binary structures and parsing routines for USB 2.0 / 3.0 control transfers according to Chapter 9
//! of the Universal Serial Bus Specification:
//! - 8-byte Standard USB Setup Packet ([`UsbSetupPacket`]).
//! - 18-byte Device Descriptor ([`UsbDeviceDescriptor`]).
//! - 9-byte Configuration Descriptor ([`UsbConfigDescriptor`]).
//! - 9-byte Interface Descriptor ([`UsbInterfaceDescriptor`]).
//! - 7-byte Endpoint Descriptor ([`UsbEndpointDescriptor`]).
//! - 9-byte HID Descriptor ([`UsbHidDescriptor`]).
//!
//! ### Descriptor Hierarchy & Concatenation
//!
//! In USB architecture, issuing a `GET_DESCRIPTOR(CONFIGURATION)` request returns a single contiguous
//! binary blob containing:
//! 1. The [`UsbConfigDescriptor`].
//! 2. For each interface: a [`UsbInterfaceDescriptor`].
//! 3. Optional class descriptors (e.g. [`UsbHidDescriptor`]).
//! 4. For each endpoint in the interface: a [`UsbEndpointDescriptor`].
//!
//! The parser [`parse_configuration_blob`] walks this concatenated stream and groups endpoints
//! under their parent interfaces.

#![allow(dead_code)]

use alloc::vec::Vec;

// USB Descriptor Types (USB 2.0 Spec Section 9.4)
/// Device Descriptor Type code (0x01).
pub const DESC_TYPE_DEVICE: u8 = 1;
/// Configuration Descriptor Type code (0x02).
pub const DESC_TYPE_CONFIGURATION: u8 = 2;
/// String Descriptor Type code (0x03).
pub const DESC_TYPE_STRING: u8 = 3;
/// Interface Descriptor Type code (0x04).
pub const DESC_TYPE_INTERFACE: u8 = 4;
/// Endpoint Descriptor Type code (0x05).
pub const DESC_TYPE_ENDPOINT: u8 = 5;
/// Device Qualifier Descriptor Type code (0x06).
pub const DESC_TYPE_DEVICE_QUALIFIER: u8 = 6;
/// HID Class Descriptor Type code (0x21).
pub const DESC_TYPE_HID: u8 = 0x21;
/// HID Report Descriptor Type code (0x22).
pub const DESC_TYPE_REPORT: u8 = 0x22;

// USB Standard Request Codes
/// Get Status standard request (0x00).
pub const REQ_GET_STATUS: u8 = 0;
/// Clear Feature standard request (0x01).
pub const REQ_CLEAR_FEATURE: u8 = 1;
/// Set Feature standard request (0x03).
pub const REQ_SET_FEATURE: u8 = 3;
/// Set Address standard request (0x05).
pub const REQ_SET_ADDRESS: u8 = 5;
/// Get Descriptor standard request (0x06).
pub const REQ_GET_DESCRIPTOR: u8 = 6;
/// Set Descriptor standard request (0x07).
pub const REQ_SET_DESCRIPTOR: u8 = 7;
/// Get Configuration standard request (0x08).
pub const REQ_GET_CONFIGURATION: u8 = 8;
/// Set Configuration standard request (0x09).
pub const REQ_SET_CONFIGURATION: u8 = 9;
/// Get Interface standard request (0x0A).
pub const REQ_GET_INTERFACE: u8 = 10;
/// Set Interface standard request (0x0B).
pub const REQ_SET_INTERFACE: u8 = 11;

// USB HID Specific Request Codes
/// HID Get Report class request (0x01).
pub const HID_REQ_GET_REPORT: u8 = 0x01;
/// HID Get Idle class request (0x02).
pub const HID_REQ_GET_IDLE: u8 = 0x02;
/// HID Get Protocol class request (0x03).
pub const HID_REQ_GET_PROTOCOL: u8 = 0x03;
/// HID Set Report class request (0x09).
pub const HID_REQ_SET_REPORT: u8 = 0x09;
/// HID Set Idle class request (0x0A).
pub const HID_REQ_SET_IDLE: u8 = 0x0A;
/// HID Set Protocol class request (0x0B).
pub const HID_REQ_SET_PROTOCOL: u8 = 0x0B;

// USB Classes
/// Class specified on an interface-by-interface basis (0x00).
pub const USB_CLASS_PER_INTERFACE: u8 = 0x00;
/// Audio device class (0x01).
pub const USB_CLASS_AUDIO: u8 = 0x01;
/// Communications and CDC Control class (0x02).
pub const USB_CLASS_COMM: u8 = 0x02;
/// Human Interface Device (HID) class (0x03).
pub const USB_CLASS_HID: u8 = 0x03;
/// Physical device class (0x05).
pub const USB_CLASS_PHYSICAL: u8 = 0x05;
/// Imaging device class (0x06).
pub const USB_CLASS_IMAGE: u8 = 0x06;
/// Printer device class (0x07).
pub const USB_CLASS_PRINTER: u8 = 0x07;
/// Mass Storage device class (0x08).
pub const USB_CLASS_MASS_STORAGE: u8 = 0x08;
/// USB Hub class (0x09).
pub const USB_CLASS_HUB: u8 = 0x09;
/// CDC Data class (0x0A).
pub const USB_CLASS_CDC_DATA: u8 = 0x0A;
/// Smart Card device class (0x0B).
pub const USB_CLASS_SMART_CARD: u8 = 0x0B;
/// Video / Webcam class (0x0E).
pub const USB_CLASS_VIDEO: u8 = 0x0E;
/// Wireless Controller class (0xE0).
pub const USB_CLASS_WIRELESS: u8 = 0xE0;
/// Miscellaneous class (0xEF).
pub const USB_CLASS_MISC: u8 = 0xEF;

// HID Protocols
/// No specific HID protocol (0x00).
pub const HID_PROTOCOL_NONE: u8 = 0x00;
/// Standard USB Keyboard Boot Protocol (0x01).
pub const HID_PROTOCOL_KEYBOARD: u8 = 0x01;
/// Standard USB Mouse Boot Protocol (0x02).
pub const HID_PROTOCOL_MOUSE: u8 = 0x02;

/// Standard 8-byte USB Setup Packet placed into a Setup Stage TRB.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UsbSetupPacket {
    /// Request characteristics (direction, type, recipient).
    pub request_type: u8,
    /// Specific standard or class request code.
    pub request: u8,
    /// Word-sized parameter varying according to request.
    pub value: u16,
    /// Word-sized parameter typically specifying interface or endpoint index.
    pub index: u16,
    /// Number of bytes transferred in the subsequent Data Stage (if any).
    pub length: u16,
}

impl UsbSetupPacket {
    /// Converts setup packet into a 64-bit integer for immediate TRB data.
    pub fn as_u64(&self) -> u64 {
        unsafe { core::mem::transmute_copy(self) }
    }

    /// Creates a standard `GET_DESCRIPTOR` setup request.
    pub fn get_descriptor(desc_type: u8, desc_index: u8, length: u16) -> Self {
        Self {
            request_type: 0x80, // Device-to-Host | Standard | Device
            request: REQ_GET_DESCRIPTOR,
            value: ((desc_type as u16) << 8) | (desc_index as u16),
            index: 0,
            length,
        }
    }

    /// Creates a standard `SET_CONFIGURATION` setup request.
    pub fn set_configuration(config_val: u8) -> Self {
        Self {
            request_type: 0x00, // Host-to-Device | Standard | Device
            request: REQ_SET_CONFIGURATION,
            value: config_val as u16,
            index: 0,
            length: 0,
        }
    }

    /// Creates a class-specific `SET_PROTOCOL` setup request for USB HID Boot Protocol.
    pub fn set_protocol(interface: u16, protocol: u16) -> Self {
        Self {
            request_type: 0x21, // Host-to-Device | Class | Interface
            request: HID_REQ_SET_PROTOCOL,
            value: protocol,
            index: interface,
            length: 0,
        }
    }

    /// Creates a class-specific `SET_IDLE` setup request to suppress repetitive HID reports.
    pub fn set_idle(interface: u16, duration_and_report_id: u16) -> Self {
        Self {
            request_type: 0x21, // Host-to-Device | Class | Interface
            request: HID_REQ_SET_IDLE,
            value: duration_and_report_id,
            index: interface,
            length: 0,
        }
    }
}

/// Standard 18-byte USB Device Descriptor.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UsbDeviceDescriptor {
    /// Size of this descriptor in bytes (always 18).
    pub length: u8,
    /// Descriptor Type (always 0x01).
    pub descriptor_type: u8,
    /// USB Specification Release Number in BCD (e.g. 0x0200 = USB 2.0).
    pub bcd_usb: u16,
    /// Class code assigned by USB-IF.
    pub device_class: u8,
    /// Subclass code assigned by USB-IF.
    pub device_subclass: u8,
    /// Protocol code assigned by USB-IF.
    pub device_protocol: u8,
    /// Maximum packet size for Endpoint 0 (8, 16, 32, 64, or 512 for USB 3).
    pub max_packet_size0: u8,
    /// Vendor ID assigned by USB-IF.
    pub vendor_id: u16,
    /// Product ID assigned by the manufacturer.
    pub product_id: u16,
    /// Device release number in BCD.
    pub bcd_device: u16,
    /// Index of string descriptor describing manufacturer.
    pub manufacturer_idx: u8,
    /// Index of string descriptor describing product.
    pub product_idx: u8,
    /// Index of string descriptor describing serial number.
    pub serial_number_idx: u8,
    /// Number of possible configurations supported by this device.
    pub num_configurations: u8,
}

impl UsbDeviceDescriptor {
    /// Returns the 16-bit Vendor ID.
    pub fn vendor_id(&self) -> u16 {
        self.vendor_id
    }

    /// Returns the 16-bit Product ID.
    pub fn product_id(&self) -> u16 {
        self.product_id
    }

    /// Returns the BCD USB specification version.
    pub fn bcd_usb(&self) -> u16 {
        self.bcd_usb
    }

    /// Returns the human-readable category of the USB class.
    pub fn class_name(&self) -> &'static str {
        match self.device_class {
            USB_CLASS_PER_INTERFACE => "Per-Interface Defined",
            USB_CLASS_AUDIO => "Audio Device",
            USB_CLASS_COMM => "Communications / CDC",
            USB_CLASS_HID => "Human Interface Device (HID)",
            USB_CLASS_PHYSICAL => "Physical Feedback Device",
            USB_CLASS_IMAGE => "Imaging Device (Camera/Scanner)",
            USB_CLASS_PRINTER => "Printer Device",
            USB_CLASS_MASS_STORAGE => "Mass Storage Device (USB Flash/Drive)",
            USB_CLASS_HUB => "USB Hub",
            USB_CLASS_CDC_DATA => "CDC Data Device",
            USB_CLASS_SMART_CARD => "Smart Card Reader",
            USB_CLASS_VIDEO => "Video Camera / UVC",
            USB_CLASS_WIRELESS => "Wireless Controller (Bluetooth)",
            USB_CLASS_MISC => "Miscellaneous Device",
            _ => "Unknown USB Class",
        }
    }
}

/// Standard 9-byte USB Configuration Descriptor.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UsbConfigDescriptor {
    /// Size of this descriptor in bytes (always 9).
    pub length: u8,
    /// Descriptor Type (always 0x02).
    pub descriptor_type: u8,
    /// Total length of data returned for this configuration (including interfaces and endpoints).
    pub total_length: u16,
    /// Number of interfaces supported by this configuration.
    pub num_interfaces: u8,
    /// Value to use as an argument to `SET_CONFIGURATION` to select this configuration.
    pub configuration_value: u8,
    /// Index of string descriptor describing this configuration.
    pub configuration_idx: u8,
    /// Configuration characteristics (Self-powered, Remote Wakeup).
    pub attributes: u8,
    /// Maximum power consumption in 2mA units (USB 2.0) or 8mA units (USB 3.0).
    pub max_power: u8,
}

impl UsbConfigDescriptor {
    /// Total length of the configuration descriptor blob in bytes.
    pub fn total_length(&self) -> u16 {
        self.total_length
    }
}

/// Standard 9-byte USB Interface Descriptor.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UsbInterfaceDescriptor {
    /// Size of this descriptor in bytes (always 9).
    pub length: u8,
    /// Descriptor Type (always 0x04).
    pub descriptor_type: u8,
    /// Number of this interface (zero-based index).
    pub interface_number: u8,
    /// Value used to select this alternate setting.
    pub alternate_setting: u8,
    /// Number of endpoints used by this interface (excluding endpoint 0).
    pub num_endpoints: u8,
    /// Class code (e.g., 0x03 for HID).
    pub interface_class: u8,
    /// Subclass code (e.g., 0x01 for Boot Interface Subclass).
    pub interface_subclass: u8,
    /// Protocol code (e.g., 0x01 for Keyboard, 0x02 for Mouse).
    pub interface_protocol: u8,
    /// Index of string descriptor describing this interface.
    pub interface_idx: u8,
}

impl UsbInterfaceDescriptor {
    /// Checks if this interface belongs to the Human Interface Device (HID) class.
    pub fn is_hid(&self) -> bool {
        self.interface_class == USB_CLASS_HID
    }

    /// Checks if this interface is a standard USB HID Boot Keyboard.
    pub fn is_boot_keyboard(&self) -> bool {
        self.interface_class == USB_CLASS_HID
            && self.interface_subclass == 0x01
            && self.interface_protocol == HID_PROTOCOL_KEYBOARD
    }

    /// Checks if this interface is a standard USB HID Boot Mouse.
    pub fn is_boot_mouse(&self) -> bool {
        self.interface_class == USB_CLASS_HID
            && self.interface_subclass == 0x01
            && self.interface_protocol == HID_PROTOCOL_MOUSE
    }
}

/// Standard 7-byte USB Endpoint Descriptor.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UsbEndpointDescriptor {
    /// Size of this descriptor in bytes (always 7).
    pub length: u8,
    /// Descriptor Type (always 0x05).
    pub descriptor_type: u8,
    /// Endpoint address: bits 0..3 endpoint number, bit 7 direction (1 = IN, 0 = OUT).
    pub endpoint_address: u8,
    /// Endpoint attributes: bits 0..1 transfer type (0=Control, 1=Isoch, 2=Bulk, 3=Interrupt).
    pub attributes: u8,
    /// Maximum packet size this endpoint can send or receive.
    pub max_packet_size: u16,
    /// Polling interval for data transfers (in 1ms frames for full/low speed, or $2^{n-1} \times 125\,\mu\text{s}$ for high/super speed).
    pub interval: u8,
}

impl UsbEndpointDescriptor {
    /// Returns true if this is an IN endpoint (device to host).
    pub fn is_in(&self) -> bool {
        (self.endpoint_address & 0x80) != 0
    }

    /// Returns the 4-bit endpoint number (0..=15).
    pub fn endpoint_number(&self) -> u8 {
        self.endpoint_address & 0x0F
    }

    /// Returns the 2-bit transfer type (0=Control, 1=Isochronous, 2=Bulk, 3=Interrupt).
    pub fn transfer_type(&self) -> u8 {
        self.attributes & 0x03
    }

    /// Returns true if this is an Interrupt endpoint.
    pub fn is_interrupt(&self) -> bool {
        self.transfer_type() == 3
    }

    /// Computes the xHCI Device Context Index (DCI) for this endpoint:
    /// $\text{DCI} = \text{EP\_Num} \times 2 + (\text{if IN } 1 \text{ else } 0)$
    pub fn dci(&self) -> u8 {
        let ep_num = self.endpoint_number();
        let in_flag = if self.is_in() { 1 } else { 0 };
        ep_num * 2 + in_flag
    }
}

/// Standard 9-byte USB HID Descriptor.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UsbHidDescriptor {
    /// Size of this descriptor in bytes (always 9).
    pub length: u8,
    /// Descriptor Type (always 0x21).
    pub descriptor_type: u8,
    /// HID Class Specification release number in BCD.
    pub bcd_hid: u16,
    /// Country code for localized hardware.
    pub country_code: u8,
    /// Number of class descriptors (always at least 1 for Report Descriptor).
    pub num_descriptors: u8,
    /// Type of class descriptor (0x22 for Report).
    pub report_descriptor_type: u8,
    /// Total size of the HID Report descriptor in bytes.
    pub report_descriptor_length: u16,
}

/// Discovered USB Interface with all associated endpoints and optional HID descriptor.
#[derive(Clone, Debug)]
pub struct ParsedInterface {
    /// Interface descriptor metadata.
    pub interface: UsbInterfaceDescriptor,
    /// List of endpoint descriptors belonging to this interface.
    pub endpoints: Vec<UsbEndpointDescriptor>,
    /// Optional HID descriptor if interface class is HID.
    pub hid_descriptor: Option<UsbHidDescriptor>,
}

/// Parses the full binary blob returned by `GET_DESCRIPTOR(CONFIGURATION)`.
///
/// Unpacks the configuration header and iterates through variable-length descriptor chunks,
/// grouping endpoints under their corresponding interface descriptors.
pub fn parse_configuration_blob(data: &[u8]) -> Option<(UsbConfigDescriptor, Vec<ParsedInterface>)> {
    if data.len() < core::mem::size_of::<UsbConfigDescriptor>() {
        return None;
    }

    let config = unsafe { core::ptr::read_unaligned(data.as_ptr() as *const UsbConfigDescriptor) };
    if config.descriptor_type != DESC_TYPE_CONFIGURATION {
        return None;
    }

    let mut interfaces = Vec::new();
    let mut current_iface: Option<ParsedInterface> = None;

    let mut offset = config.length as usize;
    let limit = data.len().min(config.total_length as usize);

    while offset + 2 <= limit {
        let desc_len = data[offset] as usize;
        if desc_len == 0 || offset + desc_len > limit {
            break;
        }
        let desc_type = data[offset + 1];

        match desc_type {
            DESC_TYPE_INTERFACE => {
                if let Some(prev) = current_iface.take() {
                    interfaces.push(prev);
                }
                if desc_len >= core::mem::size_of::<UsbInterfaceDescriptor>() {
                    let iface = unsafe {
                        core::ptr::read_unaligned(
                            data[offset..].as_ptr() as *const UsbInterfaceDescriptor
                        )
                    };
                    current_iface = Some(ParsedInterface {
                        interface: iface,
                        endpoints: Vec::new(),
                        hid_descriptor: None,
                    });
                }
            }
            DESC_TYPE_ENDPOINT => {
                if desc_len >= core::mem::size_of::<UsbEndpointDescriptor>() {
                    let ep = unsafe {
                        core::ptr::read_unaligned(
                            data[offset..].as_ptr() as *const UsbEndpointDescriptor
                        )
                    };
                    if let Some(iface) = current_iface.as_mut() {
                        iface.endpoints.push(ep);
                    }
                }
            }
            DESC_TYPE_HID => {
                if desc_len >= core::mem::size_of::<UsbHidDescriptor>() {
                    let hid = unsafe {
                        core::ptr::read_unaligned(
                            data[offset..].as_ptr() as *const UsbHidDescriptor
                        )
                    };
                    if let Some(iface) = current_iface.as_mut() {
                        iface.hid_descriptor = Some(hid);
                    }
                }
            }
            _ => {}
        }

        offset += desc_len;
    }

    if let Some(last) = current_iface {
        interfaces.push(last);
    }

    Some((config, interfaces))
}
