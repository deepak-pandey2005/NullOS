//! # Ethernet Layer 2 Frame Processing (Phase 8 - M24 Network Stack)
//!
//! This module implements **Ethernet II** (DIX Ethernet) frame encapsulation,
//! parsing, and dispatch to higher-level protocols (IPv4, ARP).
//!
//! ## Ethernet II Frame Format
//!
//! An Ethernet II frame carries a variable-length payload (46–1500 bytes).
//! The hardware NIC appends a 4-byte **Frame Check Sequence (FCS)** CRC32
//! *after* transmission; the FCS is stripped by the NIC on reception and is
//! therefore **never present** in the software-visible buffer.
//!
//! ```text
//! ┌──────────┬──────────┬───────────┬──────────────────────────┬─────────┐
//! │ Dst MAC  │ Src MAC  │ EtherType │       Payload (data)     │ FCS(*) │
//! │ 6 bytes  │ 6 bytes  │  2 bytes  │  46–1500 bytes           │ 4 bytes│
//! └──────────┴──────────┴───────────┴──────────────────────────┴─────────┘
//!   Bytes 0-5  Bytes 6-11  Bytes 12-13  Bytes 14 … (14+N-1)   (*) HW only
//! ```
//!
//! * **Destination MAC** (bytes 0–5): 48-bit hardware address of the intended
//!   recipient.  Can be a unicast address, the all-ones broadcast address
//!   (`FF:FF:FF:FF:FF:FF`), or a multicast address (bit 0 of byte 0 set).
//! * **Source MAC** (bytes 6–11): 48-bit hardware address of the sender NIC.
//!   Set by the transmitting software (or overridden by the NIC).
//! * **EtherType** (bytes 12–13): Identifies the protocol encapsulated in the
//!   payload.  Stored in **network byte order** (big-endian).  Values ≥ 1536
//!   (0x0600) are EtherType; values ≤ 1500 indicate an IEEE 802.3 length field
//!   (not used in NullOS).
//!
//! ## EtherType Values
//!
//! | Hex      | Decimal | Protocol                    |
//! |----------|---------|-----------------------------|
//! | `0x0800` | 2048    | Internet Protocol version 4 |
//! | `0x0806` | 2054    | Address Resolution Protocol  |
//! | `0x86DD` | 34525   | Internet Protocol version 6  |
//! | `0x8100` | 33024   | IEEE 802.1Q VLAN tagging      |
//! | `0x0835` | 2101    | RARP (Reverse ARP)            |
//!
//! ## Minimum Frame Size and Padding
//!
//! Ethernet requires a minimum **wire frame** of 64 bytes (60 bytes of header +
//! payload + 4-byte FCS).  The minimum *payload* is therefore **46 bytes**.  If
//! the payload is shorter, **zero-padding** is added so the total frame (before
//! FCS) is at least 60 bytes.  [`EthernetFrame::build`] handles this padding
//! automatically.
//!
//! ## MAC Address Note
//!
//! A MAC address is 6 bytes transmitted in **canonical bit order** (LSB first
//! per IEEE 802.3; most implementations show them MSB first per octet, but the
//! bits within each octet are reversed on the wire).  In NullOS we always treat
//! MACs as 6-byte arrays in the standard human-readable `AA:BB:CC:DD:EE:FF`
//! octet order.
//!
//! The broadcast address `FF:FF:FF:FF:FF:FF` is delivered to every station on
//! the same Layer-2 segment.  ARP requests are always broadcast; ARP replies are
//! unicast.

use alloc::vec::Vec;
use core::fmt;

use crate::drivers::virtio_net::{self, MacAddress};
use crate::println;

/// Length of a standard Ethernet II frame **header** in bytes.
///
/// Breakdown:
/// * 6 bytes – Destination MAC address
/// * 6 bytes – Source MAC address
/// * 2 bytes – EtherType field
///
/// Total = **14 bytes**.  The payload starts at byte offset 14.
pub const ETH_HDR_LEN: usize = 14;

/// Standard Ethernet II Protocol Type field values (**EtherType**).
///
/// The EtherType is a 16-bit big-endian field at bytes 12–13 of the Ethernet
/// frame.  It tells the receiver which network-layer protocol is carried inside
/// the frame's payload.
///
/// # Representation
///
/// `#[repr(u16)]` is declared so that the discriminant corresponds to the
/// wire-format value.  However, conversion to/from wire format always uses
/// [`EtherType::to_u16`] and [`EtherType::from_u16`] to handle the
/// `Unknown(u16)` variant and big-endian byte-swap correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum EtherType {
    /// Internet Protocol version 4 — `0x0800`.
    /// Carries IPv4 packets (which may themselves carry TCP, UDP, ICMP, etc.).
    Ipv4 = 0x0800,

    /// Address Resolution Protocol — `0x0806`.
    /// Used to resolve an IPv4 address into a 48-bit MAC address on the local
    /// Ethernet segment.  See [`super::arp`].
    Arp  = 0x0806,

    /// Any EtherType value not explicitly recognized by NullOS.
    ///
    /// For example:
    /// * `0x86DD` – IPv6
    /// * `0x8100` – IEEE 802.1Q VLAN tag
    /// * `0x8847` – MPLS unicast
    ///
    /// Frames with an `Unknown` EtherType are silently discarded in
    /// [`handle_packet`].
    Unknown(u16),
}

impl EtherType {
    /// Converts a raw `u16` (in **host** byte order) into the [`EtherType`] enum.
    ///
    /// The caller must already have byte-swapped the wire value from big-endian to
    /// host order before calling this function (e.g., via [`u16::from_be`]).
    ///
    /// # Arguments
    ///
    /// * `val` – The EtherType value in host byte order.
    ///
    /// # Returns
    ///
    /// The corresponding [`EtherType`] variant, or `Unknown(val)` for unrecognized
    /// values.
    pub const fn from_u16(val: u16) -> Self {
        match val {
            0x0800 => EtherType::Ipv4,
            0x0806 => EtherType::Arp,
            other  => EtherType::Unknown(other),
        }
    }

    /// Converts this [`EtherType`] variant back to a raw `u16` in **host** byte
    /// order.  The caller is responsible for converting to big-endian before
    /// placing the value into a wire-format buffer (e.g., `val.to_be()`).
    ///
    /// # Returns
    ///
    /// The numeric EtherType value in host byte order.
    pub const fn to_u16(self) -> u16 {
        match self {
            EtherType::Ipv4        => 0x0800,
            EtherType::Arp         => 0x0806,
            EtherType::Unknown(val) => val,
        }
    }
}

impl fmt::Display for EtherType {
    /// Formats the EtherType for human-readable display.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(EtherType::Ipv4.to_string(), "IPv4 (0x0800)");
    /// assert_eq!(EtherType::Arp.to_string(),  "ARP (0x0806)");
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EtherType::Ipv4           => write!(f, "IPv4 (0x0800)"),
            EtherType::Arp            => write!(f, "ARP (0x0806)"),
            EtherType::Unknown(code)  => write!(f, "Unknown ({:#06x})", code),
        }
    }
}

/// A **14-byte Ethernet II Frame Header** in wire format.
///
/// The struct is `#[repr(C, packed)]` so that:
/// * No padding bytes are inserted by the Rust compiler (C layout).
/// * Fields may be at unaligned offsets within the buffer (packed).
///
/// **Safety**: because the struct is `packed`, taking a reference to any
/// field that is not naturally aligned would be undefined behavior in Rust.
/// All field access goes through getter methods that use
/// [`core::ptr::read_unaligned`] internally when the struct is cast from a
/// raw pointer, or through the safe accessor methods defined below.
///
/// ## Wire Layout
///
/// ```text
/// Offset  Size  Field
/// ──────  ────  ─────────────────────────────────────────────────
///  0       6    dst      Destination MAC address (6 octets)
///  6       6    src      Source MAC address (6 octets)
/// 12       2    ethertype EtherType / IEEE 802.3 length (big-endian)
/// ```
///
/// Total: 14 bytes.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EthernetHeader {
    /// **Destination MAC address** – 6 bytes, MSB-first octet order.
    ///
    /// All-ones (`[0xFF; 6]`) is the Ethernet broadcast address and is
    /// delivered to every node on the local segment.  Bit 0 of the first byte
    /// (`dst[0] & 0x01 == 1`) indicates a multicast group address.
    pub dst: [u8; 6],

    /// **Source MAC address** – 6 bytes, MSB-first octet order.
    ///
    /// Set to the transmitter's NIC hardware address.  For NullOS this is the
    /// MAC address reported by the VirtIO NIC (see [`super::local_mac()`]).
    pub src: [u8; 6],

    /// **EtherType** – 2 bytes stored in **network byte order** (big-endian).
    ///
    /// Must be byte-swapped with [`u16::from_be`] before interpretation.
    /// Use the [`EthernetHeader::ethertype`] accessor which handles this.
    ///
    /// Note: When this value is ≤ 1500 (0x05DC) it is an IEEE 802.3 *length*
    /// field, not an EtherType.  NullOS does not support 802.3 framing.
    pub ethertype: u16, // Stored in Network Byte Order (Big-Endian)
}

impl EthernetHeader {
    /// Constructs a new [`EthernetHeader`] from the given destination MAC,
    /// source MAC, and EtherType.
    ///
    /// The `ethertype` field is stored in **big-endian** (network byte order)
    /// by calling `.to_be()` on the host-order value.
    ///
    /// # Arguments
    ///
    /// * `dst`       – Destination MAC address (6 bytes).
    /// * `src`       – Source MAC address (6 bytes), typically [`super::local_mac()`].
    /// * `ethertype` – The [`EtherType`] identifying the payload protocol.
    ///
    /// # Returns
    ///
    /// A new [`EthernetHeader`] ready to be serialized into a transmit buffer.
    pub fn new(dst: MacAddress, src: MacAddress, ethertype: EtherType) -> Self {
        Self {
            dst: dst.0,
            src: src.0,
            // Convert host-order u16 to network byte order before storing.
            ethertype: ethertype.to_u16().to_be(),
        }
    }

    /// Returns the **destination MAC address** as a [`MacAddress`] value.
    ///
    /// Simply wraps `self.dst` in the newtype to give it the full set of
    /// helper methods (`is_broadcast`, `Display`, etc.).
    pub fn dst_mac(&self) -> MacAddress {
        MacAddress(self.dst)
    }

    /// Returns the **source MAC address** as a [`MacAddress`] value.
    pub fn src_mac(&self) -> MacAddress {
        MacAddress(self.src)
    }

    /// Returns the **EtherType** field, byte-swapped from network to host order.
    ///
    /// Because the struct is `packed`, we cannot take a reference to `self.ethertype`
    /// safely.  Instead we use [`u16::from_be`] on the raw stored value, which
    /// performs the byte-swap and returns a correctly oriented host-order value.
    ///
    /// # Returns
    ///
    /// The [`EtherType`] variant corresponding to the wire-format field.
    pub fn ethertype(&self) -> EtherType {
        // self.ethertype is stored big-endian; from_be converts to host order.
        EtherType::from_u16(u16::from_be(self.ethertype))
    }
}

/// A **parsed Ethernet II frame** holding a decoded header and a borrowed slice
/// of the payload bytes.
///
/// The lifetime parameter `'a` ties the payload reference to the original raw
/// buffer, avoiding an unnecessary allocation.  The header is a *copy* of the
/// first 14 bytes (since [`EthernetHeader`] is `Copy`).
///
/// # Usage
///
/// ```rust
/// let frame = EthernetFrame::parse(&raw_bytes)?;
/// println!("Src: {}, EtherType: {}", frame.header.src_mac(), frame.header.ethertype());
/// ```
pub struct EthernetFrame<'a> {
    /// Decoded copy of the 14-byte Ethernet II header.
    pub header: EthernetHeader,

    /// Borrowed slice of the frame payload (everything after the 14-byte header).
    ///
    /// For an IPv4 frame this is the raw IPv4 packet (up to 1500 bytes).
    /// For an ARP frame this is the 28-byte ARP packet.
    /// The slice does **not** include the hardware-stripped FCS.
    pub payload: &'a [u8],
}

impl<'a> EthernetFrame<'a> {
    /// Parses a **raw byte slice** into an [`EthernetFrame`].
    ///
    /// The function uses `core::ptr::read_unaligned` to safely copy the
    /// 14-byte header out of the buffer even if the buffer is not 2-byte
    /// aligned (as may be the case with DMA ring buffers).
    ///
    /// # Arguments
    ///
    /// * `raw` – A byte slice containing the full Ethernet frame (header +
    ///           payload, no FCS).  Must be at least [`ETH_HDR_LEN`] (14) bytes.
    ///
    /// # Returns
    ///
    /// * `Ok(EthernetFrame)` – Successfully parsed frame with header copy and
    ///   payload slice.
    /// * `Err(&str)` – The slice was shorter than 14 bytes and cannot contain
    ///   a valid Ethernet II header.
    ///
    /// # Safety
    ///
    /// The `unsafe` block is needed to call `core::ptr::read_unaligned`.  The
    /// operation is safe because:
    /// * `raw.as_ptr()` is valid for reads of `ETH_HDR_LEN` bytes (checked
    ///   above by the length guard).
    /// * `read_unaligned` does not require the pointer to be aligned.
    pub fn parse(raw: &'a [u8]) -> Result<Self, &'static str> {
        if raw.len() < ETH_HDR_LEN {
            return Err("Frame too short for Ethernet header");
        }

        // SAFETY: raw.len() >= ETH_HDR_LEN bytes guaranteed above.
        // read_unaligned handles the case where raw.as_ptr() is not 2-byte aligned.
        let header = unsafe { core::ptr::read_unaligned(raw.as_ptr() as *const EthernetHeader) };
        // Payload begins immediately after the 14-byte header.
        let payload = &raw[ETH_HDR_LEN..];

        Ok(Self { header, payload })
    }

    /// Constructs and returns a **complete serialized Ethernet frame** as a
    /// heap-allocated [`Vec<u8>`], padding to the minimum frame size if needed.
    ///
    /// ## Minimum Frame Padding
    ///
    /// IEEE 802.3 mandates that the minimum *payload* of an Ethernet frame is
    /// **46 bytes** so that the total wire frame (excluding preamble and FCS)
    /// is at least 64 bytes.  If `payload.len() < 46`, zero-bytes are appended
    /// until the total buffer is 60 bytes (14-byte header + 46-byte payload).
    /// The 4-byte FCS is added by the NIC hardware automatically.
    ///
    /// ## Frame Layout
    ///
    /// ```text
    /// [ Dst MAC (6) | Src MAC (6) | EtherType (2) | Payload (N) | Padding (0..P) ]
    /// ```
    ///
    /// # Arguments
    ///
    /// * `dst`       – Destination MAC address.
    /// * `src`       – Source MAC address (the local NIC's MAC).
    /// * `ethertype` – Protocol type of the payload.
    /// * `payload`   – The raw bytes to encapsulate (max 1500 bytes for standard MTU).
    ///
    /// # Returns
    ///
    /// A `Vec<u8>` containing the complete frame, ready for [`virtio_net::send`].
    ///
    /// # Safety
    ///
    /// The `unsafe` block converts the [`EthernetHeader`] struct into a byte
    /// slice using `core::slice::from_raw_parts`.  This is safe because:
    /// * `EthernetHeader` is `#[repr(C, packed)]` with no padding.
    /// * The pointer and length are derived from a valid local variable.
    pub fn build(dst: MacAddress, src: MacAddress, ethertype: EtherType, payload: &[u8]) -> Vec<u8> {
        // Pre-allocate at least header + max(payload, 46) bytes.
        let mut frame = Vec::with_capacity(ETH_HDR_LEN + payload.len().max(46));
        let header = EthernetHeader::new(dst, src, ethertype);

        // SAFETY: EthernetHeader is #[repr(C, packed)]; no padding; size = 14.
        let hdr_bytes = unsafe {
            core::slice::from_raw_parts(&header as *const _ as *const u8, ETH_HDR_LEN)
        };
        frame.extend_from_slice(hdr_bytes);
        frame.extend_from_slice(payload);

        // Pad to standard minimum Ethernet frame payload (46 bytes payload, 60 bytes total)
        // so that every transmitted frame satisfies the IEEE 802.3 minimum frame length.
        while frame.len() < 60 {
            frame.push(0);
        }

        frame
    }
}

/// **Encapsulates and transmits** a single packet over Ethernet.
///
/// This is the primary outbound path for the ARP and IPv4 layers.  It:
/// 1. Retrieves the local MAC address from the VirtIO driver.
/// 2. Calls [`EthernetFrame::build`] to assemble the frame with proper padding.
/// 3. Passes the frame bytes to `virtio_net::send` which places them into the
///    VirtIO TX ring descriptor and notifies the hypervisor.
///
/// # Arguments
///
/// * `dst`       – Destination MAC address (unicast or broadcast `FF:FF:FF:FF:FF:FF`).
/// * `ethertype` – Layer-3 protocol type to embed in the EtherType field.
/// * `payload`   – The serialized Layer-3 packet bytes (ARP packet or IPv4 packet).
///
/// # Returns
///
/// * `Ok(())` – The frame was successfully placed into the VirtIO TX ring.
/// * `Err(&str)` – The VirtIO send failed (ring full, device not ready, etc.).
pub fn send_frame(dst: MacAddress, ethertype: EtherType, payload: &[u8]) -> Result<(), &'static str> {
    // Retrieve our own NIC MAC address to fill the source field.
    let src = super::local_mac();
    // Build the full Ethernet frame with header, payload, and minimum-size padding.
    let frame = EthernetFrame::build(dst, src, ethertype, payload);
    // Hand the frame bytes to the VirtIO driver for DMA transmission.
    virtio_net::send(&frame)
}

/// **Entry point for received Ethernet packets**, dispatching to ARP or IPv4.
///
/// Called by [`super::poll()`] for every frame drained from the VirtIO RX ring.
///
/// # Receive Filtering
///
/// NullOS operates in **normal (non-promiscuous) mode**: only frames addressed
/// to our unicast MAC or the broadcast address are processed.  Frames addressed
/// to other unicast MACs (or unrecognized multicast groups) are silently dropped.
///
/// # Dispatch Logic
///
/// After MAC filtering, the EtherType field determines where the payload goes:
///
/// | EtherType | Handler                       |
/// |-----------|-------------------------------|
/// | `0x0806`  | [`super::arp::handle_packet`] |
/// | `0x0800`  | [`super::ipv4::handle_packet`]|
/// | Other     | Silently dropped              |
///
/// # Arguments
///
/// * `raw` – The raw received frame bytes (without FCS, as delivered by the NIC).
pub fn handle_packet(raw: &[u8]) {
    // Attempt to parse the Ethernet II header; drop malformed/truncated frames.
    let frame = match EthernetFrame::parse(raw) {
        Ok(f)  => f,
        Err(_) => return, // Frame too short to contain a valid header.
    };

    // Check the destination MAC for frame filtering.
    let local_mac = super::local_mac();
    let dst = frame.header.dst_mac();

    // Accept packets addressed to our MAC or broadcast
    // Drop unicast frames intended for other hosts (not our MAC and not broadcast).
    if dst != local_mac && !dst.is_broadcast() {
        return;
    }

    // Dispatch to the appropriate Layer-3 protocol handler based on EtherType.
    match frame.header.ethertype() {
        EtherType::Arp => {
            // Address Resolution Protocol — update ARP cache, optionally reply.
            super::arp::handle_packet(&frame);
        }
        EtherType::Ipv4 => {
            // Internet Protocol version 4 — validate checksum, dispatch to TCP/UDP/ICMP.
            super::ipv4::handle_packet(&frame);
        }
        EtherType::Unknown(_) => {
            // Silently drop unknown ethertypes
            // We do not log unknown EtherTypes to avoid log spam from VLAN/CDP/LLDP frames.
        }
    }
}

/// **Runs automated self-tests** for Ethernet frame serialization and parsing.
///
/// Tests the round-trip: `build()` → `parse()` → field verification.
///
/// ## What is verified:
///
/// 1. A frame built with a 21-byte payload is padded to at least 60 bytes.
/// 2. After parsing the padded frame the destination MAC matches.
/// 3. The source MAC matches.
/// 4. The EtherType field decodes to `Ipv4`.
/// 5. The first N bytes of the payload match the original payload.
///
/// Panics on assertion failure, which halts the kernel with a diagnostic message.
pub fn run_self_tests() {
    // Define test MAC addresses in canonical octet notation.
    let src = MacAddress::new([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
    let dst = MacAddress::new([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    // A short payload to exercise minimum-size padding.
    let payload = b"Hello Ethernet Layer!";

    // Build a complete Ethernet II frame; payload is only 21 bytes, so the frame
    // must be padded with zeros to reach the 60-byte minimum.
    let frame_bytes = EthernetFrame::build(dst, src, EtherType::Ipv4, payload);
    assert!(frame_bytes.len() >= 60, "Frame must be padded to at least 60 bytes");

    // Parse the serialized bytes back into a structured frame.
    let parsed = EthernetFrame::parse(&frame_bytes).expect("Failed to parse valid Ethernet frame");

    // Verify every field of the parsed header.
    assert_eq!(parsed.header.dst_mac(), dst);
    assert_eq!(parsed.header.src_mac(), src);
    assert_eq!(parsed.header.ethertype(), EtherType::Ipv4);
    // Verify the payload prefix (the rest of the payload slice may contain padding zeros).
    assert_eq!(&parsed.payload[..payload.len()], payload);

    println!("[ETHERNET] Ethernet II frame construction & parsing verified successfully!");
}
