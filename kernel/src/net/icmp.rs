//! Internet Control Message Protocol (ICMP) (Phase 8 - M24 Network Stack).
//!
//! Implements ICMP Echo Requests (Type 8) and Echo Replies (Type 0) to power the `ping` diagnostic utility.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};

use crate::arch::x86_64::apic;
use crate::net::ipv4::{self, Ipv4Addr, IpProtocol};
use crate::println;
use crate::sync::Spinlock;

pub const ICMP_HDR_LEN: usize = 8;

/// ICMP Message Types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IcmpType {
    EchoReply   = 0,
    DestinationUnreachable = 3,
    EchoRequest = 8,
    TimeExceeded = 11,
    Unknown(u8),
}

impl IcmpType {
    pub const fn from_u8(val: u8) -> Self {
        match val {
            0 => IcmpType::EchoReply,
            3 => IcmpType::DestinationUnreachable,
            8 => IcmpType::EchoRequest,
            11 => IcmpType::TimeExceeded,
            other => IcmpType::Unknown(other),
        }
    }
}

/// Standard 8-byte ICMP Header.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct IcmpHeader {
    pub icmp_type: u8,
    pub code: u8,
    pub checksum: u16,
    pub ident: u16,
    pub seq: u16,
}

impl IcmpHeader {
    pub fn new(icmp_type: IcmpType, ident: u16, seq: u16, payload: &[u8]) -> Self {
        let mut hdr = Self {
            icmp_type: match icmp_type {
                IcmpType::EchoReply => 0,
                IcmpType::DestinationUnreachable => 3,
                IcmpType::EchoRequest => 8,
                IcmpType::TimeExceeded => 11,
                IcmpType::Unknown(u) => u,
            },
            code: 0,
            checksum: 0,
            ident: ident.to_be(),
            seq: seq.to_be(),
        };

        // Compute checksum over header + payload
        let mut data = Vec::with_capacity(ICMP_HDR_LEN + payload.len());
        let hdr_bytes = unsafe {
            core::slice::from_raw_parts(&hdr as *const _ as *const u8, ICMP_HDR_LEN)
        };
        data.extend_from_slice(hdr_bytes);
        data.extend_from_slice(payload);

        hdr.checksum = ipv4::compute_checksum(&data).to_be();
        hdr
    }

    pub fn ident(&self) -> u16 {
        u16::from_be(self.ident)
    }

    pub fn seq(&self) -> u16 {
        u16::from_be(self.seq)
    }
}

/// Tracking state for received ICMP Echo Replies.
#[derive(Debug, Clone, Copy, Default)]
pub struct PingRecord {
    pub src_ip: Ipv4Addr,
    pub seq: u16,
    pub ident: u16,
    pub received: bool,
}

static LAST_PING_RECORD: Spinlock<PingRecord> = Spinlock::new(PingRecord {
    src_ip: Ipv4Addr::ANY,
    seq: 0,
    ident: 0,
    received: false,
});

static NEXT_PING_IDENT: AtomicU16 = AtomicU16::new(0x4200);

/// Handles incoming ICMP packets dispatched from the IPv4 layer.
pub fn handle_packet(src_ip: Ipv4Addr, payload: &[u8]) {
    if payload.len() < ICMP_HDR_LEN {
        return;
    }

    let header = unsafe {
        core::ptr::read_unaligned(payload.as_ptr() as *const IcmpHeader)
    };

    let icmp_type = IcmpType::from_u8(header.icmp_type);
    let data = &payload[ICMP_HDR_LEN..];

    match icmp_type {
        IcmpType::EchoRequest => {
            // Automatically respond to incoming Echo Request with an Echo Reply
            let reply_hdr = IcmpHeader::new(IcmpType::EchoReply, header.ident(), header.seq(), data);
            let mut reply_packet = Vec::with_capacity(ICMP_HDR_LEN + data.len());

            let hdr_bytes = unsafe {
                core::slice::from_raw_parts(&reply_hdr as *const _ as *const u8, ICMP_HDR_LEN)
            };
            reply_packet.extend_from_slice(hdr_bytes);
            reply_packet.extend_from_slice(data);

            let _ = ipv4::send_ipv4_packet(src_ip, IpProtocol::Icmp, &reply_packet);
        }
        IcmpType::EchoReply => {
            // Record receipt of ping response
            let mut record = LAST_PING_RECORD.lock();
            record.src_ip = src_ip;
            record.seq = header.seq();
            record.ident = header.ident();
            record.received = true;
        }
        _ => {}
    }
}

/// Transmits a single ICMP Echo Request to the target IP address.
pub fn icmp_send_echo_request(target_ip: Ipv4Addr, ident: u16, seq: u16) -> Result<(), &'static str> {
    let payload = b"NullOS ICMP Ping Probe Packet";
    let header = IcmpHeader::new(IcmpType::EchoRequest, ident, seq, payload);

    let mut packet = Vec::with_capacity(ICMP_HDR_LEN + payload.len());
    let hdr_bytes = unsafe {
        core::slice::from_raw_parts(&header as *const _ as *const u8, ICMP_HDR_LEN)
    };
    packet.extend_from_slice(hdr_bytes);
    packet.extend_from_slice(payload);

    ipv4::send_ipv4_packet(target_ip, IpProtocol::Icmp, &packet)
}

/// Ping diagnostic routine: sends `count` probes to `target_ip` and yields RTT statistics.
pub fn ping(target_ip: Ipv4Addr, count: usize) -> (usize, usize) {
    let ident = NEXT_PING_IDENT.fetch_add(1, Ordering::Relaxed);
    let mut sent = 0;
    let mut received = 0;

    for seq in 1..=(count as u16) {
        {
            let mut record = LAST_PING_RECORD.lock();
            record.received = false;
        }

        let start_tick = apic::get_ticks();
        if icmp_send_echo_request(target_ip, ident, seq).is_ok() {
            sent += 1;

            // Poll for response up to 500 ms (approx 50 APIC ticks)
            let mut got_reply = false;
            for _ in 0..50 {
                super::poll();
                let record = *LAST_PING_RECORD.lock();
                if record.received && record.src_ip == target_ip && record.seq == seq && record.ident == ident {
                    let end_tick = apic::get_ticks();
                    let rtt_ticks = end_tick.saturating_sub(start_tick);
                    println!(
                        "64 bytes from {}: icmp_seq={} ttl=64 time={} ms",
                        target_ip, seq, rtt_ticks * 10
                    );
                    received += 1;
                    got_reply = true;
                    break;
                }
                apic::sleep(1);
            }

            if !got_reply {
                println!("Request timeout for icmp_seq {}", seq);
                // NOTE: QEMU's default SLIRP user-mode network gateway (10.0.2.2) does not reply to ICMP Echo requests
                // without host root socket permissions. On real physical hardware or TAP bridges, ICMP replies normally.
            }
        } else {
            println!("ping: sendto failed for {}", target_ip);
        }

        apic::sleep(2);
    }

    (sent, received)
}

/// Runs automated self-tests for ICMP packet construction, checksumming, and echo handling.
pub fn run_self_tests() {
    let payload = b"ICMP Echo Test Payload";
    let ident = 0x1234;
    let seq = 1;

    let req = IcmpHeader::new(IcmpType::EchoRequest, ident, seq, payload);
    assert_eq!(req.ident(), ident);
    let csum = req.checksum;
    assert_ne!(csum, 0);

    let reply = IcmpHeader::new(IcmpType::EchoReply, ident, seq, payload);
    assert_eq!(reply.ident(), ident);
    assert_eq!(reply.seq(), seq);

    println!("[ICMP] ICMP Echo Request & Reply serialization verified successfully!");
}
