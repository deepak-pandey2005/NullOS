//! # Scheduler Optimization: O(1) Priority Bitmap & Fairness (Phase 17 - M38)
//!
//! ## Overview
//! Replaces $O(N)$ linear runqueue scanning with an $O(1)$ hardware-accelerated **Priority
//! Bitmap RunQueue** modeled after Linux O(1) / modern CFS work-conserving queues:
//! - Uses a single `u64` active priority bitmask covering 64 priority classes (`0` = highest,
//!   `63` = lowest idle class).
//! - Picking the highest-priority runnable task uses `bitmap.trailing_zeros()` (compiles to
//!   x86_64 `TZCNT`/`BSF`), executing in deterministic **constant $O(1)$ time** regardless of
//!   how many queued threads exist.
//! - Tracks per-task virtual runtime (`vruntime`) within equal priority buckets for fairness.

use alloc::collections::VecDeque;

/// Total distinct scheduling priority classes tracked by the O(1) bitmap (`0..63`).
pub const NUM_PRIORITY_LEVELS: usize = 64;

/// Lightweight descriptor stored in [`O1PriorityRunQueue`] for scheduling & load balancing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedTaskEntry {
    pub tid: u64,
    /// Priority class (`0` = Real-Time/Highest, `63` = Idle).
    pub priority: u8,
    /// Accumulated virtual runtime nanoseconds for intra-priority fairness.
    pub vruntime_ns: u64,
    /// Preferred CPU affinity (`u16::MAX` = any core).
    pub cpu_affinity: u16,
}

/// Constant-time $O(1)$ priority runqueue using a 64-bit hardware bitmask.
pub struct O1PriorityRunQueue {
    /// Bit `p` is set iff `queues[p]` is non-empty (`u64::trailing_zeros()` selects min `p`).
    bitmap: u64,
    queues: [VecDeque<SchedTaskEntry>; NUM_PRIORITY_LEVELS],
    total_tasks: usize,
    /// Telemetry: tasks stolen / migrated during work-stealing balance.
    pub migrations: u64,
}

impl O1PriorityRunQueue {
    /// Creates an empty O(1) priority runqueue across 64 priority buckets.
    pub fn new() -> Self {
        const EMPTY: VecDeque<SchedTaskEntry> = VecDeque::new();
        Self {
            bitmap: 0,
            queues: [EMPTY; NUM_PRIORITY_LEVELS],
            total_tasks: 0,
            migrations: 0,
        }
    }

    /// Enqueues a runnable task in $O(1)$ time and marks its priority bit active.
    #[inline]
    pub fn push(&mut self, mut task: SchedTaskEntry) {
        let prio = (task.priority as usize).min(NUM_PRIORITY_LEVELS - 1);
        task.priority = prio as u8;
        self.queues[prio].push_back(task);
        self.bitmap |= 1u64 << prio;
        self.total_tasks += 1;
    }

    /// Selects and pops the highest-priority runnable task in **strict $O(1)$ time** via `tzcnt`/`bsf`.
    #[inline]
    pub fn pop_highest(&mut self) -> Option<SchedTaskEntry> {
        if self.bitmap == 0 {
            return None;
        }
        let highest_prio = self.bitmap.trailing_zeros() as usize;
        let task = self.queues[highest_prio].pop_front();
        if self.queues[highest_prio].is_empty() {
            self.bitmap &= !(1u64 << highest_prio);
        }
        if task.is_some() {
            self.total_tasks -= 1;
        }
        task
    }

    /// Updates a task's virtual runtime after consuming `delta_ns` weighted by priority niceness.
    #[inline]
    pub fn account_vruntime(task: &mut SchedTaskEntry, delta_ns: u64) {
        // Higher priority (smaller number) accumulates vruntime more slowly
        let weight = core::cmp::max(1, (task.priority as u64) + 1);
        task.vruntime_ns = task.vruntime_ns.saturating_add(delta_ns.saturating_mul(weight) / 32);
    }

    /// Work-stealing load balancer: moves one lowest-priority migratable task from a busy `src`
    /// queue to an idle `dst` queue to maximize multi-core CPU utilization.
    pub fn steal_for_idle_cpu(src: &mut O1PriorityRunQueue, dst: &mut O1PriorityRunQueue) -> bool {
        if src.total_tasks <= 1 {
            return false;
        }
        // Steal from lowest priority active queue (`63 - leading_zeros`) so highest-prio
        // cache-warm local tasks stay on their home core.
        let lowest_prio = 63 - (src.bitmap.leading_zeros() as usize);
        if let Some(stolen) = src.queues[lowest_prio].pop_back() {
            if src.queues[lowest_prio].is_empty() {
                src.bitmap &= !(1u64 << lowest_prio);
            }
            src.total_tasks -= 1;
            dst.push(stolen);
            dst.migrations += 1;
            return true;
        }
        false
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.total_tasks
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.total_tasks == 0
    }

    #[inline]
    pub fn active_priority_mask(&self) -> u64 {
        self.bitmap
    }
}

