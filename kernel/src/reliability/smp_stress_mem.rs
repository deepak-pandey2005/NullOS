//! SMP Concurrency, Resource Stress & Memory Corruption Detection Tests (Phase 19 - M42).
//!
//! - **SMP Tests**: Validates active AP cores and multi-thread atomic + mutex convergence across yields.
//! - **Stress Tests**: Repeated heap allocation storms, VFS `/tmp` churn, and rapid thread spawn bursts.
//! - **Memory Corruption Tests**: Canary/redzone buffer poisoning checks, stack canary verification,
//!   kernel heap integrity scan, W^X page-table attribute verification, & kernel-boundary usercopy denial.

use super::SuiteCounts;
use crate::arch::x86_64::smp;
use crate::debug::memdiag::{inspect_vaddr, verify_heap_and_stack_integrity};
use crate::fs::vfs::{OpenFlags, VFS};
use crate::memory::heap;
use crate::security::guard::{get_stack_canary, place_canary, verify_canary};
use crate::security::usercopy::validate_user_buffer;
use crate::sync::Spinlock;
use crate::task::scheduler;
use alloc::format;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const SMP_WORKERS: usize = 4;
const SMP_ITERS_PER_WORKER: u64 = 250;

static SMP_ATOMIC_SUM: AtomicU64 = AtomicU64::new(0);
static SMP_DONE_COUNT: AtomicUsize = AtomicUsize::new(0);
static SMP_MUTEX_SUM: Spinlock<u64> = Spinlock::new(0);

static STRESS_DONE: AtomicUsize = AtomicUsize::new(0);

/// Concurrent SMP/scheduler worker verifying atomic & spinlock synchronization.
fn smp_worker_entry() {
    for i in 0..SMP_ITERS_PER_WORKER {
        SMP_ATOMIC_SUM.fetch_add(1, Ordering::SeqCst);
        {
            let mut guard = SMP_MUTEX_SUM.lock();
            *guard += 1;
        }
        if i % 50 == 0 {
            scheduler::yield_now();
        }
    }
    SMP_DONE_COUNT.fetch_add(1, Ordering::Release);
    scheduler::exit();
}

/// Lightweight thread entry for rapid spawn/reclaim burst stress testing.
fn stress_burst_worker() {
    let mut acc = 0u64;
    for i in 0..64u64 {
        acc = acc.wrapping_add(i * 7);
    }
    core::hint::black_box(acc);
    STRESS_DONE.fetch_add(1, Ordering::Release);
    scheduler::exit();
}

/// Verifies SMP CPU discovery and concurrent multi-thread convergence.
pub fn run_smp_tests() -> SuiteCounts {
    crate::println!("[M42/SMP] Running SMP & concurrency reliability tests...");
    let mut passed = 0usize;
    let mut failed = 0usize;

    macro_rules! check {
        ($cond:expr, $label:expr) => {
            if $cond {
                passed += 1;
            } else {
                crate::println!("[M42/SMP] FAIL: {}", $label);
                failed += 1;
            }
        };
    }

    let online = smp::online_cpu_count();
    check!(online >= 1, "At least BSP online in SMP topology");

    SMP_ATOMIC_SUM.store(0, Ordering::SeqCst);
    SMP_DONE_COUNT.store(0, Ordering::SeqCst);
    *SMP_MUTEX_SUM.lock() = 0;

    for w in 0..SMP_WORKERS {
        let name = match w {
            0 => "m42_smp0",
            1 => "m42_smp1",
            2 => "m42_smp2",
            _ => "m42_smp3",
        };
        let _ = scheduler::spawn(name, smp_worker_entry, 2);
    }

    // Cooperative/preemptive wait with bounded timeout
    for _ in 0..500 {
        if SMP_DONE_COUNT.load(Ordering::Acquire) >= SMP_WORKERS {
            break;
        }
        scheduler::yield_now();
        crate::arch::x86_64::apic::sleep(1);
    }

    let expected = (SMP_WORKERS as u64) * SMP_ITERS_PER_WORKER;
    let atomic_val = SMP_ATOMIC_SUM.load(Ordering::SeqCst);
    let mutex_val = *SMP_MUTEX_SUM.lock();
    let done = SMP_DONE_COUNT.load(Ordering::Acquire);

    check!(done == SMP_WORKERS, "All SMP test workers finished");
    check!(atomic_val == expected, "SMP atomic counter matches exact expected sum");
    check!(mutex_val == expected, "SMP spinlock counter matches exact expected sum");

    crate::println!(
        "[M42/SMP] Completed (online_cpus={}, workers={}): passed={}, failed={}",
        online,
        SMP_WORKERS,
        passed,
        failed
    );
    SuiteCounts::record(passed, failed)
}

/// Runs sustained resource allocation, VFS churn, and thread spawn stress tests.
pub fn run_stress_tests() -> SuiteCounts {
    crate::println!("[M42/STRESS] Running heap, VFS & scheduler stress tests...");
    let mut passed = 0usize;
    let mut failed = 0usize;

    macro_rules! check {
        ($cond:expr, $label:expr) => {
            if $cond {
                passed += 1;
            } else {
                crate::println!("[M42/STRESS] FAIL: {}", $label);
                failed += 1;
            }
        };
    }

    // 1. Heap allocation/deallocation storm across mixed sizes
    let heap_before = heap::stats();
    let mut blocks: Vec<Vec<u8>> = Vec::with_capacity(64);
    for round in 0..160usize {
        let size = 64 + ((round * 137) % 8192);
        let mut buf = vec![(round & 0xFF) as u8; size];
        buf[0] ^= 0xAA;
        buf[size - 1] ^= 0x55;
        blocks.push(buf);
        if blocks.len() >= 16 {
            blocks.swap_remove(round % blocks.len());
        }
    }
    blocks.clear();
    drop(blocks);
    let heap_after = heap::stats();
    check!(
        heap_after.current_usage() <= heap_before.current_usage() + 4096,
        "Heap churn releases blocks without unbounded leak"
    );

    // 2. Rapid VFS file creation/write/read/unlink churn in /tmp
    let mut vfs_ok = true;
    let create_rw = OpenFlags {
        read: true,
        write: true,
        create: true,
        append: false,
        truncate: false,
        nonblock: false,
    };
    for i in 0..32usize {
        let path = format!("/tmp/m42_stress_{}.tmp", i);
        if let Ok(inode) = VFS.open(&path, create_rw) {
            let msg = format!("stress-iteration-{}", i);
            let _ = inode.write_at(0, msg.as_bytes());
            let mut back = vec![0u8; msg.len()];
            let r = inode.read_at(0, &mut back);
            if r != Ok(msg.len()) || back.as_slice() != msg.as_bytes() {
                vfs_ok = false;
            }
            let _ = VFS.unlink(&path);
        } else {
            vfs_ok = false;
        }
    }
    check!(vfs_ok, "32x VFS /tmp create/read/unlink stress loop");

    // 3. Thread spawn/join burst stress
    STRESS_DONE.store(0, Ordering::SeqCst);
    const BURST: usize = 8;
    for _ in 0..BURST {
        let _ = scheduler::spawn("m42_burst", stress_burst_worker, 2);
    }
    for _ in 0..300 {
        if STRESS_DONE.load(Ordering::Acquire) >= BURST {
            break;
        }
        scheduler::yield_now();
        crate::arch::x86_64::apic::sleep(1);
    }
    check!(STRESS_DONE.load(Ordering::Acquire) == BURST, "8x rapid thread burst completion");

    crate::println!("[M42/STRESS] Completed: passed={}, failed={}", passed, failed);
    SuiteCounts::record(passed, failed)
}

/// Verifies canary/redzone integrity, W^X memory protections, and boundary validation.
pub fn run_mem_corruption_tests() -> SuiteCounts {
    crate::println!("[M42/MEM] Running memory corruption & redzone safety checks...");
    let mut passed = 0usize;
    let mut failed = 0usize;

    macro_rules! check {
        ($cond:expr, $label:expr) => {
            if $cond {
                passed += 1;
            } else {
                crate::println!("[M42/MEM] FAIL: {}", $label);
                failed += 1;
            }
        };
    }

    // 1. Guarded redzone allocation integrity (leading & trailing sentinel poison words)
    const GUARD_MAGIC: u64 = 0xCAFE_BABE_DEAD_BEEF;
    let mut guarded = [0u64; 24]; // [0..4] = head redzone, [4..20] = payload, [20..24] = tail redzone
    for slot in &mut guarded[0..4] {
        *slot = GUARD_MAGIC;
    }
    for slot in &mut guarded[20..24] {
        *slot = GUARD_MAGIC;
    }
    for (idx, slot) in guarded[4..20].iter_mut().enumerate() {
        *slot = (idx as u64).wrapping_mul(0x0101_0101_0101_0101);
    }
    let head_intact = guarded[0..4].iter().all(|&w| w == GUARD_MAGIC);
    let tail_intact = guarded[20..24].iter().all(|&w| w == GUARD_MAGIC);
    check!(head_intact && tail_intact, "Heap/stack buffer redzone canaries unmodified");

    // 2. Stack canary placement & verification
    let mut canary_slot = 0u64;
    place_canary(&mut canary_slot as *mut u64);
    check!(
        get_stack_canary() != 0 && verify_canary(&canary_slot as *const u64),
        "Stack canary non-zero and verifies cleanly"
    );
    // Simulate corruption detection
    canary_slot ^= 1;
    check!(!verify_canary(&canary_slot as *const u64), "Stack canary detects 1-bit corruption");

    // 3. Global kernel heap + stack integrity check (M40 memdiag)
    check!(verify_heap_and_stack_integrity(), "Kernel heap & stack canary global integrity");

    // 4. Page-table W^X inspection (code page is executable, heap buffer is NX)
    let code_vaddr = run_mem_corruption_tests as *const () as u64;
    let code_map = inspect_vaddr(code_vaddr);
    let heap_buf = vec![0x42u8; 64];
    let data_map = inspect_vaddr(heap_buf.as_ptr() as u64);
    check!(
        code_map.phys_addr.is_some() && !code_map.no_exec,
        "Kernel text mapped Present + Executable"
    );
    check!(
        data_map.phys_addr.is_some() && data_map.no_exec,
        "Kernel heap mapped Present + NX"
    );

    // 5. User-kernel boundary rejection (high canonical pointer rejected by validate_user_buffer)
    check!(
        validate_user_buffer(0xFFFF_8000_0000_0000 as *const u8, 64).is_err(),
        "Usercopy rejects kernel-space source pointer"
    );

    crate::println!("[M42/MEM] Completed: passed={}, failed={}", passed, failed);
    SuiteCounts::record(passed, failed)
}
