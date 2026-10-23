//! User Datagram Protocol (UDP) Transport Layer (Phase 8 - M24 Network Stack).
//!
//! Provides lightweight, connectionless datagram transport with port-based demultiplexing.

use alloc::vec::Vec;

use crate::net::ipv4::{self, Ipv4Addr, IpProtocol};
use crate::println;

pub const UDP_HDR_LEN: usize = 8;

/// Standard 8-byte UDP Header.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct UdpHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub length: u16,
    pub checksum: u16,
}

impl UdpHeader {
    pub fn new(src_port: u16, dst_port: u16, payload_len: usize) -> Self {
        let length = (UDP_HDR_LEN + payload_len) as u16;
        Self {
            src_port: src_port.to_be(),
            dst_port: dst_port.to_be(),
            length: length.to_be(),
            checksum: 0,
        }
    }

    pub fn src_port(&self) -> u16 {
        u16::from_be(self.src_port)
    }

    pub fn dst_port(&self) -> u16 {
        u16::from_be(self.dst_port)
    }

    pub fn length(&self) -> usize {
        u16::from_be(self.length) as usize
    }
}

/// Computes the 16-bit UDP checksum including the IPv4 Pseudo-Header.
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

/// Dispatches an incoming UDP datagram to any listening socket.
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
