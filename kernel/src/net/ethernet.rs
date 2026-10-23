//! Ethernet Layer 2 Frame Processing (Phase 8 - M24 Network Stack).
//!
//! Handles Ethernet II frame encapsulation, decoding, and dispatch to higher layers (IPv4, ARP).

use alloc::vec::Vec;
use core::fmt;

use crate::drivers::virtio_net::{self, MacAddress};
use crate::println;

pub const ETH_HDR_LEN: usize = 14;

/// Standard Ethernet II Protocol Types (EtherType).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum EtherType {
    Ipv4 = 0x0800,
    Arp  = 0x0806,
    Unknown(u16),
}

impl EtherType {
    pub const fn from_u16(val: u16) -> Self {
        match val {
            0x0800 => EtherType::Ipv4,
            0x0806 => EtherType::Arp,
            other => EtherType::Unknown(other),
        }
    }

    pub const fn to_u16(self) -> u16 {
        match self {
            EtherType::Ipv4 => 0x0800,
            EtherType::Arp => 0x0806,
            EtherType::Unknown(val) => val,
        }
    }
}

impl fmt::Display for EtherType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EtherType::Ipv4 => write!(f, "IPv4 (0x0800)"),
            EtherType::Arp => write!(f, "ARP (0x0806)"),
            EtherType::Unknown(code) => write!(f, "Unknown ({:#06x})", code),
        }
    }
}

/// A 14-byte Ethernet II Frame Header.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EthernetHeader {
    pub dst: [u8; 6],
    pub src: [u8; 6],
    pub ethertype: u16, // Stored in Network Byte Order (Big-Endian)
}

impl EthernetHeader {
    pub fn new(dst: MacAddress, src: MacAddress, ethertype: EtherType) -> Self {
        Self {
            dst: dst.0,
            src: src.0,
            ethertype: ethertype.to_u16().to_be(),
        }
    }

    pub fn dst_mac(&self) -> MacAddress {
        MacAddress(self.dst)
    }

    pub fn src_mac(&self) -> MacAddress {
        MacAddress(self.src)
    }

    pub fn ethertype(&self) -> EtherType {
        EtherType::from_u16(u16::from_be(self.ethertype))
    }
}

/// Parsed Ethernet II frame containing header and payload slice.
pub struct EthernetFrame<'a> {
    pub header: EthernetHeader,
    pub payload: &'a [u8],
}

impl<'a> EthernetFrame<'a> {
    /// Parses a raw byte slice into an Ethernet frame.
    pub fn parse(raw: &'a [u8]) -> Result<Self, &'static str> {
        if raw.len() < ETH_HDR_LEN {
            return Err("Frame too short for Ethernet header");
        }

        let header = unsafe { core::ptr::read_unaligned(raw.as_ptr() as *const EthernetHeader) };
        let payload = &raw[ETH_HDR_LEN..];

        Ok(Self { header, payload })
    }

    /// Constructs a complete serialized Ethernet frame with padding to minimum length of 60 bytes.
    pub fn build(dst: MacAddress, src: MacAddress, ethertype: EtherType, payload: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(ETH_HDR_LEN + payload.len().max(46));
        let header = EthernetHeader::new(dst, src, ethertype);

        let hdr_bytes = unsafe {
            core::slice::from_raw_parts(&header as *const _ as *const u8, ETH_HDR_LEN)
        };
        frame.extend_from_slice(hdr_bytes);
        frame.extend_from_slice(payload);

        // Pad to standard minimum Ethernet frame payload (46 bytes payload, 60 bytes total)
        while frame.len() < 60 {
            frame.push(0);
        }

        frame
    }
}

/// Encapsulates and transmits a packet over Ethernet.
pub fn send_frame(dst: MacAddress, ethertype: EtherType, payload: &[u8]) -> Result<(), &'static str> {
    let src = super::local_mac();
    let frame = EthernetFrame::build(dst, src, ethertype, payload);
    virtio_net::send(&frame)
}

/// Entry point for received Ethernet packets, dispatching to ARP or IPv4.
pub fn handle_packet(raw: &[u8]) {
    let frame = match EthernetFrame::parse(raw) {
        Ok(f) => f,
        Err(_) => return,
    };

    let local_mac = super::local_mac();
    let dst = frame.header.dst_mac();

    // Accept packets addressed to our MAC or broadcast
    if dst != local_mac && !dst.is_broadcast() {
        return;
    }

    match frame.header.ethertype() {
        EtherType::Arp => {
            super::arp::handle_packet(&frame);
        }
        EtherType::Ipv4 => {
            super::ipv4::handle_packet(&frame);
        }
        EtherType::Unknown(_) => {
            // Silently drop unknown ethertypes
        }
    }
}

/// Runs automated self-tests for Ethernet serialization and parsing.
pub fn run_self_tests() {
    let src = MacAddress::new([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
    let dst = MacAddress::new([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    let payload = b"Hello Ethernet Layer!";

    let frame_bytes = EthernetFrame::build(dst, src, EtherType::Ipv4, payload);
    assert!(frame_bytes.len() >= 60, "Frame must be padded to at least 60 bytes");

    let parsed = EthernetFrame::parse(&frame_bytes).expect("Failed to parse valid Ethernet frame");
    assert_eq!(parsed.header.dst_mac(), dst);
    assert_eq!(parsed.header.src_mac(), src);
    assert_eq!(parsed.header.ethertype(), EtherType::Ipv4);
    assert_eq!(&parsed.payload[..payload.len()], payload);

    println!("[ETHERNET] Ethernet II frame construction & parsing verified successfully!");
}
