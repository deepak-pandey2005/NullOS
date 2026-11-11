//! # NullOS Task Execution Subsystem
//!
//! ## Subsystem Architecture Overview
//! The NullOS task management subsystem provides a complete, modern OS multitasking environment
//! supporting both lightweight kernel threads and fully isolated user-mode processes:
//!
//! ```text
//!                          +-----------------------+
//!                          |  Scheduler (M11/M24)  |
//!                          +-----------------------+
//!                                     |
//!               +---------------------+---------------------+
//!               |                                           |
//!               v                                           v
//!     +-------------------+                       +-------------------+
//!     |   Kernel Thread   |                       |   User Process    |
//!     | (Ring 0, Same AS) |                       | (Ring 3, Priv AS) |
//!     +-------------------+                       +-------------------+
//!               |                                           |
//!     +-------------------+                       +-------------------+
//!     | 64 KiB Kernel Stk |                       | Isolated Address- |
//!     | Callee-saved regs |                       | Space, User Stack |
//!     +-------------------+                       +-------------------+
//!                                                           |
//!                                                 +-------------------+
//!                                                 |  SYSCALL Handlers |
//!                                                 | (Fast sysenter)   |
//!                                                 +-------------------+
//! ```
//!
//! ### Key Sub-Components:
//! 1. **Kernel Threads ([`thread`])**:
//!    - Independent execution streams with individual 64 KiB stacks.
//!    - Architectural context switching implemented in assembly (`switch.S`), saving and
//!      restoring callee-saved registers (`%rbx`, `%rsp`, `%rbp`, `%r12`..`%r15`, `%rflags`).
//! 2. **Preemptive Round-Robin Scheduler ([`scheduler`])**:
//!    - Preemptive quantum enforced via APIC Timer interrupts (vector 32).
//!    - Manages Ready, Running, Blocked, Sleeping, and Terminated thread states.
//!    - Priority levels (0 = idle, 1 = normal, 2 = high, 3 = real-time).
//! 3. **Processes & Address Spaces ([`process`])**:
//!    - POSIX-like process lifecycle: `create_process`, `execute_user_elf`, `fork`, `exit_process`, `waitpid`.
//!    - Per-process file descriptor table ([`crate::fs::fd::FileTable`]) and virtual memory ([`crate::memory::AddressSpace`]).
//! 4. **User-Mode ELF Loader ([`elf`])**:
//!    - Parses 64-bit ELF executable headers, maps `PT_LOAD` segments into user virtual memory,
//!      zeroes `.bss`, allocates user stack at `0x0000_7FFF_FFFF_0000`, and prepares arguments (`argc`/`argv`).
//! 5. **Fast System Calls ([`syscall`])**:
//!    - Dispatches 30+ POSIX-compatible system calls (`read`, `write`, `open`, `close`, `fork`, `execve`,
//!      `waitpid`, `pipe`, `socket`, `mmap`, `futex`, `kill`) via x86_64 `SYSCALL`/`SYSRET`.
//! 6. **Dynamic Linking & Relocations ([`dynlink`])**:
//!    - Runtime relocation (`R_X86_64_RELATIVE`, `R_X86_64_GLOB_DAT`, `R_X86_64_JUMP_SLOT`)
//!      for shared ELF libraries.
//! 7. **Event Multiplexing ([`event`])**:
//!    - Readiness notifications and epoll-style wait mechanisms for scalable IPC and networking.
//! 8. **Kernel Shell ([`shell`])**:
//!    - Interactive diagnostic command-line interface with built-in commands (`help`, `ls`, `ps`, `mem`, `cat`, etc.).

#![allow(unused_imports, dead_code)]

pub mod dynlink;
pub mod elf;
pub mod event;
pub mod npkg;
pub mod process;
pub mod scheduler;
pub mod shell;
pub mod syscall;
pub mod thread;

pub use elf::{load_user_elf, LoadedUserElf};
pub use process::{create_process, execute_user_elf, exit_process, kill_process, process_count, spawn_init_process, waitpid, Process, ProcessId, ProcessState};
pub use scheduler::{
    block_current_thread, current_thread_id, exit, init, sleep, spawn, spawn_user, stats,
    thread_list, timer_tick, wake_thread, yield_now, ThreadInfo,
};
pub use syscall::run_user_mode_self_test;
pub use thread::{Thread, ThreadId, ThreadState};

use crate::println;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static TEST_COUNTER_A: AtomicUsize = AtomicUsize::new(0);
static TEST_COUNTER_B: AtomicUsize = AtomicUsize::new(0);
static WORKER_A_DONE: AtomicBool = AtomicBool::new(false);
static WORKER_B_DONE: AtomicBool = AtomicBool::new(false);

fn test_worker_a() {
    for i in 1..=3 {
        TEST_COUNTER_A.store(i, Ordering::SeqCst);
        println!("[TASK] Worker A running iteration {}/3 - yielding CPU...", i);
        yield_now();
    }
    WORKER_A_DONE.store(true, Ordering::SeqCst);
    println!("[TASK] Worker A completed.");
}

fn test_worker_b() {
    for i in 1..=3 {
        TEST_COUNTER_B.store(i, Ordering::SeqCst);
        println!("[TASK] Worker B running iteration {}/3 - yielding CPU...", i);
        yield_now();
    }
    WORKER_B_DONE.store(true, Ordering::SeqCst);
    println!("[TASK] Worker B completed.");
}

/// Executes self-tests verifying cooperative context switching between concurrent threads.
pub fn run_self_tests() {
    println!("[TASK] Running Kernel Threads verification self-tests...");

    TEST_COUNTER_A.store(0, Ordering::SeqCst);
    TEST_COUNTER_B.store(0, Ordering::SeqCst);
    WORKER_A_DONE.store(false, Ordering::SeqCst);
    WORKER_B_DONE.store(false, Ordering::SeqCst);

    let id_a = spawn("worker_a", test_worker_a, 1);
    let id_b = spawn("worker_b", test_worker_b, 1);

    println!("[TASK] Spawned test threads #{} and #{} - interleaving execution:", id_a.0, id_b.0);

    // Yield main thread repeatedly until both workers finish
    for _ in 0..10 {
        if WORKER_A_DONE.load(Ordering::SeqCst) && WORKER_B_DONE.load(Ordering::SeqCst) {
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
