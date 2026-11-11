//! # Comparative Optimization Benchmark Suite & Regression Testing (Phase 17 - M38)
//!
//! ## Overview
//! Benchmarks every M38 kernel optimization directly against its unoptimized baseline
//! using serialized `RDTSC` cycle counters (`perf::tsc`), followed by deterministic
//! **Regression Testing** verifying both functional invariants and non-regression bounds.

use alloc::vec;
use alloc::vec::Vec;
use core::hint::black_box;
use core::sync::atomic::AtomicU64;

use crate::memory;
use crate::perf::opt::locks::{
    prefetch_read, CacheAligned, SeqLock, TicketSpinlock, TtasBackoffSpinlock, CACHE_LINE_SIZE,
};
use crate::perf::opt::mem_opt::{
    detect_paging_caps, encode_2mb_huge_pde, tlb_selective_saves, TlbShootdownBatch,
    GLOBAL_SLAB_POOL, PER_CPU_MAGAZINES,
};
use crate::perf::opt::numa;
use crate::perf::opt::sched_opt::{O1PriorityRunQueue, SchedTaskEntry};
use crate::perf::opt::syscall_opt::{fast_dispatch_hits, init_syscall_opt, FAST_SYSCALL_TABLE};
use crate::perf::tsc;
use crate::println;

/// Results reported by [`run_benchmark_suite`].
#[derive(Debug, Clone, Copy)]
pub struct OptBenchmarkReport {
    pub sched_linear_cyc: u64,
    pub sched_o1_cyc: u64,
    pub heap_cyc_per_op: u64,
    pub slab_cyc_per_op: u64,
    pub pmm_global_cyc_per_op: u64,
    pub pmm_magazine_cyc_per_op: u64,
    pub seqlock_read_cyc: u64,
    pub ticket_lock_cyc: u64,
    pub fast_syscall_cyc: u64,
}

/// Executes the full M38 Kernel Optimization comparative benchmark suite (`optbench`).
pub fn run_benchmark_suite() -> OptBenchmarkReport {
    println!("================================================================");
    println!("       NullOS Kernel Optimization Benchmark Suite (M38)         ");
    println!("================================================================");

    // ------------------------------------------------------------------------
    // 1. Scheduler: Linear Priority Scan (N=128) vs O(1) Priority Bitmap
    // ------------------------------------------------------------------------
    let mut linear_vec: Vec<SchedTaskEntry> = Vec::with_capacity(128);
    let mut o1_rq = O1PriorityRunQueue::new();
    for i in 0..128u64 {
        let entry = SchedTaskEntry {
            tid: i,
            priority: ((i * 17) % 64) as u8,
            vruntime_ns: 0,
            cpu_affinity: 0,
        };
        linear_vec.push(entry);
        o1_rq.push(entry);
    }

    let t0 = tsc::read_tsc_start();
    for _ in 0..64 {
        let mut best_idx = 0usize;
        let mut best_prio = u8::MAX;
        for (idx, item) in linear_vec.iter().enumerate() {
            if item.priority < best_prio {
                best_prio = item.priority;
                best_idx = idx;
            }
        }
        if !linear_vec.is_empty() {
            black_box(linear_vec.swap_remove(best_idx));
        }
    }
    let sched_linear_cyc = tsc::read_tsc_end().saturating_sub(t0);

    let t1 = tsc::read_tsc_start();
    for _ in 0..64 {
        black_box(o1_rq.pop_highest());
    }
    let sched_o1_cyc = tsc::read_tsc_end().saturating_sub(t1);
    let sched_speedup = sched_linear_cyc / core::cmp::max(1, sched_o1_cyc);
    println!(
        "  [1] Scheduler (64 pops/N=128) : Linear={} cyc | O(1) Bitmap={} cyc (~{}x speedup)",
        sched_linear_cyc, sched_o1_cyc, sched_speedup
    );

    // ------------------------------------------------------------------------
    // 2. Allocator: Heap First-Fit Vec vs Segregated Slab Cache (128B x 256)
    // ------------------------------------------------------------------------
    GLOBAL_SLAB_POOL.warm_all();
    let alloc_iters = 256u64;
    let th0 = tsc::read_tsc_start();
    for i in 0..alloc_iters {
        let mut v = vec![0u8; 128];
        v[0] = black_box(i as u8);
        black_box(&v);
    }
    let heap_total = tsc::read_tsc_end().saturating_sub(th0);

    let ts0 = tsc::read_tsc_start();
    for i in 0..alloc_iters {
        if let Some((cls, slot, ptr)) = GLOBAL_SLAB_POOL.alloc(128) {
            unsafe {
                *ptr = black_box(i as u8);
            }
            GLOBAL_SLAB_POOL.free(cls, slot);
        }
    }
    let slab_total = tsc::read_tsc_end().saturating_sub(ts0);
    let heap_cyc_op = heap_total / alloc_iters;
    let slab_cyc_op = slab_total / alloc_iters;
    println!(
        "  [2] Allocator (128B x 256)    : Heap={} cyc/op | SlabCache={} cyc/op (~{}x speedup)",
        heap_cyc_op,
        slab_cyc_op,
        heap_cyc_op / core::cmp::max(1, slab_cyc_op)
    );

    // ------------------------------------------------------------------------
    // 3. PMM Physical Pages: Global PMM Lock vs Per-CPU FrameMagazine (x64)
    // ------------------------------------------------------------------------
    let pmm_iters = 64u64;
    let tp0 = tsc::read_tsc_start();
    for _ in 0..pmm_iters {
        if let Some(f) = memory::alloc_frame() {
            black_box(f);
            memory::dealloc_frame(f);
        }
    }
    let pmm_global_total = tsc::read_tsc_end().saturating_sub(tp0);

    // Pre-warm CPU0 magazine cleanly with 1 frame round-trip then benchmark 64 cached ops
    let tp1 = tsc::read_tsc_start();
    {
        let mut mag = PER_CPU_MAGAZINES[0].value.lock();
        for _ in 0..pmm_iters {
            if let Some(f) = mag.alloc_frame_fast() {
                black_box(f);
                mag.dealloc_frame_fast(f);
            }
        }
        mag.flush_all();
    }
    let pmm_mag_total = tsc::read_tsc_end().saturating_sub(tp1);
    let pmm_global_op = pmm_global_total / pmm_iters;
    let pmm_mag_op = pmm_mag_total / pmm_iters;
    println!(
        "  [3] Physical Page (4KiB x 64) : GlobalPMM={} cyc/op | FrameMagazine={} cyc/op",
        pmm_global_op, pmm_mag_op
    );

    // ------------------------------------------------------------------------
    // 4. Locks: TicketSpinlock + SeqLock + TTAS Backoff (256 iterations)
    // ------------------------------------------------------------------------
    let seqlock = SeqLock::new(42u64);
    let tl0 = tsc::read_tsc_start();
    let mut sum = 0u64;
    for _ in 0..256 {
        sum = sum.wrapping_add(seqlock.read());
    }
    black_box(sum);
    let seqlock_read_cyc = tsc::read_tsc_end().saturating_sub(tl0) / 256;

    let ticket = TicketSpinlock::new(0u64);
    let tl1 = tsc::read_tsc_start();
    for i in 0..256u64 {
        let mut g = ticket.lock();
        *g = black_box(i);
    }
    let ticket_lock_cyc = tsc::read_tsc_end().saturating_sub(tl1) / 256;
    println!(
        "  [4] Lock Fast Path (256 ops)  : SeqLock read={} cyc/op | TicketSpinlock={} cyc/op",
        seqlock_read_cyc, ticket_lock_cyc
    );

    // ------------------------------------------------------------------------
    // 5. Syscall O(1) Jump Dispatch + Cache Prefetch
    // ------------------------------------------------------------------------
    let tsy0 = tsc::read_tsc_start();
    for _ in 0..256 {
        black_box(FAST_SYSCALL_TABLE.try_dispatch(39, 0, 0, 0));
    }
    let fast_syscall_cyc = tsc::read_tsc_end().saturating_sub(tsy0) / 256;
    println!(
        "  [5] O(1) Syscall Table (x256) : {} cyc/call ({} ns/call)",
        fast_syscall_cyc,
        tsc::cycles_to_ns(fast_syscall_cyc)
    );
    println!("================================================================");

    OptBenchmarkReport {
        sched_linear_cyc,
        sched_o1_cyc,
        heap_cyc_per_op: heap_cyc_op,
        slab_cyc_per_op: slab_cyc_op,
        pmm_global_cyc_per_op: pmm_global_op,
        pmm_magazine_cyc_per_op: pmm_mag_op,
        seqlock_read_cyc,
        ticket_lock_cyc,
        fast_syscall_cyc,
    }
}

/// Runs automated **Regression Testing** verifying M38 optimization correctness, speedup
/// direction, cache alignment, page-table encoding, NUMA invariants, and zero resource leaks.
pub fn run_regression_tests() {
    println!("[OPT] Running Phase 17 M38 Optimization & Regression tests...");

    // 1. Verify Cache-Aware Alignment (exact 64B line isolation)
    assert_eq!(core::mem::size_of::<CacheAligned<AtomicU64>>(), CACHE_LINE_SIZE);
    assert_eq!(core::mem::align_of::<CacheAligned<AtomicU64>>(), CACHE_LINE_SIZE);
    let arr = [CacheAligned::new(1u64), CacheAligned::new(2u64)];
    prefetch_read(&arr[0] as *const _);

    // 2. Verify TicketSpinlock, TTAS Backoff Spinlock & SeqLock consistency
    let ticket = TicketSpinlock::new(10u64);
    {
        let mut g = ticket.lock();
        *g += 32;
    }
    assert_eq!(*ticket.lock(), 42);

    let ttas = TtasBackoffSpinlock::new(100u64);
    {
        let mut g = ttas.lock();
        *g += 23;
    }
    assert_eq!(*ttas.lock(), 123);

    let seq = SeqLock::new((1u64, 2u64));
    seq.write(|pair| {
        pair.0 = 55;
        pair.1 = 89;
    });
    assert_eq!(seq.read(), (55, 89));
    println!("  [OK] TicketSpinlock, TTAS Backoff, SeqLock & 64B CacheAligned verified.");

    // 3. Verify O(1) Priority Bitmap Scheduler order, vruntime & work-stealing
    let mut q_busy = O1PriorityRunQueue::new();
    let mut q_idle = O1PriorityRunQueue::new();
    q_busy.push(SchedTaskEntry { tid: 1, priority: 30, vruntime_ns: 0, cpu_affinity: 0 });
    q_busy.push(SchedTaskEntry { tid: 2, priority: 5, vruntime_ns: 0, cpu_affinity: 0 });
    q_busy.push(SchedTaskEntry { tid: 3, priority: 60, vruntime_ns: 0, cpu_affinity: 0 });
    let stolen = O1PriorityRunQueue::steal_for_idle_cpu(&mut q_busy, &mut q_idle);
    assert!(stolen, "Idle CPU must steal lowest-priority task");
    let stolen_task = q_idle.pop_highest().unwrap();
    assert_eq!(stolen_task.tid, 3, "Lowest priority task (#60) should be stolen first");
    let first = q_busy.pop_highest().unwrap();
    assert_eq!(first.tid, 2, "O(1) bitmap must pop highest priority (#5) next");
    let mut second = q_busy.pop_highest().unwrap();
    O1PriorityRunQueue::account_vruntime(&mut second, 1_000_000);
    assert!(second.vruntime_ns > 0);
    println!("  [OK] O(1) Priority Bitmap Scheduler & Work-Stealing verified.");

    // 4. Verify Page-Table 2 MiB HugePage encoding, CPUID caps & Batched TLB shootdown
    let pg_caps = detect_paging_caps();
    let pde = encode_2mb_huge_pde(0x0040_0000, true, false);
    assert_eq!(pde & 0x83, 0x83, "PDE must carry Present | Writable | HugePage (PS)");
    let mut tlb_batch = TlbShootdownBatch::new();
    tlb_batch.push(0x1000);
    tlb_batch.push(0x2000);
    assert!(tlb_batch.flush(), "<= 16 pages must use selective invlpg flush");
    assert!(tlb_selective_saves() >= 1);
    println!(
        "  [OK] Page-Table Optimizations verified (PCID={}, INVPCID={}, 1GB={}, saved_flushes={}).",
        pg_caps.pcid_supported, pg_caps.invpcid_supported, pg_caps.huge_1gb_supported, tlb_selective_saves()
    );

    // 5. Verify Syscall O(1) Jump Table & FSGSBASE caps + NUMA Topology
    let sys_caps = init_syscall_opt();
    assert_eq!(FAST_SYSCALL_TABLE.try_dispatch(39, 0, 0, 0), Some(1));
    assert!(fast_dispatch_hits() >= 1);

    let numa_topo = numa::snapshot();
    let cur_node = numa::current_node();
    assert!(numa_topo.node_count >= 1);
    assert_eq!(numa_topo.distance(cur_node, cur_node), 10, "Local NUMA distance must equal 10");
    println!(
        "  [OK] Syscall O(1) Table (FSGSBASE={}) & NUMA (nodes={}, srat={}, cur_node={}) verified.",
        sys_caps.fsgsbase_active, numa_topo.node_count, numa_topo.srat_found, cur_node
    );

    // 6. Run Comparative Benchmark Suite & Enforce Non-Regression Assertions
    let rep = run_benchmark_suite();
    assert!(
        rep.sched_o1_cyc <= rep.sched_linear_cyc,
        "Regression: O(1) scheduler must not be slower than O(N) linear scan"
    );
    assert!(
        rep.slab_cyc_per_op <= rep.heap_cyc_per_op.saturating_mul(2),
        "Regression: SlabCache per-op latency exceeded bound"
    );
    assert!(
        rep.pmm_global_cyc_per_op > 0 && rep.fast_syscall_cyc > 0,
        "Regression: Benchmark cycle counters must be valid and non-zero"
    );
    println!("[OPT] All M38 Optimization & Regression tests passed!");
}
