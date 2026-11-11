//! # Lock Optimization & Cache-Aware Data Structures (Phase 17 - M38)
//!
//! ## Overview
//! Eliminates cross-core MESI cache-coherence traffic ("cache-line ping-pong") and lock
//! starvation in multi-core kernel critical sections:
//! - [`CacheAligned<T>`]: 64-byte (`align(64)`) wrapper isolating hot per-CPU / contended
//!   fields onto dedicated L1 cache lines (preventing **False Sharing**).
//! - [`prefetch_read`] / [`prefetch_write`]: Software `PREFETCHT0` hints warming L1 cache ahead
//!   of linear traversals.
//! - [`TicketSpinlock<T>`]: Fair FIFO queue spinlock guaranteeing bounded wait order (`next_ticket`
//!   vs `now_serving`), eliminating thread starvation under heavy contention.
//! - [`TtasBackoffSpinlock<T>`]: Test-and-Test-and-Set spinlock that polls the lock bit cleanly
//!   with read-only loads (`Relaxed`) and exponential `pause` backoff before attempting `Acquire`
//!   atomic RMWs, slashing coherence bus traffic on SMP.
//! - [`SeqLock<T>`]: Sequence-counter optimistic lock allowing wait-free, zero-contention
//!   concurrent reads for read-mostly telemetry (e.g. monotonic clock, stats snapshots).

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{fence, AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::sync::{are_interrupts_enabled, disable_interrupts, enable_interrupts};

/// Standard x86_64 L1/L2/L3 cache line size in bytes.
pub const CACHE_LINE_SIZE: usize = 64;

/// 64-byte cache-line-aligned wrapper preventing **false sharing** between adjacent variables
/// accessed independently by different CPU cores.
#[repr(C, align(64))]
#[derive(Debug, Default)]
pub struct CacheAligned<T> {
    pub value: T,
}

impl<T> CacheAligned<T> {
    pub const fn new(value: T) -> Self {
        Self { value }
    }
}

/// Hints the CPU to prefetch cache line containing `ptr` into L1 data cache for reading.
#[inline(always)]
pub fn prefetch_read<T>(ptr: *const T) {
    // SAFETY: PREFETCHT0 is a hint instruction; invalid pointers are silently ignored by silicon.
    unsafe {
        core::arch::asm!(
            "prefetcht0 [{}]",
            in(reg) ptr,
            options(nostack, preserves_flags)
        );
    }
}

/// Hints the CPU to prefetch cache line containing `ptr` into L1 data cache with write intent.
#[inline(always)]
pub fn prefetch_write<T>(ptr: *const T) {
    // SAFETY: PREFETCHT0 warms cache without trapping.
    unsafe {
        core::arch::asm!(
            "prefetcht0 [{}]",
            in(reg) ptr,
            options(nostack, preserves_flags)
        );
    }
}

// ============================================================================
// 1. TicketSpinlock (Strict FIFO Fairness)
// ============================================================================

/// Fair FIFO spinlock guaranteeing every waiting CPU core enters the critical section
/// strictly in arrival order (`next_ticket` -> `now_serving`).
pub struct TicketSpinlock<T> {
    next_ticket: CacheAligned<AtomicU32>,
    now_serving: CacheAligned<AtomicU32>,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for TicketSpinlock<T> {}
unsafe impl<T: Send> Send for TicketSpinlock<T> {}

impl<T> TicketSpinlock<T> {
    pub const fn new(data: T) -> Self {
        Self {
            next_ticket: CacheAligned::new(AtomicU32::new(0)),
            now_serving: CacheAligned::new(AtomicU32::new(0)),
            data: UnsafeCell::new(data),
        }
    }

    /// Acquires a ticket atomically and spins on `now_serving` until our turn arrives.
    pub fn lock(&self) -> TicketLockGuard<'_, T> {
        let re_enable = are_interrupts_enabled();
        disable_interrupts();

        let my_ticket = self.next_ticket.value.fetch_add(1, Ordering::Relaxed);
        while self.now_serving.value.load(Ordering::Acquire) != my_ticket {
            core::hint::spin_loop();
        }

        TicketLockGuard {
            lock: self,
            re_enable_interrupts: re_enable,
        }
    }
}

pub struct TicketLockGuard<'a, T> {
    lock: &'a TicketSpinlock<T>,
    re_enable_interrupts: bool,
}

impl<T> Deref for TicketLockGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for TicketLockGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for TicketLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.now_serving.value.fetch_add(1, Ordering::Release);
        if self.re_enable_interrupts {
            enable_interrupts();
        }
    }
}

// ============================================================================
// 2. TTAS Exponential Backoff Spinlock (Reduced Coherence Bus Traffic)
// ============================================================================

/// Test-and-Test-and-Set Spinlock with bounded exponential `pause` backoff.
///
/// Spins cleanly using read-only `load(Relaxed)` while the lock is held (remaining in
/// Shared MESI cache state instead of issuing failing `LOCK CMPXCHG` invalidations).
pub struct TtasBackoffSpinlock<T> {
    locked: CacheAligned<AtomicBool>,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for TtasBackoffSpinlock<T> {}
unsafe impl<T: Send> Send for TtasBackoffSpinlock<T> {}

impl<T> TtasBackoffSpinlock<T> {
    pub const fn new(data: T) -> Self {
        Self {
            locked: CacheAligned::new(AtomicBool::new(false)),
            data: UnsafeCell::new(data),
        }
    }

    pub fn lock(&self) -> TtasGuard<'_, T> {
        let re_enable = are_interrupts_enabled();
        disable_interrupts();

        let mut backoff = 1u32;
        loop {
            // Fast atomic attempt
            if !self.locked.value.swap(true, Ordering::Acquire) {
                break;
            }
            // Wait read-only until holder releases + exponential backoff
            while self.locked.value.load(Ordering::Relaxed) {
                for _ in 0..backoff {
                    core::hint::spin_loop();
                }
                backoff = core::cmp::min(backoff << 1, 64);
            }
        }

        TtasGuard {
            lock: self,
            re_enable_interrupts: re_enable,
        }
    }
}

pub struct TtasGuard<'a, T> {
    lock: &'a TtasBackoffSpinlock<T>,
    re_enable_interrupts: bool,
}

impl<T> Deref for TtasGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for TtasGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for TtasGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.value.store(false, Ordering::Release);
        if self.re_enable_interrupts {
            enable_interrupts();
        }
    }
}

// ============================================================================
// 3. SeqLock (Wait-Free Concurrent Readers)
// ============================================================================

/// Sequence Lock (`SeqLock<T>`) providing wait-free optimistic reads without dirtying
/// cache lines on the reader path.
///
/// Odd sequence numbers indicate a write is in progress; even sequence numbers indicate
/// quiescent consistent snapshots.
pub struct SeqLock<T: Copy> {
    seq: CacheAligned<AtomicU64>,
    data: UnsafeCell<T>,
}

unsafe impl<T: Copy + Send> Sync for SeqLock<T> {}
unsafe impl<T: Copy + Send> Send for SeqLock<T> {}

impl<T: Copy> SeqLock<T> {
    pub const fn new(data: T) -> Self {
        Self {
            seq: CacheAligned::new(AtomicU64::new(0)),
            data: UnsafeCell::new(data),
        }
    }

    /// Reads a consistent snapshot cleanly without acquiring any exclusive write lock.
    #[inline]
    pub fn read(&self) -> T {
        loop {
            let s1 = self.seq.value.load(Ordering::Acquire);
            if (s1 & 1) != 0 {
                core::hint::spin_loop();
                continue;
            }
            // SAFETY: Torn reads are caught by the trailing sequence check (s1 == s2).
            let val = unsafe { core::ptr::read_volatile(self.data.get()) };
            fence(Ordering::Acquire);
            let s2 = self.seq.value.load(Ordering::Relaxed);
            if s1 == s2 {
                return val;
            }
        }
    }

    /// Mutates the protected state while holding an odd-sequence writer barrier.
    pub fn write<F>(&self, mutate: F)
    where
        F: FnOnce(&mut T),
    {
        let re_enable = are_interrupts_enabled();
        disable_interrupts();

        // Increment sequence to odd
        loop {
            let cur = self.seq.value.load(Ordering::Relaxed);
            if (cur & 1) == 0
                && self
                    .seq
                    .value
                    .compare_exchange_weak(cur, cur + 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            {
                break;
            }
            core::hint::spin_loop();
        }

        fence(Ordering::Release);
        unsafe {
            mutate(&mut *self.data.get());
        }
        fence(Ordering::Release);

        // Increment sequence to next even generation
        self.seq.value.fetch_add(1, Ordering::Release);

        if re_enable {
            enable_interrupts();
        }
    }
}

