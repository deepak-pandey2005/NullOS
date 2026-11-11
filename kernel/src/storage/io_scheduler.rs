//! # Block I/O Request Scheduler (Phase 15 - M35)
//!
//! ## Overview
//! Orders, merges, and dispatches pending block storage requests to minimize seek latency,
//! maximize sequential throughput, and prevent request starvation.
//!
//! ### Supported Scheduling Policies
//! 1. **FIFO (`Fifo`)**: Strictly first-in, first-out queue dispatch.
//! 2. **Elevator SCAN / C-LOOK (`ElevatorScan`)**: Sorts pending I/O requests by ascending
//!    Logical Block Address (LBA) starting from the current disk head position, sweeping
//!    monotonically upwards and wrapping back to the lowest pending LBA.
//! 3. **Deadline (`Deadline`)**: Assigns each incoming request an expiration tick deadline
//!    (shorter for Reads than Writes). If any request expires, it is dispatched immediately
//!    to prevent read starvation; otherwise defaults to Elevator SCAN ordering.
//!
//! ### Request Coalescing
//! Contiguous requests targeting adjacent sectors with the same operation type (Read + Read
//! or Write + Write) are eligible for merging into a single larger multi-sector transfer.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use crate::sync::Spinlock;

/// Direction/type of an I/O request in the scheduler queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoRequestOp {
    Read,
    Write,
    Flush,
}

/// Active I/O scheduling algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoSchedulingPolicy {
    Fifo,
    ElevatorScan,
    Deadline,
}

/// Queued block I/O request item scheduled for hardware dispatch.
#[derive(Debug, Clone)]
pub struct IoRequest {
    pub id: u64,
    pub op: IoRequestOp,
    pub sector: u64,
    pub sector_count: usize,
    pub data: Vec<u8>,
    pub enqueue_tick: u64,
    pub deadline_tick: u64,
}

/// Telemetry counters for the I/O scheduler.
#[derive(Debug, Clone, Copy, Default)]
pub struct IoSchedulerStats {
    pub policy: u8,
    pub queue_depth: usize,
    pub total_enqueued: u64,
    pub total_dispatched: u64,
    pub total_merged: u64,
    pub deadline_overrides: u64,
    pub head_sector: u64,
}

struct IoSchedulerInner {
    policy: IoSchedulingPolicy,
    queue: VecDeque<IoRequest>,
    head_sector: u64,
    next_id: u64,
    total_enqueued: u64,
    total_dispatched: u64,
    total_merged: u64,
    deadline_overrides: u64,
}

/// Thread-safe I/O Request Scheduler supporting FIFO, C-LOOK Elevator SCAN, and Deadline.
pub struct IoScheduler {
    inner: Spinlock<IoSchedulerInner>,
}

impl IoScheduler {
    /// Creates a new I/O scheduler with the specified initial scheduling policy.
    pub const fn new(policy: IoSchedulingPolicy) -> Self {
        Self {
            inner: Spinlock::new(IoSchedulerInner {
                policy,
                queue: VecDeque::new(),
                head_sector: 0,
                next_id: 1,
                total_enqueued: 0,
                total_dispatched: 0,
                total_merged: 0,
                deadline_overrides: 0,
            }),
        }
    }

    /// Switches the active scheduling algorithm (`Fifo`, `ElevatorScan`, or `Deadline`).
    pub fn set_policy(&self, policy: IoSchedulingPolicy) {
        self.inner.lock().policy = policy;
    }

    /// Returns the currently active scheduling policy.
    pub fn policy(&self) -> IoSchedulingPolicy {
        self.inner.lock().policy
    }

    /// Enqueues a new block I/O request, attempting contiguous merge with tail/matching entries.
    ///
    /// Returns the assigned request ID.
    pub fn enqueue(
        &self,
        op: IoRequestOp,
        sector: u64,
        sector_count: usize,
        data: Vec<u8>,
        current_tick: u64,
    ) -> u64 {
        let mut guard = self.inner.lock();
        guard.total_enqueued += 1;

        // Attempt back-merging with an existing contiguous request (Read+Read or Write+Write)
        if op != IoRequestOp::Flush && sector_count > 0 {
            let mut merged_id: Option<u64> = None;
            for req in guard.queue.iter_mut() {
                if req.op == op && req.sector + (req.sector_count as u64) == sector {
                    // Cap merged transfers at 64 sectors (32 KiB)
                    if req.sector_count + sector_count <= 64 {
                        req.sector_count += sector_count;
                        if op == IoRequestOp::Write {
                            req.data.extend_from_slice(&data);
                        }
                        merged_id = Some(req.id);
                        break;
                    }
                }
            }
            if let Some(id) = merged_id {
                guard.total_merged += 1;
                return id;
            }
        }

        let id = guard.next_id;
        guard.next_id += 1;

        // Read requests receive tighter deadlines (50 ticks = ~500ms) than Writes (200 ticks = ~2s)
        let deadline_offset = match op {
            IoRequestOp::Read => 50,
            IoRequestOp::Write => 200,
            IoRequestOp::Flush => 25,
        };

        guard.queue.push_back(IoRequest {
            id,
            op,
            sector,
            sector_count,
            data,
            enqueue_tick: current_tick,
            deadline_tick: current_tick.saturating_add(deadline_offset),
        });
        id
    }

    /// Selects and dequeues the next optimal [`IoRequest`] according to the active policy.
    pub fn dequeue_next(&self, current_tick: u64) -> Option<IoRequest> {
        let mut guard = self.inner.lock();
        if guard.queue.is_empty() {
            return None;
        }

        let idx = match guard.policy {
            IoSchedulingPolicy::Fifo => 0,
            IoSchedulingPolicy::ElevatorScan => Self::pick_elevator(&guard.queue, guard.head_sector),
            IoSchedulingPolicy::Deadline => {
                // Check if any request has reached or passed its deadline (prioritizing earliest expired)
                let mut expired_idx: Option<usize> = None;
                let mut earliest_deadline = u64::MAX;
                for (i, req) in guard.queue.iter().enumerate() {
                    if current_tick >= req.deadline_tick && req.deadline_tick < earliest_deadline {
                        earliest_deadline = req.deadline_tick;
                        expired_idx = Some(i);
                    }
                }

                if let Some(ei) = expired_idx {
                    guard.deadline_overrides += 1;
                    ei
                } else {
                    Self::pick_elevator(&guard.queue, guard.head_sector)
                }
            }
        };

        let req = guard.queue.remove(idx)?;
        guard.head_sector = req.sector.saturating_add(req.sector_count as u64);
        guard.total_dispatched += 1;
        Some(req)
    }

    /// Drains all pending requests in scheduled dispatch order.
    pub fn drain_scheduled(&self, current_tick: u64) -> Vec<IoRequest> {
        let mut out = Vec::new();
        while let Some(req) = self.dequeue_next(current_tick) {
            out.push(req);
        }
        out
    }

    /// Returns the number of requests currently waiting in the queue.
    pub fn pending_count(&self) -> usize {
        self.inner.lock().queue.len()
    }

    /// Returns telemetry statistics for the I/O scheduler.
    pub fn stats(&self) -> IoSchedulerStats {
        let guard = self.inner.lock();
        let policy_code = match guard.policy {
            IoSchedulingPolicy::Fifo => 0,
            IoSchedulingPolicy::ElevatorScan => 1,
            IoSchedulingPolicy::Deadline => 2,
        };
        IoSchedulerStats {
            policy: policy_code,
            queue_depth: guard.queue.len(),
            total_enqueued: guard.total_enqueued,
            total_dispatched: guard.total_dispatched,
            total_merged: guard.total_merged,
            deadline_overrides: guard.deadline_overrides,
            head_sector: guard.head_sector,
        }
    }

    /// C-LOOK / Elevator SCAN index selector:
    /// Picks the closest sector >= `head_sector`; if none exist ahead of the head,
    /// wraps around to the lowest sector in the queue.
    fn pick_elevator(queue: &VecDeque<IoRequest>, head_sector: u64) -> usize {
        let mut best_ahead_idx: Option<usize> = None;
        let mut best_ahead_sector = u64::MAX;

        let mut lowest_idx = 0usize;
        let mut lowest_sector = u64::MAX;

        for (i, req) in queue.iter().enumerate() {
            if req.sector >= head_sector && req.sector < best_ahead_sector {
                best_ahead_sector = req.sector;
                best_ahead_idx = Some(i);
            }
            if req.sector < lowest_sector {
                lowest_sector = req.sector;
                lowest_idx = i;
            }
        }

        best_ahead_idx.unwrap_or(lowest_idx)
    }
}

/// Global I/O Scheduler instance defaulting to `Deadline` (Elevator SCAN + anti-starvation).
pub static GLOBAL_IO_SCHEDULER: IoScheduler = IoScheduler::new(IoSchedulingPolicy::Deadline);
