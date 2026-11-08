//! USB Standard Descriptors & Setup Packets (Phase 12 - M31).
//!
//! Binary structures and parsers for USB 2.0 / 3.0 control transfers:
//! - 8-byte Standard USB Setup Packet (`UsbSetupPacket`).
//! - 18-byte Device Descriptor (`UsbDeviceDescriptor`).
//! - 9-byte Configuration Descriptor (`UsbConfigDescriptor`).
//! - 9-byte Interface Descriptor (`UsbInterfaceDescriptor`).
//! - 7-byte Endpoint Descriptor (`UsbEndpointDescriptor`).
//! - 9-byte HID Descriptor (`UsbHidDescriptor`).

#![allow(dead_code)]

use alloc::vec::Vec;

// USB Descriptor Types (USB 2.0 Spec Section 9.4)
pub const DESC_TYPE_DEVICE: u8 = 1;
pub const DESC_TYPE_CONFIGURATION: u8 = 2;
pub const DESC_TYPE_STRING: u8 = 3;
pub const DESC_TYPE_INTERFACE: u8 = 4;
pub const DESC_TYPE_ENDPOINT: u8 = 5;
pub const DESC_TYPE_DEVICE_QUALIFIER: u8 = 6;
pub const DESC_TYPE_HID: u8 = 0x21;
pub const DESC_TYPE_REPORT: u8 = 0x22;

// USB Standard Request Codes
pub const REQ_GET_STATUS: u8 = 0;
pub const REQ_CLEAR_FEATURE: u8 = 1;
pub const REQ_SET_FEATURE: u8 = 3;
pub const REQ_SET_ADDRESS: u8 = 5;
pub const REQ_GET_DESCRIPTOR: u8 = 6;
pub const REQ_SET_DESCRIPTOR: u8 = 7;
pub const REQ_GET_CONFIGURATION: u8 = 8;
pub const REQ_SET_CONFIGURATION: u8 = 9;
pub const REQ_GET_INTERFACE: u8 = 10;
pub const REQ_SET_INTERFACE: u8 = 11;

// USB HID Specific Request Codes
pub const HID_REQ_GET_REPORT: u8 = 0x01;
pub const HID_REQ_GET_IDLE: u8 = 0x02;
pub const HID_REQ_GET_PROTOCOL: u8 = 0x03;
pub const HID_REQ_SET_REPORT: u8 = 0x09;
pub const HID_REQ_SET_IDLE: u8 = 0x0A;
pub const HID_REQ_SET_PROTOCOL: u8 = 0x0B;

// USB Classes
pub const USB_CLASS_PER_INTERFACE: u8 = 0x00;
pub const USB_CLASS_AUDIO: u8 = 0x01;
pub const USB_CLASS_COMM: u8 = 0x02;
pub const USB_CLASS_HID: u8 = 0x03;
pub const USB_CLASS_PHYSICAL: u8 = 0x05;
pub const USB_CLASS_IMAGE: u8 = 0x06;
pub const USB_CLASS_PRINTER: u8 = 0x07;
pub const USB_CLASS_MASS_STORAGE: u8 = 0x08;
pub const USB_CLASS_HUB: u8 = 0x09;
pub const USB_CLASS_CDC_DATA: u8 = 0x0A;
pub const USB_CLASS_SMART_CARD: u8 = 0x0B;
pub const USB_CLASS_VIDEO: u8 = 0x0E;
pub const USB_CLASS_WIRELESS: u8 = 0xE0;
pub const USB_CLASS_MISC: u8 = 0xEF;

// HID Protocols
pub const HID_PROTOCOL_NONE: u8 = 0x00;
pub const HID_PROTOCOL_KEYBOARD: u8 = 0x01;
pub const HID_PROTOCOL_MOUSE: u8 = 0x02;

/// Standard 8-byte USB Setup Packet.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UsbSetupPacket {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

impl UsbSetupPacket {
    /// Converts setup packet into a 64-bit integer for immediate TRB data.
    pub fn as_u64(&self) -> u64 {
        unsafe { core::mem::transmute_copy(self) }
    }

    /// Creates a GET_DESCRIPTOR setup request.
    pub fn get_descriptor(desc_type: u8, desc_index: u8, length: u16) -> Self {
        Self {
            request_type: 0x80, // Device-to-Host | Standard | Device
            request: REQ_GET_DESCRIPTOR,
            value: ((desc_type as u16) << 8) | (desc_index as u16),
            index: 0,
            length,
        }
    }

    /// Creates a SET_CONFIGURATION setup request.
    pub fn set_configuration(config_val: u8) -> Self {
        Self {
            request_type: 0x00, // Host-to-Device | Standard | Device
            request: REQ_SET_CONFIGURATION,
            value: config_val as u16,
            index: 0,
            length: 0,
        }
    }

    /// Creates a SET_PROTOCOL setup request (for USB HID Boot Protocol).
    pub fn set_protocol(interface: u16, protocol: u16) -> Self {
        Self {
            request_type: 0x21, // Host-to-Device | Class | Interface
            request: HID_REQ_SET_PROTOCOL,
            value: protocol,
            index: interface,
            length: 0,
        }
    }

    /// Creates a SET_IDLE setup request (suppresses repetitive HID reports).
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
    pub length: u8,
    pub descriptor_type: u8,
    pub bcd_usb: u16,
    pub device_class: u8,
    pub device_subclass: u8,
    pub device_protocol: u8,
    pub max_packet_size0: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub bcd_device: u16,
    pub manufacturer_idx: u8,
    pub product_idx: u8,
    pub serial_number_idx: u8,
    pub num_configurations: u8,
}

impl UsbDeviceDescriptor {
    pub fn vendor_id(&self) -> u16 {
        self.vendor_id
    }

    pub fn product_id(&self) -> u16 {
        self.product_id
    }

    pub fn bcd_usb(&self) -> u16 {
        self.bcd_usb
    }

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
    pub length: u8,
    pub descriptor_type: u8,
    pub total_length: u16,
    pub num_interfaces: u8,
    pub configuration_value: u8,
    pub configuration_idx: u8,
    pub attributes: u8,
    pub max_power: u8,
}

impl UsbConfigDescriptor {
    pub fn total_length(&self) -> u16 {
        self.total_length
    }
}

/// Standard 9-byte USB Interface Descriptor.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UsbInterfaceDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub interface_number: u8,
    pub alternate_setting: u8,
    pub num_endpoints: u8,
    pub interface_class: u8,
    pub interface_subclass: u8,
    pub interface_protocol: u8,
    pub interface_idx: u8,
}

impl UsbInterfaceDescriptor {
    pub fn is_hid(&self) -> bool {
        self.interface_class == USB_CLASS_HID
    }

    pub fn is_boot_keyboard(&self) -> bool {
        self.interface_class == USB_CLASS_HID
            && self.interface_subclass == 0x01
            && self.interface_protocol == HID_PROTOCOL_KEYBOARD
    }

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
    pub length: u8,
    pub descriptor_type: u8,
    pub endpoint_address: u8,
    pub attributes: u8,
    pub max_packet_size: u16,
    pub interval: u8,
}

impl UsbEndpointDescriptor {
    pub fn is_in(&self) -> bool {
        (self.endpoint_address & 0x80) != 0
    }

    pub fn endpoint_number(&self) -> u8 {
        self.endpoint_address & 0x0F
    }

    pub fn transfer_type(&self) -> u8 {
        self.attributes & 0x03
    }

    pub fn is_interrupt(&self) -> bool {
        self.transfer_type() == 3
    }

    /// Computes the xHCI Device Context Index (DCI) for this endpoint:
    /// DCI = EP_Num * 2 + (1 if IN else 0)
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
    pub length: u8,
    pub descriptor_type: u8,
    pub bcd_hid: u16,
    pub country_code: u8,
    pub num_descriptors: u8,
    pub report_descriptor_type: u8,
    pub report_descriptor_length: u16,
}

/// Discovered USB Interface with its endpoints.
#[derive(Clone, Debug)]
pub struct ParsedInterface {
    pub interface: UsbInterfaceDescriptor,
    pub endpoints: Vec<UsbEndpointDescriptor>,
    pub hid_descriptor: Option<UsbHidDescriptor>,
}

/// Parses the full binary blob returned by GET_DESCRIPTOR(CONFIGURATION).
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
