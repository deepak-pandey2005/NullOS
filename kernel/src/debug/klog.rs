//! # Kernel Logs & Structured Logging Subsystem (Phase 18 - M40)
//!
//! ## Overview
//! Provides an allocation-free circular structured kernel log ring (`KLOG_CAPACITY = 256`)
//! complementing the raw text [`crate::drivers::console`] `dmesg` buffer:
//! - **Severity Levels**: `ERROR`, `WARN`, `INFO`, `DEBUG`, `TRACE` with live runtime threshold filtering.
//! - **Structured Metadata**: Every [`LogEntry`] records sequence number, monotonic millisecond timestamp
//!   (via TSC calibration), LAPIC CPU ID, subsystem tag, inline message text, and up to 4 typed
//!   key-value fields (`Int`, `Hex`, `Str`).
//! - **Dual Rendering**: Supports concise syslog-style text rendering (`format_text`) as well as
//!   machine-parseable JSON Lines rendering (`format_json`) via `dbg klog --json`.
//! - **`dmesg` Synchronization**: Mirrors recorded structured entries into `record_dmesg` so `dmesg`
//!   and structured log pipelines remain consistent.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

use crate::arch::x86_64::apic;
use crate::perf::tsc;
use crate::sync::Spinlock;

/// Maximum number of structured entries stored in the circular kernel log ring.
pub const KLOG_CAPACITY: usize = 256;
/// Maximum inline message byte length stored per [`LogEntry`].
pub const MAX_LOG_MSG_LEN: usize = 96;
/// Maximum typed key-value fields attached to a single structured log record.
pub const MAX_LOG_FIELDS: usize = 4;

/// Structured log severity levels ordered from highest severity (`Error = 1`) to lowest (`Trace = 5`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

impl LogLevel {
    /// Returns the uppercase 5-char / short label for this log severity.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
            Self::Trace => "TRACE",
        }
    }

    /// Parses a log level string (`error`, `warn`, `info`, `debug`, `trace`), case-insensitively.
    pub fn parse(s: &str) -> Option<Self> {
        if s.eq_ignore_ascii_case("error") || s.eq_ignore_ascii_case("err") {
            Some(Self::Error)
        } else if s.eq_ignore_ascii_case("warn") || s.eq_ignore_ascii_case("warning") {
            Some(Self::Warn)
        } else if s.eq_ignore_ascii_case("info") {
            Some(Self::Info)
        } else if s.eq_ignore_ascii_case("debug") || s.eq_ignore_ascii_case("dbg") {
            Some(Self::Debug)
        } else if s.eq_ignore_ascii_case("trace") {
            Some(Self::Trace)
        } else {
            None
        }
    }
}

/// Typed structured log attribute value (`no_std`, Copy, zero-allocation).
#[derive(Debug, Clone, Copy)]
pub enum LogValue {
    None,
    Int(i64),
    Hex(u64),
    Str(&'static str),
}

/// Key-value field attached to a structured kernel log record.
#[derive(Debug, Clone, Copy)]
pub struct LogField {
    pub key: &'static str,
    pub val: LogValue,
}

impl LogField {
    pub const fn empty() -> Self {
        Self {
            key: "",
            val: LogValue::None,
        }
    }

    #[inline]
    pub const fn int(key: &'static str, v: i64) -> Self {
        Self {
            key,
            val: LogValue::Int(v),
        }
    }

    #[inline]
    pub const fn hex(key: &'static str, v: u64) -> Self {
        Self {
            key,
            val: LogValue::Hex(v),
        }
    }

    #[inline]
    pub const fn str(key: &'static str, v: &'static str) -> Self {
        Self {
            key,
            val: LogValue::Str(v),
        }
    }
}

/// Fixed-size zero-allocation structured kernel log entry.
#[derive(Clone, Copy)]
pub struct LogEntry {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub tsc_cycles: u64,
    pub level: LogLevel,
    pub cpu_id: u16,
    pub subsystem: &'static str,
    pub msg_len: u8,
    pub msg_buf: [u8; MAX_LOG_MSG_LEN],
    pub field_count: u8,
    pub fields: [LogField; MAX_LOG_FIELDS],
}

impl LogEntry {
    const fn empty() -> Self {
        Self {
            seq: 0,
            timestamp_ms: 0,
            tsc_cycles: 0,
            level: LogLevel::Info,
            cpu_id: 0,
            subsystem: "",
            msg_len: 0,
            msg_buf: [0; MAX_LOG_MSG_LEN],
            field_count: 0,
            fields: [LogField::empty(); MAX_LOG_FIELDS],
        }
    }

    /// Returns the UTF-8 message slice stored inside this fixed entry.
    pub fn message(&self) -> &str {
        core::str::from_utf8(&self.msg_buf[..self.msg_len as usize]).unwrap_or("")
    }

    /// Formats the structured log record as human-readable text.
    pub fn format_text(&self) -> String {
        let mut out = format!(
            "[{:>6}.{:03}s][cpu{}][{:<5}][{}] {}",
            self.timestamp_ms / 1000,
            self.timestamp_ms % 1000,
            self.cpu_id,
            self.level.as_str(),
            self.subsystem,
            self.message()
        );
        for i in 0..(self.field_count as usize).min(MAX_LOG_FIELDS) {
            let f = &self.fields[i];
            if f.key.is_empty() {
                continue;
            }
            match f.val {
                LogValue::None => {}
                LogValue::Int(v) => out.push_str(&format!(" {}={}", f.key, v)),
                LogValue::Hex(v) => out.push_str(&format!(" {}={:#x}", f.key, v)),
                LogValue::Str(s) => out.push_str(&format!(" {}=\"{}\"", f.key, s)),
            }
        }
        out
    }

    /// Formats the structured log record as a JSON object string (JSON Lines compatible).
    pub fn format_json(&self) -> String {
        let mut fields_json = String::new();
        let mut first = true;
        for i in 0..(self.field_count as usize).min(MAX_LOG_FIELDS) {
            let f = &self.fields[i];
            if f.key.is_empty() {
                continue;
            }
            if !first {
                fields_json.push(',');
            }
            first = false;
            match f.val {
                LogValue::None => {}
                LogValue::Int(v) => fields_json.push_str(&format!("\"{}\":{}", f.key, v)),
                LogValue::Hex(v) => fields_json.push_str(&format!("\"{}\":\"{:#x}\"", f.key, v)),
                LogValue::Str(s) => fields_json.push_str(&format!("\"{}\":\"{}\"", f.key, s)),
            }
        }
        format!(
            "{{\"seq\":{},\"ts_ms\":{},\"cpu\":{},\"lvl\":\"{}\",\"sub\":\"{}\",\"msg\":\"{}\",\"fields\":{{{}}}}}",
            self.seq,
            self.timestamp_ms,
            self.cpu_id,
            self.level.as_str(),
            self.subsystem,
            self.message(),
            fields_json
        )
    }
}

struct KlogRing {
    write_idx: usize,
    total_recorded: u64,
    dropped_filtered: u64,
    entries: [LogEntry; KLOG_CAPACITY],
}

impl KlogRing {
    const fn new() -> Self {
        Self {
            write_idx: 0,
            total_recorded: 0,
            dropped_filtered: 0,
            entries: [LogEntry::empty(); KLOG_CAPACITY],
        }
    }
}

static KLOG_MIN_LEVEL: AtomicU8 = AtomicU8::new(LogLevel::Debug as u8);
static KLOG_SEQ: AtomicU64 = AtomicU64::new(1);
static KLOG_RING: Spinlock<KlogRing> = Spinlock::new(KlogRing::new());

/// Sets the active runtime minimum filter threshold (default `LogLevel::Debug`).
pub fn set_level(level: LogLevel) {
    KLOG_MIN_LEVEL.store(level as u8, Ordering::Relaxed);
}

/// Returns the active runtime minimum filter level.
pub fn get_level() -> LogLevel {
    match KLOG_MIN_LEVEL.load(Ordering::Relaxed) {
        1 => LogLevel::Error,
        2 => LogLevel::Warn,
        3 => LogLevel::Info,
        4 => LogLevel::Debug,
        _ => LogLevel::Trace,
    }
}

/// Records a structured log entry into `KLOG_RING` (and mirrors a formatted line into `dmesg`).
pub fn log_structured(level: LogLevel, subsystem: &'static str, msg: &str, fields: &[LogField]) {
    if (level as u8) > KLOG_MIN_LEVEL.load(Ordering::Relaxed) {
        KLOG_RING.lock().dropped_filtered += 1;
        return;
    }

    let now_tsc = tsc::read_tsc();
    let ts_ms = tsc::cycles_to_us(now_tsc) / 1000;
    let seq = KLOG_SEQ.fetch_add(1, Ordering::Relaxed);
    let cpu_id = apic::current_apic_id() as u16;

    let mut entry = LogEntry::empty();
    entry.seq = seq;
    entry.timestamp_ms = ts_ms;
    entry.tsc_cycles = now_tsc;
    entry.level = level;
    entry.cpu_id = cpu_id;
    entry.subsystem = subsystem;

    let bytes = msg.as_bytes();
    let copy_len = bytes.len().min(MAX_LOG_MSG_LEN);
    entry.msg_buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
    entry.msg_len = copy_len as u8;

    let fcount = fields.len().min(MAX_LOG_FIELDS);
    for i in 0..fcount {
        entry.fields[i] = fields[i];
    }
    entry.field_count = fcount as u8;

    {
        let mut ring = KLOG_RING.lock();
        let idx = ring.write_idx;
        ring.entries[idx] = entry;
        ring.write_idx = (idx + 1) % KLOG_CAPACITY;
        ring.total_recorded += 1;
    }

    // Mirror concise line to console dmesg ring (does not emit live console stdout noise).
    let mut line = alloc::format!("[{}][{}] {}\n", level.as_str(), subsystem, msg);
    if line.len() > 160 {
        line.truncate(159);
        line.push('\n');
    }
    crate::drivers::console::record_dmesg(&line);
}

/// Convenience helper for structured logging with zero key-value fields.
#[inline]
pub fn log(level: LogLevel, subsystem: &'static str, msg: &str) {
    log_structured(level, subsystem, msg, &[]);
}

/// Returns up to `limit` most recent buffered [`LogEntry`] records matching `min_level` and optional `subsys`.
pub fn snapshot(
    min_level: Option<LogLevel>,
    subsys: Option<&str>,
    limit: usize,
) -> Vec<LogEntry> {
    let max_lvl = min_level.unwrap_or(LogLevel::Trace) as u8;
    let ring = KLOG_RING.lock();
    let count = (ring.total_recorded as usize).min(KLOG_CAPACITY);
    let start = if ring.total_recorded as usize > KLOG_CAPACITY {
        ring.write_idx
    } else {
        0
    };

    let mut collected = Vec::new();
    for i in 0..count {
        let entry = ring.entries[(start + i) % KLOG_CAPACITY];
        if (entry.level as u8) <= max_lvl
            && subsys.map_or(true, |s| entry.subsystem.eq_ignore_ascii_case(s))
        {
            collected.push(entry);
        }
    }

    if collected.len() > limit {
        collected.drain(0..(collected.len() - limit));
    }
    collected
}

/// Renders filtered structured kernel logs as text or JSON Lines.
pub fn format_logs(
    min_level: Option<LogLevel>,
    subsys: Option<&str>,
    as_json: bool,
    limit: usize,
) -> String {
    let entries = snapshot(min_level, subsys, limit);
    if entries.is_empty() {
        return String::from("(no matching structured kernel log entries)\n");
    }
    let mut out = String::new();
    for e in &entries {
        if as_json {
            out.push_str(&e.format_json());
        } else {
            out.push_str(&e.format_text());
        }
        out.push('\n');
    }
    out
}

/// Returns `(total_recorded, buffered_count, dropped_filtered)`.
pub fn stats() -> (u64, usize, u64) {
    let ring = KLOG_RING.lock();
    (
        ring.total_recorded,
        (ring.total_recorded as usize).min(KLOG_CAPACITY),
        ring.dropped_filtered,
    )
}

