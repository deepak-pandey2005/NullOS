//! # Adaptive NAPI-Style Budgeted Network Polling (Phase 16 - M36)
//!
//! ## Overview
//! Traditional interrupt-driven packet processing suffers from **receive livelock**
//! under high packet rates because CPU cycles are consumed entirely by IRQ entry/exit
//! overhead before any userspace thread or protocol handler can drain packets.
//!
//! ## NAPI Adaptive Polling Architecture
//! Inspired by Linux NAPI ("New API"):
//! 1. **Interrupt Mitigation (`PollMode::Interrupt` -> `PollMode::Polling`)**: When
//!    packets arrive, the poller enters a pure polling loop draining up to `budget`
//!    (default `64`) frames in one shot without re-enabling per-packet IRQ notifications.
//! 2. **RSS Queue Steering**: Every incoming frame received into a zero-copy pooled
//!    [`NetBuf`] is hashed and steered into its RSS queue via [`GLOBAL_MULTIQUEUE`].
//! 3. **Batched Protocol Dispatch**: Per-queue RX rings are drained in batches and
//!    dispatched to `ethernet::handle_packet`, then returned cleanly to [`GLOBAL_NET_POOL`].
//! 4. **Dynamic Mode Switch**: Once a poll cycle drains fewer packets than `budget`
//!    (the NIC ring is empty), the subsystem drops back to `PollMode::Interrupt`.

use crate::drivers::virtio_net;
use crate::net::ethernet;
use crate::net::perf::pool::{GLOBAL_NET_POOL, NET_BUF_HEADROOM, NET_BUF_SIZE};
use crate::net::perf::rss::GLOBAL_MULTIQUEUE;
use crate::sync::Spinlock;

/// Default maximum packets processed in a single NAPI poll invocation (`budget`).
pub const DEFAULT_NAPI_BUDGET: usize = 64;

/// Operating state of the adaptive network poller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollMode {
    /// Low traffic: waiting for hardware/timer notifications before polling.
    Interrupt,
    /// High traffic burst: actively polling NIC rings with IRQ mitigation.
    Polling,
}

/// Telemetry counters recorded by the NAPI polling engine.
#[derive(Debug, Clone, Copy)]
pub struct NapiStats {
    /// Current adaptive mode (`Interrupt` vs `Polling`).
    pub mode: PollMode,
    /// Configured packet budget per poll cycle.
    pub budget: usize,
    /// Total poll invocations executed.
    pub poll_cycles: u64,
    /// Total packets drained from NIC RX ring.
    pub packets_polled: u64,
    /// Number of times polling hit the `budget` cap and remained in `PollMode::Polling`.
    pub budget_exhausted: u64,
    /// Number of poll cycles that found zero pending frames.
    pub empty_polls: u64,
    /// Number of dynamic transitions between Interrupt and Polling modes.
    pub mode_switches: u64,
}

/// Thread-safe NAPI-style adaptive network poller state.
pub struct NapiPoller {
    stats: Spinlock<NapiStats>,
}

impl NapiPoller {
    /// Constructs a new adaptive poller starting in `PollMode::Interrupt`.
    pub const fn new(default_budget: usize) -> Self {
        Self {
            stats: Spinlock::new(NapiStats {
                mode: PollMode::Interrupt,
                budget: default_budget,
                poll_cycles: 0,
                packets_polled: 0,
                budget_exhausted: 0,
                empty_polls: 0,
                mode_switches: 0,
            }),
        }
    }

    /// Executes one budgeted poll cycle (draining up to `budget` frames, defaulting to
    /// [`DEFAULT_NAPI_BUDGET`] if `0` is passed).
    ///
    /// Returns the number of packets drained and dispatched during this poll pass.
    pub fn poll(&self, budget: usize) -> usize {
        let effective_budget = if budget == 0 {
            self.stats.lock().budget
        } else {
            budget
        };

        let mut polled_this_cycle = 0usize;

        // 1. Drain up to `effective_budget` frames directly into pooled NetBuf descriptors
        //    starting at offset NET_BUF_HEADROOM (preserving 64B headroom for L2/L3/L4 tools).
        while polled_this_cycle < effective_budget {
            let Some(mut buf) = GLOBAL_NET_POOL.alloc() else {
                break;
            };
            let rx_slice = &mut buf.storage[NET_BUF_HEADROOM..NET_BUF_SIZE];
            match virtio_net::receive(rx_slice) {
                Ok(Some(len)) => {
                    buf.data_start = NET_BUF_HEADROOM;
                    buf.data_len = len;
                    if len >= 14 {
                        GLOBAL_MULTIQUEUE.steer_rx(buf);
                    } else {
                        // Discard runt frame and return buffer immediately
                        GLOBAL_NET_POOL.free(buf);
                    }
                    polled_this_cycle += 1;
                }
                _ => {
                    GLOBAL_NET_POOL.free(buf);
                    break;
                }
            }
        }

        // 2. Drain each active RSS queue in batches and dispatch to the Ethernet layer.
        let num_queues = GLOBAL_MULTIQUEUE.active_queues();
        for qid in 0..num_queues {
            loop {
                let batch = GLOBAL_MULTIQUEUE.pop_rx_batch(qid, effective_budget);
                if batch.is_empty() {
                    break;
                }
                for buf in batch {
                    if buf.data_len >= 14 {
                        ethernet::handle_packet(buf.data());
                    }
                    GLOBAL_NET_POOL.free(buf);
                }
            }
        }

        // 3. Update NAPI adaptive state machine and telemetry counters.
        let mut st = self.stats.lock();
        st.poll_cycles += 1;
        st.packets_polled += polled_this_cycle as u64;

        if polled_this_cycle == 0 {
            st.empty_polls += 1;
        }

        if polled_this_cycle >= effective_budget {
            st.budget_exhausted += 1;
            if st.mode != PollMode::Polling {
                st.mode = PollMode::Polling;
                st.mode_switches += 1;
            }
        } else if st.mode != PollMode::Interrupt {
            st.mode = PollMode::Interrupt;
            st.mode_switches += 1;
        }

        polled_this_cycle
    }

    /// Feeds a synthetic packet directly into the RSS + polling pipeline (useful for loopback
    /// testing and benchmark verification without touching physical VirtIO NIC hardware).
    pub fn inject_and_drain(&self, raw_frame: &[u8]) -> Result<usize, &'static str> {
        let mut buf = GLOBAL_NET_POOL
            .alloc()
            .ok_or("GLOBAL_NET_POOL exhausted during inject_and_drain")?;
        buf.put_payload(raw_frame)?;
        let qid = GLOBAL_MULTIQUEUE.steer_rx(buf);
        let batch = GLOBAL_MULTIQUEUE.pop_rx_batch(qid, 1);
        let count = batch.len();
        for b in batch {
            GLOBAL_NET_POOL.free(b);
        }
        let mut st = self.stats.lock();
        st.packets_polled += count as u64;
        Ok(count)
    }

    /// Sets the default per-cycle packet budget (`>= 1`).
    pub fn set_budget(&self, budget: usize) {
        self.stats.lock().budget = core::cmp::max(1, budget);
    }

    /// Returns a snapshot of NAPI polling statistics.
    pub fn stats(&self) -> NapiStats {
        *self.stats.lock()
    }
}

/// Global NAPI-style Adaptive Poller shared by `net::poll()` and shell diagnostics.
pub static GLOBAL_NAPI_POLLER: NapiPoller = NapiPoller::new(DEFAULT_NAPI_BUDGET);

