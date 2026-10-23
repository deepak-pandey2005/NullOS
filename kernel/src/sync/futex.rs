//! Fast Userspace Mutex (Futex) Subsystem (Phase 11 - M28).
//!
//! Provides the core synchronization engine for Ring 3 userspace threads:
//! - `FUTEX_WAIT`: Blocks if value matches expected condition.
//! - `FUTEX_WAKE`: Wakes up specified number of waiters on a userspace address.
//! - `FUTEX_REQUEUE`: Requeues waiters from one futex address to another (used for condvars).

use crate::sync::Spinlock;
use crate::task::process::ProcessId;
use crate::task::thread::ThreadId;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

pub const FUTEX_WAIT: u32 = 0;
pub const FUTEX_WAKE: u32 = 1;
pub const FUTEX_REQUEUE: u32 = 3;
pub const FUTEX_PRIVATE_FLAG: u32 = 128;

/// Global registry tracking blocked user threads keyed by (ProcessId, Userspace Virtual Address).
struct FutexRegistry {
    waiters: BTreeMap<(ProcessId, u64), Vec<ThreadId>>,
}

impl FutexRegistry {
    const fn new() -> Self {
        Self {
            waiters: BTreeMap::new(),
        }
    }
}

static FUTEX_REGISTRY: Spinlock<FutexRegistry> = Spinlock::new(FutexRegistry::new());

/// Implements `sys_futex(FUTEX_WAIT)`.
/// Checks if the 32-bit integer at `uaddr` equals `expected`.
/// If so, blocks the calling thread until a matching `FUTEX_WAKE` is called.
pub fn futex_wait(uaddr: u64, expected: u32) -> Result<i32, &'static str> {
    crate::security::validate_user_buffer(uaddr as *const u8, 4).map_err(|_| "EFAULT")?;

    // Atomically read value from user space
    let actual_val = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
    if actual_val != expected {
        return Err("EAGAIN");
    }

    let pid = crate::task::syscall::current_pid();
    let tid = crate::task::current_thread_id().ok_or("No active thread")?;

    {
        let mut reg = FUTEX_REGISTRY.lock();
        let queue = reg.waiters.entry((pid, uaddr)).or_insert_with(Vec::new);
        queue.push(tid);
    }

    // Block current thread until woken by FUTEX_WAKE
    crate::task::block_current_thread();

    Ok(0)
}

/// Implements `sys_futex(FUTEX_WAKE)`.
/// Wakes up at most `max_wake` threads waiting on `uaddr`.
pub fn futex_wake(uaddr: u64, max_wake: u32) -> Result<usize, &'static str> {
    crate::security::validate_user_buffer(uaddr as *const u8, 4).map_err(|_| "EFAULT")?;

    let pid = crate::task::syscall::current_pid();
    let mut to_wake: Vec<ThreadId> = Vec::new();

    {
        let mut reg = FUTEX_REGISTRY.lock();
        if let Some(queue) = reg.waiters.get_mut(&(pid, uaddr)) {
            let wake_count = core::cmp::min(max_wake as usize, queue.len());
            for _ in 0..wake_count {
                if !queue.is_empty() {
                    to_wake.push(queue.remove(0));
                }
            }
            if queue.is_empty() {
                reg.waiters.remove(&(pid, uaddr));
            }
        }
    }

    let mut woken = 0;
    for tid in to_wake {
        if crate::task::wake_thread(tid) {
            woken += 1;
        }
    }

    Ok(woken)
}

/// Implements `sys_futex(FUTEX_REQUEUE)`.
/// Wakes up to `max_wake` threads on `uaddr1` and requeues up to `max_requeue`
/// remaining threads onto `uaddr2`.
pub fn futex_requeue(
    uaddr1: u64,
    max_wake: u32,
    uaddr2: u64,
    max_requeue: u32,
) -> Result<usize, &'static str> {
    crate::security::validate_user_buffer(uaddr1 as *const u8, 4).map_err(|_| "EFAULT")?;
    crate::security::validate_user_buffer(uaddr2 as *const u8, 4).map_err(|_| "EFAULT")?;

    let pid = crate::task::syscall::current_pid();
    let mut to_wake: Vec<ThreadId> = Vec::new();

    {
        let mut reg = FUTEX_REGISTRY.lock();
        if let Some(mut queue1) = reg.waiters.remove(&(pid, uaddr1)) {
            let wake_count = core::cmp::min(max_wake as usize, queue1.len());
            for _ in 0..wake_count {
                if !queue1.is_empty() {
                    to_wake.push(queue1.remove(0));
                }
            }

            let requeue_count = core::cmp::min(max_requeue as usize, queue1.len());
            if requeue_count > 0 {
                let queue2 = reg.waiters.entry((pid, uaddr2)).or_insert_with(Vec::new);
                for _ in 0..requeue_count {
                    if !queue1.is_empty() {
                        queue2.push(queue1.remove(0));
                    }
                }
            }

            if !queue1.is_empty() {
                reg.waiters.insert((pid, uaddr1), queue1);
            }
        }
    }

    let mut woken = 0;
    for tid in to_wake {
        if crate::task::wake_thread(tid) {
            woken += 1;
        }
    }

    Ok(woken)
}

/// Self-test validating Futex wait queue mechanics.
pub fn run_self_tests() {
    // Tests are exercised in task test suite with threads
}
