//! # NullOS Debugging & Observability Subsystem (Phase 18 - M40)
//!
//! ## Architecture Overview
//! Integrates 10 cohesive diagnostic and observability pillars into NullOS:
//! 1. **Symbolizer ([`sym`])**: `KSYM` v1 binary symbol table loader (`/disk/KERNEL.SYM`)
//!    with bootstrap anchors and Rust v0 / legacy demangler.
//! 2. **Stack Unwinding ([`unwind`])**: Lock-free `CR3`-validated frame-pointer & stack-scan
//!    unwinder safe inside `#PF`, `#GP`, and `panic()` contexts.
//! 3. **Kernel Logs & Structured Logging ([`klog`])**: Fixed circular structured ring buffer
//!    with severity filters, typed fields, `dmesg` sync, and JSON Lines export.
//! 4. **Deadlock Detection ([`lockdep`])**: Runtime directed lock-dependency graph catching
//!    `A->A` self-deadlocks and `A->B->...->A` lock-order inversions.
//! 5. **Memory Diagnostics ([`memdiag`])**: Physical RAM, heap, slab, canary, and lock-free
//!    4-level `PML4->PDPT->PD->PT` inspection (`v2p`).
//! 6. **Trace Buffer Observability ([`trace_obs`])**: Subsystem filtering & address
//!    symbolization bridged to M37's `512`-event static trace ring.
//! 7. **Crash Dumps ([`crash`])**: Structured CPU exception & panic snapshots capturing
//!    registers, `CR0..CR4`, unwound frames, and active lock stacks.
//! 8. **Kernel Debugger & Debug Shell ([`debugger`])**: Breakpoints, live `#BP` (`int3`) trap
//!    handling, safe hex memory dumper, and interactive `dbg` console commands.

#![allow(dead_code)]

pub mod crash;
pub mod debugger;
pub mod klog;
pub mod lockdep;
pub mod memdiag;
pub mod sym;
pub mod trace_obs;
pub mod unwind;

use crate::println;

/// Initializes early-boot M40 debugging & observability subsystems cleanly.
pub fn init() {
    sym::init();
    lockdep::init();
    klog::log_structured(
        klog::LogLevel::Info,
        "DBG",
        "M40 Debugging & Observability initialized",
        &[klog::LogField::int("klog_cap", klog::KLOG_CAPACITY as i64)],
    );
}

/// Runs comprehensive self-tests exercising all 10 M40 Debugging & Observability features.
pub fn run_self_tests() {
    println!("[DBG] Running Phase 18 M40 Debugging & Observability self-tests...");

    // 1. Ensure runtime KSYM symbol table from /disk/KERNEL.SYM loaded if available
    let ksym_ok = sym::ensure_loaded_from_vfs();
    let (sym_count, full_ksym) = sym::stats();
    assert!(sym_count >= 5, "Bootstrap or KSYM symbol table must be populated");

    // 2. Test Rust v0 & Legacy Symbol Demangling + Live Address Resolution
    assert_eq!(
        sym::demangle_symbol("_RNvNtNtCs3jDM8YtC2L3_6kernel4task9scheduler10timer_tick"),
        "kernel::task::scheduler::timer_tick"
    );
    assert_eq!(
        sym::demangle_symbol("_ZN6kernel4main17h0123456789abcdefE"),
        "kernel::main"
    );
    let (_, kend) = sym::kernel_image_bounds();
    let resolved = sym::resolve_addr(debugger::run_breakpoint_probe as *const () as usize as u64)
        .expect("Kernel function RIP must resolve");
    assert!(resolved.base < kend);
    println!(
        "  [OK] Symbolizer & Demangler active (symbols={}, full_ksym={}, sample={}).",
        sym_count,
        full_ksym || ksym_ok,
        resolved.format_compact()
    );

    // 3. Test Live Frame-Pointer Stack Unwinding
    let bt = unwind::capture_backtrace(12);
    assert!(!bt.is_empty(), "Stack unwinder must capture active frames");
    assert!(
        bt.iter().all(|f| sym::is_kernel_addr(f.rip)),
        "All unwound frames must stay within valid kernel text bounds"
    );
    println!("  [OK] Stack Unwinder captured {} symbolized frame(s).", bt.len());

    // 4. Test Structured Kernel Logging (filtering, metadata, JSON output)
    klog::log_structured(
        klog::LogLevel::Info,
        "M40TEST",
        "Structured observability test record",
        &[
            klog::LogField::int("phase", 18),
            klog::LogField::hex("magic", 0x4D40_CAFE),
        ],
    );
    let json_logs = klog::format_logs(Some(klog::LogLevel::Info), Some("M40TEST"), true, 4);
    assert!(json_logs.contains("\"sub\":\"M40TEST\""));
    assert!(json_logs.contains("\"magic\":\"0x4d40cafe\""));
    println!("  [OK] Structured Logging & JSON Lines formatting verified.");

    // 5. Test Trace Buffer Bridge & Subsystem Filtering
    let probe_fn_addr = run_self_tests as *const () as usize as u64;
    trace_obs::emit_debug_trace(crate::perf::trace::event_id::USER_MARK, probe_fn_addr, 40);
    let trace_rep = trace_obs::format_trace_report(Some(crate::perf::trace::TraceCategory::User), 8);
    assert!(trace_rep.contains("USER"));
    println!("  [OK] Trace Buffer observability bridge verified.");

    // 6. Test Memory Diagnostics (4-level CR3 v2p walk + heap/stack canary health)
    assert!(
        memdiag::verify_heap_and_stack_integrity(),
        "Kernel heap redzone & stack canary integrity check must pass"
    );
    let mapping = memdiag::inspect_vaddr(probe_fn_addr);
    assert!(mapping.phys_addr.is_some(), "Kernel function address must translate in CR3");
    assert!(!mapping.no_exec, "Kernel .text must remain executable");
    println!(
        "  [OK] Memory Diagnostics (v2p CR3 walk={:#x}, page={} KiB, canary healthy) verified.",
        mapping.phys_addr.unwrap(),
        mapping.page_size_kb
    );

    // 7. Test Deadlock Detection Graph (AA self-deadlock + AB->BC->CA cycle rejection)
    lockdep::run_self_tests();
    println!("  [OK] Lockdep Deadlock Detector (AA + transitive cycle detection) verified.");

    // 8. Test Kernel Debugger Live #BP (INT3) Hardware Breakpoint & Crash Dump Snapshot
    let bp_id = debugger::add_breakpoint(probe_fn_addr).expect("Breakpoint slot available");
    assert!(debugger::remove_breakpoint(bp_id));
    let (bp_hits, trap_rip) = debugger::run_breakpoint_probe();
    assert_eq!(bp_hits, 1, "Live INT3 trap must be intercepted once by kdbg");
    assert!(sym::is_kernel_addr(trap_rip));

    let snap = crash::capture_manual_snapshot("M40 verification snapshot");
    let snap_txt = snap.format_report();
    assert!(snap_txt.contains("Call Backtrace:"));
    assert!(crash::dump_count() >= 1);

    // 9. Test Debug Shell CLI dispatch & unmapped memory guard
    let (bt_out, bt_rc) = debugger::dispatch_shell_command(&["bt"]);
    assert_eq!(bt_rc, 0);
    assert!(bt_out.contains("Live Kernel Backtrace"));
    let bad_mem = debugger::dump_memory(0x0000_DEAD_BEEF_0000, 16);
    assert!(bad_mem.contains("unmapped"));

    println!(
        "  [OK] Kernel Debugger (live #BP trap @ {:#x}, CrashDump #{}, Debug Shell 'dbg') verified.",
        trap_rip, snap.id
    );
    println!("[DBG] All M40 Debugging & Observability checks passed!");
}

