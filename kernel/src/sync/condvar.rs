//! # Condition Variable Synchronization Primitive
//!
//! ## Overview
//! A condition variable allows execution threads to block until a specific state
//! predicate becomes true. Condition variables are always paired with a [`super::Mutex`].
//!
//! ## Usage Pattern (Predicate Checking Loop):
//! ```rust,ignore
//! let mut guard = mutex.lock();
//! while !predicate_condition(&*guard) {
//!     guard = condvar.wait(guard);
//! }
//! // Critical section with predicate satisfied
//! ```
//!
//! ## Invariants & Mechanics:
//! 1. **Atomic Wait Sequence**:
//!    - The caller's [`ThreadId`] is enqueued on `waiters`.
//!    - The [`MutexGuard`] is released, permitting competing threads to enter the critical section.
//!    - The calling thread is suspended via [`crate::task::block_current_thread`].
//!    - When signaled via [`notify_one`] or [`notify_all`], the thread re-acquires the [`Mutex`]
//!      before [`wait`] returns.
//! 2. **Spurious Wakeup Resistance**:
//!    - Applications must always test the predicate inside a `while` loop rather than an `if` block.

use crate::sync::{MutexGuard, Spinlock};
use crate::task::thread::ThreadId;
use alloc::vec::Vec;

/// A synchronization primitive for thread signaling and state waiting.
pub struct Condvar {
    waiters: Spinlock<Vec<ThreadId>>,
}

impl Condvar {
    /// Creates a new Condition Variable with an empty wait queue.
    pub const fn new() -> Self {
        Self {
            waiters: Spinlock::new(Vec::new()),
        }
    }

    /// Atomically releases the supplied Mutex and blocks the current thread on this condition variable.
    ///
    /// Re-acquires the mutex before returning to the caller.
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
