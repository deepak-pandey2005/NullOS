//! # Counting Semaphore Synchronization Primitive
//!
//! ## Overview
//! A counting semaphore maintains an integer token count regulating concurrent
//! access to a finite set of shared resources.
//!
//! ### Operational Semantics:
//! - [`wait`]: Decrements the token counter. If the counter is $\le 0$, the calling thread
//!   is suspended and placed on the semaphore's FIFO `waiters` queue until another thread issues [`post`].
//! - [`try_wait`]: Non-blocking attempt to acquire a token; returns `true` if acquired, `false` otherwise.
//! - [`post`]: Increments the token counter by 1. If any threads are waiting in the queue,
//!   pops the head thread and returns it to the scheduler's ready queue via [`crate::task::wake_thread`].
//! - [`get_value`]: Inspects the current token counter.

use crate::sync::Spinlock;
use crate::task::thread::ThreadId;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicI64, Ordering};

/// A thread-safe counting semaphore with sleep-blocking wait queues.
pub struct Semaphore {
    count: AtomicI64,
    waiters: Spinlock<Vec<ThreadId>>,
}

impl Semaphore {
    /// Constructs a new Counting Semaphore with the given `initial_count`.
    pub const fn new(initial_count: isize) -> Self {
        Self {
            count: AtomicI64::new(initial_count as i64),
            waiters: Spinlock::new(Vec::new()),
        }
    }

    /// Decrements the semaphore counter, blocking if no tokens are available.
    pub fn wait(&self) {
        loop {
            let mut current = self.count.load(Ordering::Acquire);
            while current > 0 {
                match self.count.compare_exchange_weak(
                    current,
                    current - 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return,
                    Err(actual) => current = actual,
                }
            }

            // Counter is <= 0, must block
            if let Some(tid) = crate::task::current_thread_id() {
                {
                    let mut waiters = self.waiters.lock();
                    // Re-check count before blocking
                    let check = self.count.load(Ordering::Acquire);
                    if check > 0
                        && self
                            .count
                            .compare_exchange(check, check - 1, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok()
                    {
                        return;
                    }
                    waiters.push(tid);
                }
                crate::task::block_current_thread();
            } else {
                core::hint::spin_loop();
            }
        }
    }

    /// Attempts to decrement the semaphore counter without blocking.
    /// Returns true if the semaphore was successfully decremented, false otherwise.
    pub fn try_wait(&self) -> bool {
        let mut current = self.count.load(Ordering::Acquire);
        while current > 0 {
            match self.count.compare_exchange_weak(
                current,
                current - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
        false
    }

    /// Increments the semaphore counter and wakes up a blocked thread if one exists.
    pub fn post(&self) {
        self.count.fetch_add(1, Ordering::Release);
        let next_thread = {
            let mut waiters = self.waiters.lock();
            if !waiters.is_empty() {
                Some(waiters.remove(0))
            } else {
                None
            }
        };

        if let Some(tid) = next_thread {
            crate::task::wake_thread(tid);
        }
    }

    /// Returns the current value of the semaphore counter.
    pub fn get_value(&self) -> isize {
        self.count.load(Ordering::Relaxed) as isize
    }
}
