//! Kernel Reliability, Testing, Fuzzing & Stability Subsystem (Phase 19 - M42).
//!
//! # Architecture Overview
//! Organizes comprehensive kernel verification suites callable both programmatically during
//! automated regression runs (`run_automated_tests` -> `reliability::run_all()`) and interactively
//! from the NullOS shell (`reliability status|unit|integration|fs|net|smp|stress|mem|fuzz|soak|all`):
//!
//! - [`unit_tests`]: Deterministic kernel unit tests (PMM, O(1) runqueue, Tar/VFS/CRC32, Spinlock).
//! - [`fs_net_tests`]: RamFS + FAT32 persistence tests & Ethernet/ARP/IPv4/Socket loopback tests.
//! - [`smp_stress_mem`]: Multi-core concurrency, heap/VFS/thread stress, & redzone/canary/W^X memory safety.
//! - [`fuzz_soak`]: Deterministic seeded PRNG parser fuzzing (`elf`, `tar`, `vfs`, `net`), cross-subsystem
//!   integration tests, and bounded long-running stability soak verification.

pub mod fs_net_tests;
pub mod fuzz_soak;
pub mod smp_stress_mem;
pub mod unit_tests;

use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Cumulative assertions passed across all reliability suites since boot.
static TOTAL_PASSED: AtomicUsize = AtomicUsize::new(0);
/// Cumulative assertions failed across all reliability suites since boot.
static TOTAL_FAILED: AtomicUsize = AtomicUsize::new(0);
/// Cumulative fuzz test vectors evaluated across parser fuzzers.
static TOTAL_FUZZ_CASES: AtomicUsize = AtomicUsize::new(0);
/// Cumulative long-running soak cycles completed cleanly.
static TOTAL_SOAK_CYCLES: AtomicUsize = AtomicUsize::new(0);

/// Summary counts returned by each individual reliability suite.
#[derive(Debug, Clone, Copy)]
pub struct SuiteCounts {
    /// Number of assertions that passed cleanly.
    pub passed: usize,
    /// Number of assertions that failed.
    pub failed: usize,
}

impl SuiteCounts {
    /// Constructs a new suite summary and accumulates it into global telemetry.
    pub fn record(passed: usize, failed: usize) -> Self {
        TOTAL_PASSED.fetch_add(passed, Ordering::Relaxed);
        TOTAL_FAILED.fetch_add(failed, Ordering::Relaxed);
        Self { passed, failed }
    }
}

/// Initializes the M42 reliability telemetry counters.
pub fn init() {
    TOTAL_PASSED.store(0, Ordering::Relaxed);
    TOTAL_FAILED.store(0, Ordering::Relaxed);
    TOTAL_FUZZ_CASES.store(0, Ordering::Relaxed);
    TOTAL_SOAK_CYCLES.store(0, Ordering::Relaxed);
}

/// Records additional fuzz iterations evaluated by the M42 parser fuzzer.
pub(crate) fn record_fuzz_cases(count: usize) {
    TOTAL_FUZZ_CASES.fetch_add(count, Ordering::Relaxed);
}

/// Records completed stability soak iterations.
pub(crate) fn record_soak_cycles(count: usize) {
    TOTAL_SOAK_CYCLES.fetch_add(count, Ordering::Relaxed);
}

/// Executes every M42 reliability suite in sequence and asserts zero failures.
pub fn run_all() {
    crate::println!("====================================================");
    crate::println!(" [M42] Running Full Kernel Reliability & QA Suite   ");
    crate::println!("====================================================");

    let u = unit_tests::run();
    let f = fs_net_tests::run_fs_tests();
    let n = fs_net_tests::run_net_tests();
    let s = smp_stress_mem::run_smp_tests();
    let st = smp_stress_mem::run_stress_tests();
    let mc = smp_stress_mem::run_mem_corruption_tests();
    let fz = fuzz_soak::run_fuzz_tests();
    let ig = fuzz_soak::run_integration_tests();
    let sk = fuzz_soak::run_soak_tests(24);

    let total_pass =
        u.passed + f.passed + n.passed + s.passed + st.passed + mc.passed + fz.passed + ig.passed + sk.passed;
    let total_fail =
        u.failed + f.failed + n.failed + s.failed + st.failed + mc.failed + fz.failed + ig.failed + sk.failed;

    crate::println!(
        "[M42] Summary: passed={}, failed={}, fuzz_cases={}, soak_cycles={}",
        total_pass,
        total_fail,
        TOTAL_FUZZ_CASES.load(Ordering::Relaxed),
        TOTAL_SOAK_CYCLES.load(Ordering::Relaxed)
    );
    assert_eq!(total_fail, 0, "M42 reliability suite recorded assertion failures!");
    crate::println!("[M42] All Reliability, Fuzzing, Stress, SMP & Stability checks passed!");
}

/// Runs a named M42 reliability sub-suite interactively (or `"all"`), returning `true` on 0 failures.
pub fn run_suite(name: &str, arg: Option<usize>) -> (String, bool) {
    let counts = match name {
        "unit" => unit_tests::run(),
        "fs" => fs_net_tests::run_fs_tests(),
        "net" => fs_net_tests::run_net_tests(),
        "smp" => smp_stress_mem::run_smp_tests(),
        "stress" => smp_stress_mem::run_stress_tests(),
        "mem" | "memory" | "corrupt" => smp_stress_mem::run_mem_corruption_tests(),
        "fuzz" => fuzz_soak::run_fuzz_tests(),
        "integration" => fuzz_soak::run_integration_tests(),
        "soak" | "stability" => fuzz_soak::run_soak_tests(arg.unwrap_or(24).clamp(1, 2000)),
        "all" => {
            run_all();
            return (format_status(), TOTAL_FAILED.load(Ordering::Relaxed) == 0);
        }
        _ => {
            return (
                format!(
                    "Unknown reliability suite '{}'. Available: status, unit, fs, net, smp, stress, mem, fuzz, integration, soak [N], all\n",
                    name
                ),
                false,
            );
        }
    };

    let msg = format!(
        "[M42] Suite '{}': passed={}, failed={}\n",
        name, counts.passed, counts.failed
    );
    (msg, counts.failed == 0)
}

/// Formats cumulative M42 reliability metrics for interactive inspection via `reliability status`.
pub fn format_status() -> String {
    format!(
        "=== NullOS Reliability Telemetry (M42) ===\n\
         \x20 Total Assertions Passed : {}\n\
         \x20 Total Assertions Failed : {}\n\
         \x20 Fuzz Corpus Evaluated   : {}\n\
         \x20 Stability Soak Cycles   : {}\n",
        TOTAL_PASSED.load(Ordering::Relaxed),
        TOTAL_FAILED.load(Ordering::Relaxed),
        TOTAL_FUZZ_CASES.load(Ordering::Relaxed),
        TOTAL_SOAK_CYCLES.load(Ordering::Relaxed)
    )
}

