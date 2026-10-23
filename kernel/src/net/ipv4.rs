//! IPv4 Network Layer Protocol (Phase 8 - M24 Network Stack).
//!
//! Provides IPv4 address representation, 20-byte IPv4 packet header serialization,
//! RFC 1071 Internet Checksum calculation, packet routing, and transport-layer dispatch.

use alloc::vec::Vec;
use core::fmt;

use crate::net::ethernet::{self, EtherType, EthernetFrame};
use crate::println;

pub const IPV4_HDR_MIN_LEN: usize = 20;

/// A 4-byte standard IPv4 Address (e.g., `192.168.1.1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Ipv4Addr(pub [u8; 4]);

impl Ipv4Addr {
    pub const LOCALHOST: Self = Self([127, 0, 0, 1]);
    pub const BROADCAST: Self = Self([255, 255, 255, 255]);
    pub const ANY: Self = Self([0, 0, 0, 0]);

    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self([a, b, c, d])
    }

    pub const fn from_octets(octets: [u8; 4]) -> Self {
        Self(octets)
    }

    pub const fn octets(&self) -> [u8; 4] {
        self.0
    }

    pub fn to_u32(&self) -> u32 {
        u32::from_be_bytes(self.0)
    }

    pub fn from_u32(val: u32) -> Self {
        Self(val.to_be_bytes())
    }

    pub fn is_loopback(&self) -> bool {
        self.0[0] == 127
    }

    pub fn is_broadcast(&self) -> bool {
        *self == Self::BROADCAST
    }

    /// Checks if another IP address resides within the same subnet defined by `netmask`.
    pub fn in_same_subnet(&self, other: &Self, netmask: &Self) -> bool {
        (self.to_u32() & netmask.to_u32()) == (other.to_u32() & netmask.to_u32())
    }

    /// Parses an IPv4 address from a dotted-decimal string (e.g. `"10.0.2.15"`).
    pub fn parse(s: &str) -> Result<Self, &'static str> {
        let mut octets = [0u8; 4];
        let mut idx = 0;

        for part in s.split('.') {
            if idx >= 4 {
                return Err("Too many octets in IPv4 address");
            }
            let val = part.trim().parse::<u8>().map_err(|_| "Invalid octet")?;
            octets[idx] = val;
            idx += 1;
        }

        if idx != 4 {
            return Err("IPv4 address must contain exactly 4 octets");
        }

        Ok(Self(octets))
    }
}

impl fmt::Display for Ipv4Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0[0], self.0[1], self.0[2], self.0[3])
    }
}

/// Supported IPv4 Next-Level Protocol numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IpProtocol {
    Icmp = 1,
    Tcp  = 6,
    Udp  = 17,
    Unknown(u8),
}

impl IpProtocol {
    pub const fn from_u8(val: u8) -> Self {
        match val {
            1 => IpProtocol::Icmp,
            6 => IpProtocol::Tcp,
            17 => IpProtocol::Udp,
            other => IpProtocol::Unknown(other),
        }
    }

    pub const fn to_u8(self) -> u8 {
        match self {
            IpProtocol::Icmp => 1,
            IpProtocol::Tcp => 6,
            IpProtocol::Udp => 17,
            IpProtocol::Unknown(val) => val,
        }
    }
}

impl fmt::Display for IpProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IpProtocol::Icmp => write!(f, "ICMP (1)"),
            IpProtocol::Tcp => write!(f, "TCP (6)"),
            IpProtocol::Udp => write!(f, "UDP (17)"),
            IpProtocol::Unknown(p) => write!(f, "Protocol({})", p),
        }
    }
}

/// Standard 20-byte IPv4 Header (without options).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Ipv4Header {
    pub version_ihl: u8,    // High 4 bits: Version (4), Low 4 bits: Header length in 32-bit words (5 = 20 bytes)
    pub tos: u8,            // Type of Service / DSCP
    pub total_len: u16,      // Total packet length (header + data) in bytes
    pub identification: u16, // Packet ID
    pub flags_fragment: u16, // 3-bit flags (DF, MF) + 13-bit fragment offset
    pub ttl: u8,            // Time to Live (hop count, default 64)
    pub protocol: u8,       // Transport protocol (ICMP 1, TCP 6, UDP 17)
    pub checksum: u16,       // 16-bit one's complement header checksum
    pub src: [u8; 4],       // Source IPv4 address
    pub dst: [u8; 4],       // Destination IPv4 address
}

impl Ipv4Header {
    pub fn new(src: Ipv4Addr, dst: Ipv4Addr, protocol: IpProtocol, payload_len: usize) -> Self {
        let total_len = (IPV4_HDR_MIN_LEN + payload_len) as u16;
        let mut hdr = Self {
            version_ihl: 0x45, // Version 4, IHL 5 (5 * 4 = 20 bytes)
            tos: 0,
            total_len: total_len.to_be(),
            identification: 0x1234u16.to_be(),
            flags_fragment: 0x4000u16.to_be(), // Don't Fragment (DF) flag set
            ttl: 64,
            protocol: protocol.to_u8(),
            checksum: 0,
            src: src.octets(),
            dst: dst.octets(),
        };

        // Compute Internet Checksum over the 20-byte header
        let hdr_bytes = unsafe {
            core::slice::from_raw_parts(&hdr as *const _ as *const u8, IPV4_HDR_MIN_LEN)
        };
        hdr.checksum = compute_checksum(hdr_bytes).to_be();
        hdr
    }

    pub fn src_ip(&self) -> Ipv4Addr {
        Ipv4Addr::from_octets(self.src)
    }

    pub fn dst_ip(&self) -> Ipv4Addr {
        Ipv4Addr::from_octets(self.dst)
    }

    pub fn protocol(&self) -> IpProtocol {
        IpProtocol::from_u8(self.protocol)
    }

    pub fn total_length(&self) -> usize {
        u16::from_be(self.total_len) as usize
    }

    pub fn header_length(&self) -> usize {
        ((self.version_ihl & 0x0F) * 4) as usize
    }
}

/// Computes the RFC 1071 standard 16-bit one's complement Internet Checksum.
pub fn compute_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;

    while i + 1 < data.len() {
        let word = u16::from_be_bytes([data[i], data[i + 1]]);
        sum += word as u32;
        i += 2;
    }

    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }

    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !(sum as u16)
}

/// Encapsulates payload in an IPv4 packet, resolves destination MAC via ARP, and transmits over Ethernet.
pub fn send_ipv4_packet(dst_ip: Ipv4Addr, protocol: IpProtocol, payload: &[u8]) -> Result<(), &'static str> {
    let config = *super::INTERFACE_CONFIG.lock();
    let src_ip = config.ip;

    // Determine target IP for ARP resolution (direct host if on same subnet, or gateway router)
    let next_hop = if dst_ip.in_same_subnet(&src_ip, &config.netmask) {
        dst_ip
    } else {
        config.gateway
    };

    let target_mac = match super::arp::resolve_mac(next_hop) {
        Some(mac) => mac,
        None => return Err("Unable to resolve destination MAC address via ARP"),
    };

    let header = Ipv4Header::new(src_ip, dst_ip, protocol, payload.len());
    let mut packet = Vec::with_capacity(IPV4_HDR_MIN_LEN + payload.len());

    let hdr_bytes = unsafe {
        core::slice::from_raw_parts(&header as *const _ as *const u8, IPV4_HDR_MIN_LEN)
    };
    packet.extend_from_slice(hdr_bytes);
    packet.extend_from_slice(payload);

    ethernet::send_frame(target_mac, EtherType::Ipv4, &packet)
}

/// Entry point for received IPv4 packets from Ethernet frames.
pub fn handle_packet(frame: &EthernetFrame) {
    if frame.payload.len() < IPV4_HDR_MIN_LEN {
        return;
    }

    let header = unsafe {
        core::ptr::read_unaligned(frame.payload.as_ptr() as *const Ipv4Header)
    };

    // Verify IPv4 version
    if (header.version_ihl >> 4) != 4 {
        return;
    }

    let hdr_len = header.header_length();
    let total_len = header.total_length();

    if frame.payload.len() < total_len || total_len < hdr_len {
        return;
    }

    // Verify checksum
    let hdr_slice = &frame.payload[..hdr_len];
    if compute_checksum(hdr_slice) != 0 {
        return; // Checksum mismatch, drop corrupt packet
    }

    let payload = &frame.payload[hdr_len..total_len];
    let src_ip = header.src_ip();
    let dst_ip = header.dst_ip();

    let local_ip = super::local_ip();
    if dst_ip != local_ip && !dst_ip.is_broadcast() {
        return; // Drop packet not addressed to our IP
    }

    // Dispatch to transport layer
    match header.protocol() {
        IpProtocol::Icmp => {
            super::icmp::handle_packet(src_ip, payload);
        }
        IpProtocol::Udp => {
            super::udp::handle_packet(src_ip, payload);
        }
        IpProtocol::Tcp => {
            super::tcp::handle_packet(src_ip, payload);
        }
        IpProtocol::Unknown(_) => {}
    }
}

/// Runs automated verification self-tests for IPv4 serialization, parsing, and checksumming.
pub fn run_self_tests() {
    let src = Ipv4Addr::new(10, 0, 2, 15);
    let dst = Ipv4Addr::new(10, 0, 2, 2);
    let payload = b"IPv4 packet payload test!";

    let hdr = Ipv4Header::new(src, dst, IpProtocol::Udp, payload.len());
    let hdr_bytes = unsafe {
        core::slice::from_raw_parts(&hdr as *const _ as *const u8, IPV4_HDR_MIN_LEN)
    };

    // RFC 1071 property: Checksum of header including checksum field must evaluate to 0
    assert_eq!(compute_checksum(hdr_bytes), 0, "IPv4 checksum calculation failed");

    assert_eq!(hdr.src_ip(), src);
    assert_eq!(hdr.dst_ip(), dst);
    assert_eq!(hdr.protocol(), IpProtocol::Udp);
    assert_eq!(hdr.total_length(), IPV4_HDR_MIN_LEN + payload.len());

    // Test IP string parsing
    let parsed_ip = Ipv4Addr::parse("192.168.1.100").expect("Failed to parse valid IPv4 string");
    assert_eq!(parsed_ip, Ipv4Addr::new(192, 168, 1, 100));

    println!("[IPV4] IPv4 Header, RFC 1071 Checksum & Address parsing verified successfully!");
}
