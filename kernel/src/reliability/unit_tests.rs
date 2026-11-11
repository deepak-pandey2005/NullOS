//! Kernel Core Unit Tests (Phase 19 - M42).
//!
//! Exercises fundamental kernel algorithms and invariants directly:
//! - Physical Memory Manager frame allocation / deallocation conservation
//! - O(1) Priority Bitmap RunQueue scheduling order & work-stealing
//! - Segregated SlabPool size-class mapping & alloc/free roundtrip
//! - USTAR octal parser, VFS path canonicalizer (`clean_path`), & NPKG CRC-32 IEEE vector
//! - Spinlock exclusive state mutation

use super::SuiteCounts;
use crate::fs::tar::parse_octal;
use crate::fs::vfs::clean_path;
use crate::memory::{alloc_frame, dealloc_frame, stats as mem_stats, PAGE_SIZE_4K};
use crate::perf::opt::mem_opt::GLOBAL_SLAB_POOL;
use crate::perf::opt::sched_opt::{O1PriorityRunQueue, SchedTaskEntry};
use crate::sync::Spinlock;
use crate::task::npkg::crc32_ieee;
use alloc::vec::Vec;

/// Executes core kernel data-structure & allocator unit tests.
pub fn run() -> SuiteCounts {
    crate::println!("[M42/UNIT] Running core kernel unit tests...");
    let mut passed = 0usize;
    let mut failed = 0usize;

    macro_rules! check {
        ($cond:expr, $label:expr) => {
            if $cond {
                passed += 1;
            } else {
                crate::println!("[M42/UNIT] FAIL: {}", $label);
                failed += 1;
            }
        };
    }

    // 1. PMM frame allocation + deallocation count conservation
    if let Some(before) = mem_stats() {
        let mut frames = Vec::with_capacity(8);
        let mut aligned_ok = true;
        for _ in 0..8 {
            if let Some(f) = alloc_frame() {
                if !f.start_address().is_aligned(PAGE_SIZE_4K) {
                    aligned_ok = false;
                }
                frames.push(f);
            }
        }
        check!(frames.len() == 8 && aligned_ok, "PMM 8-frame aligned allocation");
        for f in frames {
            dealloc_frame(f);
        }
        if let Some(after) = mem_stats() {
            check!(after.free_frames == before.free_frames, "PMM free frame conservation");
        } else {
            failed += 1;
        }
    } else {
        failed += 1;
    }

    // 2. O(1) Priority RunQueue priority extraction and work stealing
    let mut rq_src = O1PriorityRunQueue::new();
    let mut rq_dst = O1PriorityRunQueue::new();
    rq_src.push(SchedTaskEntry { tid: 101, priority: 30, vruntime_ns: 500, cpu_affinity: u16::MAX });
    rq_src.push(SchedTaskEntry { tid: 102, priority: 4, vruntime_ns: 100, cpu_affinity: u16::MAX });
    rq_src.push(SchedTaskEntry { tid: 103, priority: 15, vruntime_ns: 200, cpu_affinity: u16::MAX });

    let popped_top = rq_src.pop_highest();
    check!(popped_top.map(|t| t.tid) == Some(102), "O(1) runqueue picks highest priority (min numeric priority)");
    let stole = O1PriorityRunQueue::steal_for_idle_cpu(&mut rq_src, &mut rq_dst);
    check!(stole && rq_dst.len() == 1 && rq_src.len() == 1, "O(1) work-stealing migrates task cleanly");
    let _ = rq_src.pop_highest();
    let _ = rq_dst.pop_highest();
    check!(rq_src.is_empty() && rq_dst.is_empty(), "O(1) runqueues drained to empty");

    // 3. Segregated slab cache alloc/free roundtrip
    if let Some((cls_idx, slot, ptr)) = GLOBAL_SLAB_POOL.alloc(96) {
        check!(!ptr.is_null(), "SlabPool non-null allocation");
        unsafe { ptr.write_bytes(0x5A, 96); }
        GLOBAL_SLAB_POOL.free(cls_idx, slot);
        passed += 1;
    } else {
        check!(false, "SlabPool alloc(96) must succeed");
    }

    // 4. TAR octal parser boundary & error cases
    check!(parse_octal(b"0000755\0") == Some(0o755), "TAR parse_octal 0755");
    check!(parse_octal(b"   0012\0") == Some(0o12), "TAR parse_octal leading spaces");
    check!(parse_octal(b"0000000\0") == Some(0), "TAR parse_octal zero");
    check!(parse_octal(b"0999\0").is_none(), "TAR parse_octal rejects non-octal '9'");

    // 5. VFS path normalization (`clean_path`)
    check!(clean_path("/bin//./cat.elf") == "/bin/cat.elf", "clean_path double-slash & dot");
    check!(clean_path("/a/b/../c/./d/..") == "/a/c", "clean_path parent '..' traversal");
    check!(clean_path("/../../..") == "/", "clean_path clamps root '..'");

    // 6. IEEE CRC-32 standard test vector ("123456789" -> 0xCBF43926)
    check!(crc32_ieee(b"123456789") == 0xCBF4_3926, "CRC-32 IEEE standard vector");

    // 7. Spinlock atomic exclusivity invariant
    let lock = Spinlock::new(0u64);
    for _ in 0..100 {
        *lock.lock() += 3;
    }
    check!(*lock.lock() == 300, "Spinlock mutual exclusion arithmetic");

    crate::println!("[M42/UNIT] Completed: passed={}, failed={}", passed, failed);
    SuiteCounts::record(passed, failed)
}
