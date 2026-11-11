//! # Syscall Optimization & Fast Jump-Table Dispatch (Phase 17 - M38)
//!
//! ## Overview
//! Optimizes the Ring 3 -> Ring 0 system call fast path and TLS context-switch helpers:
//! - **`FSGSBASE` Detection (`CR4.FSGSBASE` / `CPUID.07H:EBX[0]`)**: Allows single-cycle
//!   `rdfsbase` / `wrfsbase` user TLS switching vs `wrmsr(IA32_FS_BASE)` serialization penalties.
//! - **O(1) Syscall Jump Dispatch Table (`FastSyscallTable`)**: Direct `O(1)` function-pointer
//!   indexing (`table[syscall_num]`) replacing long conditional branch chains.
//! - **Lock-Free Read-Mostly Syscall Cache**: Caches hot read-only state (`FAST_CACHED_PID`,
//!   ticks) in an atomic slot for sub-microsecond syscall queries (`SYS_GETPID`, `SYS_TIME`).

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::{apic, cpu};

/// Maximum syscall slots indexed in O(1) inside [`FastSyscallTable`].
pub const MAX_FAST_SYSCALLS: usize = 64;

/// Fast syscall handler signature matching System V AMD64 arguments.
pub type FastSyscallFn = fn(u64, u64, u64) -> u64;

/// Hardware fast-TLS (`FSGSBASE`) capabilities detected on silicon.
#[derive(Debug, Clone, Copy)]
pub struct SyscallOptCaps {
    /// CPU supports `rdfsbase` / `wrfsbase` (`CPUID.07H:EBX[0]`).
    pub fsgsbase_supported: bool,
    /// `CR4.FSGSBASE` (bit 16) is currently enabled.
    pub fsgsbase_active: bool,
}

static FAST_CACHED_PID: AtomicU64 = AtomicU64::new(1);
static FAST_DISPATCH_HITS: AtomicU64 = AtomicU64::new(0);

/// Updates cached current PID when scheduler switches tasks (avoids spinlock on `getpid`).
#[inline]
pub fn update_cached_pid(pid: u64) {
    FAST_CACHED_PID.store(pid, Ordering::Relaxed);
}

#[inline]
fn fast_sys_getpid(_a1: u64, _a2: u64, _a3: u64) -> u64 {
    FAST_CACHED_PID.load(Ordering::Relaxed)
}

#[inline]
fn fast_sys_ticks(_a1: u64, _a2: u64, _a3: u64) -> u64 {
    apic::get_ticks()
}

#[inline]
fn fast_sys_rdtsc(_a1: u64, _a2: u64, _a3: u64) -> u64 {
    crate::perf::tsc::read_tsc()
}

/// O(1) Function-Pointer Jump Table for latency-critical syscalls.
pub struct FastSyscallTable {
    handlers: [Option<FastSyscallFn>; MAX_FAST_SYSCALLS],
}

impl FastSyscallTable {
    pub const fn new() -> Self {
        let mut table: [Option<FastSyscallFn>; MAX_FAST_SYSCALLS] = [None; MAX_FAST_SYSCALLS];
        table[39] = Some(fast_sys_getpid); // O(1) SYS_GETPID (39)
        table[50] = Some(fast_sys_ticks);  // O(1) Kernel monotonic ticks
        table[51] = Some(fast_sys_rdtsc);  // O(1) RDTSC cycle query
        Self { handlers: table }
    }

    /// Dispatches `num` in strict **O(1)** array-index lookup with single predictable call.
    #[inline(always)]
    pub fn try_dispatch(&self, num: u64, a1: u64, a2: u64, a3: u64) -> Option<u64> {
        let idx = num as usize;
        if idx < MAX_FAST_SYSCALLS {
            if let Some(handler) = self.handlers[idx] {
                FAST_DISPATCH_HITS.fetch_add(1, Ordering::Relaxed);
                return Some(handler(a1, a2, a3));
            }
        }
        None
    }
}

pub static FAST_SYSCALL_TABLE: FastSyscallTable = FastSyscallTable::new();

/// Probes `FSGSBASE` support and activates `CR4.FSGSBASE` (bit 16) when supported by hardware.
pub fn init_syscall_opt() -> SyscallOptCaps {
    let (max_basic, _, _, _) = cpu::cpuid(0, 0);
    let fsgsbase_supported = if max_basic >= 7 {
        let (_, ebx7, _, _) = cpu::cpuid(7, 0);
        (ebx7 & (1 << 0)) != 0
    } else {
        false
    };

    let mut fsgsbase_active = false;
    if fsgsbase_supported {
        let mut cr4: u64;
        unsafe {
            core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
            cr4 |= 1 << 16; // CR4.FSGSBASE
            core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack));
            fsgsbase_active = true;
        }
    }

    SyscallOptCaps {
        fsgsbase_supported,
        fsgsbase_active,
    }
}

pub fn fast_dispatch_hits() -> u64 {
    FAST_DISPATCH_HITS.load(Ordering::Relaxed)
}
