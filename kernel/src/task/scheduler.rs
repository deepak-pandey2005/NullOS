//! # Preemptive Round-Robin Thread Scheduler
//!
//! ## Scheduler Architecture
//! The NullOS scheduler implements preemptive, priority-aware Round-Robin multitasking.
//! It coordinates execution between:
//! - **Ready Queue (`ready_queue`)**: FIFO queue of runnable threads awaiting CPU timeslices.
//! - **Sleeping Queue (`sleeping_queue`)**: Threads waiting for a specific APIC timer tick target.
//! - **Blocked Map (`blocked_threads`)**: Threads waiting on synchronization primitives (Mutex, Condvar, Semaphore, Futex, IPC pipe).
//! - **Reap Vector (`dead_threads`)**: Deceased threads whose 64 KiB stacks cannot be freed until
//!   the CPU is safely executing on a different thread's stack.
//!
//! ## Context Switch Workflow
//! Every context switch (whether triggered voluntarily by [`yield_now`] or preemptively by [`timer_tick`]):
//! 1. Acquires [`SCHEDULER`] spinlock.
//! 2. Reaps any previously terminated threads in `dead_threads`.
//! 3. Moves current thread to `Ready` or `Blocked` queue.
//! 4. Pops the next thread from `ready_queue` and marks it `Running`.
//! 5. Updates hardware CPU registers for Ring 3 privilege transitions:
//!    - TSS `rsp0`: Set to `next.stack.top()` so interrupts in user mode land on this thread's kernel stack.
//!    - Fast Syscall Kernel RSP: Set to `next.stack.top()` so `SYSCALL` switches to this stack.
//!    - `IA32_FS_BASE` MSR: Reloads Thread-Local Storage pointer (`next.fs_base`).
//! 6. Drops the spinlock and calls assembly [`switch_threads`] (`switch.S`).
//!
//! TODO(scheduler): Multi-Core SMP Scalability (Per-CPU Runqueues):
//! - Replace the single global `SCHEDULER` spinlock with per-CPU runqueues (e.g. `PerCpuScheduler`).
//! - A single global lock creates severe bus lock contention when all APs (Application Processors)
//!   field timer ticks and attempt to reschedule concurrently.
//! - Implement work-stealing: an idle core whose local ready queue is empty should probe and steal
//!   ready threads from busy neighbor cores' queues with minimal lock contention.
//!
//! TODO(scheduler): Fair Scheduling Algorithms (CFS or MLFQ):
//! - Replace simple FIFO round-robin scheduling with a Completely Fair Scheduler (CFS) or Multi-Level
//!   Feedback Queue (MLFQ).
//! - Maintain virtual runtime (`vruntime`) tracked via high-precision APIC/TSC timestamps, prioritizing
//!   I/O-bound interactive tasks while preventing CPU starvation for compute-intensive threads.

#![allow(dead_code)]

use super::thread::{switch_threads, Thread, ThreadId, ThreadState};
use crate::println;
use crate::sync::Spinlock;
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

/// Core scheduling state holding queues and context tracking.
pub struct Scheduler {
    /// Currently executing thread on the CPU.
    current: Option<Box<Thread>>,
    /// FIFO queue of runnable threads ready for a timeslice.
    ready_queue: VecDeque<Box<Thread>>,
    /// List of threads sleeping until a future tick.
    sleeping_queue: Vec<Box<Thread>>,
    /// Map of blocked threads keyed by ThreadId.
    blocked_threads: BTreeMap<ThreadId, Box<Thread>>,
    /// Terminated threads queued for deferred deallocation.
    dead_threads: Vec<Box<Thread>>,
    /// Cumulative context switch count.
    total_switches: u64,
}

static SCHEDULER: Spinlock<Option<Scheduler>> = Spinlock::new(None);

impl Scheduler {
    /// Creates a new empty scheduler instance.
    pub const fn new() -> Self {
        Self {
            current: None,
            ready_queue: VecDeque::new(),
            sleeping_queue: Vec::new(),
            blocked_threads: BTreeMap::new(),
            dead_threads: Vec::new(),
            total_switches: 0,
        }
    }
}

/// Initializes the Preemptive Kernel Scheduler.
///
/// Encapsulates the running bootstrap execution stream into `Thread 0` ("kernel_main"),
/// sets TSS `rsp0` and syscall kernel stack pointers, and activates scheduling.
pub fn init() {
    let mut sched = Scheduler::new();
    // Establish main kernel execution context as Thread 0
    let main_thread = Box::new(Thread::new_main());
    crate::arch::x86_64::gdt::set_tss_rsp0(main_thread.stack.top());
    crate::arch::x86_64::syscall::set_kernel_rsp(main_thread.stack.top());
    sched.current = Some(main_thread);
    *SCHEDULER.lock() = Some(sched);

    println!("[TASK] Preemptive Thread Scheduler initialized (main thread active).");
}

/// Spawns a new concurrent kernel thread.
pub fn spawn(name: &str, entry: fn(), priority: u8) -> ThreadId {
    let thread = Box::new(Thread::new(name, entry, priority));
    let id = thread.id;

    if let Some(ref mut sched) = *SCHEDULER.lock() {
        println!("[TASK] Spawned thread #{} \"{}\" (stack: 64 KiB)", id.0, thread.name);
        sched.ready_queue.push_back(thread);
    }

    id
}

/// Voluntarily yields CPU execution to the next ready thread in the run queue.
pub fn yield_now() {
    let (prev_rsp_ptr, next_rsp) = {
        let mut guard = SCHEDULER.lock();
        let sched = match guard.as_mut() {
            Some(s) => s,
            None => return,
        };

        // Safely reap terminated threads now that we are on a valid running stack
        sched.dead_threads.clear();

        // If no other threads are ready, keep running
        if sched.ready_queue.is_empty() {
            return;
        }

        // Pop next thread from run queue
        let mut next = match sched.ready_queue.pop_front() {
            Some(t) => t,
            None => return,
        };

        let mut current = sched.current.take().expect("No current thread");
        if current.state == ThreadState::Running {
            current.state = ThreadState::Ready;
        }

        next.state = ThreadState::Running;
        sched.total_switches += 1;

        let next_rsp = next.rsp;
        let prev_rsp_ptr = if current.state == ThreadState::Ready {
            sched.ready_queue.push_back(current);
            &mut sched.ready_queue.back_mut().unwrap().rsp as *mut u64
        } else {
            core::ptr::null_mut()
        };

        crate::arch::x86_64::gdt::set_tss_rsp0(next.stack.top());
        crate::arch::x86_64::syscall::set_kernel_rsp(next.stack.top());
        unsafe {
            crate::arch::x86_64::syscall::write_msr(
                crate::arch::x86_64::syscall::IA32_FS_BASE_MSR,
                next.fs_base,
            );
        }
        sched.current = Some(next);

        (prev_rsp_ptr, next_rsp)
    };

    // Perform context switch outside the spinlock to avoid holding locks across stack swap
    unsafe {
        switch_threads(prev_rsp_ptr, next_rsp);
    }
}

/// Called on every periodic APIC timer interrupt to service sleep queues and preempt threads.
pub fn timer_tick(current_tick: u64) {
    let (prev_rsp_ptr, next_rsp) = {
        let mut guard = SCHEDULER.lock();
        let sched = match guard.as_mut() {
            Some(s) => s,
            None => return,
        };

        // Safely reap terminated threads
        sched.dead_threads.clear();

        // 1. Wake up sleeping threads
        let mut i = 0;
        while i < sched.sleeping_queue.len() {
            if sched.sleeping_queue[i].sleep_until <= current_tick {
                let mut thread = sched.sleeping_queue.swap_remove(i);
                thread.state = ThreadState::Ready;
                sched.ready_queue.push_back(thread);
            } else {
                i += 1;
            }
        }

        // 2. Track tick on current thread
        if let Some(ref mut current) = sched.current {
            current.total_ticks += 1;
        }

        // 3. Preempt current thread if ready queue has candidates
        if sched.ready_queue.is_empty() {
            return;
        }

        let mut next = match sched.ready_queue.pop_front() {
            Some(t) => t,
            None => return,
        };

        let mut current = sched.current.take().expect("No current thread");
        if current.state == ThreadState::Running {
            current.state = ThreadState::Ready;
        }

        next.state = ThreadState::Running;
        sched.total_switches += 1;

        let next_rsp = next.rsp;
        let prev_rsp_ptr = if current.state == ThreadState::Ready {
            sched.ready_queue.push_back(current);
            &mut sched.ready_queue.back_mut().unwrap().rsp as *mut u64
        } else {
            core::ptr::null_mut()
        };

        crate::arch::x86_64::gdt::set_tss_rsp0(next.stack.top());
        crate::arch::x86_64::syscall::set_kernel_rsp(next.stack.top());
        unsafe {
            crate::arch::x86_64::syscall::write_msr(
                crate::arch::x86_64::syscall::IA32_FS_BASE_MSR,
                next.fs_base,
            );
        }
        sched.current = Some(next);

        (prev_rsp_ptr, next_rsp)
    };

    unsafe {
        switch_threads(prev_rsp_ptr, next_rsp);
    }
}

/// Puts the current thread to sleep for `ticks` timer intervals.
pub fn sleep(ticks: u64) {
    let current_tick = crate::arch::x86_64::apic::get_ticks();
    let wake_tick = current_tick + ticks;

    let (prev_rsp_ptr, next_rsp) = {
        let mut guard = SCHEDULER.lock();
        let sched = match guard.as_mut() {
            Some(s) => s,
            None => return,
        };

        let mut next = match sched.ready_queue.pop_front() {
            Some(t) => t,
            None => {
                // No other threads ready to run, spin-wait
                drop(guard);
                crate::arch::x86_64::apic::sleep(ticks);
                return;
            }
        };

        let mut current = sched.current.take().expect("No current thread");
        current.state = ThreadState::Blocked;
        current.sleep_until = wake_tick;

        next.state = ThreadState::Running;
        sched.total_switches += 1;

        let next_rsp = next.rsp;
        sched.sleeping_queue.push(current);
        let prev_rsp_ptr = &mut sched.sleeping_queue.last_mut().unwrap().rsp as *mut u64;
        crate::arch::x86_64::gdt::set_tss_rsp0(next.stack.top());
        crate::arch::x86_64::syscall::set_kernel_rsp(next.stack.top());
        unsafe {
            crate::arch::x86_64::syscall::write_msr(
                crate::arch::x86_64::syscall::IA32_FS_BASE_MSR,
                next.fs_base,
            );
        }
        sched.current = Some(next);

        (prev_rsp_ptr, next_rsp)
    };

    unsafe {
        switch_threads(prev_rsp_ptr, next_rsp);
    }
}

/// Blocks the current thread, transitioning it to ThreadState::Blocked,
/// and immediately context switches to the next ready thread.
pub fn block_current_thread() {
    let (prev_rsp_ptr, next_rsp) = {
        let mut guard = SCHEDULER.lock();
        let sched = match guard.as_mut() {
            Some(s) => s,
            None => return,
        };

        sched.dead_threads.clear();

        let mut current = match sched.current.take() {
            Some(c) => c,
            None => return,
        };

        current.state = ThreadState::Blocked;
        let tid = current.id;
        sched.blocked_threads.insert(tid, current);
        let prev_rsp_ptr = &mut sched.blocked_threads.get_mut(&tid).unwrap().rsp as *mut u64;

        // Fetch next ready thread
        let mut next = match sched.ready_queue.pop_front() {
            Some(t) => t,
            None => {
                // If no threads ready, return and continue current thread
                let cur = sched.blocked_threads.remove(&tid).unwrap();
                sched.current = Some(cur);
                return;
            }
        };

        next.state = ThreadState::Running;
        sched.total_switches += 1;
        let next_rsp = next.rsp;

        crate::arch::x86_64::gdt::set_tss_rsp0(next.stack.top());
        crate::arch::x86_64::syscall::set_kernel_rsp(next.stack.top());
        unsafe {
            crate::arch::x86_64::syscall::write_msr(
                crate::arch::x86_64::syscall::IA32_FS_BASE_MSR,
                next.fs_base,
            );
        }
        sched.current = Some(next);

        (prev_rsp_ptr, next_rsp)
    };

    unsafe {
        switch_threads(prev_rsp_ptr, next_rsp);
    }
}

/// Unblocks a previously blocked thread by moving it back to the ready queue.
pub fn wake_thread(id: ThreadId) -> bool {
    let mut guard = SCHEDULER.lock();
    if let Some(ref mut sched) = *guard {
        if let Some(mut thread) = sched.blocked_threads.remove(&id) {
            thread.state = ThreadState::Ready;
            sched.ready_queue.push_back(thread);
            return true;
        }
    }
    false
}

/// Returns the ThreadId of the currently executing thread.
pub fn current_thread_id() -> Option<ThreadId> {
    let guard = SCHEDULER.lock();
    guard.as_ref().and_then(|s| s.current.as_ref().map(|t| t.id))
}

/// Spawns a user thread in the specified process.
pub fn spawn_user(
    name: &str,
    entry: u64,
    user_stack: u64,
    arg: u64,
    tls_base: u64,
    pid: super::process::ProcessId,
    priority: u8,
) -> ThreadId {
    let thread = Box::new(Thread::new_user(name, entry, user_stack, arg, tls_base, pid, priority));
    let id = thread.id;

    if let Some(ref mut sched) = *SCHEDULER.lock() {
        println!("[TASK] Spawned user thread #{} \"{}\" in PID {} (entry {:#x}, stack {:#x})",
            id.0, thread.name, pid.0, entry, user_stack
        );
        sched.ready_queue.push_back(thread);
    }

    id
}

/// Terminates the current thread cleanly.
pub fn exit() -> ! {
    let next_rsp = {
        let mut guard = SCHEDULER.lock();
        let sched = guard.as_mut().expect("Scheduler not active");

        if let Some(mut current) = sched.current.take() {
            println!("[TASK] Thread #{} \"{}\" finished (total ticks: {}).", current.id.0, current.name, current.total_ticks);
            current.state = ThreadState::Terminated;
            // Defer dropping until we are running on another thread's stack!
            sched.dead_threads.push(current);
        }

        // Schedule next thread
        if let Some(mut next) = sched.ready_queue.pop_front() {
            next.state = ThreadState::Running;
            let rsp = next.rsp;
            crate::arch::x86_64::gdt::set_tss_rsp0(next.stack.top());
            crate::arch::x86_64::syscall::set_kernel_rsp(next.stack.top());
            unsafe {
                crate::arch::x86_64::syscall::write_msr(
                    crate::arch::x86_64::syscall::IA32_FS_BASE_MSR,
                    next.fs_base,
                );
            }
            sched.current = Some(next);
            sched.total_switches += 1;
            rsp
        } else {
            0
        }
    };

    if next_rsp != 0 {
        let mut dummy: u64 = 0;
        unsafe {
            switch_threads(&mut dummy as *mut u64, next_rsp);
        }
    }

    // If no threads remain, halt CPU
    loop {
        unsafe {
            core::arch::asm!("sti; hlt");
        }
    }
}

/// Diagnostic thread information struct for inspection.
#[derive(Debug, Clone)]
pub struct ThreadInfo {
    pub id: ThreadId,
    pub name: alloc::string::String,
    pub state: ThreadState,
    pub priority: u8,
    pub total_ticks: u64,
    pub is_user: bool,
    pub process_id: Option<super::process::ProcessId>,
    pub stack_bottom: u64,
    pub stack_top: u64,
    pub fs_base: u64,
}

/// Returns a snapshot of all active, sleeping, and blocked threads.
pub fn thread_list() -> Vec<ThreadInfo> {
    let guard = SCHEDULER.lock();
    let mut list = Vec::new();
    if let Some(ref sched) = *guard {
        if let Some(ref cur) = sched.current {
            list.push(ThreadInfo {
                id: cur.id,
                name: cur.name.clone(),
                state: cur.state,
                priority: cur.priority,
                total_ticks: cur.total_ticks,
                is_user: cur.is_user,
                process_id: cur.process_id,
                stack_bottom: cur.stack.bottom(),
                stack_top: cur.stack.top(),
                fs_base: cur.fs_base,
            });
        }
        for t in &sched.ready_queue {
            list.push(ThreadInfo {
                id: t.id,
                name: t.name.clone(),
                state: t.state,
                priority: t.priority,
                total_ticks: t.total_ticks,
                is_user: t.is_user,
                process_id: t.process_id,
                stack_bottom: t.stack.bottom(),
                stack_top: t.stack.top(),
                fs_base: t.fs_base,
            });
        }
        for (_, t) in &sched.blocked_threads {
            list.push(ThreadInfo {
                id: t.id,
                name: t.name.clone(),
                state: t.state,
                priority: t.priority,
                total_ticks: t.total_ticks,
                is_user: t.is_user,
                process_id: t.process_id,
                stack_bottom: t.stack.bottom(),
                stack_top: t.stack.top(),
                fs_base: t.fs_base,
            });
        }
        for t in &sched.sleeping_queue {
            list.push(ThreadInfo {
                id: t.id,
                name: t.name.clone(),
                state: t.state,
                priority: t.priority,
                total_ticks: t.total_ticks,
                is_user: t.is_user,
                process_id: t.process_id,
                stack_bottom: t.stack.bottom(),
                stack_top: t.stack.top(),
                fs_base: t.fs_base,
            });
        }
    }
    list
}

/// Returns current scheduler statistics.
pub fn stats() -> (usize, u64) {
    let guard = SCHEDULER.lock();
    if let Some(ref sched) = *guard {
        (sched.ready_queue.len(), sched.total_switches)
    } else {
        (0, 0)
    }
}
