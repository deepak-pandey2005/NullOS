//! Address Resolution Protocol (ARP) (Phase 8 - M24 Network Stack).
//!
//! Provides dynamic resolution between 32-bit IPv4 addresses and 48-bit Ethernet MAC addresses.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::drivers::virtio_net::MacAddress;
use crate::net::ethernet::{self, EtherType, EthernetFrame};
use crate::net::ipv4::Ipv4Addr;
use crate::println;
use crate::sync::Spinlock;

pub const ARP_HTYPE_ETHERNET: u16 = 1;
pub const ARP_PTYPE_IPV4: u16 = 0x0800;
pub const ARP_OP_REQUEST: u16 = 1;
pub const ARP_OP_REPLY: u16 = 2;

/// Standard 28-byte ARP Packet structure for Ethernet and IPv4.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ArpPacket {
    pub htype: u16,      // Hardware type (1 = Ethernet)
    pub ptype: u16,      // Protocol type (0x0800 = IPv4)
    pub hlen: u8,        // Hardware address length (6 for MAC)
    pub plen: u8,        // Protocol address length (4 for IPv4)
    pub oper: u16,       // Operation (1 = Request, 2 = Reply)
    pub sender_mac: [u8; 6],
    pub sender_ip: [u8; 4],
    pub target_mac: [u8; 6],
    pub target_ip: [u8; 4],
}

impl ArpPacket {
    pub fn new_request(sender_mac: MacAddress, sender_ip: Ipv4Addr, target_ip: Ipv4Addr) -> Self {
        Self {
            htype: ARP_HTYPE_ETHERNET.to_be(),
            ptype: ARP_PTYPE_IPV4.to_be(),
            hlen: 6,
            plen: 4,
            oper: ARP_OP_REQUEST.to_be(),
            sender_mac: sender_mac.0,
            sender_ip: sender_ip.octets(),
            target_mac: [0u8; 6],
            target_ip: target_ip.octets(),
        }
    }

    pub fn new_reply(
        sender_mac: MacAddress,
        sender_ip: Ipv4Addr,
        target_mac: MacAddress,
        target_ip: Ipv4Addr,
    ) -> Self {
        Self {
            htype: ARP_HTYPE_ETHERNET.to_be(),
            ptype: ARP_PTYPE_IPV4.to_be(),
            hlen: 6,
            plen: 4,
            oper: ARP_OP_REPLY.to_be(),
            sender_mac: sender_mac.0,
            sender_ip: sender_ip.octets(),
            target_mac: target_mac.0,
            target_ip: target_ip.octets(),
        }
    }

    pub fn sender_mac(&self) -> MacAddress {
        MacAddress(self.sender_mac)
    }

    pub fn sender_ip(&self) -> Ipv4Addr {
        Ipv4Addr::from_octets(self.sender_ip)
    }

    pub fn target_mac(&self) -> MacAddress {
        MacAddress(self.target_mac)
    }

    pub fn target_ip(&self) -> Ipv4Addr {
        Ipv4Addr::from_octets(self.target_ip)
    }

    pub fn operation(&self) -> u16 {
        u16::from_be(self.oper)
    }

    pub fn to_bytes(&self) -> [u8; 28] {
        unsafe { core::mem::transmute_copy(self) }
    }
}

/// Global dynamic ARP Cache table mapping IPv4 addresses to MAC addresses.
pub static ARP_CACHE: Spinlock<BTreeMap<Ipv4Addr, MacAddress>> = Spinlock::new(BTreeMap::new());

/// Looks up an IPv4 address in the local ARP cache.
pub fn arp_lookup(ip: Ipv4Addr) -> Option<MacAddress> {
    ARP_CACHE.lock().get(&ip).copied()
}

/// Inserts or updates an IPv4 -> MAC mapping in the ARP cache.
pub fn arp_insert(ip: Ipv4Addr, mac: MacAddress) {
    ARP_CACHE.lock().insert(ip, mac);
}

/// Returns a snapshot of the current ARP table for CLI inspection (`arp` command).
pub fn arp_table() -> Vec<(Ipv4Addr, MacAddress)> {
    ARP_CACHE.lock().iter().map(|(&ip, &mac)| (ip, mac)).collect()
}

/// Transmits an ARP Request for the specified target IPv4 address.
pub fn send_arp_request(target_ip: Ipv4Addr) -> Result<(), &'static str> {
    let local_mac = super::local_mac();
    let local_ip = super::local_ip();

    let arp = ArpPacket::new_request(local_mac, local_ip, target_ip);
    ethernet::send_frame(MacAddress::BROADCAST, EtherType::Arp, &arp.to_bytes())
}

/// Handles incoming ARP packets from Ethernet frames.
pub fn handle_packet(frame: &EthernetFrame) {
    if frame.payload.len() < core::mem::size_of::<ArpPacket>() {
        return;
    }

    let arp = unsafe {
        core::ptr::read_unaligned(frame.payload.as_ptr() as *const ArpPacket)
    };

    if u16::from_be(arp.htype) != ARP_HTYPE_ETHERNET || u16::from_be(arp.ptype) != ARP_PTYPE_IPV4 {
        return;
    }

    let sender_ip = arp.sender_ip();
    let sender_mac = arp.sender_mac();
    let target_ip = arp.target_ip();
    let local_ip = super::local_ip();
    let local_mac = super::local_mac();

    // Cache the sender's MAC address
    arp_insert(sender_ip, sender_mac);

    match arp.operation() {
        ARP_OP_REQUEST => {
            // If the query asks for our IP, reply with our MAC!
            if target_ip == local_ip {
                let reply = ArpPacket::new_reply(local_mac, local_ip, sender_mac, sender_ip);
                let _ = ethernet::send_frame(sender_mac, EtherType::Arp, &reply.to_bytes());
            }
        }
        ARP_OP_REPLY => {
            // Already cached sender_ip -> sender_mac above!
        }
        _ => {}
    }
}

/// Resolves an IP address to a MAC address, querying via ARP if not cached.
pub fn resolve_mac(target_ip: Ipv4Addr) -> Option<MacAddress> {
    if target_ip == Ipv4Addr::BROADCAST {
        return Some(MacAddress::BROADCAST);
    }

    // 1. Check ARP cache
    if let Some(mac) = arp_lookup(target_ip) {
        return Some(mac);
    }

    // 2. Query ARP request over the network
    let _ = send_arp_request(target_ip);

    // 3. Briefly poll for ARP reply
    for _ in 0..50 {
        super::poll();
        if let Some(mac) = arp_lookup(target_ip) {
            return Some(mac);
        }
        crate::arch::x86_64::apic::sleep(1);
    }

    // Fall back to default QEMU router MAC if unresolvable on virtual network
    if target_ip == super::DEFAULT_GATEWAY {
        let default_mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        arp_insert(target_ip, default_mac);
        return Some(default_mac);
    }

    None
}

/// Runs automated self-tests for ARP packet building, parsing, and caching.
pub fn run_self_tests() {
    let my_mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    let my_ip = Ipv4Addr::new(10, 0, 2, 15);
    let gateway_ip = Ipv4Addr::new(10, 0, 2, 2);

    let req = ArpPacket::new_request(my_mac, my_ip, gateway_ip);
    assert_eq!(req.operation(), ARP_OP_REQUEST);
    assert_eq!(req.sender_mac(), my_mac);
    assert_eq!(req.sender_ip(), my_ip);
    assert_eq!(req.target_ip(), gateway_ip);

    // Test cache insertion and retrieval
    let router_mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x02]);
    arp_insert(gateway_ip, router_mac);
    assert_eq!(arp_lookup(gateway_ip), Some(router_mac));

    println!("[ARP] ARP packet encoding & table cache verified successfully!");
}
