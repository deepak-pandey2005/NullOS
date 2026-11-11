//! # IPv4 Network Layer Protocol (Phase 8 - M24 Network Stack)
//!
//! This module provides the **Internet Protocol version 4** (IPv4) implementation
//! for NullOS, covering:
//!
//! * [`Ipv4Addr`] — a 4-byte IPv4 address type with dotted-decimal I/O.
//! * [`IpProtocol`] — the 1-byte IP protocol number field.
//! * [`Ipv4Header`] — the 20-byte IPv4 header (no options), wire-format struct.
//! * [`compute_checksum`] — RFC 1071 Internet Checksum algorithm.
//! * [`send_ipv4_packet`] — build, route, and transmit an IPv4 packet.
//! * [`handle_packet`] — validate and dispatch a received IPv4 packet.
//!
//! ## IPv4 Header Format (RFC 791)
//!
//! The minimum IPv4 header is **20 bytes** (IHL = 5 × 4-byte words).  Optional
//! extension headers (timestamp, source routing, etc.) are rarely used in
//! practice and are **not supported** by NullOS.
//!
//! ```text
//! 0                   1                   2                   3
//! 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |Version|  IHL  |Type of Service|          Total Length         |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |         Identification        |Flags|      Fragment Offset    |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |  Time to Live |    Protocol   |         Header Checksum       |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                       Source Address                          |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                    Destination Address                        |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! ```
//!
//! | Bits | Field            | Description |
//! |------|------------------|-------------|
//! | 4    | Version          | Always `4` for IPv4 |
//! | 4    | IHL              | Header length in 32-bit words; `5` = 20 bytes |
//! | 8    | DSCP + ECN (TOS) | Differentiated services / explicit congestion |
//! | 16   | Total Length     | Header + payload size in bytes |
//! | 16   | Identification   | Unique ID for fragment reassembly |
//! | 3    | Flags            | Bit 1 = DF (Don't Fragment), Bit 2 = MF (More Fragments) |
//! | 13   | Fragment Offset  | Position of this fragment in units of 8 bytes |
//! | 8    | TTL              | Decremented by each router; packet dropped at 0 |
//! | 8    | Protocol         | Upper-layer: ICMP=1, TCP=6, UDP=17 |
//! | 16   | Header Checksum  | RFC 1071 one's complement sum of header words |
//! | 32   | Source Address   | Sender's IPv4 address |
//! | 32   | Destination Addr | Receiver's IPv4 address |
//!
//! ## Protocol Number Registry (IANA)
//!
//! | Decimal | Hex    | Protocol |
//! |---------|--------|----------|
//! | 1       | `0x01` | ICMP     |
//! | 2       | `0x02` | IGMP     |
//! | 6       | `0x06` | TCP      |
//! | 17      | `0x11` | UDP      |
//! | 41      | `0x29` | IPv6-in-IPv4 (6in4) |
//! | 89      | `0x59` | OSPF     |
//!
//! ## RFC 1071 Internet Checksum
//!
//! The 16-bit **one's complement checksum** is computed by:
//! 1. Treating the data as a sequence of big-endian 16-bit words.
//! 2. Summing all words using 32-bit arithmetic (to hold carry bits).
//! 3. Adding any carry (bits above bit 15) back into the lower 16 bits (fold).
//! 4. Taking the **one's complement** (bitwise NOT) of the 16-bit result.
//!
//! **Verification property**: When the checksum is included in the data and the
//! same algorithm is run, the result is `0x0000` for a valid, uncorrupted header.
//! This is used to validate received packets: `compute_checksum(hdr_bytes) == 0`.
//!
//! ## IPv4 Fragmentation
//!
//! IPv4 supports splitting packets that exceed the path MTU into **fragments**:
//! * **DF (Don't Fragment)** bit (flags bit 1): If set, the router drops the
//!   packet and sends an ICMP Type 3, Code 4 (Fragmentation Needed) error rather
//!   than fragmenting.  NullOS sets `DF = 1` on all outgoing packets.
//! * **MF (More Fragments)** bit (flags bit 2): Set on all fragments except the
//!   last one.
//! * **Fragment Offset**: The byte offset of this fragment's payload within the
//!   original datagram, divided by 8 (hence offsets are multiples of 8 bytes).
//!
//! NullOS does **not** implement fragment reassembly (all received fragments
//! must arrive as a single non-fragmented packet, or only the first fragment is
//! processed).
//!
//! ## Routing Decision
//!
//! [`send_ipv4_packet`] uses a simple **longest-prefix / direct vs. gateway** rule:
//! * If `dst_ip & netmask == local_ip & netmask` → the destination is on the
//!   same subnet; ARP for `dst_ip` directly.
//! * Otherwise → route via the default gateway; ARP for `gateway_ip` instead.

use alloc::vec::Vec;
use core::fmt;

use crate::net::ethernet::{self, EtherType, EthernetFrame};
use crate::println;

/// Minimum IPv4 header length in bytes (no options, IHL = 5).
///
/// An IPv4 header with IHL = 5 occupies 5 × 4 = **20 bytes**.  When IPv4
/// options are present (IHL > 5) the header is longer, up to 60 bytes maximum
/// (IHL = 15).  NullOS always generates 20-byte headers and ignores option
/// bytes in received packets (they are skipped via `header.header_length()`).
pub const IPV4_HDR_MIN_LEN: usize = 20;

// ──────────────────────────────────────────────────────────────────────────────
// IPv4 Address Type
// ──────────────────────────────────────────────────────────────────────────────

/// A **4-byte standard IPv4 Address** (e.g., `192.168.1.1`).
///
/// Stored internally as a `[u8; 4]` in **network byte order** (big-endian,
/// most-significant octet first), which is the canonical human-readable form
/// (e.g., `10.0.2.15` = `[10, 0, 2, 15]`).
///
/// Implements `PartialOrd + Ord` so it can be used as a [`BTreeMap`] key (see
/// the ARP cache in [`super::arp`]).  Implements `Hash` for potential future use
/// in `HashMap`.
///
/// ## Well-Known Addresses
///
/// * [`Ipv4Addr::LOCALHOST`] — `127.0.0.1` — the loopback address.  Packets to
///   this address are delivered to the same host without hitting the network.
/// * [`Ipv4Addr::BROADCAST`] — `255.255.255.255` — limited broadcast; delivered
///   to all hosts on the local segment.
/// * [`Ipv4Addr::ANY`] — `0.0.0.0` — the unspecified address, used as a wildcard
///   in `bind()` to listen on all interfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Ipv4Addr(pub [u8; 4]);

impl Ipv4Addr {
    /// Loopback address `127.0.0.1`.
    ///
    /// Packets destined for any `127.x.x.x` address (the entire `127.0.0.0/8`
    /// block) are looped back to the originating host.  NullOS does not
    /// implement a loopback interface; these addresses are recognized by
    /// [`is_loopback()`] but not specially routed.
    pub const LOCALHOST: Self = Self([127, 0, 0, 1]);

    /// Limited broadcast address `255.255.255.255`.
    ///
    /// Sent to all hosts on the local subnet without knowing the subnet address.
    /// Unlike directed broadcast (e.g., `10.0.2.255` for a `/24` subnet), the
    /// limited broadcast is never forwarded by routers.
    pub const BROADCAST: Self = Self([255, 255, 255, 255]);

    /// Unspecified / "any" address `0.0.0.0`.
    ///
    /// Used as a wildcard in socket `bind()` calls to indicate the socket
    /// should accept connections on all available network interfaces.
    pub const ANY: Self = Self([0, 0, 0, 0]);

    /// Constructs a new [`Ipv4Addr`] from four individual **octet** values.
    ///
    /// Arguments are in dotted-decimal order: `a.b.c.d`.
    ///
    /// # Arguments
    ///
    /// * `a` – Most-significant octet (e.g., `10` in `10.0.2.15`).
    /// * `b` – Second octet.
    /// * `c` – Third octet.
    /// * `d` – Least-significant octet.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let ip = Ipv4Addr::new(192, 168, 1, 1);
    /// ```
    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self([a, b, c, d])
    }

    /// Constructs a new [`Ipv4Addr`] from a 4-byte array in **network byte order**.
    ///
    /// This is the inverse of [`octets()`].  The array `[a, b, c, d]` corresponds
    /// to the dotted-decimal notation `a.b.c.d`.
    pub const fn from_octets(octets: [u8; 4]) -> Self {
        Self(octets)
    }

    /// Returns the address as a **4-byte array** in network byte order.
    ///
    /// Suitable for direct inclusion in protocol headers (ARP sender/target IP
    /// fields, IPv4 src/dst fields).
    pub const fn octets(&self) -> [u8; 4] {
        self.0
    }

    /// Converts the IPv4 address to a **32-bit unsigned integer** in host byte order.
    ///
    /// For example, `10.0.2.15` → `0x0A_00_02_0F` = `167_772_687` in decimal.
    ///
    /// Used for arithmetic operations (subnet masking, comparison) where working
    /// with individual bytes would be cumbersome.
    pub fn to_u32(&self) -> u32 {
        // from_be_bytes interprets the byte array as big-endian and returns host order.
        u32::from_be_bytes(self.0)
    }

    /// Constructs an [`Ipv4Addr`] from a **32-bit host-order integer**.
    ///
    /// This is the inverse of [`to_u32()`].
    ///
    /// # Arguments
    ///
    /// * `val` – An IPv4 address encoded as a 32-bit host-order integer.
    pub fn from_u32(val: u32) -> Self {
        // to_be_bytes converts host order to big-endian network byte order.
        Self(val.to_be_bytes())
    }

    /// Returns `true` if the address is in the **loopback range** (`127.0.0.0/8`).
    ///
    /// Any address whose first octet is `127` is a loopback address per RFC 1122
    /// §3.2.1.3.  The most common loopback address is `127.0.0.1`.
    pub fn is_loopback(&self) -> bool {
        self.0[0] == 127
    }

    /// Returns `true` if the address is the **limited broadcast** (`255.255.255.255`).
    pub fn is_broadcast(&self) -> bool {
        *self == Self::BROADCAST
    }

    /// Checks if another IP address resides within the **same subnet** defined by `netmask`.
    ///
    /// Two hosts are on the same subnet if their addresses, when ANDed with the
    /// subnet mask, yield the same **network prefix**:
    ///
    /// ```text
    /// (self.to_u32() & mask) == (other.to_u32() & mask)
    /// ```
    ///
    /// For example, with netmask `255.255.255.0` (/24):
    /// * `10.0.2.15` and `10.0.2.100` → same subnet (`10.0.2.0`).
    /// * `10.0.2.15` and `10.0.3.1`  → different subnets.
    ///
    /// # Arguments
    ///
    /// * `other`   – The peer IPv4 address to compare against.
    /// * `netmask` – The subnet mask (e.g., `255.255.255.0` for /24).
    ///
    /// # Returns
    ///
    /// `true` if `self` and `other` share the same network prefix.
    pub fn in_same_subnet(&self, other: &Self, netmask: &Self) -> bool {
        (self.to_u32() & netmask.to_u32()) == (other.to_u32() & netmask.to_u32())
    }

    /// Parses an IPv4 address from a **dotted-decimal string** (e.g. `"10.0.2.15"`).
    ///
    /// Splits the string on `.` and parses each of the four parts as a `u8`
    /// (0–255).  Rejects strings with fewer or more than 4 octets and any
    /// octet that cannot be parsed as a decimal integer in [0, 255].
    ///
    /// # Arguments
    ///
    /// * `s` – A string slice in `"a.b.c.d"` format.
    ///
    /// # Returns
    ///
    /// * `Ok(Ipv4Addr)` – Successfully parsed address.
    /// * `Err(&str)` – The string is not a valid dotted-decimal IPv4 address.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let ip = Ipv4Addr::parse("192.168.1.100")?;
    /// assert_eq!(ip, Ipv4Addr::new(192, 168, 1, 100));
    /// ```
    pub fn parse(s: &str) -> Result<Self, &'static str> {
        let mut octets = [0u8; 4];
        let mut idx = 0;

        for part in s.split('.') {
            if idx >= 4 {
                return Err("Too many octets in IPv4 address");
            }
            // Trim whitespace and parse as u8; reject non-numeric or out-of-range values.
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
    /// Formats the IPv4 address in standard **dotted-decimal notation**
    /// (e.g., `"10.0.2.15"`), suitable for display in log messages and the shell.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0[0], self.0[1], self.0[2], self.0[3])
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// IP Protocol Number
// ──────────────────────────────────────────────────────────────────────────────

/// **Supported IPv4 Next-Level Protocol numbers** (from the IANA protocol
/// number registry, also defined in RFC 790 and successors).
///
/// Stored as a `u8` in the IPv4 header's Protocol field (byte offset 9).  The
/// value identifies which transport-layer (or control) protocol is carried
/// inside the IPv4 payload.
///
/// ## Known Values
///
/// | Decimal | Name | RFC  |
/// |---------|------|------|
/// | 1       | ICMP | 792  |
/// | 6       | TCP  | 793  |
/// | 17      | UDP  | 768  |
///
/// Any other value maps to `Unknown(u8)` and causes the packet to be silently
/// discarded at the IPv4 dispatch step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IpProtocol {
    /// **Internet Control Message Protocol** — protocol number `1`.
    ///
    /// Used for diagnostics (`ping`) and error signalling (Destination
    /// Unreachable, Time Exceeded, Redirect, etc.).
    Icmp = 1,

    /// **Transmission Control Protocol** — protocol number `6`.
    ///
    /// Connection-oriented, reliable, ordered byte-stream transport.
    Tcp  = 6,

    /// **User Datagram Protocol** — protocol number `17`.
    ///
    /// Connectionless, unreliable datagram transport with minimal overhead.
    Udp  = 17,

    /// Any protocol number not explicitly recognized by NullOS.
    ///
    /// Such packets are silently dropped by [`handle_packet`].
    Unknown(u8),
}

impl IpProtocol {
    /// Converts a raw `u8` protocol field value into the [`IpProtocol`] enum.
    ///
    /// # Arguments
    ///
    /// * `val` – The 1-byte protocol number read from the IPv4 header.
    ///
    /// # Returns
    ///
    /// The matching [`IpProtocol`] variant, or `Unknown(val)` for unrecognized values.
    pub const fn from_u8(val: u8) -> Self {
        match val {
            1  => IpProtocol::Icmp,
            6  => IpProtocol::Tcp,
            17 => IpProtocol::Udp,
            other => IpProtocol::Unknown(other),
        }
    }

    /// Converts this [`IpProtocol`] variant to its raw `u8` wire value.
    ///
    /// Used when constructing IPv4 headers and UDP/TCP pseudo-headers.
    ///
    /// # Returns
    ///
    /// The 1-byte protocol number for use in the IPv4 Protocol field.
    pub const fn to_u8(self) -> u8 {
        match self {
            IpProtocol::Icmp       => 1,
            IpProtocol::Tcp        => 6,
            IpProtocol::Udp        => 17,
            IpProtocol::Unknown(val) => val,
        }
    }
}

impl fmt::Display for IpProtocol {
    /// Formats the protocol for human-readable display.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IpProtocol::Icmp       => write!(f, "ICMP (1)"),
            IpProtocol::Tcp        => write!(f, "TCP (6)"),
            IpProtocol::Udp        => write!(f, "UDP (17)"),
            IpProtocol::Unknown(p) => write!(f, "Protocol({})", p),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// IPv4 Header
// ──────────────────────────────────────────────────────────────────────────────

/// **Standard 20-byte IPv4 Header** (IHL = 5, no options).
///
/// All multi-byte fields are stored in **network byte order** (big-endian).
/// Use the accessor methods ([`src_ip`], [`dst_ip`], etc.) which perform the
/// byte-swap to host order automatically.
///
/// ## Safety Note
///
/// `#[repr(C, packed)]` removes all padding bytes, making the struct exactly
/// 20 bytes.  Direct field access requires care because misaligned reads are
/// undefined behavior in Rust.  Always use accessor methods or
/// `core::ptr::read_unaligned` when casting from raw byte buffers.
///
/// ## Field Layout (wire order, all big-endian multi-byte fields)
///
/// ```text
/// Offset  Bytes  Field
///  0       1     version_ihl   — Version (4) in bits 7-4, IHL in bits 3-0
///  1       1     tos           — Type of Service / DSCP+ECN
///  2       2     total_len     — Total length (header + payload)
///  4       2     identification — Packet ID for fragment reassembly
///  6       2     flags_fragment — Flags (3 bits) + Fragment Offset (13 bits)
///  8       1     ttl           — Time to Live (hop limit)
///  9       1     protocol      — Next-layer protocol number
/// 10       2     checksum      — RFC 1071 header checksum
/// 12       4     src           — Source IPv4 address
/// 16       4     dst           — Destination IPv4 address
///                              Total: 20 bytes
/// ```
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Ipv4Header {
    /// **Version + IHL** packed into one byte.
    ///
    /// * Bits 7–4: IP version.  Always `4` for IPv4.
    /// * Bits 3–0: Internet Header Length in 32-bit words.  `5` = 20 bytes.
    ///
    /// Combined value for a standard 20-byte IPv4-4 header: `0x45`.
    pub version_ihl: u8,    // High 4 bits: Version (4), Low 4 bits: Header length in 32-bit words (5 = 20 bytes)

    /// **Type of Service / DSCP + ECN**.
    ///
    /// Originally defined as a "precedence + TOS" byte (RFC 791); redefined by
    /// RFC 2474 as DSCP (6 bits) + ECN (2 bits).  NullOS sets this to `0`
    /// (best-effort, no ECN).
    pub tos: u8,            // Type of Service / DSCP

    /// **Total length** of the IP datagram (header + payload) in bytes.
    /// Stored in network byte order.  Maximum value: 65535.  Minimum: 20
    /// (header only, no payload).  NullOS enforces the 1500-byte Ethernet MTU
    /// at a higher level (the VirtIO driver rejects larger sends).
    pub total_len: u16,      // Total packet length (header + data) in bytes

    /// **Identification** — a 16-bit value used to group fragments belonging
    /// to the same original datagram.  Fragments from the same datagram share
    /// the same ID; the receiver uses (src, dst, proto, ID) as a 4-tuple key
    /// to reassemble them.  NullOS sets this to a fixed constant (`0x1234`)
    /// because it does not fragment packets (DF is always set).
    pub identification: u16, // Packet ID

    /// **Flags (3 bits) + Fragment Offset (13 bits)**, packed big-endian.
    ///
    /// ```text
    /// Bit 15: Reserved (must be 0)
    /// Bit 14: DF (Don't Fragment) — NullOS always sets this bit.
    /// Bit 13: MF (More Fragments) — Set on all fragments except the last.
    /// Bits 12–0: Fragment Offset in units of 8 bytes.
    /// ```
    ///
    /// NullOS sends `0x4000` (DF=1, MF=0, offset=0) in network byte order,
    /// meaning "don't fragment this packet, it is not a fragment."
    pub flags_fragment: u16, // 3-bit flags (DF, MF) + 13-bit fragment offset

    /// **Time to Live** — the maximum number of router hops this packet may
    /// traverse.  Each router decrements TTL by 1; if it reaches 0, the router
    /// drops the packet and sends an ICMP Type 11 (Time Exceeded) back to the
    /// source.  This prevents routing loops from consuming network resources
    /// indefinitely.  NullOS uses the standard default of **64**.
    pub ttl: u8,            // Time to Live (hop count, default 64)

    /// **Protocol number** identifying the transport-layer payload.
    /// See [`IpProtocol`] for the enum representation.
    /// * `1`  → ICMP
    /// * `6`  → TCP
    /// * `17` → UDP
    pub protocol: u8,       // Transport protocol (ICMP 1, TCP 6, UDP 17)

    /// **Header checksum** — RFC 1071 one's complement 16-bit sum of the header.
    /// Stored in network byte order.  Computed with the checksum field zeroed,
    /// then the result is placed here.  Receivers verify by running
    /// `compute_checksum(header)` and checking for `0`.
    pub checksum: u16,       // 16-bit one's complement header checksum

    /// **Source IPv4 address** in network byte order (4 bytes, MSB first).
    /// Set to the local interface IP address for outgoing packets.
    pub src: [u8; 4],       // Source IPv4 address

    /// **Destination IPv4 address** in network byte order (4 bytes, MSB first).
    /// Used by routers to forward the packet toward the ultimate destination.
    pub dst: [u8; 4],       // Destination IPv4 address
}

impl Ipv4Header {
    /// Constructs a new **20-byte IPv4 header** with the checksum pre-computed.
    ///
    /// Sets the following fields to standard values:
    /// * `version_ihl = 0x45` — IPv4, no options, 20-byte header.
    /// * `tos = 0`             — Best-effort, no DSCP/ECN marking.
    /// * `identification = 0x1234` — Fixed ID (no fragmentation).
    /// * `flags_fragment = 0x4000` — DF=1, MF=0, offset=0 (never fragment).
    /// * `ttl = 64`            — Standard default hop limit per RFC 1122.
    /// * `checksum`           — Computed by [`compute_checksum`] over the header.
    ///
    /// # Arguments
    ///
    /// * `src`         – Source IPv4 address (local interface).
    /// * `dst`         – Destination IPv4 address.
    /// * `protocol`    – Transport-layer protocol ([`IpProtocol`]).
    /// * `payload_len` – Length of the payload in bytes.
    ///
    /// # Returns
    ///
    /// A fully initialized [`Ipv4Header`] with a valid checksum, ready to be
    /// serialized into a transmit buffer.
    pub fn new(src: Ipv4Addr, dst: Ipv4Addr, protocol: IpProtocol, payload_len: usize) -> Self {
        // Total length = header (20) + payload.
        let total_len = (IPV4_HDR_MIN_LEN + payload_len) as u16;
        let mut hdr = Self {
            version_ihl:    0x45,                       // Version 4, IHL 5 (5 * 4 = 20 bytes)
            tos:            0,                           // Best-effort, no QoS
            total_len:      total_len.to_be(),           // Network byte order
            identification: 0x1234u16.to_be(),           // Fixed ID; DF=1 so no fragmentation
            flags_fragment: 0x4000u16.to_be(),           // Don't Fragment (DF) flag set
            ttl:            64,                          // Standard TTL (Linux/BSD default)
            protocol:       protocol.to_u8(),
            checksum:       0,                           // Will be filled in below
            src:            src.octets(),
            dst:            dst.octets(),
        };

        // Compute Internet Checksum over the 20-byte header.
        // The checksum field must be zero during the computation (RFC 791 §3.1).
        // SAFETY: &hdr is valid for IPV4_HDR_MIN_LEN bytes; repr(C, packed) → no padding.
        let hdr_bytes = unsafe {
            core::slice::from_raw_parts(&hdr as *const _ as *const u8, IPV4_HDR_MIN_LEN)
        };
        // Store checksum in network byte order.
        hdr.checksum = compute_checksum(hdr_bytes).to_be();
        hdr
    }

    /// Returns the **source IP address** in host-readable [`Ipv4Addr`] form.
    pub fn src_ip(&self) -> Ipv4Addr {
        Ipv4Addr::from_octets(self.src)
    }

    /// Returns the **destination IP address** in host-readable [`Ipv4Addr`] form.
    pub fn dst_ip(&self) -> Ipv4Addr {
        Ipv4Addr::from_octets(self.dst)
    }

    /// Returns the **protocol** field as an [`IpProtocol`] enum variant.
    ///
    /// Uses [`IpProtocol::from_u8`] to convert the raw byte value.  Unrecognized
    /// protocol numbers become `IpProtocol::Unknown(n)`.
    pub fn protocol(&self) -> IpProtocol {
        IpProtocol::from_u8(self.protocol)
    }

    /// Returns the **total packet length** (header + payload) in bytes, converted
    /// from network byte order to host order.
    ///
    /// This value must be used to determine where the payload ends when the
    /// received Ethernet frame may be padded to the minimum 60-byte frame size.
    pub fn total_length(&self) -> usize {
        u16::from_be(self.total_len) as usize
    }

    /// Returns the **header length** in bytes, derived from the IHL field.
    ///
    /// IHL is the number of 32-bit words in the header.  `IHL × 4 = bytes`.
    /// For a standard 20-byte header, IHL = 5.  For headers with options,
    /// IHL can be up to 15 (60 bytes).
    ///
    /// Used to locate the start of the payload:
    /// `payload = &packet[header_length()..total_length()]`
    pub fn header_length(&self) -> usize {
        // Extract the lower 4 bits of version_ihl and multiply by 4.
        ((self.version_ihl & 0x0F) * 4) as usize
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// RFC 1071 Internet Checksum
// ──────────────────────────────────────────────────────────────────────────────

/// Computes the **RFC 1071 standard 16-bit one's complement Internet Checksum**.
///
/// This algorithm is used for IPv4 header checksums, ICMP checksums, and the
/// pseudo-header component of TCP/UDP checksums.
///
/// ## Algorithm
///
/// 1. Sum consecutive big-endian 16-bit words using 32-bit accumulation.
/// 2. If the data has an odd number of bytes, append a zero byte (pad the last
///    byte to a 16-bit word with zero in the low byte position).
/// 3. Fold the 32-bit sum: add the high 16 bits (carry) into the low 16 bits,
///    repeating until no carry remains.
/// 4. Take the one's complement (bitwise NOT) of the 16-bit result.
///
/// ## Verification Property
///
/// If the checksum field is included in the input data (i.e., the data contains
/// the checksum as it appears on the wire), and the data is not corrupt, then
/// `compute_checksum(data) == 0x0000`.  This is because adding the one's
/// complement of the sum to the sum yields all-ones (0xFFFF in one's complement
/// == 0x0000 in two's complement after the final NOT).
///
/// ## Example
///
/// For the byte sequence `[0x45, 0x00, ...]` (IPv4 header), the function
/// returns a 16-bit checksum in **host byte order**.  The caller must call
/// `.to_be()` before storing it in a network packet.
///
/// # Arguments
///
/// * `data` – The byte slice to checksum.  For IPv4 headers this is exactly
///   20 bytes; for ICMP it is the ICMP header + data; for TCP/UDP it includes
///   the pseudo-header.
///
/// # Returns
///
/// The 16-bit one's complement checksum in **host byte order**.
pub fn compute_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0; // 32-bit accumulator to hold carry bits during summation.
    let mut i = 0;        // Byte index into the data slice.

    // Process 2 bytes at a time (big-endian 16-bit words).
    while i + 1 < data.len() {
        // Combine two consecutive bytes into a big-endian 16-bit word.
        let word = u16::from_be_bytes([data[i], data[i + 1]]);
        sum += word as u32; // Add to accumulator; overflow goes into bits 16-31.
        i += 2;
    }

    // Handle the trailing odd byte by padding with a zero byte (high byte = data, low byte = 0).
    if i < data.len() {
        // Shift the last byte into the high byte position of a 16-bit word.
        sum += (data[i] as u32) << 8;
    }

    // Fold the 32-bit sum: add carry (bits 16-31) into the lower 16 bits.
    // Repeat until no carry remains (at most 2 iterations needed).
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    // One's complement: flip all bits in the 16-bit result.
    // The result is in host byte order; callers must call .to_be() for wire format.
    !(sum as u16)
}

// ──────────────────────────────────────────────────────────────────────────────
// IPv4 Packet TX / RX
// ──────────────────────────────────────────────────────────────────────────────

/// **Encapsulates payload in an IPv4 packet**, resolves the destination MAC via
/// ARP, and transmits the resulting Ethernet frame over the VirtIO NIC.
///
/// ## Routing Decision
///
/// A simple **direct vs. gateway** routing decision is made:
/// * If `dst_ip` is on the same `/24` subnet as the local IP (determined via
///   `in_same_subnet` using the configured netmask), ARP for `dst_ip` directly.
/// * Otherwise, ARP for the **default gateway** IP instead.  The gateway will
///   then forward the packet toward the destination.
///
/// This two-entry routing table (direct route + default route) is sufficient
/// for the QEMU SLIRP setup and simple home networks.
///
/// ## Frame Assembly
///
/// 1. Build a 20-byte [`Ipv4Header`] with the checksum pre-computed.
/// 2. Append the payload bytes.
/// 3. Pass the resulting IPv4 packet to [`ethernet::send_frame`] for Ethernet
///    encapsulation and VirtIO transmission.
///
/// # Arguments
///
/// * `dst_ip`   – Destination IPv4 address.
/// * `protocol` – The transport-layer protocol of the payload.
/// * `payload`  – The serialized transport-layer segment (TCP, UDP, or ICMP data).
///
/// # Returns
///
/// * `Ok(())` – The packet was successfully placed into the VirtIO TX ring.
/// * `Err(&str)` – ARP resolution failed or the VirtIO send failed.
pub fn send_ipv4_packet(dst_ip: Ipv4Addr, protocol: IpProtocol, payload: &[u8]) -> Result<(), &'static str> {
    let config = *super::INTERFACE_CONFIG.lock();
    let src_ip  = config.ip;

    // Short-circuit local loopback destinations (127.0.0.0/8 or self IP) directly to the
    // local transport protocol handler instead of transmitting out the NIC toward the gateway.
    if dst_ip.is_loopback() || dst_ip == src_ip {
        let loopback_src = if dst_ip.is_loopback() { dst_ip } else { src_ip };
        match protocol {
            IpProtocol::Udp => super::udp::handle_packet(loopback_src, payload),
            IpProtocol::Tcp => super::tcp::handle_packet(loopback_src, payload),
            _ => {}
        }
        return Ok(());
    }

    // Determine target IP for ARP resolution (direct host if on same subnet, or gateway router)
    // If the destination is on the same subnet, ARP for it directly.
    // If the destination is on a different network, ARP for the gateway.
    let next_hop = if dst_ip.in_same_subnet(&src_ip, &config.netmask) {
        dst_ip            // On-link: resolve destination directly.
    } else {
        config.gateway    // Off-link: resolve the default gateway.
    };

    // Resolve the next-hop IP to a MAC address (cache hit or ARP request/reply).
    let target_mac = match super::arp::resolve_mac(next_hop) {
        Some(mac) => mac,
        None      => return Err("Unable to resolve destination MAC address via ARP"),
    };

    // Build the 20-byte IPv4 header with correct checksum.
    let header = Ipv4Header::new(src_ip, dst_ip, protocol, payload.len());
    let mut packet = Vec::with_capacity(IPV4_HDR_MIN_LEN + payload.len());

    // Serialize the header struct as raw bytes.
    // SAFETY: Ipv4Header is #[repr(C, packed)], 20 bytes, no padding.
    let hdr_bytes = unsafe {
        core::slice::from_raw_parts(&header as *const _ as *const u8, IPV4_HDR_MIN_LEN)
    };
    packet.extend_from_slice(hdr_bytes);  // Append the 20-byte header.
    packet.extend_from_slice(payload);    // Append the transport-layer payload.

    // Hand the assembled IPv4 packet to the Ethernet layer for frame encapsulation.
    ethernet::send_frame(target_mac, EtherType::Ipv4, &packet)
}

/// **Entry point for received IPv4 packets** from Ethernet frames.
///
/// Called by [`super::ethernet::handle_packet`] when the EtherType is `0x0800`.
///
/// ## Validation Steps
///
/// 1. **Minimum length**: payload must be ≥ 20 bytes (minimum header).
/// 2. **Version check**: the high nibble of `version_ihl` must be `4`.
/// 3. **Length consistency**: `total_length` must be ≤ payload buffer length,
///    and IHL must be ≤ total_length.
/// 4. **Header checksum**: `compute_checksum(header_bytes)` must equal `0`
///    (the RFC 1071 verification property).
/// 5. **Destination filter**: only accept packets destined for our IP or
///    the limited broadcast `255.255.255.255`.
///
/// ## Dispatch
///
/// After validation, the payload (starting at `hdr_len` within `frame.payload`)
/// is dispatched to the appropriate upper-layer handler:
/// * `ICMP (1)` → [`super::icmp::handle_packet`]
/// * `UDP (17)` → [`super::udp::handle_packet`]
/// * `TCP (6)`  → [`super::tcp::handle_packet`]
/// * Others → silently discarded.
///
/// # Arguments
///
/// * `frame` – The Ethernet frame carrying the IPv4 packet in its payload.
pub fn handle_packet(frame: &EthernetFrame) {
    // Guard: Ethernet payload must contain at least a minimal IPv4 header.
    if frame.payload.len() < IPV4_HDR_MIN_LEN {
        return;
    }

    // Parse the IPv4 header from the Ethernet payload.
    // SAFETY: frame.payload.len() >= 20 (checked above); read_unaligned handles alignment.
    let header = unsafe {
        core::ptr::read_unaligned(frame.payload.as_ptr() as *const Ipv4Header)
    };

    // Verify IPv4 version
    // The high nibble of version_ihl must be 4.  Any other value is not IPv4.
    if (header.version_ihl >> 4) != 4 {
        return; // Not an IPv4 packet; discard.
    }

    let hdr_len   = header.header_length();
    let total_len = header.total_length();

    // Verify consistency: total_len cannot exceed the received buffer, and the
    // header cannot be larger than the total packet.
    if frame.payload.len() < total_len || total_len < hdr_len {
        return; // Truncated or malformed packet.
    }

    // Verify checksum
    // The RFC 1071 property: sum over header bytes (including the checksum field)
    // must equal 0x0000 for a valid, uncorrupted header.
    let hdr_slice = &frame.payload[..hdr_len];
    if compute_checksum(hdr_slice) != 0 {
        return; // Checksum mismatch, drop corrupt packet
    }

    // Extract the transport-layer payload (after the IP header, up to total_len).
    // Bytes beyond total_len are Ethernet padding and must not be passed upward.
    let payload = &frame.payload[hdr_len..total_len];
    let src_ip  = header.src_ip();
    let dst_ip  = header.dst_ip();

    // Drop packets not addressed to our IP or the limited broadcast.
    let local_ip = super::local_ip();
    if dst_ip != local_ip && !dst_ip.is_broadcast() {
        return; // Drop packet not addressed to our IP
    }

    // Dispatch to transport layer based on the Protocol field.
    match header.protocol() {
        IpProtocol::Icmp => {
            // ICMP: ping replies, error messages.
            super::icmp::handle_packet(src_ip, payload);
        }
        IpProtocol::Udp => {
            // UDP: connectionless datagrams.
            super::udp::handle_packet(src_ip, payload);
        }
        IpProtocol::Tcp => {
            // TCP: connection-oriented streams.
            super::tcp::handle_packet(src_ip, payload);
        }
        IpProtocol::Unknown(_) => {
            // Unknown protocol number — silently discard.
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Self-Tests
// ──────────────────────────────────────────────────────────────────────────────

/// **Runs automated verification self-tests** for IPv4 serialization, parsing,
/// and checksumming.
///
/// ## Tests performed:
///
/// 1. Build an IPv4 header for a UDP packet and verify the RFC 1071 checksum
///    property (checksum over header including the checksum field equals 0).
/// 2. Verify that the accessor methods return the correct field values.
/// 3. Parse an IPv4 address from a dotted-decimal string and compare.
///
/// Panics on assertion failure, halting the kernel with a diagnostic.
pub fn run_self_tests() {
    let src     = Ipv4Addr::new(10, 0, 2, 15);
    let dst     = Ipv4Addr::new(10, 0, 2, 2);
    let payload = b"IPv4 packet payload test!";

    // Build a header for a UDP packet; this internally computes and embeds the checksum.
    let hdr = Ipv4Header::new(src, dst, IpProtocol::Udp, payload.len());

    // Serialize the header back to bytes for checksum verification.
    // SAFETY: Ipv4Header is repr(C, packed), 20 bytes, no padding.
    let hdr_bytes = unsafe {
        core::slice::from_raw_parts(&hdr as *const _ as *const u8, IPV4_HDR_MIN_LEN)
    };

    // RFC 1071 property: Checksum of header including checksum field must evaluate to 0
    // If the header is not corrupted and the checksum was correctly computed,
    // running the checksum algorithm again over the entire header (including the
    // checksum field itself) must return 0x0000.
    assert_eq!(compute_checksum(hdr_bytes), 0, "IPv4 checksum calculation failed");

    // Verify field accessors return the correct values.
    assert_eq!(hdr.src_ip(),       src,                                    "Source IP mismatch");
    assert_eq!(hdr.dst_ip(),       dst,                                    "Destination IP mismatch");
    assert_eq!(hdr.protocol(),     IpProtocol::Udp,                        "Protocol mismatch");
    assert_eq!(hdr.total_length(), IPV4_HDR_MIN_LEN + payload.len(),       "Total length mismatch");

    // Test IP string parsing: parse a dotted-decimal string and compare.
    let parsed_ip = Ipv4Addr::parse("192.168.1.100").expect("Failed to parse valid IPv4 string");
    assert_eq!(parsed_ip, Ipv4Addr::new(192, 168, 1, 100), "Parsed IP mismatch");

    println!("[IPV4] IPv4 Header, RFC 1071 Checksum & Address parsing verified successfully!");
}
