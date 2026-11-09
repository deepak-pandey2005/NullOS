//! # USB 3.0 xHCI Transfer Request Block (TRB) Definitions
//!
//! Implements TRB data structures according to the xHCI Specification 1.2:
//! - 16-byte fixed-size descriptors.
//! - **Command Ring TRBs**: Enable Slot, Disable Slot, Address Device, Configure Endpoint, Evaluate Context, No-Op.
//! - **Transfer Ring TRBs**: Normal, Setup Stage, Data Stage, Status Stage, Link TRB.
//! - **Event Ring TRBs**: Command Completion, Transfer Event, Port Status Change.
//! - Producer Cycle State (PCS) tracking and completion codes.
//!
//! ### TRB Structure Layout (16 Bytes)
//!
//! ```text
//! Byte 0..7   : Parameter (64-bit Physical Address, Data Buffer, or Immediate Data)
//! Byte 8..11  : Status (Transfer Length, Interrupter Target, Completion Code)
//! Byte 12..15 : Control (Cycle Bit, Chain, IOC, TRB Type, Direction, Slot/Endpoint ID)
//! ```

#![allow(dead_code)]

/// TRB Type: Normal Transfer TRB (Bulk, Interrupt).
pub const TRB_TYPE_NORMAL: u32 = 1;
/// TRB Type: Setup Stage for USB Control transfers on EP0.
pub const TRB_TYPE_SETUP_STAGE: u32 = 2;
/// TRB Type: Data Stage for USB Control transfers on EP0.
pub const TRB_TYPE_DATA_STAGE: u32 = 3;
/// TRB Type: Status Stage for USB Control transfers on EP0.
pub const TRB_TYPE_STATUS_STAGE: u32 = 4;
/// TRB Type: Isochronous Transfer TRB.
pub const TRB_TYPE_ISOCH: u32 = 5;
/// TRB Type: Link TRB connecting segments of a ring.
pub const TRB_TYPE_LINK: u32 = 6;
/// TRB Type: Event Data TRB.
pub const TRB_TYPE_EVENT_DATA: u32 = 7;
/// TRB Type: No-Op Command TRB.
pub const TRB_TYPE_NOOP_COMMAND: u32 = 8;
/// TRB Type: Enable Slot Command TRB.
pub const TRB_TYPE_ENABLE_SLOT: u32 = 9;
/// TRB Type: Disable Slot Command TRB.
pub const TRB_TYPE_DISABLE_SLOT: u32 = 10;
/// TRB Type: Address Device Command TRB.
pub const TRB_TYPE_ADDRESS_DEVICE: u32 = 11;
/// TRB Type: Configure Endpoint Command TRB.
pub const TRB_TYPE_CONFIGURE_ENDPOINT: u32 = 12;
/// TRB Type: Evaluate Context Command TRB.
pub const TRB_TYPE_EVALUATE_CONTEXT: u32 = 13;
/// TRB Type: Reset Endpoint Command TRB.
pub const TRB_TYPE_RESET_ENDPOINT: u32 = 14;
/// TRB Type: Stop Endpoint Command TRB.
pub const TRB_TYPE_STOP_ENDPOINT: u32 = 15;
/// TRB Type: Set TR Dequeue Pointer Command TRB.
pub const TRB_TYPE_SET_TR_DEQUEUE: u32 = 16;
/// TRB Type: Reset Device Command TRB.
pub const TRB_TYPE_RESET_DEVICE: u32 = 17;
/// TRB Type: Force Event Command TRB.
pub const TRB_TYPE_FORCE_EVENT: u32 = 18;
/// TRB Type: Transfer Event (generated on Event Ring).
pub const TRB_TYPE_TRANSFER_EVENT: u32 = 32;
/// TRB Type: Command Completion Event.
pub const TRB_TYPE_COMMAND_COMPLETION: u32 = 33;
/// TRB Type: Port Status Change Event.
pub const TRB_TYPE_PORT_STATUS_CHANGE: u32 = 34;
/// TRB Type: Bandwidth Request Event.
pub const TRB_TYPE_BANDWIDTH_REQUEST: u32 = 35;
/// TRB Type: Doorbell Event.
pub const TRB_TYPE_DOORBELL_EVENT: u32 = 36;
/// TRB Type: Host Controller Event.
pub const TRB_TYPE_HOST_CONTROLLER_EVENT: u32 = 37;
/// TRB Type: Device Notification Event.
pub const TRB_TYPE_DEVICE_NOTIFICATION: u32 = 38;
/// TRB Type: Microframe Index Wrap Event.
pub const TRB_TYPE_MFINDEX_WRAP: u32 = 39;

// TRB Control Flags
/// Cycle bit (bit 0): toggled to mark TRB ownership between software and hardware.
pub const TRB_FLAG_CYCLE: u32 = 1 << 0;
/// Toggle Cycle bit (bit 1): Used in Link TRB to invert the cycle state upon ring wrap.
pub const TRB_FLAG_TOGGLE_CYCLE: u32 = 1 << 1;
/// Interrupt on Short Packet (ISP) flag.
pub const TRB_FLAG_ISP: u32 = 1 << 2;
/// Chain bit (CHAIN): links consecutive TRBs into an atomic transaction.
pub const TRB_FLAG_CHAIN: u32 = 1 << 4;
/// Interrupt On Completion (IOC): instructs controller to issue an event on completion.
pub const TRB_FLAG_IOC: u32 = 1 << 5;
/// Immediate Data (IDT): parameter field holds raw data bytes rather than a pointer.
pub const TRB_FLAG_IDT: u32 = 1 << 6;
/// Block Set Address Request (BSR): do not issue USB SET_ADDRESS request to physical device.
pub const TRB_FLAG_BSR: u32 = 1 << 9;
/// Direction bit: `1` indicates device-to-host (IN), `0` indicates host-to-device (OUT).
pub const TRB_FLAG_DIR_IN: u32 = 1 << 16;

// xHCI Completion Codes (xHCI Spec Section 6.4.5)
/// Invalid completion code.
pub const CC_INVALID: u8 = 0;
/// Transaction completed successfully.
pub const CC_SUCCESS: u8 = 1;
/// DMA data buffer overrun/underrun error.
pub const CC_DATA_BUFFER_ERROR: u8 = 2;
/// Babble detected on the USB bus.
pub const CC_BABBLE_DETECTED: u8 = 3;
/// USB transaction error (CRC, timeout, PID corruption).
pub const CC_USB_TRANSACTION_ERROR: u8 = 4;
/// TRB structure malformed or invalid type.
pub const CC_TRB_ERROR: u8 = 5;
/// Endpoint returned a STALL handshake.
pub const CC_STALL_ERROR: u8 = 6;
/// Host controller internal resource exhaustion.
pub const CC_RESOURCE_ERROR: u8 = 7;
/// Insufficient periodic bus bandwidth.
pub const CC_BANDWIDTH_ERROR: u8 = 8;
/// All hardware device slots are exhausted.
pub const CC_NO_SLOTS_AVAILABLE: u8 = 9;
/// Target slot is in Disabled state.
pub const CC_SLOT_NOT_ENABLED: u8 = 11;
/// Target endpoint is not enabled or configured.
pub const CC_ENDPOINT_NOT_ENABLED: u8 = 12;
/// Inbound transfer received fewer bytes than buffer size (short packet).
pub const CC_SHORT_PACKET: u8 = 13;
/// Parameter error in input context.
pub const CC_PARAMETER_ERROR: u8 = 17;
/// Context state transitions invalid.
pub const CC_CONTEXT_STATE_ERROR: u8 = 19;
/// Event ring is completely full.
pub const CC_EVENT_RING_FULL: u8 = 21;
/// Command ring stopped by driver.
pub const CC_COMMAND_RING_STOPPED: u8 = 24;
/// Command execution aborted.
pub const CC_COMMAND_ABORTED: u8 = 25;
/// Stopped by command.
pub const CC_STOPPED: u8 = 26;

/// 16-byte Transfer Request Block (TRB) aligned to 16 bytes.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Trb {
    /// Parameter field: physical memory pointer or immediate data.
    pub parameter: u64,
    /// Status field: transfer length, completion code, and interrupter target.
    pub status: u32,
    /// Control field: TRB type, cycle state, IOC, and endpoint/slot routing.
    pub control: u32,
}

impl Trb {
    /// Constructs a blank, zeroed TRB.
    pub const fn empty() -> Self {
        Self {
            parameter: 0,
            status: 0,
            control: 0,
        }
    }

    /// Returns the TRB Type (bits 10..15 of control).
    #[inline]
    pub fn trb_type(&self) -> u32 {
        (self.control >> 10) & 0x3F
    }

    /// Returns the Cycle Bit (bit 0 of control).
    #[inline]
    pub fn cycle_bit(&self) -> bool {
        (self.control & TRB_FLAG_CYCLE) != 0
    }

    /// Sets or clears the Cycle Bit.
    #[inline]
    pub fn set_cycle_bit(&mut self, c: bool) {
        if c {
            self.control |= TRB_FLAG_CYCLE;
        } else {
            self.control &= !TRB_FLAG_CYCLE;
        }
    }

    /// Returns the completion code from an Event TRB (bits 24..31 of status).
    #[inline]
    pub fn completion_code(&self) -> u8 {
        ((self.status >> 24) & 0xFF) as u8
    }

    /// Returns a human-readable name for the completion code.
    pub fn completion_code_name(&self) -> &'static str {
        match self.completion_code() {
            CC_SUCCESS => "Success",
            CC_DATA_BUFFER_ERROR => "Data Buffer Error",
            CC_BABBLE_DETECTED => "Babble Detected",
            CC_USB_TRANSACTION_ERROR => "USB Transaction Error",
            CC_TRB_ERROR => "TRB Error",
            CC_STALL_ERROR => "Stall Error",
            CC_RESOURCE_ERROR => "Resource Error",
            CC_BANDWIDTH_ERROR => "Bandwidth Error",
            CC_NO_SLOTS_AVAILABLE => "No Slots Available",
            CC_SLOT_NOT_ENABLED => "Slot Not Enabled",
            CC_ENDPOINT_NOT_ENABLED => "Endpoint Not Enabled",
            CC_SHORT_PACKET => "Short Packet",
            CC_PARAMETER_ERROR => "Parameter Error",
            CC_CONTEXT_STATE_ERROR => "Context State Error",
            CC_EVENT_RING_FULL => "Event Ring Full",
            CC_COMMAND_RING_STOPPED => "Command Ring Stopped",
            CC_COMMAND_ABORTED => "Command Aborted",
            CC_STOPPED => "Stopped",
            _ => "Unknown Completion Code",
        }
    }

    /// Returns the Slot ID from a Command Completion or Transfer Event TRB (bits 24..31 of control).
    #[inline]
    pub fn slot_id(&self) -> u8 {
        ((self.control >> 24) & 0xFF) as u8
    }

    /// Returns the Endpoint ID from a Transfer Event TRB (bits 16..20 of control).
    #[inline]
    pub fn endpoint_id(&self) -> u8 {
        ((self.control >> 16) & 0x1F) as u8
    }

    /// Returns the Port ID from a Port Status Change TRB (bits 24..31 of parameter).
    #[inline]
    pub fn port_id(&self) -> u8 {
        ((self.parameter >> 24) & 0xFF) as u8
    }

    /// Returns remaining transfer length from a Transfer Event TRB (bits 0..23 of status).
    #[inline]
    pub fn remaining_length(&self) -> u32 {
        self.status & 0xFF_FFFF
    }

    // --- Command TRB Constructors ---

    /// Creates an "Enable Slot" Command TRB.
    pub fn enable_slot() -> Self {
        Self {
            parameter: 0,
            status: 0,
            control: TRB_TYPE_ENABLE_SLOT << 10,
        }
    }

    /// Creates a "Disable Slot" Command TRB.
    pub fn disable_slot(slot_id: u8) -> Self {
        Self {
            parameter: 0,
            status: 0,
            control: (TRB_TYPE_DISABLE_SLOT << 10) | ((slot_id as u32) << 24),
        }
    }

    /// Creates an "Address Device" Command TRB.
    pub fn address_device(input_context_phys_addr: u64, slot_id: u8, block_set_address: bool) -> Self {
        let mut ctrl = (TRB_TYPE_ADDRESS_DEVICE << 10) | ((slot_id as u32) << 24);
        if block_set_address {
            ctrl |= TRB_FLAG_BSR;
        }
        Self {
            parameter: input_context_phys_addr,
            status: 0,
            control: ctrl,
        }
    }

    /// Creates a "Configure Endpoint" Command TRB.
    pub fn configure_endpoint(input_context_phys_addr: u64, slot_id: u8) -> Self {
        Self {
            parameter: input_context_phys_addr,
            status: 0,
            control: (TRB_TYPE_CONFIGURE_ENDPOINT << 10) | ((slot_id as u32) << 24),
        }
    }

    /// Creates an "Evaluate Context" Command TRB.
    pub fn evaluate_context(input_context_phys_addr: u64, slot_id: u8) -> Self {
        Self {
            parameter: input_context_phys_addr,
            status: 0,
            control: (TRB_TYPE_EVALUATE_CONTEXT << 10) | ((slot_id as u32) << 24),
        }
    }

    /// Creates a "No-Op" Command TRB.
    pub fn noop() -> Self {
        Self {
            parameter: 0,
            status: 0,
            control: TRB_TYPE_NOOP_COMMAND << 10,
        }
    }

    // --- Transfer TRB Constructors ---

    /// Creates a Link TRB pointing to the start of the ring to wrap around.
    pub fn link(target_phys_addr: u64, toggle_cycle: bool) -> Self {
        let mut ctrl = TRB_TYPE_LINK << 10;
        if toggle_cycle {
            ctrl |= TRB_FLAG_TOGGLE_CYCLE;
        }
        Self {
            parameter: target_phys_addr,
            status: 0,
            control: ctrl,
        }
    }

    /// Creates a "Setup Stage" TRB for USB Control transfers on EP0.
    /// `transfer_type`: 0 = No Data, 2 = OUT Data Stage, 3 = IN Data Stage.
    pub fn setup_stage(setup_data: u64, transfer_type: u32, immediate: bool) -> Self {
        let mut ctrl = (TRB_TYPE_SETUP_STAGE << 10) | ((transfer_type & 0x03) << 16);
        if immediate {
            ctrl |= TRB_FLAG_IDT;
        }
        Self {
            parameter: setup_data,
            status: 8, // 8-byte setup packet length in bits 0..16
            control: ctrl,
        }
    }

    /// Creates a "Data Stage" TRB for Control transfers.
    pub fn data_stage(buf_phys_addr: u64, length: u32, direction_in: bool) -> Self {
        let mut ctrl = TRB_TYPE_DATA_STAGE << 10;
        if direction_in {
            ctrl |= TRB_FLAG_DIR_IN;
        }
        Self {
            parameter: buf_phys_addr,
            status: length & 0x1FFFF,
            control: ctrl,
        }
    }

    /// Creates a "Status Stage" TRB for completing a Control transfer.
    pub fn status_stage(direction_in: bool) -> Self {
        let mut ctrl = (TRB_TYPE_STATUS_STAGE << 10) | TRB_FLAG_IOC;
        if direction_in {
            ctrl |= TRB_FLAG_DIR_IN;
        }
        Self {
            parameter: 0,
            status: 0,
            control: ctrl,
        }
    }

    /// Creates a "Normal" TRB for Bulk or Interrupt data transfers.
    pub fn normal(buf_phys_addr: u64, length: u32, ioc: bool) -> Self {
        let mut ctrl = TRB_TYPE_NORMAL << 10;
        if ioc {
            ctrl |= TRB_FLAG_IOC;
        }
        Self {
            parameter: buf_phys_addr,
            status: length & 0x1FFFF,
            control: ctrl,
        }
    }
}
