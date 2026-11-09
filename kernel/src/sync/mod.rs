//! # Thread-Safe and Interrupt-Safe Synchronization Primitives
//!
//! ## Subsystem Architecture & Concurrency Rules
//! Operating system kernels execute concurrently across multi-core processors and
//! asynchronous hardware interrupt contexts. Uncontrolled access to shared data structures
//! creates race conditions, deadlocks, and memory corruption.
//!
//! NullOS provides a tiered synchronization hierarchy:
//! 1. **Interrupt-Safe Spinlock ([`Spinlock`])**:
//!    - Used for short, critical sections protecting hardware drivers and core kernel queues.
//!    - **Deadlock Prevention Contract**: Disables interrupts (`cli`) on the local CPU before
//!      spinning to acquire the atomic lock. This prevents an interrupt handler on the same core
//!      from attempting to acquire the already-held spinlock, which would cause an unrecoverable deadlock.
//!    - Restores the previous interrupt flag (`RFLAGS.IF`) when the [`SpinlockGuard`] is dropped.
//! 2. **Sleep-Blocking Mutex ([`Mutex`])**:
//!    - Used for longer operations (VFS operations, process table mutation).
//!    - If contended, the calling thread is put to sleep in the scheduler's blocked queue,
//!      yielding CPU cycles to other ready threads.
//! 3. **Condition Variables ([`Condvar`])**:
//!    - Coordinates producer-consumer workflows and state signaling (`wait`, `notify_one`, `notify_all`).
//! 4. **Counting Semaphores ([`Semaphore`])**:
//!    - Manages bounded access to finite pools of resources.
//! 5. **Fast Userspace Mutexes ([`futex`])**:
//!    - Zero-syscall uncontended locking for userspace POSIX threads via `futex(2)`.

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

/// Disables interrupts on the current CPU core using the `cli` instruction.
#[inline]
pub fn disable_interrupts() {
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
}

/// Enables interrupts on the current CPU core using the `sti` instruction.
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
    /// Creates a new spinlock protecting the given `data`.
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    /// Acquires the spinlock, disabling interrupts until the guard is dropped.
    ///
    /// Spins with `core::hint::spin_loop()` until the atomic flag transitions from `false` to `true`.
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

/// RAII guard providing mutable reference access to data protected by a [`Spinlock`].
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
