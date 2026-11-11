//! # Kernel Crash Dump & Panic Snapshot Recorder (Phase 18 - M40)
//!
//! ## Overview
//! Captures structured diagnostic crash snapshots (`CrashDump`) on:
//! - Hardware CPU exceptions (`#PF`, `#GP`, `#UD`, `#DF`, etc. via [`record_exception`])
//! - Runtime Rust panics (`#[panic_handler]` via [`record_panic`])
//! - On-demand diagnostic snapshots (`dbg crash` via [`capture_manual_snapshot`])
//!
//! Each snapshot stores register context, control registers (`CR0/CR2/CR3/CR4`),
//! symbolized unwound stack frames, held lockdep locks, and recent log tail.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::{apic, idt::ExceptionContext};
use crate::perf::tsc;
use crate::sync::Spinlock;

use super::klog::{self, LogLevel};
use super::lockdep;
use super::unwind;

/// Maximum crash snapshots retained in the kernel crash history ring.
pub const MAX_CRASH_DUMPS: usize = 4;
/// Maximum unwound stack frames embedded inside each [`CrashDump`].
pub const MAX_CRASH_FRAMES: usize = 12;

/// Origin category of a recorded crash snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashKind {
    CpuException(u64),
    KernelPanic,
    DiagnosticSnapshot,
}

/// Architectural register & control-register snapshot captured at crash time.
#[derive(Debug, Clone, Copy, Default)]
pub struct CpuRegSnapshot {
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub cr0: u64,
    pub cr2: u64,
    pub cr3: u64,
    pub cr4: u64,
}

impl CpuRegSnapshot {
    /// Reads CPU control registers (`CR0`, `CR2`, `CR3`, `CR4`) cleanly.
    #[inline]
    pub fn read_control_regs() -> (u64, u64, u64, u64) {
        let (cr0, cr2, cr3, cr4): (u64, u64, u64, u64);
        unsafe {
            core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack));
            core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack));
            core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
            core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
        }
        (cr0, cr2, cr3, cr4)
    }

    /// Captures live register state from the current CPU execution context.
    #[inline(never)]
    pub fn capture_current() -> Self {
        let (cr0, cr2, cr3, cr4) = Self::read_control_regs();
        let (rip, rsp, rbp, rflags, cs_val, ss_val): (u64, u64, u64, u64, u16, u16);
        unsafe {
            core::arch::asm!(
                "lea {rip}, [rip + 0]",
                "mov {rsp}, rsp",
                "mov {rbp}, rbp",
                "pushfq; pop {rflags}",
                "mov {cs:x}, cs",
                "mov {ss:x}, ss",
                rip = out(reg) rip,
                rsp = out(reg) rsp,
                rbp = out(reg) rbp,
                rflags = out(reg) rflags,
                cs = out(reg) cs_val,
                ss = out(reg) ss_val,
                options(nomem)
            );
        }
        Self {
            rip,
            cs: cs_val as u64,
            rflags,
            rsp,
            rbp,
            ss: ss_val as u64,
            cr0,
            cr2,
            cr3,
            cr4,
            ..Default::default()
        }
    }

    /// Builds a [`CpuRegSnapshot`] from an architectural [`ExceptionContext`].
    pub fn from_exception(ctx: &ExceptionContext) -> Self {
        let (cr0, cr2, cr3, cr4) = Self::read_control_regs();
        Self {
            rip: ctx.rip,
            cs: ctx.cs,
            rflags: ctx.rflags,
            rsp: ctx.rsp,
            ss: ctx.ss,
            rax: ctx.rax,
            rbx: ctx.rbx,
            rcx: ctx.rcx,
            rdx: ctx.rdx,
            rsi: ctx.rsi,
            rdi: ctx.rdi,
            rbp: ctx.rbp,
            r8: ctx.r8,
            r9: ctx.r9,
            r10: ctx.r10,
            r11: ctx.r11,
            r12: ctx.r12,
            r13: ctx.r13,
            r14: ctx.r14,
            r15: ctx.r15,
            cr0,
            cr2,
            cr3,
            cr4,
        }
    }
}

/// Complete captured crash/fault diagnostic record.
#[derive(Debug, Clone)]
pub struct CrashDump {
    pub id: u64,
    pub kind: CrashKind,
    pub cpu_id: u16,
    pub tsc: u64,
    pub error_code: u64,
    pub summary: String,
    pub regs: CpuRegSnapshot,
    pub backtrace: Vec<String>,
    pub held_locks: Vec<&'static str>,
}

impl CrashDump {
    /// Formats this crash dump into a comprehensive human-readable report.
    pub fn format_report(&self) -> String {
        let kind_str = match self.kind {
            CrashKind::CpuException(v) => format!("CPU Exception Vector {}", v),
            CrashKind::KernelPanic => String::from("Kernel Panic"),
            CrashKind::DiagnosticSnapshot => String::from("On-Demand Diagnostic Snapshot"),
        };
        let mut out = format!(
            "=== NullOS Crash Dump #{} ({}) ===\n  CPU={} | TSC={} | ErrorCode={:#x}\n  Summary: {}\n",
            self.id, kind_str, self.cpu_id, self.tsc, self.error_code, self.summary
        );
        out.push_str(&format!(
            "  RIP={:#018x} CS={:#06x} RFLAGS={:#018x}\n  RSP={:#018x} RBP={:#018x} SS={:#06x}\n",
            self.regs.rip, self.regs.cs, self.regs.rflags, self.regs.rsp, self.regs.rbp, self.regs.ss
        ));
        out.push_str(&format!(
            "  CR0={:#018x} CR2={:#018x} CR3={:#018x} CR4={:#018x}\n",
            self.regs.cr0, self.regs.cr2, self.regs.cr3, self.regs.cr4
        ));
        if !self.held_locks.is_empty() {
            out.push_str(&format!("  Held Locks: {:?}\n", self.held_locks));
        }
        out.push_str("  Call Backtrace:\n");
        for line in &self.backtrace {
            out.push_str(line);
            out.push('\n');
        }
        out
    }
}

static CRASH_SEQ: AtomicU64 = AtomicU64::new(1);
static CRASH_HISTORY: Spinlock<Vec<CrashDump>> = Spinlock::new(Vec::new());

fn store_dump(dump: CrashDump) {
    let mut hist = CRASH_HISTORY.lock();
    if hist.len() >= MAX_CRASH_DUMPS {
        hist.remove(0);
    }
    hist.push(dump);
}

/// Captures a structured [`CrashDump`] from a hardware CPU exception context.
pub fn record_exception(ctx: &ExceptionContext, name: &str) -> CrashDump {
    let regs = CpuRegSnapshot::from_exception(ctx);
    let frames = unwind::unwind_from(ctx.rbp, ctx.rsp, ctx.rip, MAX_CRASH_FRAMES);
    let backtrace: Vec<String> = frames.iter().map(|f| f.format_line()).collect();
    let cpu = apic::current_apic_id() as u16;
    let dump = CrashDump {
        id: CRASH_SEQ.fetch_add(1, Ordering::Relaxed),
        kind: CrashKind::CpuException(ctx.vector),
        cpu_id: cpu,
        tsc: tsc::read_tsc(),
        error_code: ctx.error_code,
        summary: String::from(name),
        regs,
        backtrace,
        held_locks: lockdep::held_locks_snapshot(cpu as usize),
    };
    klog::log_structured(
        LogLevel::Error,
        "CRASH",
        "CPU exception crash dump captured",
        &[
            klog::LogField::int("vec", ctx.vector as i64),
            klog::LogField::hex("rip", ctx.rip),
            klog::LogField::hex("err", ctx.error_code),
        ],
    );
    store_dump(dump.clone());
    dump
}

/// Captures a structured [`CrashDump`] during `#[panic_handler]` execution.
pub fn record_panic(info: &PanicInfo) -> CrashDump {
    let regs = CpuRegSnapshot::capture_current();
    let frames = unwind::unwind_from(regs.rbp, regs.rsp, regs.rip, MAX_CRASH_FRAMES);
    let backtrace: Vec<String> = frames.iter().map(|f| f.format_line()).collect();
    let cpu = apic::current_apic_id() as u16;
    let summary = if let Some(loc) = info.location() {
        format!("{}:{}", loc.file(), loc.line())
    } else {
        String::from("panic(no location)")
    };
    let dump = CrashDump {
        id: CRASH_SEQ.fetch_add(1, Ordering::Relaxed),
        kind: CrashKind::KernelPanic,
        cpu_id: cpu,
        tsc: tsc::read_tsc(),
        error_code: 0,
        summary,
        regs,
        backtrace,
        held_locks: lockdep::held_locks_snapshot(cpu as usize),
    };
    store_dump(dump.clone());
    dump
}

/// Captures an on-demand diagnostic crash snapshot (`reason`) without faulting.
pub fn capture_manual_snapshot(reason: &str) -> CrashDump {
    let regs = CpuRegSnapshot::capture_current();
    let frames = unwind::unwind_from(regs.rbp, regs.rsp, regs.rip, MAX_CRASH_FRAMES);
    let backtrace: Vec<String> = frames.iter().map(|f| f.format_line()).collect();
    let cpu = apic::current_apic_id() as u16;
    let dump = CrashDump {
        id: CRASH_SEQ.fetch_add(1, Ordering::Relaxed),
        kind: CrashKind::DiagnosticSnapshot,
        cpu_id: cpu,
        tsc: tsc::read_tsc(),
        error_code: 0,
        summary: String::from(reason),
        regs,
        backtrace,
        held_locks: lockdep::held_locks_snapshot(cpu as usize),
    };
    store_dump(dump.clone());
    dump
}

/// Returns the most recent captured [`CrashDump`], if any.
pub fn latest_dump() -> Option<CrashDump> {
    CRASH_HISTORY.lock().last().cloned()
}

/// Returns the total number of stored dumps in `CRASH_HISTORY`.
pub fn dump_count() -> usize {
    CRASH_HISTORY.lock().len()
}

/// Formats all stored crash dumps (or reports when none have been captured).
pub fn format_all_dumps() -> String {
    let hist = CRASH_HISTORY.lock();
    if hist.is_empty() {
        return String::from("No crash dumps recorded.\n");
    }
    let mut out = String::new();
    for d in hist.iter() {
        out.push_str(&d.format_report());
    }
    out
}
