//! # Kernel Trace Buffer Observability Bridge (Phase 18 - M40)
//!
//! ## Overview
//! Extends the static lock-free M37 [`crate::perf::trace`] ring buffer (`512` events)
//! with subsystem filtering, ring utilization metrics, and inline address symbolization
//! for kernel debugging and interactive `dbg trace` inspection.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::perf::trace::{self, TraceCategory, TraceEvent};
use crate::perf::tsc;

/// Parses a subsystem filter token (`sched`, `syscall`, `irq`, `mem`, `lock`, `net`, `stor`, `user`).
pub fn parse_category(name: &str) -> Option<TraceCategory> {
    if name.eq_ignore_ascii_case("sched") {
        Some(TraceCategory::Sched)
    } else if name.eq_ignore_ascii_case("syscall") {
        Some(TraceCategory::Syscall)
    } else if name.eq_ignore_ascii_case("irq") || name.eq_ignore_ascii_case("interrupt") {
        Some(TraceCategory::Interrupt)
    } else if name.eq_ignore_ascii_case("mem") || name.eq_ignore_ascii_case("memory") {
        Some(TraceCategory::Memory)
    } else if name.eq_ignore_ascii_case("lock") {
        Some(TraceCategory::Lock)
    } else if name.eq_ignore_ascii_case("net") {
        Some(TraceCategory::Net)
    } else if name.eq_ignore_ascii_case("stor") || name.eq_ignore_ascii_case("storage") {
        Some(TraceCategory::Storage)
    } else if name.eq_ignore_ascii_case("user") {
        Some(TraceCategory::User)
    } else {
        None
    }
}

/// Returns up to `limit` filtered trace events from the global M37 ring buffer.
pub fn filtered_events(cat: Option<TraceCategory>, limit: usize) -> Vec<TraceEvent> {
    let all = trace::recent_events(trace::TRACE_RING_CAPACITY);
    let mut matching: Vec<TraceEvent> = all
        .into_iter()
        .filter(|ev| cat.map_or(true, |c| ev.category == c))
        .collect();
    if matching.len() > limit {
        matching.drain(0..(matching.len() - limit));
    }
    matching
}

/// Emits a debug observability marker into the kernel trace ring.
#[inline]
pub fn emit_debug_trace(event_id: u16, arg0: u64, arg1: u64) {
    trace::record(TraceCategory::User, event_id, 0, arg0, arg1);
}

/// Renders filtered trace buffer events along with capacity and overflow telemetry.
pub fn format_trace_report(cat: Option<TraceCategory>, limit: usize) -> String {
    let st = trace::stats();
    let boot = tsc::info().boot_tsc;
    let events = filtered_events(cat, limit);

    let mut out = format!(
        "=== Trace Buffer Observability (M40) ===\n  Enabled={} | Mask={:#04x} | Buffered={}/{} | Total={} | Dropped={}\n",
        st.enabled,
        st.category_mask,
        st.buffered_events,
        st.capacity,
        st.total_recorded,
        st.dropped_filtered
    );

    if events.is_empty() {
        out.push_str("  (no matching trace records)\n");
        return out;
    }

    for ev in events {
        let rel_us = tsc::cycles_to_us(ev.timestamp_tsc.saturating_sub(boot));
        let sym_note = if super::sym::is_kernel_addr(ev.arg0) {
            super::sym::resolve_addr(ev.arg0)
                .map(|r| format!(" ({})", r.format_compact()))
                .unwrap_or_default()
        } else {
            String::new()
        };
        out.push_str(&format!(
            "  [{:>8} us] CPU#{} {:<7} ev={:<3} tid={:<3} arg0={:#x}{} arg1={}\n",
            rel_us,
            ev.cpu_id,
            ev.category.as_str(),
            ev.event_id,
            ev.tid,
            ev.arg0,
            sym_note,
            ev.arg1
        ));
    }

    out
}

