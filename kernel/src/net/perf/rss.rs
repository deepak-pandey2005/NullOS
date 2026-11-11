//! # Receive Side Scaling (RSS) & Multiqueue Networking (Phase 16 - M36)
//!
//! ## Overview
//! Distributes incoming and outgoing network packet flows across multiple independent
//! queue pairs aligned with CPU cores (`cpu_affinity`) to scale throughput linearly on
//! Symmetric Multiprocessing (SMP) systems.
//!
//! ## RSS Toeplitz Hash Algorithm
//! For every incoming IPv4 packet:
//! 1. Extract the 4-tuple `(src_ip, dst_ip, src_port, dst_port)` (or 2-tuple for non-TCP/UDP).
//! 2. Compute the standard 32-bit Microsoft **Toeplitz Hash** over the byte tuple using
//!    a 40-byte symmetric secret key (`DEFAULT_RSS_KEY`).
//! 3. Look up `indirection_table[hash & 0x7F]` to select the target RX queue (`0..num_queues`).
//!
//! Because hashing is symmetric-flow deterministic, all packets belonging to the same TCP
//! or UDP connection map consistently to the exact same queue and CPU core, preserving
//! packet ordering while eliminating cross-core spinlock contention.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use crate::net::ipv4::Ipv4Addr;
use crate::net::perf::pool::{NetBuf, GLOBAL_NET_POOL};
use crate::sync::Spinlock;

/// Standard 40-byte Microsoft RSS Toeplitz secret key.
pub const DEFAULT_RSS_KEY: [u8; 40] = [
    0x6D, 0x5A, 0x56, 0xDA, 0x25, 0x5B, 0x0E, 0xC2,
    0x41, 0x67, 0x25, 0x3D, 0x43, 0xA3, 0x8F, 0xB0,
    0xD0, 0xCA, 0x2B, 0xCB, 0xAE, 0x7B, 0x30, 0xB4,
    0x77, 0xCB, 0x2D, 0xA3, 0x80, 0x30, 0xF2, 0x0C,
    0x6A, 0x42, 0xB7, 0x3B, 0xBE, 0xAC, 0x01, 0xFA,
];

/// Standard 128-entry RSS Redirection / Indirection Table size.
pub const RSS_INDIRECTION_SIZE: usize = 128;

/// Maximum supported multiqueue hardware/software queue pairs.
pub const MAX_NET_QUEUES: usize = 8;

/// Computes the 32-bit Microsoft RSS Toeplitz Hash over `input` bytes using a 40-byte key.
pub fn toeplitz_hash(input: &[u8], key: &[u8; 40]) -> u32 {
    let mut result: u32 = 0;
    let mut key_word = u32::from_be_bytes([key[0], key[1], key[2], key[3]]);
    let mut key_byte_idx = 4usize;

    for &byte in input {
        for bit in (0..8).rev() {
            if (byte >> bit) & 1 != 0 {
                result ^= key_word;
            }
            let next_bit = if key_byte_idx < key.len() {
                ((key[key_byte_idx] >> bit) & 1) as u32
            } else {
                0
            };
            key_word = (key_word << 1) | next_bit;
        }
        if key_byte_idx < key.len() {
            key_byte_idx += 1;
        }
    }

    result
}

/// Computes an RSS Toeplitz hash over an IPv4 TCP/UDP 4-tuple `(src_ip, dst_ip, src_port, dst_port)`.
#[inline]
pub fn rss_hash_ipv4_4tuple(
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
) -> u32 {
    let mut tuple = [0u8; 12];
    tuple[0..4].copy_from_slice(&src_ip.0);
    tuple[4..8].copy_from_slice(&dst_ip.0);
    tuple[8..10].copy_from_slice(&src_port.to_be_bytes());
    tuple[10..12].copy_from_slice(&dst_port.to_be_bytes());
    toeplitz_hash(&tuple, &DEFAULT_RSS_KEY)
}

/// Computes an RSS Toeplitz hash over an IPv4 2-tuple `(src_ip, dst_ip)`.
#[inline]
pub fn rss_hash_ipv4_2tuple(src_ip: Ipv4Addr, dst_ip: Ipv4Addr) -> u32 {
    let mut tuple = [0u8; 8];
    tuple[0..4].copy_from_slice(&src_ip.0);
    tuple[4..8].copy_from_slice(&dst_ip.0);
    toeplitz_hash(&tuple, &DEFAULT_RSS_KEY)
}

/// Extracts flow hash directly from a raw Ethernet frame buffer (IPv4 TCP/UDP 4-tuple or 2-tuple).
pub fn classify_ethernet_frame_rss(frame: &[u8]) -> u32 {
    if frame.len() < 14 {
        return 0;
    }
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    if ethertype != 0x0800 || frame.len() < 34 {
        // Non-IPv4: hash MAC pair
        return toeplitz_hash(&frame[0..12], &DEFAULT_RSS_KEY);
    }

    let ip_hdr = &frame[14..];
    let ihl = ((ip_hdr[0] & 0x0F) as usize) * 4;
    if ihl < 20 || ip_hdr.len() < ihl {
        return 0;
    }

    let proto = ip_hdr[9];
    let src_ip = Ipv4Addr([ip_hdr[12], ip_hdr[13], ip_hdr[14], ip_hdr[15]]);
    let dst_ip = Ipv4Addr([ip_hdr[16], ip_hdr[17], ip_hdr[18], ip_hdr[19]]);

    if (proto == 6 || proto == 17) && ip_hdr.len() >= ihl + 4 {
        let l4 = &ip_hdr[ihl..];
        let src_port = u16::from_be_bytes([l4[0], l4[1]]);
        let dst_port = u16::from_be_bytes([l4[2], l4[3]]);
        rss_hash_ipv4_4tuple(src_ip, dst_ip, src_port, dst_port)
    } else {
        rss_hash_ipv4_2tuple(src_ip, dst_ip)
    }
}

/// Per-queue telemetry statistics.
#[derive(Debug, Clone, Copy, Default)]
pub struct QueueStats {
    pub queue_id: usize,
    pub cpu_affinity: usize,
    pub rx_depth: usize,
    pub tx_depth: usize,
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// Isolated single-queue pair (RX + TX rings) mapped to a specific CPU core.
pub struct NetQueuePair {
    pub queue_id: usize,
    pub cpu_affinity: usize,
    rx_ring: VecDeque<NetBuf>,
    tx_ring: VecDeque<NetBuf>,
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

impl NetQueuePair {
    fn new(queue_id: usize, cpu_affinity: usize) -> Self {
        Self {
            queue_id,
            cpu_affinity,
            rx_ring: VecDeque::with_capacity(64),
            tx_ring: VecDeque::with_capacity(64),
            rx_packets: 0,
            tx_packets: 0,
            rx_bytes: 0,
            tx_bytes: 0,
        }
    }

    fn stats(&self) -> QueueStats {
        QueueStats {
            queue_id: self.queue_id,
            cpu_affinity: self.cpu_affinity,
            rx_depth: self.rx_ring.len(),
            tx_depth: self.tx_ring.len(),
            rx_packets: self.rx_packets,
            tx_packets: self.tx_packets,
            rx_bytes: self.rx_bytes,
            tx_bytes: self.tx_bytes,
        }
    }
}

/// Multiqueue + RSS Coordinator distributing packets across `num_queues` isolated lock rings.
pub struct MultiQueueSubsystem {
    num_queues: Spinlock<usize>,
    indirection_table: Spinlock<[u16; RSS_INDIRECTION_SIZE]>,
    queues: [Spinlock<NetQueuePair>; MAX_NET_QUEUES],
}

impl MultiQueueSubsystem {
    /// Constructs a static multiqueue subsystem ready for initialization with `num_queues`.
    pub const fn new_default() -> Self {
        Self {
            num_queues: Spinlock::new(4),
            indirection_table: Spinlock::new([0u16; RSS_INDIRECTION_SIZE]),
            queues: [
                Spinlock::new(NetQueuePair { queue_id: 0, cpu_affinity: 0, rx_ring: VecDeque::new(), tx_ring: VecDeque::new(), rx_packets: 0, tx_packets: 0, rx_bytes: 0, tx_bytes: 0 }),
                Spinlock::new(NetQueuePair { queue_id: 1, cpu_affinity: 1, rx_ring: VecDeque::new(), tx_ring: VecDeque::new(), rx_packets: 0, tx_packets: 0, rx_bytes: 0, tx_bytes: 0 }),
                Spinlock::new(NetQueuePair { queue_id: 2, cpu_affinity: 2, rx_ring: VecDeque::new(), tx_ring: VecDeque::new(), rx_packets: 0, tx_packets: 0, rx_bytes: 0, tx_bytes: 0 }),
                Spinlock::new(NetQueuePair { queue_id: 3, cpu_affinity: 3, rx_ring: VecDeque::new(), tx_ring: VecDeque::new(), rx_packets: 0, tx_packets: 0, rx_bytes: 0, tx_bytes: 0 }),
                Spinlock::new(NetQueuePair { queue_id: 4, cpu_affinity: 4, rx_ring: VecDeque::new(), tx_ring: VecDeque::new(), rx_packets: 0, tx_packets: 0, rx_bytes: 0, tx_bytes: 0 }),
                Spinlock::new(NetQueuePair { queue_id: 5, cpu_affinity: 5, rx_ring: VecDeque::new(), tx_ring: VecDeque::new(), rx_packets: 0, tx_packets: 0, rx_bytes: 0, tx_bytes: 0 }),
                Spinlock::new(NetQueuePair { queue_id: 6, cpu_affinity: 6, rx_ring: VecDeque::new(), tx_ring: VecDeque::new(), rx_packets: 0, tx_packets: 0, rx_bytes: 0, tx_bytes: 0 }),
                Spinlock::new(NetQueuePair { queue_id: 7, cpu_affinity: 7, rx_ring: VecDeque::new(), tx_ring: VecDeque::new(), rx_packets: 0, tx_packets: 0, rx_bytes: 0, tx_bytes: 0 }),
            ],
        }
    }

    /// Configures active queue count (typically matching online SMP CPU cores) and populates
    /// the 128-entry RSS indirection table evenly across all active queues.
    pub fn configure(&self, active_queues: usize) {
        let count = core::cmp::max(1, core::cmp::min(active_queues, MAX_NET_QUEUES));
        *self.num_queues.lock() = count;

        let mut table = self.indirection_table.lock();
        for (i, slot) in table.iter_mut().enumerate() {
            *slot = (i % count) as u16;
        }
    }

    /// Returns the number of configured active queues.
    pub fn active_queues(&self) -> usize {
        *self.num_queues.lock()
    }

    /// Resolves the target queue index via the RSS Indirection Table for a 32-bit flow hash.
    #[inline]
    pub fn select_queue(&self, rss_hash: u32) -> usize {
        let idx = (rss_hash as usize) & (RSS_INDIRECTION_SIZE - 1);
        let q = self.indirection_table.lock()[idx] as usize;
        let active = *self.num_queues.lock();
        q % active
    }

    /// Steers a received [`NetBuf`] to its RSS-selected RX queue, updating telemetry counters.
    pub fn steer_rx(&self, mut buf: NetBuf) -> usize {
        let hash = classify_ethernet_frame_rss(buf.data());
        let qid = self.select_queue(hash);
        buf.rss_hash = hash;
        buf.queue_id = qid as u16;

        let len = buf.data_len as u64;
        let mut q = self.queues[qid].lock();
        q.rx_packets += 1;
        q.rx_bytes += len;
        q.rx_ring.push_back(buf);
        qid
    }

    /// Pops up to `max_batch` packets from queue `qid`'s RX ring.
    pub fn pop_rx_batch(&self, qid: usize, max_batch: usize) -> Vec<NetBuf> {
        let active = *self.num_queues.lock();
        if qid >= active {
            return Vec::new();
        }
        let mut q = self.queues[qid].lock();
        let count = core::cmp::min(max_batch, q.rx_ring.len());
        let mut batch = Vec::with_capacity(count);
        for _ in 0..count {
            if let Some(buf) = q.rx_ring.pop_front() {
                batch.push(buf);
            }
        }
        batch
    }

    /// Enqueues an outgoing [`NetBuf`] onto the specified TX queue (selected by core ID or flow hash).
    pub fn enqueue_tx(&self, qid_hint: usize, mut buf: NetBuf) {
        let active = *self.num_queues.lock();
        let qid = qid_hint % active;
        buf.queue_id = qid as u16;

        let len = buf.data_len as u64;
        let mut q = self.queues[qid].lock();
        q.tx_packets += 1;
        q.tx_bytes += len;
        q.tx_ring.push_back(buf);
    }

    /// Drains up to `max_batch` TX buffers from queue `qid`, returning them cleanly to [`GLOBAL_NET_POOL`]
    /// once transmitted via `tx_fn`.
    pub fn flush_tx_batch<F>(&self, qid: usize, max_batch: usize, mut tx_fn: F) -> usize
    where
        F: FnMut(&[u8]) -> Result<(), &'static str>,
    {
        let active = *self.num_queues.lock();
        if qid >= active {
            return 0;
        }
        let mut drained = Vec::new();
        {
            let mut q = self.queues[qid].lock();
            let count = core::cmp::min(max_batch, q.tx_ring.len());
            for _ in 0..count {
                if let Some(buf) = q.tx_ring.pop_front() {
                    drained.push(buf);
                }
            }
        }
        let mut sent = 0usize;
        for buf in drained {
            if tx_fn(buf.data()).is_ok() {
                sent += 1;
            }
            GLOBAL_NET_POOL.free(buf);
        }
        sent
    }

    /// Returns a snapshot of per-queue statistics for all active queues.
    pub fn all_queue_stats(&self) -> Vec<QueueStats> {
        let active = *self.num_queues.lock();
        let mut out = Vec::with_capacity(active);
        for i in 0..active {
            out.push(self.queues[i].lock().stats());
        }
        out
    }
}

/// Global Multiqueue & RSS Engine instance.
pub static GLOBAL_MULTIQUEUE: MultiQueueSubsystem = MultiQueueSubsystem::new_default();

