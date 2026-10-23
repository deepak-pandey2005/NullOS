//! Thread-safe and interrupt-safe synchronization primitives for NullOS.
#![allow(unused_imports, dead_code)]

pub mod condvar;
pub mod futex;
pub mod mutex;
pub mod semaphore;

pub use condvar::Condvar;
pub use futex::{futex_requeue, futex_wait, futex_wake, FUTEX_PRIVATE_FLAG, FUTEX_REQUEUE, FUTEX_WAIT, FUTEX_WAKE};
pub use mutex::{Mutex, MutexGuard};
pub use semaphore::Semaphore;

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// Read RFLAGS register to check if interrupts are currently enabled (IF flag, bit 9).
#[inline]
pub fn are_interrupts_enabled() -> bool {
    let rflags: u64;
    unsafe {
        core::arch::asm!("pushfq; pop {}", out(reg) rflags, options(nomem, preserves_flags));
    }
    (rflags & (1 << 9)) != 0
}

/// Disable interrupts on the current CPU core.
#[inline]
pub fn disable_interrupts() {
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
}

/// Enable interrupts on the current CPU core.
#[inline]
pub fn enable_interrupts() {
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack));
    }
}

/// An interrupt-safe spinlock protecting shared kernel structures.
///
/// Disables interrupts while the lock is held to prevent deadlocks when accessed
/// from both normal thread context and interrupt handlers. Restores previous interrupt state on drop.
pub struct Spinlock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for Spinlock<T> {}
unsafe impl<T: Send> Send for Spinlock<T> {}

impl<T> Spinlock<T> {
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    /// Acquires the spinlock, disabling interrupts until the guard is dropped.
    pub fn lock(&self) -> SpinlockGuard<'_, T> {
        let re_enable = are_interrupts_enabled();
        disable_interrupts();

        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }

        SpinlockGuard {
            lock: self,
            re_enable_interrupts: re_enable,
        }
    }
}

pub struct SpinlockGuard<'a, T> {
    lock: &'a Spinlock<T>,
    re_enable_interrupts: bool,
}

impl<T> Deref for SpinlockGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SpinlockGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinlockGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        if self.re_enable_interrupts {
            enable_interrupts();
        }
    }
}
