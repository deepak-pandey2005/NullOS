//! # Time Stamp Counter (TSC) Support & Cycle Counters (Phase 17 - M37)
//!
//! ## Overview
//! Provides hardware-backed, sub-nanosecond cycle timing via the x86_64 `RDTSC` and
//! `RDTSCP` instructions alongside CPUID capability probing and timer-calibrated
//! frequency estimation.
//!
//! ## Serialization & Out-of-Order Execution
//! Modern x86_64 cores execute instructions out of order, meaning a bare `RDTSC` can
//! float ahead or behind surrounding instructions inside tight microbenchmarks.
//! - [`read_tsc`] provides the lowest-overhead (~20 cycles) timestamp for coarse deltas.
//! - [`read_tsc_start`] inserts an `LFENCE; RDTSC` serialization fence before measurement.
//! - [`read_tsc_end`] uses `RDTSCP; LFENCE` (when available) or `MFENCE; RDTSC; LFENCE`
//!   so all measured instructions finish retiring before the ending timestamp is sampled.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::x86_64::{apic, cpu};

/// Default fallback TSC frequency (2.0 GHz) prior to hardware calibration.
pub const DEFAULT_TSC_FREQ_HZ: u64 = 2_000_000_000;

static TSC_FREQ_HZ: AtomicU64 = AtomicU64::new(DEFAULT_TSC_FREQ_HZ);
static TSC_CALIBRATED: AtomicBool = AtomicBool::new(false);
static HAS_RDTSC: AtomicBool = AtomicBool::new(true);
static HAS_RDTSCP: AtomicBool = AtomicBool::new(false);
static HAS_INVARIANT_TSC: AtomicBool = AtomicBool::new(false);
static BOOT_TSC: AtomicU64 = AtomicU64::new(0);

/// Hardware Time Stamp Counter capability report probed from CPUID leaves.
#[derive(Debug, Clone, Copy)]
pub struct TscInfo {
    /// CPU supports `RDTSC` instruction (`CPUID.01H:EDX[4]`).
    pub rdtsc_supported: bool,
    /// CPU supports `RDTSCP` serializing read + `IA32_TSC_AUX` (`CPUID.80000001H:EDX[27]`).
    pub rdtscp_supported: bool,
    /// TSC runs at a constant rate across P-states/C-states/deep sleep (`CPUID.80000007H:EDX[8]`).
    pub invariant_tsc: bool,
    /// Calibrated or CPUID-reported TSC frequency in Hertz.
    pub frequency_hz: u64,
    /// Frequency expressed in Megahertz (`MHz`).
    pub frequency_mhz: u64,
    /// Whether live calibration against hardware timers succeeded.
    pub calibrated: bool,
    /// Baseline TSC snapshot captured during `tsc::init()`.
    pub boot_tsc: u64,
}

/// Reads the raw 64-bit x86_64 Time Stamp Counter via `RDTSC` (non-serializing, low overhead).
#[inline(always)]
pub fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: RDTSC has no memory side effects and is permitted in Ring 0 whenever CR4.TSD=0.
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Reads a **start-serialized** 64-bit TSC using `LFENCE; RDTSC` to prevent later instructions
/// from speculating before the timestamp is captured.
#[inline(always)]
pub fn read_tsc_start() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: LFENCE serializes instruction stream dispatch prior to RDTSC.
    unsafe {
        core::arch::asm!(
            "lfence",
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Reads an **end-serialized** 64-bit TSC (`RDTSCP; LFENCE` if supported, else `MFENCE; RDTSC; LFENCE`).
#[inline(always)]
pub fn read_tsc_end() -> u64 {
    if HAS_RDTSCP.load(Ordering::Relaxed) {
        let lo: u32;
        let hi: u32;
        // SAFETY: RDTSCP waits for all prior instructions to execute before sampling TSC.
        // We clobber ECX (IA32_TSC_AUX processor ID) cleanly via `out("ecx") _`.
        unsafe {
            core::arch::asm!(
                "rdtscp",
                "lfence",
                out("eax") lo,
                out("edx") hi,
                out("ecx") _,
                options(nomem, nostack, preserves_flags)
            );
        }
        ((hi as u64) << 32) | (lo as u64)
    } else {
        let lo: u32;
        let hi: u32;
        // SAFETY: MFENCE + LFENCE fence prior stores/loads and instruction execution around RDTSC.
        unsafe {
            core::arch::asm!(
                "mfence",
                "rdtsc",
                "lfence",
                out("eax") lo,
                out("edx") hi,
                options(nomem, nostack, preserves_flags)
            );
        }
        ((hi as u64) << 32) | (lo as u64)
    }
}

/// Probes CPUID for TSC features and calibrates TSC frequency against CPUID leaf `0x15`/`0x16`
/// or the Local APIC / PIT microsecond delay timer.
pub fn init() {
    let (max_basic, _, _, _) = cpu::cpuid(0, 0);
    let (_, _, _, edx1) = cpu::cpuid(1, 0);
    let rdtsc_ok = (edx1 & (1 << 4)) != 0;
    HAS_RDTSC.store(rdtsc_ok, Ordering::Relaxed);

    let (max_ext, _, _, _) = cpu::cpuid(0x8000_0000, 0);
    let rdtscp_ok = if max_ext >= 0x8000_0001 {
        let (_, _, _, edx_ext1) = cpu::cpuid(0x8000_0001, 0);
        (edx_ext1 & (1 << 27)) != 0
    } else {
        false
    };
    HAS_RDTSCP.store(rdtscp_ok, Ordering::Relaxed);

    let invariant_ok = if max_ext >= 0x8000_0007 {
        let (_, _, _, edx_ext7) = cpu::cpuid(0x8000_0007, 0);
        (edx_ext7 & (1 << 8)) != 0
    } else {
        false
    };
    HAS_INVARIANT_TSC.store(invariant_ok, Ordering::Relaxed);

    // Attempt 1: CPUID Leaf 0x15 (Time Stamp Counter and Nominal Core Crystal Clock)
    let mut detected_hz: Option<u64> = None;
    if max_basic >= 0x15 {
        let (denom, numer, crystal_hz, _) = cpu::cpuid(0x15, 0);
        if denom != 0 && numer != 0 && crystal_hz != 0 {
            detected_hz = Some((crystal_hz as u64) * (numer as u64) / (denom as u64));
        }
    }

    // Attempt 2: CPUID Leaf 0x16 (Processor Base Frequency in MHz)
    if detected_hz.is_none() && max_basic >= 0x16 {
        let (base_mhz, _, _, _) = cpu::cpuid(0x16, 0);
        if base_mhz != 0 {
            detected_hz = Some((base_mhz as u64) * 1_000_000);
        }
    }

    // Attempt 3: Empirical calibration over a 2000 us spin delay
    if detected_hz.is_none() && rdtsc_ok {
        let t0 = read_tsc_start();
        apic::spin_delay_us(2_000);
        let t1 = read_tsc_end();
        let delta = t1.saturating_sub(t0);
        if delta > 10_000 {
            // 2000 us = 1/500th of a second -> scale by 500 to estimate Hz
            let est = delta.saturating_mul(500);
            // Clamp to plausible x86_64 range [100 MHz, 10 GHz]
            detected_hz = Some(est.clamp(100_000_000, 10_000_000_000));
        }
    }

    let final_hz = detected_hz.unwrap_or(DEFAULT_TSC_FREQ_HZ);
    TSC_FREQ_HZ.store(final_hz, Ordering::Relaxed);
    TSC_CALIBRATED.store(detected_hz.is_some(), Ordering::Relaxed);
    BOOT_TSC.store(read_tsc_start(), Ordering::Relaxed);
}

/// Returns current TSC hardware capabilities and calibrated frequency snapshot.
pub fn info() -> TscInfo {
    let freq = TSC_FREQ_HZ.load(Ordering::Relaxed);
    TscInfo {
        rdtsc_supported: HAS_RDTSC.load(Ordering::Relaxed),
        rdtscp_supported: HAS_RDTSCP.load(Ordering::Relaxed),
        invariant_tsc: HAS_INVARIANT_TSC.load(Ordering::Relaxed),
        frequency_hz: freq,
        frequency_mhz: freq / 1_000_000,
        calibrated: TSC_CALIBRATED.load(Ordering::Relaxed),
        boot_tsc: BOOT_TSC.load(Ordering::Relaxed),
    }
}

/// Converts a raw cycle delta into nanoseconds using the calibrated TSC frequency.
#[inline]
pub fn cycles_to_ns(cycles: u64) -> u64 {
    let freq = core::cmp::max(1, TSC_FREQ_HZ.load(Ordering::Relaxed));
    let wide = (cycles as u128).saturating_mul(1_000_000_000);
    (wide / (freq as u128)) as u64
}

/// Converts a raw cycle delta into microseconds using the calibrated TSC frequency.
#[inline]
pub fn cycles_to_us(cycles: u64) -> u64 {
    cycles_to_ns(cycles) / 1_000
}

/// Returns microseconds elapsed since `tsc::init()` booted.
#[inline]
pub fn uptime_us() -> u64 {
    let now = read_tsc();
    let boot = BOOT_TSC.load(Ordering::Relaxed);
    cycles_to_us(now.saturating_sub(boot))
}

/// High-precision serialized stopwatch (`CycleCounter`) for scoping kernel code blocks.
pub struct CycleCounter {
    start: u64,
}

impl CycleCounter {
    /// Starts a serialized cycle measurement immediately.
    #[inline(always)]
    pub fn start() -> Self {
        Self {
            start: read_tsc_start(),
        }
    }

    /// Stops measurement with serialization and returns elapsed CPU cycles.
    #[inline(always)]
    pub fn elapsed_cycles(&self) -> u64 {
        read_tsc_end().saturating_sub(self.start)
    }

    /// Stops measurement and returns elapsed time in nanoseconds.
    #[inline(always)]
    pub fn elapsed_ns(&self) -> u64 {
        cycles_to_ns(self.elapsed_cycles())
    }
}

