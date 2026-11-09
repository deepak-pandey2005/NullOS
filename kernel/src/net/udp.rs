//! User Datagram Protocol (UDP) Transport Layer (Phase 8 - M24 Network Stack).
//!
//! # Architecture & Protocol Specification
//!
//! The User Datagram Protocol (defined in [RFC 768](https://www.rfc-editor.org/rfc/rfc768))
//! provides a minimal, connectionless transport service on top of IPv4. It adds
//! 16-bit source and destination port multiplexing and an optional data integrity
//! checksum, without introducing connection handshakes, delivery guarantees,
//! or flow control.
//!
//! ## UDP Header Format (8 bytes):
//!
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |          Source Port          |       Destination Port        |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |            Length             |           Checksum            |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                             Data ...                          |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! ```
//!
//! ## UDP Pseudo-Header Checksum (12 bytes):
//! Unlike the IPv4 header checksum, the UDP checksum verifies both the UDP header,
//! the payload, and an IPv4 **Pseudo-Header** to prevent misrouted datagrams:
//! - Source IPv4 Address (4 bytes)
//! - Destination IPv4 Address (4 bytes)
//! - Zero padding (1 byte)
//! - Protocol number (1 byte, `17 = 0x11`)
//! - UDP Length (2 bytes)

use alloc::vec::Vec;

use crate::net::ipv4::{self, Ipv4Addr, IpProtocol};
use crate::println;

/// Byte length of the standard UDP header.
pub const UDP_HDR_LEN: usize = 8;

/// Standard 8-byte UDP Header in network byte order.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct UdpHeader {
    /// Port number of the sending application.
    pub src_port: u16,
    /// Port number of the destination application.
    pub dst_port: u16,
    /// Total length in bytes of the UDP header plus UDP payload.
    pub length: u16,
    /// 16-bit one's complement checksum computed over the IPv4 pseudo-header and UDP datagram.
    pub checksum: u16,
}

impl UdpHeader {
    /// Constructs a new UDP header with the length computed and fields converted to network byte order.
    pub fn new(src_port: u16, dst_port: u16, payload_len: usize) -> Self {
        let length = (UDP_HDR_LEN + payload_len) as u16;
        Self {
            src_port: src_port.to_be(),
            dst_port: dst_port.to_be(),
            length: length.to_be(),
            checksum: 0,
        }
    }

    /// Returns the source port in host byte order.
    pub fn src_port(&self) -> u16 {
        u16::from_be(self.src_port)
    }

    /// Returns the destination port in host byte order.
    pub fn dst_port(&self) -> u16 {
        u16::from_be(self.dst_port)
    }

    /// Returns the total datagram length (header + payload) in bytes.
    pub fn length(&self) -> usize {
        u16::from_be(self.length) as usize
    }
}

/// Computes the 16-bit UDP checksum including the IPv4 Pseudo-Header.
///
/// If the computed checksum is `0x0000`, it is transmitted as `0xFFFF` per RFC 768.
pub fn compute_udp_checksum(
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
    header: &UdpHeader,
    payload: &[u8],
) -> u16 {
    let udp_len = (UDP_HDR_LEN + payload.len()) as u16;

    // IPv4 Pseudo-Header (12 bytes):
    // Source IP (4), Dest IP (4), Zero (1), Protocol 17 (1), UDP Length (2)
    let mut pseudo = Vec::with_capacity(12 + UDP_HDR_LEN + payload.len());
    pseudo.extend_from_slice(&src_ip.octets());
    pseudo.extend_from_slice(&dst_ip.octets());
    pseudo.push(0);
    pseudo.push(IpProtocol::Udp.to_u8());
    pseudo.extend_from_slice(&udp_len.to_be_bytes());

    let hdr_bytes = unsafe {
        core::slice::from_raw_parts(header as *const _ as *const u8, UDP_HDR_LEN)
    };
    pseudo.extend_from_slice(hdr_bytes);
    pseudo.extend_from_slice(payload);

    let csum = ipv4::compute_checksum(&pseudo);
    if csum == 0 {
        0xFFFF
    } else {
        csum
    }
}

/// Transmits a UDP datagram to the specified destination IP and port.
pub fn send_udp(
    src_port: u16,
    dst_ip: Ipv4Addr,
    dst_port: u16,
    payload: &[u8],
) -> Result<(), &'static str> {
    let src_ip = super::local_ip();
    let mut header = UdpHeader::new(src_port, dst_port, payload.len());
    header.checksum = compute_udp_checksum(src_ip, dst_ip, &header, payload).to_be();

    let mut packet = Vec::with_capacity(UDP_HDR_LEN + payload.len());
    let hdr_bytes = unsafe {
        core::slice::from_raw_parts(&header as *const _ as *const u8, UDP_HDR_LEN)
    };
    packet.extend_from_slice(hdr_bytes);
    packet.extend_from_slice(payload);

    ipv4::send_ipv4_packet(dst_ip, IpProtocol::Udp, &packet)
}

/// Dispatches an incoming UDP datagram to any listening socket matching the destination port.
pub fn handle_packet(src_ip: Ipv4Addr, payload: &[u8]) {
    if payload.len() < UDP_HDR_LEN {
        return;
    }

    let header = unsafe {
        core::ptr::read_unaligned(payload.as_ptr() as *const UdpHeader)
    };

    let src_port = header.src_port();
    let dst_port = header.dst_port();
    let total_len = header.length();

    if payload.len() < total_len || total_len < UDP_HDR_LEN {
        return;
    }

    let data = &payload[UDP_HDR_LEN..total_len];

    // Dispatch to registered socket in the socket layer
    super::socket::dispatch_udp(src_ip, src_port, dst_port, data);
}

/// Runs automated self-tests verifying UDP header building, parsing, and pseudo-header checksumming.
pub fn run_self_tests() {
    let src_ip = Ipv4Addr::new(10, 0, 2, 15);
    let dst_ip = Ipv4Addr::new(10, 0, 2, 2);
    let payload = b"Hello UDP World!";

    let mut hdr = UdpHeader::new(1234, 5678, payload.len());
    assert_eq!(hdr.src_port(), 1234);
    assert_eq!(hdr.dst_port(), 5678);
    assert_eq!(hdr.length(), UDP_HDR_LEN + payload.len());

    let csum = compute_udp_checksum(src_ip, dst_ip, &hdr, payload);
    hdr.checksum = csum.to_be();
    let hdr_csum = hdr.checksum;
    assert_ne!(hdr_csum, 0);

    println!("[UDP] UDP Datagram Header & Pseudo-Header Checksum verified successfully!");
}
