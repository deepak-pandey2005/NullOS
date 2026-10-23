//! Sleeping Mutex Primitive (Phase 11 - M28).
//!
//! Unlike `Spinlock`, which spins with interrupts disabled, `Mutex` blocks the current thread
//! and yields CPU execution to other threads if the lock is held.

use crate::sync::Spinlock;
use crate::task::thread::ThreadId;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

pub struct Mutex<T> {
    locked: AtomicBool,
    waiters: Spinlock<Vec<ThreadId>>,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for Mutex<T> {}
unsafe impl<T: Send> Send for Mutex<T> {}

impl<T> Mutex<T> {
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            waiters: Spinlock::new(Vec::new()),
            data: UnsafeCell::new(data),
        }
    }

    /// Acquires the mutex. If the mutex is already locked, the current thread is blocked
    /// until the lock is released.
    pub fn lock(&self) -> MutexGuard<'_, T> {
        loop {
            // Fast path: Uncontended lock acquisition
            if self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return MutexGuard { mutex: self };
            }

            // Slow path: Contended, enqueue thread on wait list and block
            if let Some(tid) = crate::task::current_thread_id() {
                {
                    let mut waiters = self.waiters.lock();
                    // Double check if lock became free
                    if self
                        .locked
                        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                        .is_ok()
                    {
                        return MutexGuard { mutex: self };
                    }
                    waiters.push(tid);
                }
                crate::task::block_current_thread();
            } else {
                // If scheduler is not running yet, spin briefly
                core::hint::spin_loop();
            }
        }
    }

    /// Attempts to acquire the mutex without blocking.
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(MutexGuard { mutex: self })
        } else {
            None
        }
    }

    /// Internal release implementation invoked on MutexGuard drop.
    fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
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
}

pub struct MutexGuard<'a, T> {
    pub mutex: &'a Mutex<T>,
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}
