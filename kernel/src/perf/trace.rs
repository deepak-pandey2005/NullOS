//! # Kernel Event Tracing & Static Tracepoints (Phase 17 - M37)
//!
//! ## Overview
//! Provides an IRQ-safe, heap-free static circular ring buffer (`TRACE_RING_CAPACITY = 512`)
//! capturing high-resolution (`RDTSC`) kernel tracepoints across subsystems:
//! - Scheduler wakeups & preemptive context switches (`TraceCategory::Sched`)
//! - System call dispatch & return latencies (`TraceCategory::Syscall`)
//! - Hardware interrupts & timer ticks (`TraceCategory::Interrupt`)
//! - Memory frame allocations / page faults (`TraceCategory::Memory`)
//! - Spinlock contention events (`TraceCategory::Lock`)
//! - Network & Storage I/O completions (`TraceCategory::Net`, `TraceCategory::Storage`)
//!
//! Because entries live inside a fixed static array protected by an interrupt-disabling
//! [`Spinlock`], tracepoints never allocate memory and can be invoked safely inside
//! timer ISRs, syscall dispatchers, and SMP boot paths.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::arch::x86_64::apic;
use crate::perf::tsc;
use crate::println;
use crate::sync::Spinlock;

/// Capacity of the circular kernel trace buffer (power of 2 for fast wraparound).
pub const TRACE_RING_CAPACITY: usize = 512;

/// Well-known kernel tracepoint subsystem categories.
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceCategory {
    Sched = 0,
    Syscall = 1,
    Interrupt = 2,
    Memory = 3,
    Lock = 4,
    Net = 5,
    Storage = 6,
    User = 7,
}

impl TraceCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sched => "SCHED",
            Self::Syscall => "SYSCALL",
            Self::Interrupt => "IRQ",
            Self::Memory => "MEM",
            Self::Lock => "LOCK",
            Self::Net => "NET",
            Self::Storage => "STOR",
            Self::User => "USER",
        }
    }

    pub const fn bit(self) -> u32 {
        1u32 << (self as u16)
    }
}

/// Well-known static tracepoint event IDs.
pub mod event_id {
    pub const SCHED_TICK: u16 = 1;
    pub const SCHED_SWITCH: u16 = 2;
    pub const SCHED_WAKEUP: u16 = 3;
    pub const SYSCALL_EXIT: u16 = 10;
    pub const IRQ_TIMER: u16 = 20;
    pub const MEM_ALLOC: u16 = 30;
    pub const MEM_FREE: u16 = 31;
    pub const LOCK_CONTENDED: u16 = 40;
    pub const USER_MARK: u16 = 100;
}

/// Single fixed-size 32-byte timestamped kernel tracepoint record.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TraceEvent {
    /// Raw TSC timestamp when the tracepoint fired.
    pub timestamp_tsc: u64,
    /// Primary subsystem category (`Sched`, `Syscall`, etc.).
    pub category: TraceCategory,
    /// Event identifier within the category.
    pub event_id: u16,
    /// Logical CPU / LAPIC ID on which the event fired.
    pub cpu_id: u16,
    /// Associated thread ID (or `0` for boot/idle/ISR).
    pub tid: u16,
    /// First event-specific payload argument (e.g., syscall number, duration cycles).
    pub arg0: u64,
    /// Second event-specific payload argument (e.g., next TID, latency cycles).
    pub arg1: u64,
}

impl TraceEvent {
    pub const fn empty() -> Self {
        Self {
            timestamp_tsc: 0,
            category: TraceCategory::Sched,
            event_id: 0,
            cpu_id: 0,
            tid: 0,
            arg0: 0,
            arg1: 0,
        }
    }
}

struct TraceRing {
    write_idx: usize,
    total_recorded: u64,
    dropped_filtered: u64,
    events: [TraceEvent; TRACE_RING_CAPACITY],
}

impl TraceRing {
    const fn new() -> Self {
        Self {
            write_idx: 0,
            total_recorded: 0,
            dropped_filtered: 0,
            events: [TraceEvent::empty(); TRACE_RING_CAPACITY],
        }
    }
}

/// Global tracing summary telemetry.
#[derive(Debug, Clone, Copy)]
pub struct TraceStats {
    pub enabled: bool,
    pub category_mask: u32,
    pub capacity: usize,
    pub total_recorded: u64,
    pub buffered_events: usize,
    pub dropped_filtered: u64,
}

static TRACE_ENABLED: AtomicBool = AtomicBool::new(true);
static TRACE_CATEGORY_MASK: AtomicU32 = AtomicU32::new(0xFF);
static TRACE_DROP_COUNTER: AtomicU64 = AtomicU64::new(0);
static TRACE_RING: Spinlock<TraceRing> = Spinlock::new(TraceRing::new());

/// Enables or disables all kernel tracepoint collection globally.
pub fn set_enabled(enabled: bool) {
    TRACE_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Sets the bitmask of enabled [`TraceCategory`] flags.
pub fn set_category_mask(mask: u32) {
    TRACE_CATEGORY_MASK.store(mask, Ordering::Relaxed);
}

/// Clears all recorded trace events from the ring buffer.
pub fn clear() {
    let mut ring = TRACE_RING.lock();
    ring.write_idx = 0;
    ring.total_recorded = 0;
    ring.dropped_filtered = 0;
    TRACE_DROP_COUNTER.store(0, Ordering::Relaxed);
}

/// Records a static kernel tracepoint event into the circular ring buffer.
#[inline]
pub fn record(category: TraceCategory, event_id: u16, tid: u16, arg0: u64, arg1: u64) {
    if !TRACE_ENABLED.load(Ordering::Relaxed)
        || (TRACE_CATEGORY_MASK.load(Ordering::Relaxed) & category.bit()) == 0
    {
        TRACE_DROP_COUNTER.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let ev = TraceEvent {
        timestamp_tsc: tsc::read_tsc(),
        category,
        event_id,
        cpu_id: apic::current_apic_id() as u16,
        tid,
        arg0,
        arg1,
    };

    let mut ring = TRACE_RING.lock();
    let idx = ring.write_idx & (TRACE_RING_CAPACITY - 1);
    ring.events[idx] = ev;
    ring.write_idx = ring.write_idx.wrapping_add(1);
    ring.total_recorded += 1;
}

/// Ergonomic tracepoint emission macro (`tracepoint!(TraceCategory::Sched, event_id, tid, a0, a1)`).
#[macro_export]
macro_rules! tracepoint {
    ($cat:expr, $id:expr, $tid:expr, $arg0:expr, $arg1:expr) => {
        $crate::perf::trace::record($cat, $id, $tid, $arg0, $arg1)
    };
}

/// Returns up to `max_count` most recent captured trace events in chronological order.
pub fn recent_events(max_count: usize) -> Vec<TraceEvent> {
    let ring = TRACE_RING.lock();
    let available = core::cmp::min(ring.total_recorded as usize, TRACE_RING_CAPACITY);
    let count = core::cmp::min(max_count, available);
    let mut out = Vec::with_capacity(count);
    let start = ring.write_idx.wrapping_sub(count);
    for i in 0..count {
        let idx = start.wrapping_add(i) & (TRACE_RING_CAPACITY - 1);
        out.push(ring.events[idx]);
    }
    out
}

/// Returns current circular trace buffer telemetry.
pub fn stats() -> TraceStats {
    let ring = TRACE_RING.lock();
    TraceStats {
        enabled: TRACE_ENABLED.load(Ordering::Relaxed),
        category_mask: TRACE_CATEGORY_MASK.load(Ordering::Relaxed),
        capacity: TRACE_RING_CAPACITY,
        total_recorded: ring.total_recorded,
        buffered_events: core::cmp::min(ring.total_recorded as usize, TRACE_RING_CAPACITY),
        dropped_filtered: TRACE_DROP_COUNTER.load(Ordering::Relaxed),
    }
}

/// Prints the most recent `limit` tracepoints cleanly to the kernel console (`trace`).
pub fn dump_recent(limit: usize) {
    let st = stats();
    let events = recent_events(limit);
    let boot = tsc::info().boot_tsc;
    println!(
        "=== Kernel Trace Buffer (recorded={}, buffered={}/{}) ===",
        st.total_recorded, st.buffered_events, st.capacity
    );
    for ev in events {
        let rel_us = tsc::cycles_to_us(ev.timestamp_tsc.saturating_sub(boot));
        println!(
            "  [{:>8} us] CPU#{} {:<7} ev={:<3} tid={:<3} arg0={:#x} arg1={}",
            rel_us,
            ev.cpu_id,
            ev.category.as_str(),
            ev.event_id,
            ev.tid,
            ev.arg0,
            ev.arg1
        );
    }
}

