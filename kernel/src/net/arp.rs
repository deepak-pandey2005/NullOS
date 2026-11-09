//! # Address Resolution Protocol (ARP) (Phase 8 - M24 Network Stack)
//!
//! This module implements the **Address Resolution Protocol** as defined in
//! [RFC 826](https://www.rfc-editor.org/rfc/rfc826).  ARP bridges Layer 2
//! (Ethernet, 48-bit MAC addresses) and Layer 3 (IPv4, 32-bit IP addresses)
//! by allowing a host to discover the hardware address of another host on the
//! same local segment given only its IP address.
//!
//! ## Why ARP is Necessary
//!
//! Ethernet switches and NICs route frames using **MAC addresses**, not IP
//! addresses.  When NullOS wants to transmit an IPv4 packet to `10.0.2.2`, the
//! Ethernet driver needs to know the MAC address to place in the frame header.
//! ARP provides this mapping.
//!
//! ## ARP Packet Format (28 bytes for Ethernet/IPv4)
//!
//! ```text
//! ┌────────┬────────┬──────┬──────┬────────┐
//! │ HType  │ PType  │ HLen │ PLen │  Oper  │
//! │ 2 bytes│ 2 bytes│1 byte│1 byte│ 2 bytes│
//! ├────────┴────────┴──────┴──────┴────────┤
//! │          Sender Hardware Address        │
//! │                 (6 bytes)               │
//! ├─────────────────────────────────────────┤
//! │          Sender Protocol Address        │
//! │                 (4 bytes)               │
//! ├─────────────────────────────────────────┤
//! │          Target Hardware Address        │
//! │                 (6 bytes)               │
//! ├─────────────────────────────────────────┤
//! │          Target Protocol Address        │
//! │                 (4 bytes)               │
//! └─────────────────────────────────────────┘
//! ```
//!
//! | Field  | Size | Description |
//! |--------|------|-------------|
//! | HType  | 2    | Hardware type: `1` = Ethernet 10 Mbps |
//! | PType  | 2    | Protocol type: `0x0800` = IPv4 |
//! | HLen   | 1    | Hardware address length: `6` bytes (MAC) |
//! | PLen   | 1    | Protocol address length: `4` bytes (IPv4) |
//! | Oper   | 2    | Operation: `1` = Request, `2` = Reply |
//! | SHA    | 6    | Sender Hardware Address (MAC) |
//! | SPA    | 4    | Sender Protocol Address (IPv4) |
//! | THA    | 6    | Target Hardware Address (MAC) — zeroed in Request |
//! | TPA    | 4    | Target Protocol Address (IPv4) |
//!
//! ## ARP Request / Reply Sequence
//!
//! ```text
//! Host A                             Host B (10.0.2.2)
//!   │  Who has 10.0.2.2? Tell 10.0.2.15  │
//!   │  ─────── ARP Request (broadcast) ──►│
//!   │                                     │  (B sees its own IP in TPA)
//!   │  10.0.2.2 is at 52:54:00:12:34:56  │
//!   │◄──────── ARP Reply (unicast) ───────│
//!   │                                     │
//!   │  (A caches B's MAC, then sends IP packet)
//! ```
//!
//! ## ARP Cache
//!
//! NullOS maintains a **dynamic ARP cache** (a [`BTreeMap`] from [`Ipv4Addr`] to
//! [`MacAddress`]) protected by a [`Spinlock`].  Entries are added:
//! 1. When an ARP reply is received (the sender's mapping is extracted).
//! 2. When *any* ARP packet is received — the sender's IP→MAC is *always* cached
//!    (this is the "gratuitous ARP" optimization: if you hear from a host, cache it).
//!
//! ## ARP Request Generation
//!
//! [`send_arp_request`] builds a 28-byte ARP packet where the **Target Hardware
//! Address is all zeros** (the MAC is unknown — that is the point of the request).
//! The Ethernet frame carrying the ARP request uses the **broadcast destination
//! MAC** (`FF:FF:FF:FF:FF:FF`) so that every host on the segment receives it.
//!
//! ## ARP Reply Handling
//!
//! When a host receives an ARP request for *its own IP*, it sends a **unicast
//! reply** directly back to the sender, filling in its own MAC as the Sender
//! Hardware Address.  [`handle_packet`] detects this and updates the ARP cache.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::drivers::virtio_net::MacAddress;
use crate::net::ethernet::{self, EtherType, EthernetFrame};
use crate::net::ipv4::Ipv4Addr;
use crate::println;
use crate::sync::Spinlock;

// ──────────────────────────────────────────────────────────────────────────────
// ARP Constants (RFC 826)
// ──────────────────────────────────────────────────────────────────────────────

/// ARP Hardware Type for **Ethernet** (10 Mbps).
///
/// Value `1` is assigned to Ethernet in the IANA Hardware Types registry.
/// Other values include Token Ring (6), Frame Relay (15), ATM (19), etc.
/// NullOS only supports Ethernet, so only `1` is accepted on input.
pub const ARP_HTYPE_ETHERNET: u16 = 1;

/// ARP Protocol Type for **IPv4**.
///
/// The same numeric namespace as EtherType is used for the protocol type.
/// `0x0800` selects IPv4, identical to `EtherType::Ipv4`.
pub const ARP_PTYPE_IPV4: u16 = 0x0800;

/// ARP Operation code for a **Request**.
///
/// A request packet asks "Who has IP *X*? Tell IP *Y*."  The target hardware
/// address field (THA) is filled with zeros because it is the unknown quantity.
/// The frame is sent to Ethernet broadcast `FF:FF:FF:FF:FF:FF`.
pub const ARP_OP_REQUEST: u16 = 1;

/// ARP Operation code for a **Reply**.
///
/// A reply packet answers "IP *X* is at MAC *M*."  It is sent as a **unicast**
/// Ethernet frame directly to the hardware address of the requester.
pub const ARP_OP_REPLY: u16 = 2;

// ──────────────────────────────────────────────────────────────────────────────
// ARP Packet Structure
// ──────────────────────────────────────────────────────────────────────────────

/// **Standard 28-byte ARP Packet** for the Ethernet/IPv4 combination.
///
/// All multi-byte fields are stored in **network byte order** (big-endian).
/// Use the accessor methods ([`sender_mac`], [`sender_ip`], etc.) which perform
/// the byte-swap to host order automatically.
///
/// ## Safety Note
///
/// `#[repr(C, packed)]` ensures the struct has no padding bytes and a size of
/// exactly 28 bytes.  Direct field access requires care because misaligned
/// reads are UB in Rust.  All reads from raw pointers use
/// `core::ptr::read_unaligned`.
///
/// ## Field Reference
///
/// ```text
/// Offset  Bytes  Field
///  0       2     htype       — Hardware type (network order)
///  2       2     ptype       — Protocol type (network order)
///  4       1     hlen        — Hardware address length (always 6)
///  5       1     plen        — Protocol address length (always 4)
///  6       2     oper        — Operation code (network order)
///  8       6     sender_mac  — Sender hardware (MAC) address
/// 14       4     sender_ip   — Sender protocol (IPv4) address
/// 18       6     target_mac  — Target hardware (MAC) address
/// 24       4     target_ip   — Target protocol (IPv4) address
///                             Total: 28 bytes
/// ```
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ArpPacket {
    /// **Hardware type** — identifies the link-layer technology.
    /// `1` for Ethernet (network byte order).  NullOS rejects any other value.
    pub htype: u16,      // Hardware type (1 = Ethernet)

    /// **Protocol type** — identifies the network-layer protocol being mapped.
    /// `0x0800` for IPv4 (network byte order).  NullOS rejects any other value.
    pub ptype: u16,      // Protocol type (0x0800 = IPv4)

    /// **Hardware address length** in bytes.
    /// Always `6` for Ethernet (a 48-bit / 6-byte MAC address).
    pub hlen: u8,        // Hardware address length (6 for MAC)

    /// **Protocol address length** in bytes.
    /// Always `4` for IPv4 (a 32-bit / 4-byte address).
    pub plen: u8,        // Protocol address length (4 for IPv4)

    /// **Operation code** — `1` for Request, `2` for Reply (network byte order).
    /// Use [`operation()`] to get the host-order value.
    pub oper: u16,       // Operation (1 = Request, 2 = Reply)

    /// **Sender Hardware Address (SHA)** — MAC address of the host that sent
    /// this ARP packet.  In a request this is the requester's own MAC.  In a
    /// reply this is the responder's MAC (the answer to the query).
    pub sender_mac: [u8; 6],

    /// **Sender Protocol Address (SPA)** — IPv4 address of the host that sent
    /// this ARP packet.  Big-endian byte order.
    pub sender_ip: [u8; 4],

    /// **Target Hardware Address (THA)** — the MAC address being sought.
    /// In an **ARP Request** this is zeroed (`[0x00; 6]`) because the sender
    /// does not yet know it.  In an **ARP Reply** this is set to the querier's
    /// MAC (the reply is directed at a specific host).
    pub target_mac: [u8; 6],

    /// **Target Protocol Address (TPA)** — the IPv4 address being queried.
    /// In a request, this is the IP whose MAC we want to discover.
    /// In a reply, this is the IPv4 of the original requester.
    pub target_ip: [u8; 4],
}

impl ArpPacket {
    /// Builds a new **ARP Request** packet.
    ///
    /// The request asks: "Who on this segment has IP address `target_ip`?
    /// Please send your MAC to `sender_ip` / `sender_mac`."
    ///
    /// The **Target Hardware Address** is zeroed because it is unknown — that
    /// is the purpose of the request.  The resulting packet is sent inside an
    /// Ethernet frame addressed to the broadcast MAC `FF:FF:FF:FF:FF:FF` so
    /// that every host on the local segment receives and processes it.
    ///
    /// All multi-byte fields are byte-swapped to network order with `.to_be()`.
    ///
    /// # Arguments
    ///
    /// * `sender_mac` – Our own MAC address (goes into SHA field).
    /// * `sender_ip`  – Our own IPv4 address (goes into SPA field).
    /// * `target_ip`  – The IPv4 address whose MAC we are asking for (goes into TPA).
    ///
    /// # Returns
    ///
    /// A 28-byte [`ArpPacket`] ready for serialization and transmission.
    pub fn new_request(sender_mac: MacAddress, sender_ip: Ipv4Addr, target_ip: Ipv4Addr) -> Self {
        Self {
            htype: ARP_HTYPE_ETHERNET.to_be(),  // Network byte order: 0x00_01
            ptype: ARP_PTYPE_IPV4.to_be(),       // Network byte order: 0x08_00
            hlen: 6,                              // Always 6 for Ethernet MAC
            plen: 4,                              // Always 4 for IPv4
            oper: ARP_OP_REQUEST.to_be(),         // Network byte order: 0x00_01
            sender_mac: sender_mac.0,
            sender_ip: sender_ip.octets(),
            // Target MAC is zeroed — unknown (that's what we're asking).
            target_mac: [0u8; 6],
            target_ip: target_ip.octets(),
        }
    }

    /// Builds a new **ARP Reply** packet.
    ///
    /// The reply answers an earlier ARP request with: "The MAC address for
    /// `sender_ip` is `sender_mac`."  It is sent as a unicast Ethernet frame
    /// directly to the requester (`target_mac` / `target_ip`).
    ///
    /// # Arguments
    ///
    /// * `sender_mac` – Our MAC address (the answer being provided).
    /// * `sender_ip`  – Our IPv4 address (the IP that was queried).
    /// * `target_mac` – The MAC address of the original requester.
    /// * `target_ip`  – The IPv4 address of the original requester.
    ///
    /// # Returns
    ///
    /// A 28-byte [`ArpPacket`] with operation `2` (Reply), ready to serialize.
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
            oper: ARP_OP_REPLY.to_be(), // Network byte order: 0x00_02
            sender_mac: sender_mac.0,
            sender_ip: sender_ip.octets(),
            target_mac: target_mac.0,
            target_ip: target_ip.octets(),
        }
    }

    /// Returns the **Sender Hardware Address** (SHA) as a [`MacAddress`].
    ///
    /// For an ARP request: the requester's own MAC.
    /// For an ARP reply: the MAC address that answers the query.
    pub fn sender_mac(&self) -> MacAddress {
        MacAddress(self.sender_mac)
    }

    /// Returns the **Sender Protocol Address** (SPA) as an [`Ipv4Addr`].
    ///
    /// This is the IPv4 address of the host that transmitted this ARP packet.
    /// It should always be cached in the ARP table when any ARP packet is received.
    pub fn sender_ip(&self) -> Ipv4Addr {
        Ipv4Addr::from_octets(self.sender_ip)
    }

    /// Returns the **Target Hardware Address** (THA) as a [`MacAddress`].
    ///
    /// In a request: `[0,0,0,0,0,0]` (the unknown we are asking for).
    /// In a reply: the MAC of the original requester (recipient of this reply).
    pub fn target_mac(&self) -> MacAddress {
        MacAddress(self.target_mac)
    }

    /// Returns the **Target Protocol Address** (TPA) as an [`Ipv4Addr`].
    ///
    /// In a request: the IPv4 address being queried.
    /// In a reply: the IPv4 of the original requester.
    pub fn target_ip(&self) -> Ipv4Addr {
        Ipv4Addr::from_octets(self.target_ip)
    }

    /// Returns the **operation code** in host byte order.
    ///
    /// * `1` = [`ARP_OP_REQUEST`] — "Who has IP X?"
    /// * `2` = [`ARP_OP_REPLY`]   — "IP X is at MAC M."
    ///
    /// Uses [`u16::from_be`] to convert the stored network-order value.
    pub fn operation(&self) -> u16 {
        u16::from_be(self.oper)
    }

    /// Serializes the [`ArpPacket`] into a **28-byte array** for transmission.
    ///
    /// Uses `core::mem::transmute_copy` because the struct is `#[repr(C, packed)]`
    /// with no padding, making a byte-for-byte copy safe and correct.
    ///
    /// # Safety
    ///
    /// `transmute_copy` is safe here because:
    /// * `ArpPacket` is `Copy` (no drop glue).
    /// * `size_of::<ArpPacket>() == 28 == size_of::<[u8; 28]>()`.
    /// * The packed repr guarantees no padding between fields.
    pub fn to_bytes(&self) -> [u8; 28] {
        // SAFETY: ArpPacket is #[repr(C, packed)], Copy, 28 bytes, no padding.
        unsafe { core::mem::transmute_copy(self) }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// ARP Cache
// ──────────────────────────────────────────────────────────────────────────────

/// **Global dynamic ARP Cache** — maps IPv4 addresses to their corresponding
/// Ethernet MAC addresses on the local segment.
///
/// Protected by a [`Spinlock`] for thread-safe access.  Uses a [`BTreeMap`]
/// (ordered by IPv4 address) rather than a hash map to avoid a hash dependency
/// in the kernel.
///
/// ## Lifetime of Cache Entries
///
/// Currently entries are **never evicted**.  On a live network, stale entries
/// can accumulate (e.g., after a host is replaced by one with a different MAC).
/// A production implementation would timestamp entries and evict them after
/// `ARP_CACHE_TIMEOUT` (typically 20 minutes per RFC 1122 §2.3.2.1).
///
/// ## Gratuitous ARP
///
/// RFC 5227 describes "gratuitous ARP" — a host may broadcast an ARP reply for
/// its own IP during boot to pre-populate neighbor caches and detect IP conflicts.
/// NullOS does not send gratuitous ARPs on startup but correctly handles them on
/// receipt (they update the cache because we always cache the sender's mapping).
pub static ARP_CACHE: Spinlock<BTreeMap<Ipv4Addr, MacAddress>> = Spinlock::new(BTreeMap::new());

/// Looks up an IPv4 address in the **local ARP cache**.
///
/// A cache hit avoids the round-trip latency of sending an ARP request and
/// waiting for a reply (typically < 1 ms on a LAN but can be significant in
/// polling-mode kernels without interrupt-driven RX).
///
/// # Arguments
///
/// * `ip` – The IPv4 address to look up.
///
/// # Returns
///
/// * `Some(MacAddress)` – The cached MAC address for `ip`.
/// * `None` – No entry for `ip` exists in the cache; an ARP request must be
///   sent.
pub fn arp_lookup(ip: Ipv4Addr) -> Option<MacAddress> {
    // Brief spinlock hold: clone the value (Copy) and release the lock.
    ARP_CACHE.lock().get(&ip).copied()
}

/// Inserts or **updates** an IPv4 → MAC mapping in the ARP cache.
///
/// If an entry for `ip` already exists (e.g., from a previous ARP request), it
/// is overwritten with the new `mac`.  This correctly handles host MAC changes
/// (e.g., after a VM live-migration or NIC replacement).
///
/// # Arguments
///
/// * `ip`  – The IPv4 address to associate with `mac`.
/// * `mac` – The 48-bit MAC address of the host at `ip`.
pub fn arp_insert(ip: Ipv4Addr, mac: MacAddress) {
    ARP_CACHE.lock().insert(ip, mac);
}

/// Returns a **snapshot** of the current ARP table as a `Vec` of `(IPv4, MAC)`
/// pairs, ordered by IPv4 address.
///
/// Used by the `arp` shell command to display the neighbor table in human-
/// readable form.  The snapshot is taken under the spinlock and immediately
/// released; the caller receives a stable copy.
///
/// # Returns
///
/// A `Vec<(Ipv4Addr, MacAddress)>` with one entry per cached mapping, sorted
/// by IPv4 address (ascending).
pub fn arp_table() -> Vec<(Ipv4Addr, MacAddress)> {
    ARP_CACHE.lock().iter().map(|(&ip, &mac)| (ip, mac)).collect()
}

// ──────────────────────────────────────────────────────────────────────────────
// ARP Packet TX / RX
// ──────────────────────────────────────────────────────────────────────────────

/// **Transmits an ARP Request** for the specified target IPv4 address.
///
/// Builds an ARP request with our own IP/MAC as the sender and the target IP
/// as the query address.  The request is wrapped in an Ethernet frame addressed
/// to the **broadcast MAC** so every host on the local segment receives it.
///
/// # Arguments
///
/// * `target_ip` – The IPv4 address whose hardware (MAC) address we want to
///   discover.
///
/// # Returns
///
/// * `Ok(())` – The ARP request frame was placed into the VirtIO TX ring.
/// * `Err(&str)` – TX ring is full or the VirtIO driver is unavailable.
pub fn send_arp_request(target_ip: Ipv4Addr) -> Result<(), &'static str> {
    let local_mac = super::local_mac();
    let local_ip  = super::local_ip();

    // Build the 28-byte ARP request packet.
    let arp = ArpPacket::new_request(local_mac, local_ip, target_ip);

    // Encapsulate in an Ethernet frame with broadcast destination.
    // The broadcast MAC ensures every host on the LAN segment receives the frame.
    ethernet::send_frame(MacAddress::BROADCAST, EtherType::Arp, &arp.to_bytes())
}

/// **Handles incoming ARP packets** from Ethernet frames.
///
/// Called by [`super::ethernet::handle_packet`] when the EtherType field is
/// `0x0806` (ARP).  Performs the following steps:
///
/// 1. **Length check**: Verifies the payload contains at least 28 bytes.
/// 2. **Field validation**: Rejects packets with unexpected `htype` or `ptype`
///    (e.g., non-Ethernet, non-IPv4 ARP variants).
/// 3. **Gratuitous caching**: Always caches the sender's IP→MAC mapping,
///    regardless of whether the packet is a request or reply.  This is correct
///    per RFC 826 and handles both normal replies and gratuitous ARPs.
/// 4. **Request dispatch**: If the operation is a request (`Oper = 1`) *and*
///    the target IP matches our local IP, we send a unicast ARP reply.
/// 5. **Reply dispatch**: If the operation is a reply (`Oper = 2`), the sender's
///    mapping was already cached in step 3; no additional action is needed.
///
/// # Arguments
///
/// * `frame` – The parsed Ethernet frame carrying the ARP packet in its payload.
pub fn handle_packet(frame: &EthernetFrame) {
    // Validate that the payload is large enough to hold a complete ARP packet.
    if frame.payload.len() < core::mem::size_of::<ArpPacket>() {
        return; // Truncated ARP packet — discard silently.
    }

    // SAFETY: Payload is at least 28 bytes (checked above).
    // read_unaligned handles potential misalignment of the payload buffer.
    let arp = unsafe {
        core::ptr::read_unaligned(frame.payload.as_ptr() as *const ArpPacket)
    };

    // Validate hardware and protocol type fields.
    // We only handle Ethernet (htype=1) + IPv4 (ptype=0x0800) ARP.
    if u16::from_be(arp.htype) != ARP_HTYPE_ETHERNET || u16::from_be(arp.ptype) != ARP_PTYPE_IPV4 {
        return; // Unsupported hardware/protocol combination.
    }

    let sender_ip  = arp.sender_ip();
    let sender_mac = arp.sender_mac();
    let target_ip  = arp.target_ip();
    let local_ip   = super::local_ip();
    let local_mac  = super::local_mac();

    // Cache the sender's MAC address
    // RFC 826: "The receiver updates (or creates) the entry for the sender's
    // protocol address / hardware address pair in the translation table."
    // This covers both request and reply senders.
    arp_insert(sender_ip, sender_mac);

    match arp.operation() {
        ARP_OP_REQUEST => {
            // If the query asks for our IP, reply with our MAC!
            // We only reply if the target IP in the request matches our own IP.
            // Responding to requests for other IPs would constitute ARP spoofing.
            if target_ip == local_ip {
                // Build a unicast ARP reply directed at the requester.
                let reply = ArpPacket::new_reply(local_mac, local_ip, sender_mac, sender_ip);
                // Send the reply as a unicast frame to the requester's MAC address.
                let _ = ethernet::send_frame(sender_mac, EtherType::Arp, &reply.to_bytes());
            }
        }
        ARP_OP_REPLY => {
            // Already cached sender_ip -> sender_mac above!
            // The mapping was stored in the gratuitous caching step.  Nothing
            // more to do — resolve_mac's polling loop will pick it up.
        }
        _ => {
            // Unknown ARP operation — silently discard.
        }
    }
}

/// **Resolves an IP address to a MAC address**, using the ARP cache when possible
/// and falling back to a live ARP request if the mapping is not cached.
///
/// ## Resolution Algorithm
///
/// 1. **Special case — broadcast**: `255.255.255.255` always maps to Ethernet
///    broadcast `FF:FF:FF:FF:FF:FF` without any ARP query.
/// 2. **Cache hit**: If the ARP cache already contains a mapping for `target_ip`,
///    return it immediately.
/// 3. **ARP request**: Send a broadcast ARP request for `target_ip`.
/// 4. **Polling loop**: Call [`super::poll()`] up to 50 times (~500 ms with
///    1 ms sleep intervals) waiting for the ARP reply to arrive.
/// 5. **QEMU gateway fallback**: If resolution fails and `target_ip` is the
///    QEMU default gateway (`10.0.2.2`), inject a hard-coded MAC for the SLIRP
///    virtual router (`52:54:00:12:34:56`).  This is a pragmatic workaround
///    because QEMU SLIRP does not reliably respond to ARP requests in all
///    configurations.
///
/// # Arguments
///
/// * `target_ip` – The IPv4 address whose MAC address is needed.
///
/// # Returns
///
/// * `Some(MacAddress)` – The resolved MAC address.
/// * `None` – Resolution failed after all retries and fallbacks.
pub fn resolve_mac(target_ip: Ipv4Addr) -> Option<MacAddress> {
    // The IPv4 limited broadcast address always maps to Ethernet broadcast.
    if target_ip == Ipv4Addr::BROADCAST {
        return Some(MacAddress::BROADCAST);
    }

    // 1. Check ARP cache — fast path, no network traffic required.
    if let Some(mac) = arp_lookup(target_ip) {
        return Some(mac);
    }

    // 2. Query ARP request over the network
    // Send the ARP request and begin polling for a reply.
    let _ = send_arp_request(target_ip);

    // 3. Briefly poll for ARP reply
    // Poll the VirtIO RX ring up to 50 times, sleeping 1 ms between each poll.
    // The ARP reply should arrive within a few milliseconds on a healthy network.
    for _ in 0..50 {
        super::poll();
        if let Some(mac) = arp_lookup(target_ip) {
            return Some(mac); // Reply received; return the resolved MAC.
        }
        crate::arch::x86_64::apic::sleep(1); // Sleep ~1 ms before retrying.
    }

    // Fall back to default QEMU router MAC if unresolvable on virtual network
    // QEMU SLIRP does not always reply to ARP requests (requires host privileges).
    // We inject the well-known QEMU SLIRP MAC so that routing to the gateway works.
    if target_ip == super::DEFAULT_GATEWAY {
        let default_mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        arp_insert(target_ip, default_mac);
        return Some(default_mac);
    }

    None // Could not resolve — caller should drop the packet or return an error.
}

// ──────────────────────────────────────────────────────────────────────────────
// Self-Tests
// ──────────────────────────────────────────────────────────────────────────────

/// **Runs automated self-tests** for ARP packet building, parsing, and caching.
///
/// ## Tests performed:
///
/// 1. Build an ARP Request and verify all fields (operation, sender MAC/IP,
///    target IP).
/// 2. Insert a fake router MAC into the ARP cache and verify the lookup.
///
/// No network traffic is generated during these tests.
pub fn run_self_tests() {
    // Define test addresses representing a typical QEMU SLIRP setup.
    let my_mac     = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    let my_ip      = Ipv4Addr::new(10, 0, 2, 15);
    let gateway_ip = Ipv4Addr::new(10, 0, 2, 2);

    // Build an ARP request and verify all fields decode correctly.
    let req = ArpPacket::new_request(my_mac, my_ip, gateway_ip);
    assert_eq!(req.operation(), ARP_OP_REQUEST,  "ARP operation must be REQUEST (1)");
    assert_eq!(req.sender_mac(), my_mac,         "Sender MAC mismatch");
    assert_eq!(req.sender_ip(), my_ip,           "Sender IP mismatch");
    assert_eq!(req.target_ip(), gateway_ip,      "Target IP mismatch");
    // Target MAC must be zeroed in a request (we don't know it yet).
    assert_eq!(req.target_mac(), MacAddress::new([0, 0, 0, 0, 0, 0]),
               "Target MAC must be zeroed in ARP Request");

    // Test cache insertion and retrieval
    // Simulate receiving an ARP reply from the gateway by directly inserting its MAC.
    let router_mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x02]);
    arp_insert(gateway_ip, router_mac);
    assert_eq!(arp_lookup(gateway_ip), Some(router_mac),
               "ARP cache lookup returned wrong MAC after insertion");

    println!("[ARP] ARP packet encoding & table cache verified successfully!");
}
