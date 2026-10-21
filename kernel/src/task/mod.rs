//! NullOS Task Execution Subsystem (Phase 4).
//!
//! Subsystems:
//! - M10: Kernel Threads, Stack Frames, and Assembly Context Switching
//! - M11: Preemptive Multitasking Round-Robin Scheduler & Sleep Primitives
#![allow(unused_imports, dead_code)]

pub mod elf;
pub mod process;
pub mod scheduler;
pub mod syscall;
pub mod thread;

pub use elf::{load_user_elf, LoadedUserElf};
pub use process::{create_process, exit_process, process_count, spawn_init_process, waitpid, Process, ProcessId, ProcessState};
pub use scheduler::{exit, init, sleep, spawn, stats, timer_tick, yield_now};
pub use syscall::run_user_mode_self_test;
pub use thread::{Thread, ThreadId, ThreadState};

use crate::println;
use core::sync::atomic::{AtomicUsize, Ordering};

static TEST_COUNTER_A: AtomicUsize = AtomicUsize::new(0);
static TEST_COUNTER_B: AtomicUsize = AtomicUsize::new(0);

fn test_worker_a() {
    for i in 1..=3 {
        TEST_COUNTER_A.store(i, Ordering::SeqCst);
        println!("[TASK] Worker A running iteration {}/3 - yielding CPU...", i);
        yield_now();
    }
    println!("[TASK] Worker A completed.");
}

fn test_worker_b() {
    for i in 1..=3 {
        TEST_COUNTER_B.store(i, Ordering::SeqCst);
        println!("[TASK] Worker B running iteration {}/3 - yielding CPU...", i);
        yield_now();
    }
    println!("[TASK] Worker B completed.");
}

/// Executes self-tests verifying cooperative context switching between concurrent threads.
pub fn run_self_tests() {
    println!("[TASK] Running Kernel Threads verification self-tests...");

    TEST_COUNTER_A.store(0, Ordering::SeqCst);
    TEST_COUNTER_B.store(0, Ordering::SeqCst);

    let id_a = spawn("worker_a", test_worker_a, 1);
    let id_b = spawn("worker_b", test_worker_b, 1);

    println!("[TASK] Spawned test threads #{} and #{} - interleaving execution:", id_a.0, id_b.0);

    // Yield main thread repeatedly until both workers finish
    for _ in 0..10 {
        if TEST_COUNTER_A.load(Ordering::SeqCst) >= 3 && TEST_COUNTER_B.load(Ordering::SeqCst) >= 3 {
            break;
        }
        yield_now();
    }

    assert_eq!(TEST_COUNTER_A.load(Ordering::SeqCst), 3, "Worker A did not finish 3 iterations");
    assert_eq!(TEST_COUNTER_B.load(Ordering::SeqCst), 3, "Worker B did not finish 3 iterations");

    let (_, switches) = stats();
    println!(
        "[TASK] All Kernel Threads self-tests passed successfully! (Cooperative multitasking, Context Switch count: {})",
        switches
    );

    // Run User Mode Ring 3 and SYSCALL self-test (Phase 5 - M12, M13)
    syscall::run_user_mode_self_test();
}
