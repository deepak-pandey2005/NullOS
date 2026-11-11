//! # Kernel Optimization Coordinator (Phase 17 - M38)
//!
//! ## Overview
//! Brings together all M38 latency and scalability optimizations inside NullOS:
//! - [`locks`]: `CacheAligned`, `TicketSpinlock`, `TtasBackoffSpinlock`, `SeqLock`, prefetch hints.
//! - [`sched_opt`]: O(1) 64-level priority bitmap runqueue & work-stealing load balancer.
//! - [`mem_opt`]: Segregated `SlabCache`, per-CPU `FrameMagazine`, 2 MiB HugePage PDEs & batched TLB.
//! - [`syscall_opt`]: O(1) `FastSyscallTable` jump table & `CR4.FSGSBASE` fast TLS activation.
//! - [`numa`]: ACPI `SRAT`/`SLIT` NUMA domain topology & locality distance matrix.
//! - [`bench_reg`]: Comparative before/after benchmarks & automated regression gate.

#![allow(dead_code)]

pub mod bench_reg;
pub mod locks;
pub mod mem_opt;
pub mod numa;
pub mod sched_opt;
pub mod syscall_opt;

use crate::println;

/// Initializes M38 optimization subsystems (NUMA SRAT/SLIT discovery, FSGSBASE, Slab warming).
pub fn init() {
    numa::init();
    let sys_caps = syscall_opt::init_syscall_opt();
    let pg_caps = mem_opt::detect_paging_caps();
    mem_opt::GLOBAL_SLAB_POOL.warm_all();

    let ntopo = numa::snapshot();
    println!(
        "[OPT] M38 Optimizations active: Slab(7 classes) | NUMA={} node(s) (SRAT={}) | PCID={} FSGSBASE={}",
        ntopo.node_count,
        ntopo.srat_found,
        pg_caps.pcid_supported,
        sys_caps.fsgsbase_active
    );
}

/// Prints formatted M38 Kernel Optimization telemetry (`optstat` shell command).
pub fn print_status() {
    let pg_caps = mem_opt::detect_paging_caps();
    let sys_caps = syscall_opt::init_syscall_opt();
    let ntopo = numa::snapshot();

    println!("=== NullOS Kernel Optimization Status (M38) ===");
    println!(
        "  Paging Caps   : PCID={} INVPCID={} 1GB_Huge={} | invlpg_saves={}",
        pg_caps.pcid_supported,
        pg_caps.invpcid_supported,
        pg_caps.huge_1gb_supported,
        mem_opt::tlb_selective_saves()
    );
    println!(
        "  Syscall Opt   : FSGSBASE={} fast_table_hits={}",
        sys_caps.fsgsbase_active,
        syscall_opt::fast_dispatch_hits()
    );
    println!(
        "  Allocator Opt : SlabPool hits={} | Per-CPU FrameMagazines=8x{} frames",
        mem_opt::GLOBAL_SLAB_POOL.total_hits(),
        mem_opt::FRAME_MAGAZINE_CAP
    );
    println!(
        "  NUMA Topology : nodes={} srat={} slit={}",
        ntopo.node_count, ntopo.srat_found, ntopo.slit_found
    );
    for i in 0..ntopo.node_count {
        let node = ntopo.nodes[i];
        println!(
            "    Node #{}: cpus_mask={:#x} mem={} MiB local_dist={}",
            node.node_id,
            node.cpu_mask,
            node.mem_bytes / (1024 * 1024),
            ntopo.distance(node.node_id, node.node_id)
        );
    }
}

/// Runs automated M38 self-tests, comparative benchmarks, and regression gates.
pub fn run_self_tests() {
    init();
    bench_reg::run_regression_tests();
}

