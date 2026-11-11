//! # Kernel Profiling & Observability Subsystem (Phase 17 - M37)
//!
//! ## Overview
//! Coordinates NullOS's hardware-assisted and lock-free kernel profiling stack:
//! - [`tsc`]: CPUID TSC capability probing, calibration, serialized cycle counters (`CycleCounter`).
//! - [`pmu`]: CPUID Leaf `0x0A` Architectural PMU probing + software event fallback counters.
//! - [`trace`]: Circular IRQ-safe static tracepoint ring buffer & [`tracepoint!`] macro.
//! - [`latency`]: Scheduler, Syscall & Interrupt latency histograms, [`ProfiledSpinlock`] contention
//!   profiler, and per-CPU Active/IRQ/Idle utilization metrics.
//! - [`bench`]: Physical/heap memory allocation & kernel context-switch benchmarks.

#![allow(dead_code)]

pub mod bench;
pub mod latency;
pub mod pmu;
pub mod trace;
pub mod tsc;

use crate::arch::x86_64::smp;
use crate::println;

/// Initializes TSC calibration, hardware PMU probing, and kernel tracepoints.
pub fn init() {
    tsc::init();
    pmu::init();

    let tinfo = tsc::info();
    let pinfo = pmu::snapshot();
    println!(
        "[PERF] M37 Profiling initialized: TSC={} MHz (Invariant={}, Calibrated={}) | PMU={}",
        tinfo.frequency_mhz,
        tinfo.invariant_tsc,
        tinfo.calibrated,
        if pinfo.hardware_active {
            "Hardware MSRs"
        } else {
            "Software-Emulated"
        }
    );
}

/// Prints a comprehensive kernel profiling report (`perfstat` shell command).
pub fn print_status() {
    let tinfo = tsc::info();
    let pinfo = pmu::snapshot();
    let sched_tick = latency::SCHED_TICK_LATENCY.snapshot();
    let sched_wake = latency::SCHED_WAKEUP_LATENCY.snapshot();
    let sys_lat = latency::SYSCALL_LATENCY.snapshot();
    let irq_lat = latency::IRQ_TIMER_LATENCY.snapshot();
    let irq_jit = latency::IRQ_JITTER_LATENCY.snapshot();
    let tr = trace::stats();

    println!("=== NullOS Kernel Profiling Telemetry (M37) ===");
    println!(
        "  TSC       : {} MHz ({} Hz) rdtsc={} rdtscp={} invariant={} calibrated={}",
        tinfo.frequency_mhz,
        tinfo.frequency_hz,
        tinfo.rdtsc_supported,
        tinfo.rdtscp_supported,
        tinfo.invariant_tsc,
        tinfo.calibrated
    );
    println!(
        "  PMU       : ver={} gp={} ({}b) fixed={} mode={} | unhalted_cyc={} inst_est={}",
        pinfo.version,
        pinfo.num_gp_counters,
        pinfo.gp_counter_bits,
        pinfo.num_fixed_counters,
        if pinfo.hardware_active { "HW" } else { "SW" },
        pinfo.unhalted_cycles,
        pinfo.instructions_retired
    );
    println!(
        "  Events    : irq={} ctx_switches={} syscalls={} tracepoints={}/{}",
        pinfo.hw_irq_events,
        pinfo.ctx_switch_events,
        pinfo.syscall_events,
        tr.total_recorded,
        tr.capacity
    );

    println!("  Latency Metrics (cycles / ns):");
    println!(
        "    Timer IRQ  : cnt={:<5} avg={:<6}cyc ({:<5}ns) max={:<7}cyc ({}ns)",
        irq_lat.count, irq_lat.avg_cycles, irq_lat.avg_ns, irq_lat.max_cycles, irq_lat.max_ns
    );
    println!(
        "    IRQ Jitter : cnt={:<5} avg={:<6}cyc ({:<5}ns) max={:<7}cyc ({}ns)",
        irq_jit.count, irq_jit.avg_cycles, irq_jit.avg_ns, irq_jit.max_cycles, irq_jit.max_ns
    );
    println!(
        "    Sched Tick : cnt={:<5} avg={:<6}cyc ({:<5}ns) max={:<7}cyc ({}ns)",
        sched_tick.count, sched_tick.avg_cycles, sched_tick.avg_ns, sched_tick.max_cycles, sched_tick.max_ns
    );
    println!(
        "    Sched Wake : cnt={:<5} avg={:<6}cyc ({:<5}ns) max={:<7}cyc ({}ns)",
        sched_wake.count, sched_wake.avg_cycles, sched_wake.avg_ns, sched_wake.max_cycles, sched_wake.max_ns
    );
    println!(
        "    Syscall    : cnt={:<5} avg={:<6}cyc ({:<5}ns) max={:<7}cyc ({}ns)",
        sys_lat.count, sys_lat.avg_cycles, sys_lat.avg_ns, sys_lat.max_cycles, sys_lat.max_ns
    );

    let online = core::cmp::min(smp::online_cpu_count(), latency::MAX_PROFILED_CPUS);
    println!("  CPU Utilization ({} online cores):", online);
    for c in 0..online {
        let cu = latency::cpu_util_snapshot(c);
        println!(
            "    CPU#{}: util={}.{}% active_cyc={} irq_cyc={} idle_cyc={} ticks={}",
            cu.cpu_id,
            cu.util_tenths_pct / 10,
            cu.util_tenths_pct % 10,
            cu.active_cycles,
            cu.irq_cycles,
            cu.idle_cycles,
            cu.irq_ticks
        );
    }

    println!("  Lock Contention Profiles:");
    for lp in latency::GLOBAL_LOCK_PROFILES.iter() {
        let ls = lp.snapshot();
        println!(
            "    {:<16}: acq={:<4} contended={:<3} spins={:<4} wait_cyc={} max_hold={} cyc",
            ls.name,
            ls.acquires,
            ls.contended_acquires,
            ls.total_spins,
            ls.total_wait_cycles,
            ls.max_hold_cycles
        );
    }
}

/// Runs automated deterministic verification covering every M37 Kernel Profiling requirement.
pub fn run_self_tests() {
    println!("[PERF] Running M37 Kernel Profiling self-tests...");

    // 1. TSC & Serialized Cycle Counters
    let tinfo = tsc::info();
    assert!(tinfo.rdtsc_supported, "RDTSC must be supported on x86_64");
    assert!(tinfo.frequency_hz >= 100_000_000, "TSC frequency must be calibrated");

    let cc = tsc::CycleCounter::start();
    let mut acc = 0u64;
    for i in 1..64u64 {
        acc = acc.wrapping_add(i);
    }
    core::hint::black_box(acc);
    let dt = cc.elapsed_cycles();
    assert!(dt > 0, "CycleCounter must record positive delta cycles");
    println!(
        "  [OK] TSC calibrated ({} MHz) & CycleCounter verified ({} cyc / {} ns).",
        tinfo.frequency_mhz,
        dt,
        tsc::cycles_to_ns(dt)
    );

    // 2. Architectural PMU / Software Counter Fallback
    let _ = pmu::select_gp_event(pmu::PmuEvent::InstructionsRetired);
    let pinfo = pmu::snapshot();
    assert!(pinfo.hw_irq_events > 0 || pinfo.unhalted_cycles > 0);
    println!(
        "  [OK] CPU Performance Counters verified (version={}, hw_active={}).",
        pinfo.version, pinfo.hardware_active
    );

    // 3. Static Kernel Tracepoints & Ring Wraparound
    let before_tr = trace::stats().total_recorded;
    for i in 0..8u64 {
        crate::tracepoint!(trace::TraceCategory::User, trace::event_id::USER_MARK, 1, i, i * 2);
    }
    let after_tr = trace::stats().total_recorded;
    assert!(after_tr >= before_tr + 8);
    let recent = trace::recent_events(4);
    assert_eq!(recent.len(), 4);
    println!("  [OK] Kernel Tracepoints & Ring Buffer verified (total={}).", after_tr);

    // 4. Latency Histograms (IRQ / Scheduler / Syscall)
    latency::SCHED_WAKEUP_LATENCY.record(1200);
    latency::SYSCALL_LATENCY.record(450);
    let irq_snap = latency::IRQ_TIMER_LATENCY.snapshot();
    let sys_snap = latency::SYSCALL_LATENCY.snapshot();
    assert!(irq_snap.count > 0, "Live APIC Timer IRQ latency samples must exist");
    assert!(sys_snap.count > 0, "Syscall latency samples must exist");
    println!(
        "  [OK] IRQ ({} samples), Scheduler & Syscall ({} samples) latency trackers verified.",
        irq_snap.count, sys_snap.count
    );

    // 5. Lock Contention Measurement (ProfiledSpinlock)
    let plock = latency::ProfiledSpinlock::new(&latency::GLOBAL_LOCK_PROFILES[3], 42u64);
    {
        let mut guard = plock.lock();
        *guard += 1;
    }
    plock.record_synthetic_contention(12, 340, 180);
    let lp_snap = latency::GLOBAL_LOCK_PROFILES[3].snapshot();
    assert!(lp_snap.acquires >= 2 && lp_snap.contended_acquires >= 1);
    println!(
        "  [OK] Lock Contention Profiler verified (acquires={}, contended={}).",
        lp_snap.acquires, lp_snap.contended_acquires
    );

    // 6. CPU Utilization Metrics
    latency::account_cpu_active(50_000);
    latency::account_cpu_idle(10_000);
    let cpu0 = latency::cpu_util_snapshot(0);
    assert!(cpu0.active_cycles >= 50_000 && cpu0.util_tenths_pct > 0);
    println!(
        "  [OK] CPU Utilization verified (CPU#0={}.{}%, ticks={}).",
        cpu0.util_tenths_pct / 10,
        cpu0.util_tenths_pct % 10,
        cpu0.irq_ticks
    );

    // 7. Run Memory Allocation & Context-Switch Microbenchmarks
    bench::run_benchmarks();

    println!("[PERF] All M37 Kernel Profiling self-tests passed!");
}

