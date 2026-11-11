//! # Kernel Profiling Microbenchmarks (Phase 17 - M37)
//!
//! ## Overview
//! Measures core OS hot-path performance in CPU cycles (`RDTSC` serialized) and
//! calibrated nanoseconds:
//! 1. **Memory Allocation Benchmarks**:
//!    - Physical Frame Allocator (`alloc_frame` / `dealloc_frame`) 4 KiB page cycle latency.
//!    - Kernel Heap (`Vec<u8>`) small (64B), medium (512B), and page-sized (4096B) alloc/free.
//! 2. **Context-Switch Benchmarks**:
//!    - Cooperative thread ping-pong context switches (`switch_threads` assembly save/restore +
//!      TSS `RSP0` + `IA32_FS_BASE` MSR reload + scheduler queue hop).
//!    - Scheduler empty-queue `yield_now()` fast-path check cycles.
//! 3. **Tracepoint & Profiled Lock Overhead**:
//!    - Cost of emitting a static kernel tracepoint vs uncontended `ProfiledSpinlock`.

use alloc::vec;
use core::hint::black_box;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::memory;
use crate::perf::latency::{ProfiledSpinlock, GLOBAL_LOCK_PROFILES};
use crate::perf::trace::{event_id, TraceCategory};
use crate::perf::tsc;
use crate::println;
use crate::task;

static CTX_PING: AtomicBool = AtomicBool::new(false);
static CTX_DONE: AtomicBool = AtomicBool::new(false);
static CTX_ROUNDS: AtomicU64 = AtomicU64::new(0);

fn context_switch_worker() {
    let mut done_rounds = 0u64;
    while done_rounds < 32 {
        while !CTX_PING.load(Ordering::Acquire) {
            if CTX_DONE.load(Ordering::Acquire) {
                return;
            }
            task::yield_now();
        }
        CTX_ROUNDS.fetch_add(1, Ordering::Release);
        CTX_PING.store(false, Ordering::Release);
        done_rounds += 1;
        task::yield_now();
    }
}

/// Runs the Phase 17 M37 kernel profiling benchmark suite (`kbench` / `perfbench`).
pub fn run_benchmarks() {
    println!("================================================================");
    println!("         NullOS Kernel Profiling Benchmarks (M37)               ");
    println!("================================================================");

    // 1. Memory Allocation Benchmarks — Physical Frame Allocator (PMM)
    let pmm_iters = 128u64;
    let t0 = tsc::read_tsc_start();
    for _ in 0..pmm_iters {
        if let Some(frame) = memory::alloc_frame() {
            black_box(frame);
            memory::dealloc_frame(frame);
        }
    }
    let pmm_cycles = tsc::read_tsc_end().saturating_sub(t0);
    println!(
        "  [1] PMM Frame Alloc+Free (4KiB x {}): {} cyc ({} cyc/op, {} ns/op)",
        pmm_iters,
        pmm_cycles,
        pmm_cycles / pmm_iters,
        tsc::cycles_to_ns(pmm_cycles / pmm_iters)
    );

    // 2. Memory Allocation Benchmarks — Kernel Heap Allocator (64B, 512B, 4096B)
    for &sz in &[64usize, 512usize, 4096usize] {
        let heap_iters = 256u64;
        let t = tsc::read_tsc_start();
        for _ in 0..heap_iters {
            let mut v = vec![0u8; sz];
            v[0] = black_box(0x5A);
            v[sz - 1] = black_box(0xA5);
            black_box(&v);
        }
        let cyc = tsc::read_tsc_end().saturating_sub(t);
        println!(
            "  [2] Heap Alloc+Free ({:>4}B x {})   : {} cyc ({} cyc/op, {} ns/op)",
            sz,
            heap_iters,
            cyc,
            cyc / heap_iters,
            tsc::cycles_to_ns(cyc / heap_iters)
        );
    }

    // 3. Context-Switch Benchmark — Real Kernel Thread Ping-Pong Switches
    CTX_PING.store(false, Ordering::Release);
    CTX_DONE.store(false, Ordering::Release);
    CTX_ROUNDS.store(0, Ordering::Release);

    let _worker_tid = task::scheduler::spawn("perf_ctx_bench", context_switch_worker, 1);

    let t_ctx_start = tsc::read_tsc_start();
    let mut bounces = 0u64;
    while bounces < 32 {
        CTX_PING.store(true, Ordering::Release);
        while CTX_PING.load(Ordering::Acquire) {
            task::yield_now();
        }
        bounces += 1;
    }
    let ctx_cycles = tsc::read_tsc_end().saturating_sub(t_ctx_start);
    CTX_DONE.store(true, Ordering::Release);
    task::yield_now(); // Allow worker thread to exit cleanly

    // Each bounce switches A -> B -> A = 2 real kernel context switches (minimum)
    let total_switches = core::cmp::max(1, bounces * 2);
    println!(
        "  [3] Thread Context Switch (64 hops)  : {} cyc ({} cyc/switch, {} ns/switch)",
        ctx_cycles,
        ctx_cycles / total_switches,
        tsc::cycles_to_ns(ctx_cycles / total_switches)
    );

    // 4. Tracepoint & ProfiledSpinlock Overhead
    let trace_iters = 128u64;
    let t_tr = tsc::read_tsc_start();
    for i in 0..trace_iters {
        crate::tracepoint!(TraceCategory::User, event_id::USER_MARK, 0, i, 1);
    }
    let tr_cycles = tsc::read_tsc_end().saturating_sub(t_tr);
    println!(
        "  [4] Static Tracepoint Emit (x{})    : {} cyc ({} cyc/ev, {} ns/ev)",
        trace_iters,
        tr_cycles,
        tr_cycles / trace_iters,
        tsc::cycles_to_ns(tr_cycles / trace_iters)
    );

    let lock = ProfiledSpinlock::new(&GLOBAL_LOCK_PROFILES[3], 0u64);
    let lock_iters = 256u64;
    let t_lk = tsc::read_tsc_start();
    for i in 0..lock_iters {
        let mut g = lock.lock();
        *g = black_box(i);
    }
    let lk_cycles = tsc::read_tsc_end().saturating_sub(t_lk);
    println!(
        "  [5] ProfiledSpinlock Uncontended     : {} cyc ({} cyc/lock, {} ns/lock)",
        lk_cycles,
        lk_cycles / lock_iters,
        tsc::cycles_to_ns(lk_cycles / lock_iters)
    );

    println!("================================================================");
}

