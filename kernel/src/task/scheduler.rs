//! Preemptive Round-Robin Thread Scheduler (Phase 4 - M11).
//!
//! Features:
//! - Cooperative yielding via `yield_now()`.
//! - Preemptive multitasking triggered by APIC Timer interrupts.
//! - Sleep and wake primitives with tick tracking.
//! - Complete clean thread lifecycle termination.
#![allow(dead_code)]

use super::thread::{switch_threads, Thread, ThreadId, ThreadState};
use crate::println;
use crate::sync::Spinlock;
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

pub struct Scheduler {
    current: Option<Box<Thread>>,
    ready_queue: VecDeque<Box<Thread>>,
    sleeping_queue: Vec<Box<Thread>>,
    dead_threads: Vec<Box<Thread>>,
    total_switches: u64,
}

static SCHEDULER: Spinlock<Option<Scheduler>> = Spinlock::new(None);

impl Scheduler {
    pub const fn new() -> Self {
        Self {
            current: None,
            ready_queue: VecDeque::new(),
            sleeping_queue: Vec::new(),
            dead_threads: Vec::new(),
            total_switches: 0,
        }
    }
}

/// Initializes the Preemptive Kernel Scheduler.
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
        println!("[TASK] Spawned thread #{} \"{}\" (stack: 16 KiB)", id.0, thread.name);
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

        let prev_rsp_ptr = &mut current.rsp as *mut u64;
        let next_rsp = next.rsp;

        if current.state == ThreadState::Ready {
            sched.ready_queue.push_back(current);
        }

        crate::arch::x86_64::gdt::set_tss_rsp0(next.stack.top());
        crate::arch::x86_64::syscall::set_kernel_rsp(next.stack.top());
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

        let prev_rsp_ptr = &mut current.rsp as *mut u64;
        let next_rsp = next.rsp;

        if current.state == ThreadState::Ready {
            sched.ready_queue.push_back(current);
        }

        crate::arch::x86_64::gdt::set_tss_rsp0(next.stack.top());
        crate::arch::x86_64::syscall::set_kernel_rsp(next.stack.top());
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

        let prev_rsp_ptr = &mut current.rsp as *mut u64;
        let next_rsp = next.rsp;

        sched.sleeping_queue.push(current);
        crate::arch::x86_64::gdt::set_tss_rsp0(next.stack.top());
        crate::arch::x86_64::syscall::set_kernel_rsp(next.stack.top());
        sched.current = Some(next);

        (prev_rsp_ptr, next_rsp)
    };

    unsafe {
        switch_threads(prev_rsp_ptr, next_rsp);
    }
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

/// Returns current scheduler statistics.
pub fn stats() -> (usize, u64) {
    let guard = SCHEDULER.lock();
    if let Some(ref sched) = *guard {
        (sched.ready_queue.len(), sched.total_switches)
    } else {
        (0, 0)
    }
}
