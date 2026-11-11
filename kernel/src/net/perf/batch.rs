//! # Zero-Copy Packet Framing & Packet Batching (Phase 16 - M36)
//!
//! ## Overview
//! 1. **Zero-Copy Framing**: Uses [`NetBuf`] front headroom (`NET_BUF_HEADROOM = 64B`)
//!    so L4 (UDP/TCP), L3 (IPv4), and L2 (Ethernet II) headers are prepended directly
//!    in front of user payloads without allocating or copying payload buffers.
//! 2. **Packet Batching (`PacketBatch`)**: Groups multiple packets (up to `MAX_BATCH_SIZE = 32`)
//!    for vectorized RX/TX processing, amortizing lock acquisitions and protocol dispatch
//!    across batches instead of locking per single packet.

use alloc::vec::Vec;

use crate::drivers::virtio_net::MacAddress;
use crate::net::ethernet::ETH_HDR_LEN;
use crate::net::ipv4::{compute_checksum, Ipv4Addr, IPV4_HDR_MIN_LEN};
use crate::net::perf::pool::{NetBuf, GLOBAL_NET_POOL};
use crate::net::udp::UDP_HDR_LEN;
use crate::sync::Spinlock;

const ETHERTYPE_IPV4: u16 = 0x0800;
const IP_PROTO_UDP: u8 = 17;

/// Maximum packets grouped in a single TX/RX batch.
pub const MAX_BATCH_SIZE: usize = 32;

/// Reusable fixed-capacity batch container carrying zero-copy [`NetBuf`] descriptors.
pub struct PacketBatch {
    pub packets: Vec<NetBuf>,
}

impl PacketBatch {
    /// Creates an empty batch pre-sized to `MAX_BATCH_SIZE`.
    pub fn new() -> Self {
        Self {
            packets: Vec::with_capacity(MAX_BATCH_SIZE),
        }
    }

    /// Appends a packet buffer to the batch, returning `Err(buf)` if batch is already full.
    #[inline]
    pub fn push(&mut self, buf: NetBuf) -> Result<(), NetBuf> {
        if self.packets.len() >= MAX_BATCH_SIZE {
            return Err(buf);
        }
        self.packets.push(buf);
        Ok(())
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.packets.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }

    /// Returns all packet buffers in this batch cleanly back to [`GLOBAL_NET_POOL`].
    pub fn recycle_all(&mut self) {
        while let Some(buf) = self.packets.pop() {
            GLOBAL_NET_POOL.free(buf);
        }
    }
}

/// Statistics tracking zero-copy and packet-batching efficiency.
#[derive(Debug, Clone, Copy, Default)]
pub struct BatchStats {
    pub total_batches: u64,
    pub total_packets_batched: u64,
    pub zerocopy_frames_built: u64,
    pub bytes_saved_from_copy: u64,
}

static BATCH_STATS: Spinlock<BatchStats> = Spinlock::new(BatchStats {
    total_batches: 0,
    total_packets_batched: 0,
    zerocopy_frames_built: 0,
    bytes_saved_from_copy: 0,
});

/// Returns snapshot telemetry of batching & zero-copy framing operations.
pub fn batch_stats() -> BatchStats {
    *BATCH_STATS.lock()
}

/// Builds a complete Ethernet II + IPv4 + UDP packet completely **in-place** inside
/// a pooled [`NetBuf`] using headroom prepending (Zero-Copy TX construction).
///
/// The payload is placed once into the buffer, and L4 -> L3 -> L2 headers are pushed
/// backward into the 64-byte front headroom without moving or reallocating payload bytes.
pub fn build_udp_ipv4_zerocopy(
    src_mac: MacAddress,
    dst_mac: MacAddress,
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
    payload: &[u8],
) -> Result<NetBuf, &'static str> {
    let mut buf = GLOBAL_NET_POOL.alloc().ok_or("GLOBAL_NET_POOL exhausted")?;
    buf.put_payload(payload)?;

    let payload_len = payload.len();

    // 1. Prepend UDP header (8 bytes) in-place
    let udp_len = (UDP_HDR_LEN + payload_len) as u16;
    {
        let hdr = buf.push_header(UDP_HDR_LEN)?;
        hdr[0..2].copy_from_slice(&src_port.to_be_bytes());
        hdr[2..4].copy_from_slice(&dst_port.to_be_bytes());
        hdr[4..6].copy_from_slice(&udp_len.to_be_bytes());
        hdr[6..8].copy_from_slice(&0u16.to_be_bytes()); // Optional for UDP IPv4 or computed below
    }

    // 2. Prepend IPv4 header (20 bytes) in-place
    let ip_total_len = (IPV4_HDR_MIN_LEN + UDP_HDR_LEN + payload_len) as u16;
    {
        let ip_hdr = buf.push_header(IPV4_HDR_MIN_LEN)?;
        ip_hdr[0] = 0x45; // Version 4, IHL 5 (20 bytes)
        ip_hdr[1] = 0x00; // DSCP / ECN
        ip_hdr[2..4].copy_from_slice(&ip_total_len.to_be_bytes());
        ip_hdr[4..6].copy_from_slice(&1u16.to_be_bytes()); // Identification
        ip_hdr[6..8].copy_from_slice(&0x4000u16.to_be_bytes()); // Don't Fragment
        ip_hdr[8] = 64; // TTL
        ip_hdr[9] = IP_PROTO_UDP;
        ip_hdr[10..12].copy_from_slice(&[0, 0]);
        ip_hdr[12..16].copy_from_slice(&src_ip.0);
        ip_hdr[16..20].copy_from_slice(&dst_ip.0);
        let csum = compute_checksum(ip_hdr);
        ip_hdr[10..12].copy_from_slice(&csum.to_be_bytes());
    }

    // 3. Prepend Ethernet II header (14 bytes) in-place
    {
        let eth_hdr = buf.push_header(ETH_HDR_LEN)?;
        eth_hdr[0..6].copy_from_slice(&dst_mac.0);
        eth_hdr[6..12].copy_from_slice(&src_mac.0);
        eth_hdr[12..14].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
    }

    let mut st = BATCH_STATS.lock();
    st.zerocopy_frames_built += 1;
    st.bytes_saved_from_copy += payload_len as u64;
    Ok(buf)
}

/// Processes a batch of outgoing [`NetBuf`] frames through the provided transmission closure,
/// recycling every buffer back to [`GLOBAL_NET_POOL`] upon completion.
pub fn dispatch_tx_batch<F>(batch: &mut PacketBatch, mut tx_fn: F) -> usize
where
    F: FnMut(&[u8]) -> Result<(), &'static str>,
{
    if batch.is_empty() {
        return 0;
    }

    let count = batch.len();
    let mut transmitted = 0usize;
    for pkt in batch.packets.iter() {
        if tx_fn(pkt.data()).is_ok() {
            transmitted += 1;
        }
    }
    batch.recycle_all();

    let mut st = BATCH_STATS.lock();
    st.total_batches += 1;
    st.total_packets_batched += count as u64;
    transmitted
}
