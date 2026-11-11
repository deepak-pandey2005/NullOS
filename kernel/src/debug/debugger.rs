//! # Live Kernel Debugger (`kdbg`) & Interactive Debug Shell (Phase 18 - M40)
//!
//! ## Overview
//! Provides an interactive in-kernel debugger runtime and `dbg` shell dispatcher:
//! - **CPU & Control Register Inspection**: Inspects live `CR0/CR2/CR3/CR4/RFLAGS/RSP/RBP/RIP`.
//! - **Software Breakpoints (`#BP` / `int3`) & Trap Probe**: Tracks registered breakpoints
//!   and intercepts kernel `#BP` (vector 3) traps cleanly in `exception_handler` when armed,
//!   recording trap telemetry and resuming seamlessly via `iretq`.
//! - **Safe Memory Examiner**: Hex/ASCII inspection (`dump_memory`) guarded by lock-free CR3
//!   mapping validation so probing unmapped addresses returns a safe error instead of `#PF`.
//! - **Debug Shell Subcommands (`dbg ...`)**: Unified debugging console exposing `regs`,
//!   `bt`, `sym`, `mem`, `v2p`, `heap`, `klog`, `trace`, `locks`, `threads`, `crash`, `bp`.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::x86_64::idt::ExceptionContext;
use crate::sync::Spinlock;

use super::{crash, klog, lockdep, memdiag, sym, trace_obs, unwind};

/// Maximum software/diagnostic breakpoint slots tracked by `kdbg`.
pub const MAX_BREAKPOINTS: usize = 8;

/// Registered debugger breakpoint slot.
#[derive(Debug, Clone)]
pub struct Breakpoint {
    pub id: usize,
    pub addr: u64,
    pub symbol: String,
    pub enabled: bool,
    pub hits: u64,
}

static PROBE_TRAP_ARMED: AtomicBool = AtomicBool::new(false);
static PROBE_TRAP_HITS: AtomicU64 = AtomicU64::new(0);
static LAST_TRAP_RIP: AtomicU64 = AtomicU64::new(0);
static BREAKPOINTS: Spinlock<Vec<Breakpoint>> = Spinlock::new(Vec::new());

/// Arms or disarms handling of kernel-mode `#BP` (`int3`, vector 3) traps by `kdbg`.
pub fn set_probe_armed(armed: bool) {
    PROBE_TRAP_ARMED.store(armed, Ordering::SeqCst);
}

/// Handles a kernel-mode CPU debug trap (`#BP` vector 3 or `#DB` vector 1) inside `exception_handler`.
///
/// Returns `true` when the trap was intentionally intercepted by `kdbg` and execution should
/// resume safely at `ctx.rip` via `iretq`, or `false` if normal fault handling should proceed.
pub fn handle_debug_trap(ctx: &ExceptionContext) -> bool {
    if ctx.vector != 3 {
        return false;
    }

    // Match against registered breakpoint addresses (`int3` advances RIP by 1 past 0xCC)
    let fault_site = ctx.rip.saturating_sub(1);
    let mut matched = false;
    {
        let mut bps = BREAKPOINTS.lock();
        for bp in bps.iter_mut() {
            if bp.enabled && (bp.addr == fault_site || bp.addr == ctx.rip) {
                bp.hits += 1;
                matched = true;
            }
        }
    }

    if PROBE_TRAP_ARMED.swap(false, Ordering::SeqCst) || matched {
        PROBE_TRAP_HITS.fetch_add(1, Ordering::SeqCst);
        LAST_TRAP_RIP.store(ctx.rip, Ordering::SeqCst);
        klog::log_structured(
            klog::LogLevel::Debug,
            "KDBG",
            "Handled #BP software breakpoint trap",
            &[klog::LogField::hex("rip", ctx.rip)],
        );
        trace_obs::emit_debug_trace(crate::perf::trace::event_id::USER_MARK, ctx.rip, 3);
        return true;
    }

    false
}

/// Registers a tracked breakpoint descriptor at `addr` for debugger telemetry.
pub fn add_breakpoint(addr: u64) -> Result<usize, &'static str> {
    let mut bps = BREAKPOINTS.lock();
    if bps.len() >= MAX_BREAKPOINTS {
        return Err("Maximum breakpoint slots reached");
    }
    let id = bps.len() + 1;
    let sym_name = sym::resolve_addr(addr)
        .map(|s| s.format_compact())
        .unwrap_or_else(|| format!("{:#x}", addr));
    bps.push(Breakpoint {
        id,
        addr,
        symbol: sym_name,
        enabled: true,
        hits: 0,
    });
    Ok(id)
}

/// Clears a registered breakpoint by numeric slot `id`.
pub fn remove_breakpoint(id: usize) -> bool {
    let mut bps = BREAKPOINTS.lock();
    if let Some(pos) = bps.iter().position(|b| b.id == id) {
        bps.remove(pos);
        true
    } else {
        false
    }
}

/// Lists all active debugger breakpoints.
pub fn format_breakpoints() -> String {
    let bps = BREAKPOINTS.lock();
    if bps.is_empty() {
        return format!(
            "No breakpoints registered (total #BP probe hits={}).\n",
            PROBE_TRAP_HITS.load(Ordering::Relaxed)
        );
    }
    let mut out = String::from("=== Kernel Breakpoints ===\n");
    for bp in bps.iter() {
        out.push_str(&format!(
            "  #{} {:#018x} [{}] hits={} ({})\n",
            bp.id,
            bp.addr,
            if bp.enabled { "EN" } else { "DIS" },
            bp.hits,
            bp.symbol
        ));
    }
    out
}

/// Triggers a controlled hardware `int3` software breakpoint trap and verifies clean `iretq` resumption.
#[inline(never)]
pub fn run_breakpoint_probe() -> (u64, u64) {
    let before = PROBE_TRAP_HITS.load(Ordering::SeqCst);
    set_probe_armed(true);
    unsafe {
        core::arch::asm!("int3", options(nomem, nostack));
    }
    let after = PROBE_TRAP_HITS.load(Ordering::SeqCst);
    let rip = LAST_TRAP_RIP.load(Ordering::SeqCst);
    (after.saturating_sub(before), rip)
}

/// Safely reads and hex-dumps up to `len` (capped at 256) bytes starting at `addr`.
///
/// Validates every 4 KiB page boundary lock-free via [`unwind::is_addr_mapped_lockfree`]
/// before dereferencing memory, preventing `#PF` panics on unmapped kernel addresses.
pub fn dump_memory(addr: u64, len: usize) -> String {
    let clamped = len.clamp(1, 256);
    if !unwind::is_addr_mapped_lockfree(addr) {
        return format!("dbg mem: address {:#018x} is unmapped or non-canonical.\n", addr);
    }

    let mut out = format!("=== Kernel Memory Dump @ {:#018x} ({} bytes) ===\n", addr, clamped);
    let mut offset = 0usize;
    while offset < clamped {
        let row_addr = addr.saturating_add(offset as u64);
        let row_len = (clamped - offset).min(16);
        out.push_str(&format!("  {:#018x}: ", row_addr));

        let mut ascii = [b'.'; 16];
        for i in 0..row_len {
            let cur = row_addr + i as u64;
            if unwind::is_addr_mapped_lockfree(cur) {
                let byte = unsafe { *(cur as *const u8) };
                out.push_str(&format!("{:02x} ", byte));
                ascii[i] = if byte >= 0x20 && byte <= 0x7E { byte } else { b'.' };
            } else {
                out.push_str("?? ");
            }
        }
        for _ in row_len..16 {
            out.push_str("   ");
        }
        let ascii_str = core::str::from_utf8(&ascii[..row_len]).unwrap_or("");
        out.push_str(&format!(" |{}|\n", ascii_str));
        offset += row_len;
    }
    out
}

/// Formats a live CPU control-register & stack-frame summary for the debugger.
pub fn format_regs() -> String {
    let snap = crash::CpuRegSnapshot::capture_current();
    let sym_name = sym::resolve_addr(snap.rip)
        .map(|s| s.format_compact())
        .unwrap_or_else(|| String::from("<unknown>"));
    format!(
        "=== Live Kernel CPU Registers ===\n  RIP={:#018x} ({})\n  RSP={:#018x} RBP={:#018x} RFLAGS={:#018x}\n  CS={:#06x} SS={:#06x}\n  CR0={:#018x} CR2={:#018x}\n  CR3={:#018x} CR4={:#018x}\n",
        snap.rip, sym_name, snap.rsp, snap.rbp, snap.rflags, snap.cs, snap.ss, snap.cr0, snap.cr2, snap.cr3, snap.cr4
    )
}

/// Parses hexadecimal (`0x...`) or decimal numeric arguments safely.
pub fn parse_u64_arg(s: &str) -> Option<u64> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        s.parse::<u64>().ok()
    }
}

/// Formats a summary of all scheduler threads for debugger inspection (`dbg threads`).
fn format_threads() -> String {
    let threads = crate::task::thread_list();
    let mut out = format!("=== Active Kernel/User Threads ({}) ===\n", threads.len());
    for t in threads {
        out.push_str(&format!(
            "  TID={:<3} state={:?} prio={} ticks={} name=\"{}\"\n",
            t.id.0, t.state, t.priority, t.total_ticks, t.name
        ));
    }
    out
}

/// Dispatches interactive `dbg` / `kdbg` debug shell commands cleanly.
pub fn dispatch_shell_command(args: &[&str]) -> (String, i32) {
    let sub = args.first().copied().unwrap_or("help");
    match sub {
        "help" | "-h" | "--help" => (
            String::from(
                "NullOS Kernel Debugger & Observability (M40):\n\
                 \x20 dbg regs               - Dump live CPU registers & CR0..CR4\n\
                 \x20 dbg bt                 - Unwind & symbolize current kernel call stack\n\
                 \x20 dbg sym <addr|name>    - Resolve hex address or search kernel symbols\n\
                 \x20 dbg mem <addr> [len]   - Safely hex-dump mapped kernel memory\n\
                 \x20 dbg v2p <addr>         - Walk 4-level CR3 page tables for virtual address\n\
                 \x20 dbg heap               - Inspect PMM, heap, slab, canary & stack integrity\n\
                 \x20 dbg klog [lvl] [--json]- Display structured kernel logs (text or JSON)\n\
                 \x20 dbg trace [subsys]     - Inspect filtered M37/M40 kernel trace buffers\n\
                 \x20 dbg locks              - Show lockdep dependency graph & deadlock status\n\
                 \x20 dbg threads            - List scheduler threads & execution states\n\
                 \x20 dbg crash [snap]       - View recorded crash dumps or take snapshot\n\
                 \x20 dbg bp [list|probe]    - Inspect breakpoints or run live INT3 trap probe\n\
                 \x20 dbg selftest           - Execute M40 debugging & observability test suite\n",
            ),
            0,
        ),
        "regs" => (format_regs(), 0),
        "bt" | "backtrace" => {
            let frames = unwind::capture_backtrace(16);
            (
                format!("=== Live Kernel Backtrace ({} frames) ===\n{}", frames.len(), unwind::format_backtrace(&frames)),
                0,
            )
        }
        "sym" => {
            let Some(q) = args.get(1) else {
                let (cnt, ksym) = sym::stats();
                return (
                    format!(
                        "Symbolizer: {} symbols loaded (full KSYM={}). Usage: dbg sym <0xaddr|substring>\n",
                        cnt, ksym
                    ),
                    0,
                );
            };
            if let Some(addr) = parse_u64_arg(q) {
                match sym::resolve_addr(addr) {
                    Some(res) => (
                        format!("{:#018x} => {} (base={:#x}, size={})\n", addr, res.format_compact(), res.base, res.size),
                        0,
                    ),
                    None => (format!("{:#018x} => <no kernel symbol>\n", addr), 1),
                }
            } else {
                let hits = sym::search_symbols(q, 16);
                if hits.is_empty() {
                    (format!("No kernel symbols matching '{}'.\n", q), 1)
                } else {
                    let mut out = format!("=== Kernel Symbols matching '{}' ({}) ===\n", q, hits.len());
                    for h in hits {
                        out.push_str(&format!("  {:#018x} (size={:<5}) {}\n", h.addr, h.size, h.name));
                    }
                    (out, 0)
                }
            }
        }
        "mem" => {
            let Some(addr_str) = args.get(1) else {
                return (String::from("Usage: dbg mem <hex_addr> [len]\n"), 1);
            };
            let Some(addr) = parse_u64_arg(addr_str) else {
                return (format!("Invalid address '{}'\n", addr_str), 1);
            };
            let len = args.get(2).and_then(|s| parse_u64_arg(s)).unwrap_or(64) as usize;
            (dump_memory(addr, len), 0)
        }
        "v2p" => {
            let Some(addr_str) = args.get(1) else {
                return (String::from("Usage: dbg v2p <hex_addr>\n"), 1);
            };
            let Some(addr) = parse_u64_arg(addr_str) else {
                return (format!("Invalid virtual address '{}'\n", addr_str), 1);
            };
            (memdiag::format_v2p(addr), 0)
        }
        "heap" | "memdiag" => (memdiag::format_report(), 0),
        "klog" => {
            let as_json = args.iter().any(|&a| a == "--json");
            let lvl = args.iter().filter_map(|&a| klog::LogLevel::parse(a)).next();
            (klog::format_logs(lvl, None, as_json, 24), 0)
        }
        "trace" => {
            let cat = args.get(1).and_then(|s| trace_obs::parse_category(s));
            (trace_obs::format_trace_report(cat, 24), 0)
        }
        "locks" | "lockdep" => (lockdep::format_status(), 0),
        "threads" => (format_threads(), 0),
        "crash" => {
            if args.get(1) == Some(&"snap") {
                let d = crash::capture_manual_snapshot("dbg crash snap");
                (d.format_report(), 0)
            } else {
                (crash::format_all_dumps(), 0)
            }
        }
        "bp" => {
            if args.get(1) == Some(&"probe") {
                let (hits, rip) = run_breakpoint_probe();
                (format!("#BP software breakpoint probe completed: hits={} rip={:#018x}\n", hits, rip), 0)
            } else {
                (format_breakpoints(), 0)
            }
        }
        "selftest" => {
            super::run_self_tests();
            (String::from("[DBG] All M40 Debugging & Observability checks passed.\n"), 0)
        }
        _ => (
            format!("Unknown dbg subcommand '{}'. Try 'dbg help'.\n", sub),
            1,
        ),
    }
}
