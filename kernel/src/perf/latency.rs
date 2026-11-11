//! # Latency Histograms, Lock Contention & CPU Utilization Metrics (Phase 17 - M37)
//!
//! ## Overview
//! Provides lock-free (`AtomicU64`) timing aggregators, 8-bucket power-of-2 histograms,
//! contended spinlock instrumentation ([`ProfiledSpinlock`]), and per-CPU utilization
//! tracking for live kernel observability:
//! - **Scheduler Latency (`SCHED_WAKEUP_LATENCY` & `SCHED_TICK_CYCLES`)**: Tracks wakeup-to-dispatch
//!   delay and tick preemption overhead.
//! - **Syscall Latency (`SYSCALL_LATENCY`)**: Captures every `syscall_dispatcher` invocation duration.
//! - **Interrupt Latency (`IRQ_TIMER_LATENCY` & `IRQ_JITTER`)**: Measures timer ISR execution time and period jitter.
//! - **Lock Contention Measurement (`ProfiledSpinlock`, `LOCK_PROFILES`)**: Quantifies spin iterations,
//!   wait cycles, and critical-section hold duration per lock site.
//! - **CPU Utilization Metrics (`CPU_METRICS`)**: Tracks per-core Active, IRQ, and Idle cycles.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::x86_64::apic;
use crate::perf::tsc;
use crate::sync::{are_interrupts_enabled, disable_interrupts, enable_interrupts};

/// Maximum SMP cores tracked in per-CPU utilization telemetry.
pub const MAX_PROFILED_CPUS: usize = 8;

/// Maximum global lock contention registries displayed in `perfstat`.
pub const MAX_LOCK_SLOTS: usize = 8;

/// Lock-free atomic cycle-latency tracker with min/max/avg and 8 power-of-2 histogram buckets.
pub struct LatencyTracker {
    count: AtomicU64,
    total_cycles: AtomicU64,
    min_cycles: AtomicU64,
    max_cycles: AtomicU64,
    last_cycles: AtomicU64,
    /// Buckets: [0]=<1K, [1]=1K..4K, [2]=4K..16K, [3]=16K..64K, [4]=64K..256K, [5]=256K..1M, [6]=1M..4M, [7]=>=4M cycles
    buckets: [AtomicU64; 8],
}

/// Snapshot view of a [`LatencyTracker`].
#[derive(Debug, Clone, Copy, Default)]
pub struct LatencySnapshot {
    pub count: u64,
    pub total_cycles: u64,
    pub avg_cycles: u64,
    pub min_cycles: u64,
    pub max_cycles: u64,
    pub last_cycles: u64,
    pub avg_ns: u64,
    pub max_ns: u64,
    pub buckets: [u64; 8],
}

impl LatencyTracker {
    pub const fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            total_cycles: AtomicU64::new(0),
            min_cycles: AtomicU64::new(u64::MAX),
            max_cycles: AtomicU64::new(0),
            last_cycles: AtomicU64::new(0),
            buckets: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
        }
    }

    /// Records a single measured cycle duration atomically without locking.
    #[inline]
    pub fn record(&self, cycles: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.total_cycles.fetch_add(cycles, Ordering::Relaxed);
        self.last_cycles.store(cycles, Ordering::Relaxed);

        let _ = self
            .min_cycles
            .fetch_min(cycles, Ordering::Relaxed);
        let _ = self
            .max_cycles
            .fetch_max(cycles, Ordering::Relaxed);

        let bucket_idx = if cycles < 1_024 {
            0
        } else if cycles < 4_096 {
            1
        } else if cycles < 16_384 {
            2
        } else if cycles < 65_536 {
            3
        } else if cycles < 262_144 {
            4
        } else if cycles < 1_048_576 {
            5
        } else if cycles < 4_194_304 {
            6
        } else {
            7
        };
        self.buckets[bucket_idx].fetch_add(1, Ordering::Relaxed);
    }

    /// Captures a point-in-time statistical summary.
    pub fn snapshot(&self) -> LatencySnapshot {
        let count = self.count.load(Ordering::Relaxed);
        let total = self.total_cycles.load(Ordering::Relaxed);
        let raw_min = self.min_cycles.load(Ordering::Relaxed);
        let max = self.max_cycles.load(Ordering::Relaxed);
        let avg = if count > 0 { total / count } else { 0 };
        LatencySnapshot {
            count,
            total_cycles: total,
            avg_cycles: avg,
            min_cycles: if count > 0 { raw_min } else { 0 },
            max_cycles: max,
            last_cycles: self.last_cycles.load(Ordering::Relaxed),
            avg_ns: tsc::cycles_to_ns(avg),
            max_ns: tsc::cycles_to_ns(max),
            buckets: [
                self.buckets[0].load(Ordering::Relaxed),
                self.buckets[1].load(Ordering::Relaxed),
                self.buckets[2].load(Ordering::Relaxed),
                self.buckets[3].load(Ordering::Relaxed),
                self.buckets[4].load(Ordering::Relaxed),
                self.buckets[5].load(Ordering::Relaxed),
                self.buckets[6].load(Ordering::Relaxed),
                self.buckets[7].load(Ordering::Relaxed),
            ],
        }
    }
}

/// Global Scheduler wakeup-to-dispatch latency tracker.
pub static SCHED_WAKEUP_LATENCY: LatencyTracker = LatencyTracker::new();
/// Global Scheduler timer tick processing latency tracker.
pub static SCHED_TICK_LATENCY: LatencyTracker = LatencyTracker::new();
/// Global System Call (`syscall_dispatcher`) execution latency tracker.
pub static SYSCALL_LATENCY: LatencyTracker = LatencyTracker::new();
/// Global Hardware Interrupt (APIC Timer ISR) latency tracker.
pub static IRQ_TIMER_LATENCY: LatencyTracker = LatencyTracker::new();
/// Global Interrupt Period Jitter tracker (cycle drift between consecutive timer ticks).
pub static IRQ_JITTER_LATENCY: LatencyTracker = LatencyTracker::new();

static LAST_TIMER_IRQ_TSC: AtomicU64 = AtomicU64::new(0);
static EXPECTED_TICK_CYCLES: AtomicU64 = AtomicU64::new(0);

/// Records one APIC timer ISR sample (execution cycles + inter-tick arrival jitter).
#[inline]
pub fn record_timer_irq(isr_start_tsc: u64, isr_cycles: u64) {
    IRQ_TIMER_LATENCY.record(isr_cycles);

    let prev = LAST_TIMER_IRQ_TSC.swap(isr_start_tsc, Ordering::Relaxed);
    if prev != 0 {
        let period = isr_start_tsc.saturating_sub(prev);
        let expected = EXPECTED_TICK_CYCLES.load(Ordering::Relaxed);
        if expected == 0 {
            EXPECTED_TICK_CYCLES.store(period, Ordering::Relaxed);
        } else {
            let jitter = period.abs_diff(expected);
            IRQ_JITTER_LATENCY.record(jitter);
        }
    }

    let cpu = (apic::current_apic_id() as usize) % MAX_PROFILED_CPUS;
    CPU_METRICS[cpu].irq_cycles.fetch_add(isr_cycles, Ordering::Relaxed);
    CPU_METRICS[cpu].irq_ticks.fetch_add(1, Ordering::Relaxed);
}

// ============================================================================
// Lock Contention Measurement (ProfiledSpinlock)
// ============================================================================

/// Atomic contention telemetry associated with a named spinlock instance.
pub struct LockProfile {
    pub name: &'static str,
    pub acquires: AtomicU64,
    pub contended_acquires: AtomicU64,
    pub total_spins: AtomicU64,
    pub total_wait_cycles: AtomicU64,
    pub total_hold_cycles: AtomicU64,
    pub max_hold_cycles: AtomicU64,
}

impl LockProfile {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            acquires: AtomicU64::new(0),
            contended_acquires: AtomicU64::new(0),
            total_spins: AtomicU64::new(0),
            total_wait_cycles: AtomicU64::new(0),
            total_hold_cycles: AtomicU64::new(0),
            max_hold_cycles: AtomicU64::new(0),
        }
    }

    pub fn snapshot(&self) -> LockProfileSnapshot {
        LockProfileSnapshot {
            name: self.name,
            acquires: self.acquires.load(Ordering::Relaxed),
            contended_acquires: self.contended_acquires.load(Ordering::Relaxed),
            total_spins: self.total_spins.load(Ordering::Relaxed),
            total_wait_cycles: self.total_wait_cycles.load(Ordering::Relaxed),
            total_hold_cycles: self.total_hold_cycles.load(Ordering::Relaxed),
            max_hold_cycles: self.max_hold_cycles.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LockProfileSnapshot {
    pub name: &'static str,
    pub acquires: u64,
    pub contended_acquires: u64,
    pub total_spins: u64,
    pub total_wait_cycles: u64,
    pub total_hold_cycles: u64,
    pub max_hold_cycles: u64,
}

/// Pre-registered global contention profile slots for core subsystems & self-tests.
pub static GLOBAL_LOCK_PROFILES: [LockProfile; 4] = [
    LockProfile::new("scheduler.rq"),
    LockProfile::new("memory.pmm"),
    LockProfile::new("net.multiqueue"),
    LockProfile::new("perf.test_lock"),
];

/// Interrupt-safe instrumented spinlock measuring lock wait, spin loops, and hold duration.
pub struct ProfiledSpinlock<'p, T> {
    locked: AtomicBool,
    profile: &'p LockProfile,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for ProfiledSpinlock<'_, T> {}
unsafe impl<T: Send> Send for ProfiledSpinlock<'_, T> {}

impl<'p, T> ProfiledSpinlock<'p, T> {
    pub const fn new(profile: &'p LockProfile, data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            profile,
            data: UnsafeCell::new(data),
        }
    }

    /// Acquires the spinlock, recording contention spins and wait latency if already held.
    pub fn lock(&self) -> ProfiledSpinlockGuard<'_, 'p, T> {
        let re_enable = are_interrupts_enabled();
        disable_interrupts();

        let wait_start = tsc::read_tsc();
        let mut spins = 0u64;
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spins += 1;
            core::hint::spin_loop();
        }
        let acquired_tsc = tsc::read_tsc();

        self.profile.acquires.fetch_add(1, Ordering::Relaxed);
        if spins > 0 {
            self.profile.contended_acquires.fetch_add(1, Ordering::Relaxed);
            self.profile.total_spins.fetch_add(spins, Ordering::Relaxed);
            self.profile
                .total_wait_cycles
                .fetch_add(acquired_tsc.saturating_sub(wait_start), Ordering::Relaxed);
        }

        ProfiledSpinlockGuard {
            lock: self,
            acquired_tsc,
            re_enable_interrupts: re_enable,
        }
    }

    /// Simulates contention accounting from a multi-core spin wait (used for contention self-tests).
    pub fn record_synthetic_contention(&self, spins: u64, wait_cycles: u64, hold_cycles: u64) {
        self.profile.acquires.fetch_add(1, Ordering::Relaxed);
        self.profile.contended_acquires.fetch_add(1, Ordering::Relaxed);
        self.profile.total_spins.fetch_add(spins, Ordering::Relaxed);
        self.profile.total_wait_cycles.fetch_add(wait_cycles, Ordering::Relaxed);
        self.profile.total_hold_cycles.fetch_add(hold_cycles, Ordering::Relaxed);
        let _ = self.profile.max_hold_cycles.fetch_max(hold_cycles, Ordering::Relaxed);
    }
}

pub struct ProfiledSpinlockGuard<'a, 'p, T> {
    lock: &'a ProfiledSpinlock<'p, T>,
    acquired_tsc: u64,
    re_enable_interrupts: bool,
}

impl<T> Deref for ProfiledSpinlockGuard<'_, '_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for ProfiledSpinlockGuard<'_, '_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for ProfiledSpinlockGuard<'_, '_, T> {
    fn drop(&mut self) {
        let hold = tsc::read_tsc().saturating_sub(self.acquired_tsc);
        self.lock.profile.total_hold_cycles.fetch_add(hold, Ordering::Relaxed);
        let _ = self.lock.profile.max_hold_cycles.fetch_max(hold, Ordering::Relaxed);
        self.lock.locked.store(false, Ordering::Release);
        if self.re_enable_interrupts {
            enable_interrupts();
        }
    }
}

// ============================================================================
// Per-CPU Utilization Metrics
// ============================================================================

/// Atomic cycle accounting per logical CPU core.
pub struct CpuUtilCounters {
    pub active_cycles: AtomicU64,
    pub irq_cycles: AtomicU64,
    pub idle_cycles: AtomicU64,
    pub irq_ticks: AtomicU64,
}

impl CpuUtilCounters {
    pub const fn new() -> Self {
        Self {
            active_cycles: AtomicU64::new(0),
            irq_cycles: AtomicU64::new(0),
            idle_cycles: AtomicU64::new(0),
            irq_ticks: AtomicU64::new(0),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CpuUtilSnapshot {
    pub cpu_id: usize,
    pub active_cycles: u64,
    pub irq_cycles: u64,
    pub idle_cycles: u64,
    pub irq_ticks: u64,
    /// Utilization in tenths of a percent (`0..=1000`, e.g., `875` = `87.5%`).
    pub util_tenths_pct: u64,
}

pub static CPU_METRICS: [CpuUtilCounters; MAX_PROFILED_CPUS] = [
    CpuUtilCounters::new(),
    CpuUtilCounters::new(),
    CpuUtilCounters::new(),
    CpuUtilCounters::new(),
    CpuUtilCounters::new(),
    CpuUtilCounters::new(),
    CpuUtilCounters::new(),
    CpuUtilCounters::new(),
];

/// Records active kernel execution cycles on the current CPU core.
#[inline]
pub fn account_cpu_active(cycles: u64) {
    let cpu = (apic::current_apic_id() as usize) % MAX_PROFILED_CPUS;
    CPU_METRICS[cpu].active_cycles.fetch_add(cycles, Ordering::Relaxed);
}

/// Records low-power idle (`hlt` / shell wait) cycles on the current CPU core.
#[inline]
pub fn account_cpu_idle(cycles: u64) {
    let cpu = (apic::current_apic_id() as usize) % MAX_PROFILED_CPUS;
    CPU_METRICS[cpu].idle_cycles.fetch_add(cycles, Ordering::Relaxed);
}

/// Returns CPU utilization snapshot for logical core `cpu_id`.
pub fn cpu_util_snapshot(cpu_id: usize) -> CpuUtilSnapshot {
    let idx = cpu_id % MAX_PROFILED_CPUS;
    let active = CPU_METRICS[idx].active_cycles.load(Ordering::Relaxed);
    let irq = CPU_METRICS[idx].irq_cycles.load(Ordering::Relaxed);
    let idle = CPU_METRICS[idx].idle_cycles.load(Ordering::Relaxed);
    let ticks = CPU_METRICS[idx].irq_ticks.load(Ordering::Relaxed);
    let busy = active.saturating_add(irq);
    let total = busy.saturating_add(idle);
    let util_tenths_pct = if total > 0 {
        core::cmp::min(1000, busy.saturating_mul(1000) / total)
    } else if ticks > 0 {
        1000 // Active BSP executing kernel work
    } else {
        0
    };
    CpuUtilSnapshot {
        cpu_id: idx,
        active_cycles: active,
        irq_cycles: irq,
        idle_cycles: idle,
        irq_ticks: ticks,
        util_tenths_pct,
    }
}

