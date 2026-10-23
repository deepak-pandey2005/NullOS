//! NullOS Network Stack (Phase 8 - M24 Network Stack).
//!
//! Comprehensive layered TCP/IP network protocol stack:
//! - Layer 2: Ethernet II frame parsing and generation ([`ethernet`])
//! - Layer 3: ARP (Address Resolution Protocol) with dynamic cache ([`arp`])
//! - Layer 3: IPv4 packet parsing, fragment handling, and routing ([`ipv4`])
//! - Layer 3: ICMP (Internet Control Message Protocol) & `ping` ([`icmp`])
//! - Layer 4: UDP (User Datagram Protocol) datagram transport ([`udp`])
//! - Layer 4: TCP (Transmission Control Protocol) state machine ([`tcp`])
//! - Layer 5: Socket abstraction & POSIX networking API ([`socket`])
#![allow(dead_code)]

pub mod arp;
pub mod ethernet;
pub mod icmp;
pub mod ipv4;
pub mod socket;
pub mod tcp;
pub mod udp;

pub use arp::arp_table;
pub use ipv4::Ipv4Addr;
#[allow(unused_imports)]
pub use tcp::TcpState;

use crate::drivers::virtio_net::{self, MacAddress};
use crate::println;
use crate::sync::Spinlock;

/// Default network configuration parameters (QEMU user-mode NAT network).
pub const DEFAULT_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 15);
pub const DEFAULT_NETMASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);
pub const DEFAULT_GATEWAY: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 2);
pub const DEFAULT_DNS: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 3);

/// Host network configuration state.
#[derive(Debug, Clone, Copy)]
pub struct InterfaceConfig {
    pub ip: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub gateway: Ipv4Addr,
    pub dns: Ipv4Addr,
}

impl Default for InterfaceConfig {
    fn default() -> Self {
        Self {
            ip: DEFAULT_IP,
            netmask: DEFAULT_NETMASK,
            gateway: DEFAULT_GATEWAY,
            dns: DEFAULT_DNS,
        }
    }
}

/// Global host network configuration.
pub static INTERFACE_CONFIG: Spinlock<InterfaceConfig> = Spinlock::new(InterfaceConfig {
    ip: DEFAULT_IP,
    netmask: DEFAULT_NETMASK,
    gateway: DEFAULT_GATEWAY,
    dns: DEFAULT_DNS,
});

/// Returns the active local IPv4 address.
pub fn local_ip() -> Ipv4Addr {
    INTERFACE_CONFIG.lock().ip
}

/// Returns the active local MAC address.
pub fn local_mac() -> MacAddress {
    virtio_net::get_mac().unwrap_or(MacAddress::ZERO)
}

/// Initializes the NullOS TCP/IP Network Stack.
pub fn init() {
    println!("----------------------------------------");
    println!(" [NET] Initializing TCP/IP Network Stack");
    println!("----------------------------------------");

    let mac = local_mac();
    let config = *INTERFACE_CONFIG.lock();

    println!("[NET] Interface: eth0");
    println!("[NET]   MAC:     {}", mac);
    println!("[NET]   IPv4:    {}", config.ip);
    println!("[NET]   Netmask: {}", config.netmask);
    println!("[NET]   Gateway: {}", config.gateway);
    println!("[NET]   DNS:     {}", config.dns);
    println!("[NET] TCP/IP Protocol Layers active: Ethernet, ARP, IPv4, ICMP, UDP, TCP.");
}

/// Polls the network hardware for incoming packets and routes them through the protocol stack.
pub fn poll() {
    let mut rx_buf = [0u8; 1536];
    while let Ok(Some(len)) = virtio_net::receive(&mut rx_buf) {
        if len >= 14 {
            ethernet::handle_packet(&rx_buf[..len]);
        }
    }
}

/// Runs automated verification self-tests covering every layer of the network stack.
pub fn run_self_tests() {
    println!("[NET] Running TCP/IP Network Stack verification self-tests...");

    // 1. Ethernet Header & Serialization Test
    ethernet::run_self_tests();

    // 2. ARP Protocol & Cache Test
    arp::run_self_tests();

    // 3. IPv4 Header, Routing & Checksum Test
    ipv4::run_self_tests();

    // 4. ICMP Echo / Ping Test
    icmp::run_self_tests();

    // 5. UDP Datagram Transport Test
    udp::run_self_tests();

    // 6. TCP State Machine & Handshake Test
    tcp::run_self_tests();

    // 7. Sockets API Test
    socket::run_self_tests();

    println!("[NET] All TCP/IP Network Stack self-tests passed successfully!");
}
