//! # NullOS Network Stack (Phase 8 - M24 Network Stack)
//!
//! This module is the **root of the NullOS TCP/IP network stack**. It owns the global
//! interface configuration state, exposes the top-level `init`, `poll`, and
//! `run_self_tests` entry points, and re-exports the most commonly used types.
//!
//! ## Architecture – the Layered TCP/IP Model
//!
//! NullOS follows the classic ARPA / Internet layered model (sometimes called the
//! "DoD model"), which maps roughly onto the ISO-OSI 7-layer model:
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────────────┐
//! │  Layer 5 – Socket / Session  │  socket.rs  │  Berkeley Sockets API       │
//! │                              │             │  socket(), bind(), connect() │
//! │                              │             │  listen(), accept(), send()  │
//! │                              │             │  recv(), close()             │
//! ├──────────────────────────────┼─────────────┼─────────────────────────────┤
//! │  Layer 4 – Transport         │  tcp.rs     │  RFC 793 connection-oriented │
//! │                              │             │  byte-stream, 3-way HS,      │
//! │                              │             │  flow/congestion control     │
//! │                              │  udp.rs     │  Connectionless datagrams,   │
//! │                              │             │  no ordering / delivery      │
//! ├──────────────────────────────┼─────────────┼─────────────────────────────┤
//! │  Layer 3 – Network           │  ipv4.rs    │  32-bit addressing, routing, │
//! │                              │             │  fragmentation, TTL, RFC1071 │
//! │                              │             │  checksum                    │
//! │                              │  icmp.rs    │  Control & diagnostic msgs   │
//! │                              │             │  (ping / unreachable / TTL)  │
//! │                              │  arp.rs     │  IPv4 ↔ MAC resolution       │
//! ├──────────────────────────────┼─────────────┼─────────────────────────────┤
//! │  Layer 2 – Link / Ethernet   │  ethernet.rs│  Ethernet II framing,        │
//! │                              │             │  EtherType dispatch          │
//! ├──────────────────────────────┼─────────────┼─────────────────────────────┤
//! │  Layer 1 – Physical / Driver │  virtio_net │  VirtIO network device,      │
//! │                              │  (drivers/) │  DMA ring buffers, MMIO      │
//! └──────────────────────────────┴─────────────┴─────────────────────────────┘
//! ```
//!
//! ## Packet Receive Path (bottom → top)
//!
//! 1. `poll()` calls `virtio_net::receive()` to drain the VirtIO RX ring.
//! 2. Each raw byte slice (up to 1536 bytes, standard jumbo-safe MTU) is passed
//!    to `ethernet::handle_packet()`.
//! 3. The Ethernet handler checks the destination MAC (unicast/broadcast) and
//!    dispatches on the 2-byte EtherType field:
//!    * `0x0806` → `arp::handle_packet()` – ARP cache update + optional reply.
//!    * `0x0800` → `ipv4::handle_packet()` – checksum validation + protocol dispatch.
//! 4. IPv4 dispatches further on the 1-byte Protocol field:
//!    * `1`  → `icmp::handle_packet()` – echo reply or record ping response.
//!    * `17` → `udp::handle_packet()`  – demux by destination port.
//!    * `6`  → `tcp::handle_packet()`  – state machine + demux by port.
//! 5. UDP/TCP both call into `socket::dispatch_udp` / `socket::dispatch_tcp` to
//!    place data into the correct [`Socket`]'s `rx_queue` and drive TCP state
//!    transitions.
//!
//! ## Packet Transmit Path (top → bottom)
//!
//! Applications call `socket::send()` → `udp::send_udp()` or
//! `tcp::send_tcp_segment()` → `ipv4::send_ipv4_packet()` → `ethernet::send_frame()`
//! → `virtio_net::send()` → DMA to NIC ring.
//!
//! ## Network Configuration
//!
//! NullOS currently uses **static** IP configuration tailored for QEMU's SLIRP
//! user-mode NAT network (see [`DEFAULT_IP`], [`DEFAULT_GATEWAY`] etc.).  The
//! configuration is stored in the global [`INTERFACE_CONFIG`] spinlock.
//!
//! ## Submodules
//!
//! * [`ethernet`] – Layer 2: Ethernet II frame parsing, construction, and dispatch.
//! * [`arp`]      – Layer 3: ARP cache, request generation, reply handling.
//! * [`ipv4`]     – Layer 3: IPv4 header, routing, checksum, transport dispatch.
//! * [`icmp`]     – Layer 3: ICMP echo request/reply (`ping`), error messages.
//! * [`udp`]      – Layer 4: UDP header, pseudo-header checksum, datagram TX/RX.
//! * [`tcp`]      – Layer 4: TCP header, flags, state machine, 3-way handshake.
//! * [`socket`]   – Layer 5: Berkeley socket API, connection table, demuxing.
#![allow(dead_code)]

pub mod arp;
pub mod ethernet;
pub mod icmp;
pub mod ipv4;
pub mod perf;
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
///
/// QEMU's SLIRP user-mode networking automatically creates a virtual subnet
/// `10.0.2.0/24` with the following well-known addresses:
///
/// | Purpose  | Address       |
/// |----------|---------------|
/// | Guest VM | `10.0.2.15`   |
/// | Gateway  | `10.0.2.2`    |
/// | DNS      | `10.0.2.3`    |
/// | SMB/CIFS | `10.0.2.4`    |
///
/// These values are compiled-in constants.  On physical hardware a DHCP client
/// would negotiate them at runtime.
// TODO(dhcp): Currently, NullOS relies on static IP defaults tailored to QEMU SLIRP (10.0.2.15/24).
// On real physical hardware connected to a home or corporate router, static IP assignment fails or
// causes IP conflicts. Implement a DHCP client protocol (RFC 2131) over UDP ports 67/68 to lease
// a dynamic IPv4 address, subnet mask, default gateway router, and DNS servers.
//
// TODO(dns): Implement a DNS client resolver (RFC 1035) over UDP port 53. Currently, applications
// can only connect to literal numeric IPv4 addresses; hostnames cannot be resolved.

/// Local guest IP address assigned by QEMU SLIRP (`10.0.2.15`).
pub const DEFAULT_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 15);

/// Subnet mask for the QEMU SLIRP `/24` network (`255.255.255.0`).
/// A `/24` prefix means the top 24 bits identify the network; the bottom 8
/// bits address individual hosts (up to 254 usable).
pub const DEFAULT_NETMASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);

/// Default gateway IP — QEMU SLIRP router (`10.0.2.2`).
/// All packets destined for addresses *outside* the local subnet are forwarded
/// to this router.  It performs NAT on behalf of the guest.
pub const DEFAULT_GATEWAY: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 2);

/// DNS resolver address provided by QEMU SLIRP (`10.0.2.3`).
/// Queries are forwarded by SLIRP to the host's configured DNS server.
pub const DEFAULT_DNS: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 3);

/// Runtime network interface configuration for the primary (and only) NullOS
/// network interface `eth0`.
///
/// All four fields are standard IPv4 addresses stored as 4-byte arrays in
/// network (big-endian) byte order inside [`Ipv4Addr`].
///
/// # Fields
///
/// * `ip`      – The IPv4 address assigned to this host interface.
/// * `netmask` – The subnet mask; combined with `ip` via bitwise AND it yields
///               the network prefix used to decide direct vs. routed delivery.
/// * `gateway` – The first-hop router for destinations outside the local subnet.
/// * `dns`     – The DNS resolver used by the hostname lookup subsystem (once
///               implemented via TODO(dns)).
#[derive(Debug, Clone, Copy)]
pub struct InterfaceConfig {
    /// The IPv4 address assigned to this host interface (e.g. `10.0.2.15`).
    pub ip: Ipv4Addr,
    /// Subnet mask in dotted-decimal form (e.g. `255.255.255.0` for `/24`).
    /// Used to determine whether a destination IP is on the local segment or
    /// must be routed through the gateway.
    pub netmask: Ipv4Addr,
    /// Default gateway router IPv4 address (e.g. `10.0.2.2`).
    /// Packets whose destination is not in `ip & netmask` are sent here.
    pub gateway: Ipv4Addr,
    /// DNS nameserver address (e.g. `10.0.2.3`).
    /// Used for hostname-to-IP resolution (not yet implemented; see TODO(dns)).
    pub dns: Ipv4Addr,
}

impl Default for InterfaceConfig {
    /// Returns the QEMU SLIRP static default interface configuration.
    fn default() -> Self {
        Self {
            ip: DEFAULT_IP,
            netmask: DEFAULT_NETMASK,
            gateway: DEFAULT_GATEWAY,
            dns: DEFAULT_DNS,
        }
    }
}

/// Global host network configuration protected by a [`Spinlock`].
///
/// All layers of the stack read from this when they need the local IP address
/// (e.g., to fill the IPv4 source field or the ARP sender IP field).
///
/// # Concurrency
///
/// A `Spinlock` is used because NullOS currently runs on a single CPU core
/// without preemptive scheduling.  When SMP is introduced the spinlock will
/// still be safe, though contention should remain minimal since writes happen
/// only during initialization (or future DHCP lease renewal).
pub static INTERFACE_CONFIG: Spinlock<InterfaceConfig> = Spinlock::new(InterfaceConfig {
    ip: DEFAULT_IP,
    netmask: DEFAULT_NETMASK,
    gateway: DEFAULT_GATEWAY,
    dns: DEFAULT_DNS,
});

/// Returns the **active** local IPv4 address of `eth0`.
///
/// This is a convenience wrapper that acquires the [`INTERFACE_CONFIG`] spinlock,
/// reads the `ip` field, and releases the lock before returning — keeping the
/// lock hold-time to an absolute minimum.
///
/// # Returns
///
/// The [`Ipv4Addr`] currently configured for the primary interface.  Defaults
/// to `10.0.2.15` unless overwritten by DHCP or manual configuration.
pub fn local_ip() -> Ipv4Addr {
    INTERFACE_CONFIG.lock().ip
}

/// Returns the **active** local MAC address of the VirtIO NIC (`eth0`).
///
/// Queries the VirtIO network driver for the hardware MAC address that was
/// negotiated with the hypervisor during device initialization.  Falls back to
/// [`MacAddress::ZERO`] (`00:00:00:00:00:00`) if the driver is not yet ready,
/// which should not occur after `init()` has been called.
///
/// # Returns
///
/// The 6-byte [`MacAddress`] programmed into the VirtIO NIC by QEMU.
/// For example, QEMU typically assigns `52:54:00:xx:xx:xx` (locally administered,
/// unicast range).
pub fn local_mac() -> MacAddress {
    virtio_net::get_mac().unwrap_or(MacAddress::ZERO)
}

/// **Initializes** the NullOS TCP/IP Network Stack.
///
/// This function must be called once during kernel boot, after the VirtIO
/// network device driver has been initialized (so that [`local_mac()`] returns
/// a valid address).
///
/// It performs the following:
/// 1. Reads the local MAC from the VirtIO driver.
/// 2. Reads the static interface configuration from [`INTERFACE_CONFIG`].
/// 3. Prints a formatted diagnostic banner summarizing the interface state.
///
/// No packets are transmitted during initialization; ARP/IP are passive until
/// [`poll()`] is first called.
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
    perf::init();
}

/// **Polls** the VirtIO network hardware for incoming packets and routes each
/// packet up through the full protocol stack.
///
/// # Design: Cooperative Polling
///
/// NullOS uses a **cooperative polling model** (no interrupt-driven RX) for
/// simplicity.  `poll()` must be called periodically by:
/// * The kernel main loop.
/// * Any blocking operation that waits for a network event (e.g., ARP reply,
///   ICMP echo reply, TCP SYN-ACK).  This avoids deadlock: the caller holds no
///   spinlocks while waiting, and periodic polling delivers the awaited packet.
///
/// # Packet Processing Loop
///
/// Each call drains the entire VirtIO RX ring in a `while` loop:
/// * `virtio_net::receive(&mut rx_buf)` — copies one available frame into the
///   provided 1536-byte stack buffer.  Returns `Ok(Some(len))` while frames are
///   available, `Ok(None)` when the ring is empty.
/// * Frames shorter than the minimum Ethernet header (14 bytes) are discarded.
/// * Valid frames are forwarded to [`ethernet::handle_packet()`] for full
///   protocol-stack processing.
///
/// # Buffer Size
///
/// The 1536-byte buffer covers any standard Ethernet frame (max payload 1500
/// bytes + 14-byte header = 1514 bytes) with a small safety margin.  Jumbo
/// frames (> 1500 byte payload) are not supported.
pub fn poll() {
    // Route VirtIO RX polling through the Phase 16 NAPI adaptive budgeted poller,
    // RSS multiqueue steering, and zero-copy pooled NetBuf ring.
    perf::polling::GLOBAL_NAPI_POLLER.poll(perf::polling::DEFAULT_NAPI_BUDGET);
}

/// **Runs automated verification self-tests** covering every layer of the
/// network stack in order from Layer 2 up to Layer 5.
///
/// Each sub-test is a pure in-memory unit test that does **not** transmit real
/// packets (or uses the network only as a side effect in ICMP/ping).  Assertions
/// use `assert!` / `assert_eq!` and will `panic!` on failure, halting the
/// kernel with a clear error message.
///
/// # Test Sequence
///
/// 1. **Ethernet** – frame construction, minimum-length padding, parse round-trip.
/// 2. **ARP** – packet encoding, ARP cache insertion/lookup.
/// 3. **IPv4** – header construction, RFC 1071 checksum, dotted-decimal parsing.
/// 4. **ICMP** – echo request/reply serialization, checksum.
/// 5. **UDP** – header construction, pseudo-header checksum.
/// 6. **TCP** – header construction, flag accessors, pseudo-header checksum,
///             state display.
/// 7. **Socket** – UDP bind/dispatch/recv lifecycle; TCP listen state machine.
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
