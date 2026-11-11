//! # High-Performance TCP Protocol Optimizations (Phase 16 - M36)
//!
//! ## Overview
//! Implements modern TCP congestion control, RTT/RTO estimation, header extensions,
//! and latency-reduction heuristics compliant with IETF standards:
//! - **Nagle's Algorithm & `TCP_NODELAY`** (RFC 896): Coalesces small writes when unacknowledged
//!   data is in flight unless `nodelay` is set for interactive/low-latency streams.
//! - **Delayed ACK & `TCP_QUICKACK`** (RFC 1122): Defers standalone pure ACKs up to `delayed_ack_ms`
//!   (default 40ms) or sends immediately every 2 full MSS segments.
//! - **Window Scaling** (RFC 7323): Scales the 16-bit advertised TCP window by `2^rcv_wscale`
//!   (up to shift count 14, expanding maximum window from 64 KiB to 1 GiB).
//! - **Selective Acknowledgment (SACK)** (RFC 2018): Tracks up to 4 out-of-order contiguous
//!   byte blocks (`[left_edge, right_edge)`) so only missing gaps are retransmitted.
//! - **Jacobson / Karels SRTT & RTO** (RFC 6298): Smooth round-trip time estimation using
//!   `EWMA` (`alpha = 1/8`, `beta = 1/4`) to prevent premature spurious retransmissions.
//! - **Reno Congestion Control**: Slow Start (`cwnd += mss`), Congestion Avoidance
//!   (`cwnd += mss * mss / cwnd`), and Fast Retransmit / Fast Recovery on 3 duplicate ACKs.
//! - **Checksum Offload Helpers**: Fast 16-bit pseudo-header incremental/partial checksum math.

use alloc::vec::Vec;
use crate::net::ipv4::Ipv4Addr;

/// Default Ethernet/IPv4 TCP Maximum Segment Size (1500 MTU - 20B IPv4 - 20B TCP).
pub const DEFAULT_TCP_MSS: u32 = 1460;

/// Maximum RFC 7323 Window Scale shift count allowed by specification.
pub const MAX_WINDOW_SCALE: u8 = 14;

/// Maximum RFC 2018 SACK blocks allowed inside TCP options header (40 bytes option budget).
pub const MAX_SACK_BLOCKS: usize = 4;

/// Represents one contiguous out-of-order byte range received beyond `rcv_nxt` (RFC 2018).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SackBlock {
    /// First byte sequence number of the contiguous received block.
    pub left_edge: u32,
    /// Sequence number immediately following the last byte of the block (exclusive).
    pub right_edge: u32,
}

impl SackBlock {
    /// Returns the byte length covered by this SACK block (wrapping-safe).
    #[inline]
    pub fn len(&self) -> u32 {
        self.right_edge.wrapping_sub(self.left_edge)
    }

    /// Serializes this SACK block into big-endian 8-byte wire format.
    #[inline]
    pub fn to_be_bytes(&self) -> [u8; 8] {
        let mut out = [0u8; 8];
        out[0..4].copy_from_slice(&self.left_edge.to_be_bytes());
        out[4..8].copy_from_slice(&self.right_edge.to_be_bytes());
        out
    }
}

/// Tunable TCP socket performance policy flags.
#[derive(Debug, Clone, Copy)]
pub struct TcpOptConfig {
    /// Disable Nagle's coalescing for immediate transmission (`TCP_NODELAY`).
    pub nodelay: bool,
    /// Enable Delayed ACK (RFC 1122); disabled transiently by `quickack`.
    pub delayed_ack: bool,
    /// Force immediate pure ACK on next received segment (`TCP_QUICKACK`).
    pub quickack: bool,
    /// Maximum delayed ACK timer threshold in milliseconds (typically 40ms).
    pub delayed_ack_ms: u32,
    /// Enable RFC 7323 Window Scaling option negotiation.
    pub window_scaling: bool,
    /// Local receive window shift factor (`0..=14`).
    pub rcv_wscale: u8,
    /// Enable RFC 2018 Selective Acknowledgment (`SACK_PERMITTED`).
    pub sack_enabled: bool,
    /// Enable TX/RX partial pseudo-header checksum offload path.
    pub checksum_offload: bool,
}

impl Default for TcpOptConfig {
    fn default() -> Self {
        Self {
            nodelay: false,
            delayed_ack: true,
            quickack: false,
            delayed_ack_ms: 40,
            window_scaling: true,
            rcv_wscale: 7, // Scales advertised 65535 -> ~8 MiB effective window
            sack_enabled: true,
            checksum_offload: true,
        }
    }
}

/// Congestion-control phase of an optimized TCP connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CongestionState {
    SlowStart,
    CongestionAvoidance,
    FastRecovery,
}

/// High-Performance TCP Optimization Control Block tracking RTT, congestion window,
/// SACK scoreboard, Nagle gating, and Delayed ACK coalescing.
#[derive(Debug, Clone)]
pub struct TcpOptControlBlock {
    pub config: TcpOptConfig,
    /// Negotiated peer send window scale shift count (from peer's SYN option).
    pub snd_wscale: u8,
    /// Congestion window (`cwnd`) tracked in bytes.
    pub cwnd: u32,
    /// Slow-start threshold (`ssthresh`) tracked in bytes.
    pub ssthresh: u32,
    /// Current congestion control state.
    pub cc_state: CongestionState,
    /// Segment size (`MSS`) in bytes.
    pub mss: u32,
    /// Smoothed RTT (RFC 6298) in milliseconds (fixed-point scaled by 8 for accuracy).
    pub srtt_ms_x8: u32,
    /// RTT variation (RFC 6298) in milliseconds (scaled by 4 for accuracy).
    pub rttvar_ms_x4: u32,
    /// Computed Retransmission Timeout (`RTO`) in milliseconds (clamped `[200, 60000]`).
    pub rto_ms: u32,
    /// Consecutive duplicate ACKs seen for Fast Retransmit (triggers at `3`).
    pub dup_ack_count: u8,
    /// Last highest cumulative ACK observed.
    pub last_ack: u32,
    /// Count of packets pending a delayed ACK (fires immediately at `>= 2`).
    pub unacked_rx_segments: u8,
    /// Active out-of-order RFC 2018 SACK blocks.
    pub sack_blocks: Vec<SackBlock>,
    /// Telemetry: Fast Retransmit events triggered by 3 duplicate ACKs.
    pub fast_retransmits: u64,
    /// Telemetry: Pure ACKs coalesced/saved via Delayed ACK.
    pub delayed_acks_saved: u64,
}

impl TcpOptControlBlock {
    /// Initializes a control block with standard RFC defaults (IW = 10*MSS per RFC 6928).
    pub fn new(config: TcpOptConfig, mss: u32) -> Self {
        let clamped_mss = core::cmp::max(536, mss);
        let mut cfg = config;
        if cfg.rcv_wscale > MAX_WINDOW_SCALE {
            cfg.rcv_wscale = MAX_WINDOW_SCALE;
        }
        Self {
            config: cfg,
            snd_wscale: 0,
            cwnd: clamped_mss * 10, // Initial Window = 10 MSS (RFC 6928)
            ssthresh: 65535 * 16,
            cc_state: CongestionState::SlowStart,
            mss: clamped_mss,
            srtt_ms_x8: 0,
            rttvar_ms_x4: 0,
            rto_ms: 1000, // RFC 6298 initial RTO = 1 second
            dup_ack_count: 0,
            last_ack: 0,
            unacked_rx_segments: 0,
            sack_blocks: Vec::with_capacity(MAX_SACK_BLOCKS),
            fast_retransmits: 0,
            delayed_acks_saved: 0,
        }
    }

    /// Configures peer window scaling after observing SYN/SYN-ACK option.
    pub fn set_peer_wscale(&mut self, shift: u8) {
        self.snd_wscale = core::cmp::min(shift, MAX_WINDOW_SCALE);
    }

    /// Decodes a peer's 16-bit advertised wire window into true scaled bytes (`win << snd_wscale`).
    #[inline]
    pub fn decode_peer_window(&self, raw_win: u16) -> u32 {
        (raw_win as u32) << self.snd_wscale
    }

    /// Encodes a local receive buffer capacity into the 16-bit wire header value (`bytes >> rcv_wscale`).
    #[inline]
    pub fn encode_local_window(&self, recv_space_bytes: u32) -> u16 {
        let scaled = recv_space_bytes >> self.config.rcv_wscale;
        core::cmp::min(scaled, u16::MAX as u32) as u16
    }

    /// Evaluates **Nagle's Algorithm** (RFC 896) to decide whether a segment of `pending_len`
    /// bytes should be transmitted immediately given `bytes_in_flight` unacknowledged.
    ///
    /// Returns `true` if allowed to transmit immediately, `false` if coalescing should wait.
    #[inline]
    pub fn should_send_nagle(&self, pending_len: usize, bytes_in_flight: usize) -> bool {
        if self.config.nodelay {
            return true;
        }
        if pending_len >= self.mss as usize {
            return true;
        }
        bytes_in_flight == 0
    }

    /// Evaluates **Delayed ACK** (RFC 1122) upon receiving an in-order segment of `seg_len` bytes.
    ///
    /// Returns `true` if an ACK should be sent immediately (every 2 segments, when `quickack`
    /// is active, or when Delayed ACK is disabled), or `false` if deferred.
    pub fn on_rx_in_order(&mut self, seg_len: usize) -> bool {
        if seg_len == 0 {
            return false;
        }
        if !self.config.delayed_ack || self.config.quickack {
            self.config.quickack = false;
            self.unacked_rx_segments = 0;
            return true;
        }
        self.unacked_rx_segments += 1;
        if self.unacked_rx_segments >= 2 {
            self.unacked_rx_segments = 0;
            true
        } else {
            self.delayed_acks_saved += 1;
            false
        }
    }

    /// Records an out-of-order byte range into the **RFC 2018 SACK Scoreboard**,
    /// merging overlapping/adjacent blocks and capping at `MAX_SACK_BLOCKS` (4).
    pub fn record_sack_block(&mut self, left_edge: u32, right_edge: u32) {
        if !self.config.sack_enabled || left_edge == right_edge {
            return;
        }
        // Merge with an existing overlapping/adjacent block if present
        for blk in self.sack_blocks.iter_mut() {
            if !(right_edge < blk.left_edge || left_edge > blk.right_edge) {
                blk.left_edge = core::cmp::min(blk.left_edge, left_edge);
                blk.right_edge = core::cmp::max(blk.right_edge, right_edge);
                return;
            }
        }
        if self.sack_blocks.len() == MAX_SACK_BLOCKS {
            // Keep most recent block at index 0 by evicting oldest
            self.sack_blocks.pop();
        }
        self.sack_blocks.insert(0, SackBlock { left_edge, right_edge });
    }

    /// Clears SACK blocks whose bytes have now been cumulatively acknowledged.
    pub fn prune_sack_below(&mut self, rcv_nxt: u32) {
        self.sack_blocks.retain(|blk| blk.right_edge > rcv_nxt);
    }

    /// Updates **RFC 6298 Jacobson/Karels SRTT, RTTVAR, and RTO** from a measured RTT sample (`rtt_ms`).
    pub fn update_rtt(&mut self, rtt_ms: u32) {
        let sample = core::cmp::max(1, rtt_ms);
        if self.srtt_ms_x8 == 0 {
            // First measurement: SRTT = R, RTTVAR = R/2
            self.srtt_ms_x8 = sample * 8;
            self.rttvar_ms_x4 = sample * 2;
        } else {
            let srtt = self.srtt_ms_x8 / 8;
            let err = srtt.abs_diff(sample);
            // RTTVAR <- (3/4)*RTTVAR + (1/4)*|SRTT - R'|
            self.rttvar_ms_x4 = (3 * self.rttvar_ms_x4 + err * 4) / 4;
            // SRTT <- (7/8)*SRTT + (1/8)*R'
            self.srtt_ms_x8 = (7 * self.srtt_ms_x8 + sample * 8) / 8;
        }
        let srtt_ms = self.srtt_ms_x8 / 8;
        let rttvar_ms = self.rttvar_ms_x4 / 4;
        let raw_rto = srtt_ms + core::cmp::max(1, 4 * rttvar_ms);
        self.rto_ms = raw_rto.clamp(200, 60_000);
    }

    /// Processes an incoming ACK number and drives **TCP Reno Congestion Control**.
    ///
    /// Returns `true` if a **Fast Retransmit** was triggered by observing 3 duplicate ACKs.
    pub fn on_ack(&mut self, ack_num: u32, newly_acked_bytes: u32) -> bool {
        if newly_acked_bytes == 0 && ack_num == self.last_ack {
            // Duplicate ACK handling
            self.dup_ack_count = self.dup_ack_count.saturating_add(1);
            if self.dup_ack_count == 3 {
                // Fast Retransmit & enter Fast Recovery
                self.ssthresh = core::cmp::max(self.cwnd / 2, self.mss * 2);
                self.cwnd = self.ssthresh + 3 * self.mss;
                self.cc_state = CongestionState::FastRecovery;
                self.fast_retransmits += 1;
                return true;
            } else if self.cc_state == CongestionState::FastRecovery {
                // Inflate cwnd for each additional duplicate ACK
                self.cwnd = self.cwnd.saturating_add(self.mss);
            }
            return false;
        }

        // Fresh cumulative ACK advances window
        self.last_ack = ack_num;
        self.dup_ack_count = 0;

        if self.cc_state == CongestionState::FastRecovery {
            // Exit Fast Recovery into Congestion Avoidance
            self.cwnd = self.ssthresh;
            self.cc_state = CongestionState::CongestionAvoidance;
        } else if self.cwnd < self.ssthresh {
            // Slow Start: exponential increase (~1 MSS per ACKed segment)
            self.cc_state = CongestionState::SlowStart;
            let inc = core::cmp::min(newly_acked_bytes, self.mss);
            self.cwnd = self.cwnd.saturating_add(inc);
        } else {
            // Congestion Avoidance: linear increase (~1 MSS per RTT)
            self.cc_state = CongestionState::CongestionAvoidance;
            let inc = core::cmp::max(1, (self.mss * self.mss) / core::cmp::max(1, self.cwnd));
            self.cwnd = self.cwnd.saturating_add(inc);
        }
        false
    }

    /// Handles an RTO timeout event: halves `ssthresh` and collapses `cwnd` back to `1 MSS`.
    pub fn on_rto_timeout(&mut self) {
        self.ssthresh = core::cmp::max(self.cwnd / 2, self.mss * 2);
        self.cwnd = self.mss;
        self.dup_ack_count = 0;
        self.cc_state = CongestionState::SlowStart;
        self.rto_ms = core::cmp::min(self.rto_ms.saturating_mul(2), 60_000);
    }

    /// Computes a hardware-offload-ready 16-bit IPv4/TCP pseudo-header partial checksum seed
    /// (`src_ip + dst_ip + proto(6) + tcp_len`) without walking payload bytes.
    #[inline]
    pub fn tcp_pseudo_checksum_seed(src_ip: Ipv4Addr, dst_ip: Ipv4Addr, tcp_len: u16) -> u16 {
        let mut sum: u32 = 0;
        sum += u16::from_be_bytes([src_ip.0[0], src_ip.0[1]]) as u32;
        sum += u16::from_be_bytes([src_ip.0[2], src_ip.0[3]]) as u32;
        sum += u16::from_be_bytes([dst_ip.0[0], dst_ip.0[1]]) as u32;
        sum += u16::from_be_bytes([dst_ip.0[2], dst_ip.0[3]]) as u32;
        sum += 6u32; // TCP IP protocol number
        sum += tcp_len as u32;
        while (sum >> 16) != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        sum as u16
    }
}

