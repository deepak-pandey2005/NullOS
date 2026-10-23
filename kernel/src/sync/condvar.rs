//! Condition Variable Primitive (Phase 11 - M28).
//!
//! Enables threads to block while waiting for a condition and wake up when signaled.

use crate::sync::{MutexGuard, Spinlock};
use crate::task::thread::ThreadId;
use alloc::vec::Vec;

pub struct Condvar {
    waiters: Spinlock<Vec<ThreadId>>,
}

impl Condvar {
    pub const fn new() -> Self {
        Self {
            waiters: Spinlock::new(Vec::new()),
        }
    }

    /// Atomically releases the supplied Mutex and blocks the current thread on this condition variable.
    /// Re-acquires the mutex before returning.
    pub fn wait<'a, T>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
        let mutex = guard.mutex;
        let tid = match crate::task::current_thread_id() {
            Some(id) => id,
            None => return guard,
        };

        // 1. Enqueue current thread on wait list
        {
            let mut waiters = self.waiters.lock();
            waiters.push(tid);
        }

        // 2. Drop mutex to let other threads enter
        drop(guard);

        // 3. Block current thread until signaled
        crate::task::block_current_thread();

        // 4. Re-acquire mutex before returning to caller
        mutex.lock()
    }

    /// Wakes up a single thread waiting on this condition variable.
    pub fn notify_one(&self) {
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

    /// Wakes up all threads waiting on this condition variable.
    pub fn notify_all(&self) {
        let to_wake: Vec<ThreadId> = {
            let mut waiters = self.waiters.lock();
            core::mem::take(&mut *waiters)
        };

        for tid in to_wake {
            crate::task::wake_thread(tid);
        }
    }
}
