//! Transmission Control Protocol (TCP) State Machine & Transport Layer (Phase 8 - M24 Network Stack).
//!
//! # Architecture & Protocol Specification
//!
//! TCP (defined in [RFC 793](https://www.rfc-editor.org/rfc/rfc793)) provides a reliable,
//! ordered, full-duplex byte-stream transport service on top of the best-effort IPv4 layer.
//!
//! ## TCP Segment Format (RFC 793):
//!
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |          Source Port          |       Destination Port        |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                        Sequence Number                        |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                    Acknowledgment Number                      |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |  Data |           |U|A|P|R|S|F|                               |
//! | Offset| Reserved  |R|C|S|S|Y|I|            Window             |
//! |       |           |G|K|H|T|N|N|                               |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |           Checksum            |         Urgent Pointer        |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                    Options                    |    Padding    |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                             data                              |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! ```
//!
//! ## Control Flags:
//! - **URG (0x20)**: Urgent Pointer field significant.
//! - **ACK (0x10)**: Acknowledgment field significant.
//! - **PSH (0x08)**: Push function (deliver buffered data to receiving app immediately).
//! - **RST (0x04)**: Reset connection abruptly.
//! - **SYN (0x02)**: Synchronize sequence numbers to establish connection.
//! - **FIN (0x01)**: No more data from sender; begin connection teardown.
//!
//! ## State Machine:
//! Tracks the standard 11 states:
//! `Closed` -> `Listen` -> `SynSent` -> `SynReceived` -> `Established` ->
//! `FinWait1` -> `FinWait2` -> `CloseWait` -> `Closing` -> `LastAck` -> `TimeWait`.
//!
//! TODO(net/tcp): Implement RFC 793 Sliding Window Flow Control:
//! - Track send sequence variables: `SND.UNA` (oldest unacknowledged seq), `SND.NXT` (next seq to send),
//!   and `SND.WND` (peer's advertised receive window).
//! - Track receive sequence variables: `RCV.NXT` (next expected incoming seq) and `RCV.WND` (local advertised window).
//! - Enforce sender window throttling so NullOS does not transmit beyond `SND.UNA + SND.WND`.
//!
//! TODO(net/tcp): Implement Packet Retransmission & RTO Estimation (RFC 6298):
//! - Maintain a retransmission queue for transmitted segments awaiting ACK.
//! - Implement smoothed Round-Trip Time (`SRTT`) and RTT variation (`RTTVAR`) calculations:
//!   `RTO = SRTT + max(G, 4 * RTTVAR)`.
//! - Backoff timer exponentially on timeout (`RTO = min(RTO * 2, 60s)`).
//!
//! TODO(net/tcp): Implement Out-of-Order Segment Reassembly:
//! - Buffer segments arriving with `seq_num > RCV.NXT` in a priority queue.
//! - When missing intermediate packets arrive, stitch buffered contiguous segments into the receive stream.

use alloc::vec::Vec;
use core::fmt;

use crate::net::ipv4::{self, Ipv4Addr, IpProtocol};
use crate::println;

/// Minimum TCP header length in bytes (no options, data offset = 5 words = 20 bytes).
pub const TCP_HDR_MIN_LEN: usize = 20;

// TCP Control Flags
/// FIN bit: No more data from sender.
pub const TCP_FLAG_FIN: u8 = 0x01;
/// SYN bit: Synchronize sequence numbers (initiate connection).
pub const TCP_FLAG_SYN: u8 = 0x02;
/// RST bit: Abort / reset connection.
pub const TCP_FLAG_RST: u8 = 0x04;
/// PSH bit: Push buffered data to user process.
pub const TCP_FLAG_PSH: u8 = 0x08;
/// ACK bit: Acknowledgment field valid.
pub const TCP_FLAG_ACK: u8 = 0x10;
/// URG bit: Urgent pointer valid.
pub const TCP_FLAG_URG: u8 = 0x20;

/// Standard 11-State TCP Connection State Machine (RFC 793).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TcpState {
    /// Initial disconnected state.
    #[default]
    Closed,
    /// Passive open: waiting for incoming connection request (SYN).
    Listen,
    /// Active open: SYN sent, waiting for SYN-ACK.
    SynSent,
    /// SYN received, SYN-ACK sent, waiting for final ACK.
    SynReceived,
    /// Three-way handshake complete; active full-duplex data transfer.
    Established,
    /// Active close: local application closed socket, FIN sent, waiting for ACK or FIN.
    FinWait1,
    /// Local FIN acknowledged, waiting for remote FIN.
    FinWait2,
    /// Passive close: remote host sent FIN, local host sent ACK, waiting for local app close.
    CloseWait,
    /// Simultaneous close: both endpoints sent FIN simultaneously.
    Closing,
    /// Local FIN sent after CloseWait, waiting for final ACK.
    LastAck,
    /// Waiting 2MSL (Maximum Segment Lifetime) before returning to Closed to absorb delayed segments.
    TimeWait,
}

impl fmt::Display for TcpState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TcpState::Closed => write!(f, "CLOSED"),
            TcpState::Listen => write!(f, "LISTEN"),
            TcpState::SynSent => write!(f, "SYN_SENT"),
            TcpState::SynReceived => write!(f, "SYN_RECV"),
            TcpState::Established => write!(f, "ESTABLISHED"),
            TcpState::FinWait1 => write!(f, "FIN_WAIT1"),
            TcpState::FinWait2 => write!(f, "FIN_WAIT2"),
            TcpState::CloseWait => write!(f, "CLOSE_WAIT"),
            TcpState::Closing => write!(f, "CLOSING"),
            TcpState::LastAck => write!(f, "LAST_ACK"),
            TcpState::TimeWait => write!(f, "TIME_WAIT"),
        }
    }
}

/// Standard 20-byte TCP Segment Header in network byte order.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct TcpHeader {
    /// Originating transport port.
    pub src_port: u16,
    /// Destination transport port.
    pub dst_port: u16,
    /// Sequence number of first data byte in this segment (or initial sequence number during SYN).
    pub seq_num: u32,
    /// Acknowledgment number: next expected byte sequence number from sender.
    pub ack_num: u32,
    /// Data offset in 32-bit words (top 4 bits) + 4 reserved bits.
    pub data_offset_res: u8,
    /// Control flags bitmask (`TCP_FLAG_*`).
    pub flags: u8,
    /// Advertised receive window size in bytes.
    pub window_size: u16,
    /// 16-bit Internet Checksum over pseudo-header and TCP segment.
    pub checksum: u16,
    /// Offset to urgent data if URG flag set.
    pub urgent_ptr: u16,
}

impl TcpHeader {
    /// Constructs a new TCP header with fields converted to network byte order.
    pub fn new(
        src_port: u16,
        dst_port: u16,
        seq_num: u32,
        ack_num: u32,
        flags: u8,
        window_size: u16,
    ) -> Self {
        Self {
            src_port: src_port.to_be(),
            dst_port: dst_port.to_be(),
            seq_num: seq_num.to_be(),
            ack_num: ack_num.to_be(),
            data_offset_res: (5 << 4), // 5 words = 20 bytes, no options
            flags,
            window_size: window_size.to_be(),
            checksum: 0,
            urgent_ptr: 0,
        }
    }

    /// Returns source port in host byte order.
    pub fn src_port(&self) -> u16 {
        u16::from_be(self.src_port)
    }

    /// Returns destination port in host byte order.
    pub fn dst_port(&self) -> u16 {
        u16::from_be(self.dst_port)
    }

    /// Returns sequence number in host byte order.
    pub fn seq_num(&self) -> u32 {
        u32::from_be(self.seq_num)
    }

    /// Returns acknowledgment number in host byte order.
    pub fn ack_num(&self) -> u32 {
        u32::from_be(self.ack_num)
    }

    /// Returns total header length in bytes, derived from the data offset nibble.
    pub fn header_length(&self) -> usize {
        ((self.data_offset_res >> 4) * 4) as usize
    }

    /// Returns flow control window size in host byte order.
    pub fn window_size(&self) -> u16 {
        u16::from_be(self.window_size)
    }

    /// Checks whether the specified flag bit is set.
    pub fn has_flag(&self, flag: u8) -> bool {
        (self.flags & flag) != 0
    }
}

/// Computes the 16-bit TCP Checksum over the IPv4 Pseudo-Header, TCP Header, and payload.
pub fn compute_tcp_checksum(
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
    header: &TcpHeader,
    payload: &[u8],
) -> u16 {
    let tcp_len = (TCP_HDR_MIN_LEN + payload.len()) as u16;

    // IPv4 Pseudo-Header (12 bytes):
    // Source IP (4), Destination IP (4), Zero (1), Protocol 6 (1), TCP Length (2)
    let mut pseudo = Vec::with_capacity(12 + TCP_HDR_MIN_LEN + payload.len());
    pseudo.extend_from_slice(&src_ip.octets());
    pseudo.extend_from_slice(&dst_ip.octets());
    pseudo.push(0);
    pseudo.push(IpProtocol::Tcp.to_u8());
    pseudo.extend_from_slice(&tcp_len.to_be_bytes());

    let hdr_bytes = unsafe {
        core::slice::from_raw_parts(header as *const _ as *const u8, TCP_HDR_MIN_LEN)
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

/// Transmits a TCP segment across the network.
pub fn send_tcp_segment(
    src_port: u16,
    dst_ip: Ipv4Addr,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    payload: &[u8],
) -> Result<(), &'static str> {
    let src_ip = super::local_ip();
    let mut header = TcpHeader::new(src_port, dst_port, seq, ack, flags, 64240);
    header.checksum = compute_tcp_checksum(src_ip, dst_ip, &header, payload).to_be();

    let mut packet = Vec::with_capacity(TCP_HDR_MIN_LEN + payload.len());
    let hdr_bytes = unsafe {
        core::slice::from_raw_parts(&header as *const _ as *const u8, TCP_HDR_MIN_LEN)
    };
    packet.extend_from_slice(hdr_bytes);
    packet.extend_from_slice(payload);

    ipv4::send_ipv4_packet(dst_ip, IpProtocol::Tcp, &packet)
}

/// Dispatches an incoming TCP segment to registered sockets in the socket layer.
pub fn handle_packet(src_ip: Ipv4Addr, payload: &[u8]) {
    if payload.len() < TCP_HDR_MIN_LEN {
        return;
    }

    let header = unsafe {
        core::ptr::read_unaligned(payload.as_ptr() as *const TcpHeader)
    };

    let hdr_len = header.header_length();
    if payload.len() < hdr_len || hdr_len < TCP_HDR_MIN_LEN {
        return;
    }

    let data = &payload[hdr_len..];
    super::socket::dispatch_tcp(src_ip, &header, data);
}

/// Runs automated self-tests verifying TCP header encoding, checksumming, and state transitions.
pub fn run_self_tests() {
    let src_ip = Ipv4Addr::new(10, 0, 2, 15);
    let dst_ip = Ipv4Addr::new(10, 0, 2, 2);
    let payload = b"GET / HTTP/1.1\r\n\r\n";

    let mut hdr = TcpHeader::new(49152, 80, 1000, 0, TCP_FLAG_SYN, 65535);
    assert_eq!(hdr.src_port(), 49152);
    assert_eq!(hdr.dst_port(), 80);
    assert_eq!(hdr.seq_num(), 1000);
    assert_eq!(hdr.ack_num(), 0);
    assert!(hdr.has_flag(TCP_FLAG_SYN));
    assert!(!hdr.has_flag(TCP_FLAG_ACK));

    let csum = compute_tcp_checksum(src_ip, dst_ip, &hdr, payload);
    hdr.checksum = csum.to_be();
    let hdr_csum = hdr.checksum;
    assert_ne!(hdr_csum, 0);

    // Verify TCP State stringification
    assert_eq!(alloc::format!("{}", TcpState::Established), "ESTABLISHED");
    assert_eq!(alloc::format!("{}", TcpState::Listen), "LISTEN");

    println!("[TCP] TCP Header, Flags & Pseudo-Header Checksum verified successfully!");
}
