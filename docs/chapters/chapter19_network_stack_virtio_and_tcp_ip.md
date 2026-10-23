# Chapter 19: The Network Stack — VirtIO-Net, Ethernet, ARP, IPv4, ICMP, UDP, TCP & Sockets

> *"The Internet is not a physical object; it is an agreed-upon set of conversational protocols. Operating systems are the polyglots that give raw electrical voltages semantic meaning across the globe."*

---

## 19.1 Introduction: From Isolated Silicon to the Global Internet

In the preceding eighteen chapters, we transformed cold silicon and motherboard capacitors into a fully self-hosting, multitasking computing environment:
- In **Phases 1–4**, we initialized the CPU, configured GDT/TSS segmentation, established preemptive APIC scheduling, mapped 4-level paging, and managed physical memory.
- In **Phases 5–7**, we crossed the privilege boundary into Ring 3 user mode, implemented fast `SYSCALL`/`SYSRET` transitions, constructed a Virtual File System with persistent FAT32 block storage, implemented a C Standard Library (`libc`), and created an interactive Unix shell (`nsh`) capable of executing user ELF binaries with pipelines and redirection.

Yet, despite all this sophistication, our system remained an **isolated island**. It had no mechanism to exchange information with other machines.

**Phase 8: Networking** bridges this gap. In Milestone 23, we implemented the low-level **VirtIO-Net Paravirtualized Network Driver**, giving NullOS direct packet I/O access to the host machine's virtual network card. In Milestone 24, we constructed the **TCP/IP Network Stack from scratch in `#![no_std]` Rust**, implementing every layer of the standard networking model:
1. **Layer 2 (Data Link)**: Ethernet II framing, MAC addressing, MTU enforcement, and 60-byte padding.
2. **Layer 3 (Network Resolution)**: Address Resolution Protocol (ARP) with dynamic cache tracking.
3. **Layer 3 (Internet Protocol)**: IPv4 packet formatting, RFC 1071 Internet checksumming, subnet mask evaluation, and gateway routing.
4. **Layer 3 (Diagnostics)**: Internet Control Message Protocol (ICMP) Echo Request/Reply responder and the `ping` diagnostic utility.
5. **Layer 4 (Transport - Datagram)**: User Datagram Protocol (UDP) with IPv4 pseudo-header checksumming and port demultiplexing.
6. **Layer 4 (Transport - Stream)**: Transmission Control Protocol (TCP) with segment header framing, 11-state finite state machine, and 3-way handshake coordination.
7. **Layer 5 (Application/API)**: Berkeley Sockets abstraction (`socket`, `bind`, `listen`, `accept`, `connect`, `send`, `recv`, `close`).
8. **Shell Diagnostics**: Unix diagnostic CLI commands (`ifconfig`, `arp`, `netstat`, `ping`).

```
 +-----------------------------------------------------------------------+
 |                 Application Layer / Shell Utilities                   |
 |                 (ifconfig, arp, ping, netstat, nsh)                   |
 +-----------------------------------------------------------------------+
                                    |
                                    v
 +-----------------------------------------------------------------------+
 |                 Berkeley Sockets API (`Socket` Trait)                 |
 |            socket(), bind(), listen(), accept(), connect(),           |
 |                         send(), recv(), close()                       |
 +-----------------------------------------------------------------------+
                     |                               |
                     v                               v
    +---------------------------------+  +-------------------------------+
    |   UDP (User Datagram Protocol)  |  | TCP (Transmission Control)    |
    |   - Connectionless Datagrams    |  | - 11-State Finite Machine     |
    |   - 8-Byte Header + Pseudo-CS   |  | - 20-Byte Header + Pseudo-CS  |
    |   - Port Demultiplexing         |  | - 3-Way Handshake (SYN/ACK)   |
    +---------------------------------+  +-------------------------------+
                     \                               /
                      \                             /
                       v                           v
 +-----------------------------------------------------------------------+
 |                IPv4 (Internet Protocol Version 4)                     |
 | - 20-Byte Header (RFC 791)           - Subnet Masking & Local Route   |
 | - RFC 1071 One's Complement Checksum - Default Gateway (10.0.2.2)     |
 +-----------------------------------------------------------------------+
            |                                           ^
            | (IP to MAC Lookup)                        | (Echo Reply)
            v                                           |
 +---------------------------------+  +----------------------------------+
 |  ARP (Address Resolution Proto) |  |   ICMP (Control Message Proto)   |
 |  - 28-Byte Frame (RFC 826)      |  |   - Echo Request (Type 8)        |
 |  - BTreeMap Dynamic MAC Cache   |  |   - Echo Reply (Type 0)          |
 |  - Broadcast Query & Auto-Reply |  |   - RTT Timing Measurement       |
 +---------------------------------+  +----------------------------------+
            \                                           /
             \                                         /
              v                                       v
 +-----------------------------------------------------------------------+
 |                      Ethernet II (Layer 2 Framing)                    |
 | - 14-Byte Header (Dest MAC [6], Src MAC [6], EtherType [2])           |
 | - Minimum Payload Padding (60-byte Ethernet frame standard)           |
 +-----------------------------------------------------------------------+
                                    |
                                    v
 +-----------------------------------------------------------------------+
 |               VirtIO-Net Network Driver (Paravirtualized)             |
 | - PCI Discovery (0x1AF4:0x1000 / 0x1041)                              |
 | - Split Virtqueues (Queue 0 RX Ring [32 bufs], Queue 1 TX Ring)       |
 | - 10-Byte `VirtIoNetHdr` + DMA Buffer Memory Management               |
 +-----------------------------------------------------------------------+
                                    |
                                    v
 +-----------------------------------------------------------------------+
 |               QEMU Virtual SLIRP Network / Physical Wire              |
 +-----------------------------------------------------------------------+
```

---

## 19.2 The Physical & Data Link Layer: VirtIO-Net Driver

### 19.2.1 PCI Device Discovery and MAC Address Extraction

The VirtIO Network device adheres to the paravirtualized I/O specification originally developed by Rusty Russell. When QEMU is launched with:
```bash
-netdev user,id=net0 -device virtio-net-pci,netdev=net0,mac=52:54:00:12:34:56
```
the kernel's PCI subsystem discovers a device with:
- **Vendor ID**: `0x1AF4` (Red Hat / VirtIO)
- **Device ID**: `0x1000` (Legacy VirtIO Network) or `0x1041` (Modern VirtIO-1.0 Network)
- **BAR0**: Legacy I/O port base address (e.g., `0x60E0`)

Under the legacy VirtIO-PCI specification, device-specific configuration space starts at offset `0x14` (20 bytes) beyond the standard VirtIO header. For a network card, bytes `0x14` through `0x19` contain the 6-byte factory hardware MAC address:

```rust
// kernel/src/drivers/virtio_net.rs
let mut mac = [0u8; 6];
for i in 0..6 {
    mac[i] = unsafe { x86_64::instructions::port::PortReadOnly::<u8>::new(io_base + 0x14 + i).read() };
}
```

### 19.2.2 Dual Split Virtqueues: Queue 0 (RX) and Queue 1 (TX)

VirtIO relies on **Split Virtqueues**—shared memory ring buffers partitioned into three regions:
1. **Descriptor Table**: Array of 16-byte descriptors containing 64-bit physical guest addresses (`addr`), length (`len`), flags (`flags`), and chaining links (`next`).
2. **Available Ring**: Driver-to-device circular ring where the kernel places descriptor indices ready for hardware processing.
3. **Used Ring**: Device-to-driver circular ring where the virtual hardware records completed descriptor indices and bytes transferred.

VirtIO-Net mandates two separate virtqueues:
- **Queue 0 (Receive Ring)**: Pre-populated by the driver with 32 free buffer descriptors. Whenever an Ethernet frame arrives from the network, the hardware writes the packet into one of these buffers, increments the Used Ring index, and triggers an interrupt.
- **Queue 1 (Transmit Ring)**: Used on-demand by the driver when the kernel wishes to transmit an outbound packet.

### 19.2.3 The 10-Byte `VirtIoNetHdr`

Unlike raw physical Ethernet controllers, VirtIO-Net prepends a mandatory 10-byte header to every transmitted and received buffer:

```rust
#[repr(C, packed)]
pub struct VirtIoNetHdr {
    pub flags: u8,
    pub gso_type: u8,
    pub hdr_len: u16,
    pub gso_size: u16,
    pub csum_start: u16,
    pub csum_offset: u16,
}
```

When transmitting a frame, the driver zeroes this 10-byte header, appends the raw Ethernet payload, writes the physical buffer address into the TX Descriptor Table, updates the Available Ring, and kicks the device via the `QueueNotify` I/O port (`io_base + 0x10`).

---

## 19.3 Layer 2: Ethernet II Framing

At Layer 2, all network communication occurs via **Ethernet II frames** (IEEE 802.3). An Ethernet II header is exactly 14 bytes:

```rust
// kernel/src/net/ethernet.rs
#[repr(C, packed)]
pub struct EthernetHeader {
    pub dst_mac: [u8; 6], // Destination Hardware MAC
    pub src_mac: [u8; 6], // Source Hardware MAC
    pub ethertype: u16,   // Big-Endian Next-Level Protocol
}
```

### 19.3.1 EtherTypes
The 16-bit `ethertype` field identifies the payload protocol:
- `0x0806`: **ARP** (Address Resolution Protocol)
- `0x0800`: **IPv4** (Internet Protocol Version 4)
- `0x86DD`: **IPv6** (Internet Protocol Version 6)

### 19.3.2 Minimum Frame Size and Padding
The Ethernet standard requires that the minimum size of an Ethernet frame on the wire is **64 bytes** (including the 4-byte CRC Frame Check Sequence appended by hardware). Excluding the CRC, the driver must ensure that any transmitted frame has an absolute minimum length of **60 bytes** (14 bytes header + 46 bytes payload). If an outbound ARP or ICMP packet is smaller than 46 bytes, it is padded with zeroes up to 60 bytes:

```rust
if packet.len() < ETH_MIN_PACKET_LEN {
    packet.resize(ETH_MIN_PACKET_LEN, 0);
}
```

---

## 19.4 Layer 3: Address Resolution Protocol (ARP)

### 19.4.1 The IP-to-MAC Disconnect
When an operating system wants to send an IPv4 packet to IP address `10.0.2.2`, the network interface card cannot accept an IP address—it requires a 48-bit physical MAC address (e.g., `52:54:00:12:34:02`). The **Address Resolution Protocol (RFC 826)** bridges this gap.

```
       Host (10.0.2.15)                          Gateway Router (10.0.2.2)
              |                                              |
              | --- ARP Request (Broadcast FF:FF:FF:FF) ---> |
              |     "Who has 10.0.2.2? Tell 10.0.2.15"       |
              |                                              |
              | <--- ARP Reply (Unicast 52:54:00:12:34:56) - |
              |      "10.0.2.2 is at 52:54:00:12:34:02"      |
```

### 19.4.2 ARP Packet Layout (28 Bytes)
```rust
#[repr(C, packed)]
pub struct ArpPacket {
    pub hw_type: u16,        // Hardware Type (1 = Ethernet)
    pub proto_type: u16,     // Protocol Type (0x0800 = IPv4)
    pub hw_size: u8,         // Hardware Address Length (6)
    pub proto_size: u8,      // Protocol Address Length (4)
    pub opcode: u16,         // Opcode (1 = Request, 2 = Reply)
    pub sender_mac: [u8; 6], // Sender Hardware Address
    pub sender_ip: [u8; 4],  // Sender Protocol Address
    pub target_mac: [u8; 6], // Target Hardware Address
    pub target_ip: [u8; 4],  // Target Protocol Address
}
```

### 19.4.3 Dynamic ARP Cache Table
NullOS maintains a dynamic in-memory ARP cache:
```rust
static ARP_TABLE: Spinlock<BTreeMap<Ipv4Addr, MacAddress>> = Spinlock::new(BTreeMap::new());
```
When an ARP Reply arrives, the kernel records the mapping. If an outbound packet is addressed to an unknown IP, the kernel broadcasts an ARP Request (`FF:FF:FF:FF:FF:FF`), waits for the reply, and caches the result.

---

## 19.5 Layer 3: Internet Protocol Version 4 (IPv4)

IPv4 (RFC 791) provides host-to-host packet routing across heterogeneous networks.

### 19.5.1 The 20-Byte IPv4 Header
```rust
// kernel/src/net/ipv4.rs
#[repr(C, packed)]
pub struct Ipv4Header {
    pub version_ihl: u8,    // High 4 bits: Version (4), Low 4 bits: IHL (5 = 20 bytes)
    pub tos: u8,            // Type of Service
    pub total_len: u16,      // Total packet length (Big-Endian)
    pub identification: u16, // Packet ID
    pub flags_fragment: u16, // Flags (DF) + Fragment Offset
    pub ttl: u8,            // Time to Live (64)
    pub protocol: u8,       // Next Protocol (ICMP=1, TCP=6, UDP=17)
    pub checksum: u16,       // RFC 1071 Header Checksum (Big-Endian)
    pub src: [u8; 4],       // Source IP
    pub dst: [u8; 4],       // Destination IP
}
```

### 19.5.2 The RFC 1071 Internet Checksum Algorithm
To detect bit-rot and corruption during transit, IPv4 computes a 16-bit one's complement sum of all 16-bit words in the header:
1. Treat the header as an array of 16-bit big-endian words.
2. Accumulate the words into a 32-bit integer sum.
3. Fold the 32-bit sum into 16 bits by repeatedly adding the carry (`sum >> 16`) to the low 16 bits (`sum & 0xFFFF`).
4. Take the one's complement bitwise NOT (`!sum`).

```rust
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
```

> **Crucial Implementation Insight**: Because x86_64 is little-endian, storing the computed checksum into a packed network struct requires `.to_be()` byte-swapping:
> ```rust
> hdr.checksum = compute_checksum(hdr_bytes).to_be();
> ```
> This guarantees that when a receiving host reads the header bytes in network byte order, the checksum of the entire 20 bytes (including the checksum field) evaluates to exactly `0x0000`.

### 19.5.3 Subnet Routing and Gateway Traversal
Before sending an IPv4 packet, the kernel checks whether the target IP resides on the local subnet:
```rust
let next_hop = if dst_ip.in_same_subnet(&src_ip, &config.netmask) {
    dst_ip
} else {
    config.gateway
};
```
If the destination is outside the local subnet (e.g., `8.8.8.8`), the packet is routed to the default gateway router (`10.0.2.2`).

---

## 19.6 Layer 3: ICMP & The Ping Utility

**Internet Control Message Protocol (RFC 792)** provides diagnostic and error reporting capabilities.

```rust
#[repr(C, packed)]
pub struct IcmpHeader {
    pub icmp_type: u8, // 8 = Echo Request, 0 = Echo Reply
    pub code: u8,      // 0
    pub checksum: u16, // RFC 1071 Checksum over ICMP header + payload
    pub ident: u16,    // Identifier
    pub seq: u16,      // Sequence Number
}
```

### 19.6.1 Automated Echo Responder
When an incoming Echo Request is received by `kernel/src/net/icmp.rs`, the kernel automatically swaps the source and destination IP addresses, sets `icmp_type` to `0` (Echo Reply), recomputes the checksum, and transmits the response back through the VirtIO driver.

### 19.6.2 The Kernel `ping` Command
The shell command `ping <ip>` generates Echo Requests with incrementing sequence numbers, records the APIC timer tick timestamp upon departure, polls the network driver for the corresponding Echo Reply, and computes round-trip time (RTT):
```
nullos> ping 10.0.2.2
PING 10.0.2.2 56(84) bytes of data.
64 bytes from 10.0.2.2: icmp_seq=1 ttl=64 time=2 ms
64 bytes from 10.0.2.2: icmp_seq=2 ttl=64 time=1 ms
64 bytes from 10.0.2.2: icmp_seq=3 ttl=64 time=1 ms

--- 10.0.2.2 ping statistics ---
3 packets transmitted, 3 received, 0% packet loss
```

---

## 19.7 Layer 4: User Datagram Protocol (UDP)

UDP (RFC 768) provides lightweight, connectionless datagram transport without handshakes, retransmissions, or ordering guarantees.

### 19.7.1 The 8-Byte UDP Header
```rust
#[repr(C, packed)]
pub struct UdpHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub length: u16,   // Header (8) + Payload length
    pub checksum: u16, // Computed over IPv4 Pseudo-Header + UDP datagram
}
```

### 19.7.2 The IPv4 Pseudo-Header
Unlike IPv4 where the checksum covers only the IP header, UDP and TCP include a **12-byte Pseudo-Header** that binds the transport segment to the underlying IP routing metadata:
1. Source IP Address (4 bytes)
2. Destination IP Address (4 bytes)
3. Zero Padding (1 byte)
4. Protocol Number (1 byte: `17` for UDP, `6` for TCP)
5. UDP/TCP Length (2 bytes)

This ensures that a datagram routed to an unintended IP or protocol is discarded even if the transport header itself was uncorrupted.

---

## 19.8 Layer 4: Transmission Control Protocol (TCP)

TCP (RFC 793) converts unreliable packet networks into reliable, ordered, duplex byte streams.

### 19.8.1 The 20-Byte TCP Header
```rust
#[repr(C, packed)]
pub struct TcpHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq_num: u32,
    pub ack_num: u32,
    pub data_offset_res: u8, // High 4 bits: Header length in 32-bit words (5 = 20 bytes)
    pub flags: u8,           // Control flags: SYN, ACK, FIN, RST, PSH, URG
    pub window_size: u16,    // Flow control receive window
    pub checksum: u16,       // Computed over Pseudo-Header + TCP segment
    pub urgent_ptr: u16,
}
```

### 19.8.2 The 11-State TCP State Machine
NullOS implements the complete RFC 793 finite state machine:
```
                               +---------+
                               | CLOSED  |
                               +---------+
                               /         \
                     Passive  /           \ Active
                      Open   /             \ Open (SYN)
                            v               v
                     +--------+          +----------+
                     | LISTEN |          | SYN_SENT |
                     +--------+          +----------+
                         |                    |
             Receive SYN |                    | Receive SYN+ACK
             Send SYN+ACK|                    | Send ACK
                         v                    v
                   +----------+         +-------------+
                   | SYN_RCVD |         | ESTABLISHED |<----+
                   +----------+         +-------------+     |
                         |                    |             |
             Receive ACK |                    | Close (FIN) | Established
                         v                    v             | Data Stream
                   +-------------+      +-------------+     |
                   | ESTABLISHED |----->| FIN_WAIT_1  |-----+
                   +-------------+      +-------------+
```

| State | Description |
|---|---|
| `CLOSED` | Fictitious initial state; socket exists without active connection. |
| `LISTEN` | Server socket waiting for inbound connection requests from remote clients. |
| `SYN_SENT` | Client socket sent a `SYN` segment and awaits `SYN+ACK`. |
| `SYN_RECEIVED` | Server received client `SYN`, responded with `SYN+ACK`, awaiting final `ACK`. |
| `ESTABLISHED` | Open, reliable duplex connection transferring data. |
| `FIN_WAIT_1` | First phase of active connection termination; sent `FIN`. |
| `FIN_WAIT_2` | Received `ACK` for our `FIN`; waiting for peer's `FIN`. |
| `CLOSE_WAIT` | Passive close; received peer's `FIN`, sent `ACK`, notifying application. |
| `CLOSING` | Simultaneous close; sent `FIN` and received peer's `FIN` before ACK arrived. |
| `LAST_ACK` | Passive close; sent own `FIN`, waiting for peer's final `ACK`. |
| `TIME_WAIT` | 2MSL wait to ensure remote peer received the final ACK. |

### 19.8.3 The 3-Way Handshake
When an application calls `connect(remote_ip, remote_port)`:
1. **Client $\rightarrow$ Server**: Client generates an Initial Sequence Number ($ISN_C$) and sends a segment with `flags = SYN`, `seq = ISN_C`. State transitions to `SYN_SENT`.
2. **Server $\rightarrow$ Client**: Server allocates state, selects $ISN_S$, and responds with `flags = SYN | ACK`, `seq = ISN_S`, `ack = ISN_C + 1`. State transitions to `SYN_RECEIVED`.
3. **Client $\rightarrow$ Server**: Client receives `SYN | ACK`, transitions to `ESTABLISHED`, and sends `flags = ACK`, `seq = ISN_C + 1`, `ack = ISN_S + 1`. Both endpoints are now ready to stream payload bytes.

---

## 19.9 Layer 5: Sockets Layer & POSIX API

To prevent applications from manipulating raw Ethernet frames or IP headers directly, NullOS provides the **Berkeley Sockets Layer** (`kernel/src/net/socket.rs`).

### 19.9.1 Socket Descriptors and Global Socket Table
Each socket is identified by an integer handle (`SocketHandle`) stored in `SOCKET_TABLE`:
```rust
pub static SOCKET_TABLE: Spinlock<BTreeMap<SocketHandle, Socket>> = Spinlock::new(BTreeMap::new());
```

### 19.9.2 Core Socket Primitives
- `socket(domain: AddressFamily, socket_type: SocketType) -> Result<SocketHandle, &'static str>`
  Allocates a new socket structure (`AF_INET`, `SOCK_DGRAM` for UDP, `SOCK_STREAM` for TCP).
- `bind(handle: SocketHandle, port: u16) -> Result<(), &'static str>`
  Assigns a local transport port number to the socket.
- `listen(handle: SocketHandle, backlog: usize) -> Result<(), &'static str>`
  Places a TCP socket into the `LISTEN` state.
- `connect(handle: SocketHandle, remote_ip: Ipv4Addr, remote_port: u16) -> Result<(), &'static str>`
  Initiates a 3-way TCP handshake or associates a UDP socket with a peer.
- `send(handle: SocketHandle, data: &[u8]) -> Result<usize, &'static str>`
  Encapsulates payload into UDP datagrams or TCP segments and dispatches through the IP layer.
- `recv(handle: SocketHandle, buffer: &mut [u8]) -> Result<usize, &'static str>`
  Pulls arriving data from the socket's internal FIFO ring buffer.
- `close(handle: SocketHandle) -> Result<(), &'static str>`
  Gracefully tears down the connection (`FIN` handshake for TCP) and frees socket table resources.

---

## 19.10 Shell Diagnostics: ifconfig, arp, netstat & ping

Milestone 24 integrates four standard diagnostic utilities directly into the NullOS shell (`nsh`):

### 1. `ifconfig`
Displays the MAC address, assigned IPv4 address, subnet netmask, default gateway, MTU, and VirtIO hardware RX/TX packet counters:
```
nullos> ifconfig
eth0: flags=4163<UP,BROADCAST,RUNNING,MULTICAST>  mtu 1500
        inet 10.0.2.15  netmask 255.255.255.0  broadcast 10.0.2.255
        ether 52:54:00:12:34:56  txqueuelen 1000  (Ethernet)
        RX packets 42  bytes 3420 (3.3 KiB)
        TX packets 56  bytes 4812 (4.7 KiB)
```

### 2. `arp`
Inspects the active IP-to-MAC resolution cache table:
```
nullos> arp
IP Address       HW Type    HW Address        
----------------------------------------------------
10.0.2.2         ether      52:54:00:12:34:02
10.0.2.3         ether      52:54:00:12:34:03
```

### 3. `netstat`
Inspects active network sockets, local/foreign endpoints, and current TCP states:
```
nullos> netstat
Proto  Local Address          Foreign Address        State       
--------------------------------------------------------------------
tcp    0.0.0.0:80             0.0.0.0:0              LISTEN
tcp    10.0.2.15:49152        10.0.2.2:8080          ESTABLISHED
udp    0.0.0.0:53             0.0.0.0:0              N/A
```

### 4. `ping <ip>`
Transmits ICMP Echo Requests and measures round-trip latency.

---

## 19.11 Architectural Verification

During boot, the kernel executes automated self-tests across all layers of the network stack before launching the shell:

```
----------------------------------------
 [VIRTIO-NET] Probing Network Hardware  
----------------------------------------
[VIRTIO-NET] Found VirtIO Network Card at PCI 00:02.0 (Vendor: 0x1af4, Device: 0x1000)
[VIRTIO-NET] Initialized 'eth0': Hardware MAC: 52:54:00:12:34:56
[VIRTIO-NET] Virtqueues active: RX Queue (size: 256), TX Queue (size: 256)
[VIRTIO-NET] Testing packet transmission on 'eth0' (MAC: 52:54:00:12:34:56)...
[VIRTIO-NET] Successfully transmitted 60-byte raw Ethernet test frame via Virtqueue!
[VIRTIO-NET] Driver statistics verified: 1 packet sent (60 bytes).
[VIRTIO-NET] All VirtIO Network driver self-tests passed successfully!
----------------------------------------
 [NET] Initializing TCP/IP Network Stack
----------------------------------------
[NET] Interface: eth0
[NET]   MAC:     52:54:00:12:34:56
[NET]   IPv4:    10.0.2.15
[NET]   Netmask: 255.255.255.0
[NET]   Gateway: 10.0.2.2
[NET]   DNS:     10.0.2.3
[NET] TCP/IP Protocol Layers active: Ethernet, ARP, IPv4, ICMP, UDP, TCP.
[NET] Running TCP/IP Network Stack verification self-tests...
[ETHERNET] Ethernet II frame construction & parsing verified successfully!
[ARP] ARP packet encoding & table cache verified successfully!
[IPV4] IPv4 Header, RFC 1071 Checksum & Address parsing verified successfully!
[ICMP] ICMP Echo Request & Reply serialization verified successfully!
[UDP] UDP Datagram Header & Pseudo-Header Checksum verified successfully!
[TCP] TCP Header, Flags & Pseudo-Header Checksum verified successfully!
[SOCKET] Sockets API, UDP demux & TCP connection tracking verified successfully!
[NET] All TCP/IP Network Stack self-tests passed successfully!
```

---

## 19.12 Summary

With Phase 8 complete:
- **Milestone 23 (Network Driver)**: Implemented PCI discovery, dual Split Virtqueues (Queue 0 RX / Queue 1 TX), DMA buffer allocations, and hardware MAC discovery.
- **Milestone 24 (Network Stack)**: Constructed a full TCP/IP networking stack in `#![no_std]` Rust, featuring Ethernet II framing, ARP, IPv4 with RFC 1071 checksumming, ICMP Echo response, UDP, the 11-state TCP state machine, the Berkeley Sockets API, and interactive shell diagnostics.

NullOS is no longer an isolated machine: it can now discover network peers, resolve addresses, transmit and receive datagrams, establish reliable stream connections, and participate in the global Internet.
