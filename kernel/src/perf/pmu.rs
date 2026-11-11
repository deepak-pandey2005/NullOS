//! # CPU Architectural Performance Monitoring Unit (PMU) (Phase 17 - M37)
//!
//! ## Overview
//! Detects and interfaces with x86_64 hardware Performance Monitoring Counters via
//! `CPUID` Leaf `0x0A` (Architectural Performance Monitoring) and MSRs:
//! - `IA32_PERFEVTSEL0..N` (`0x186+`): General-purpose event selector registers.
//! - `IA32_PMC0..N` (`0xC1+`): Programmable event accumulator MSRs.
//! - `IA32_FIXED_CTR0..2` (`0x309..0x30B`): Fixed-function retired instructions,
//!   unhalted core cycles, and unhalted reference cycles.
//!
//! ## Hypervisor / QEMU TCG Safety
//! Under QEMU TCG (`qemu64` / non-KVM software emulation), `CPUID.0AH:EAX[7:0]`
//! frequently reports `PMU Version == 0` (meaning PMU MSRs are unimplemented and
//! accessing them with `rdmsr`/`wrmsr` would raise `#GP(0)`).
//! This driver strictly gates hardware MSR accesses on `pmu_version > 0` and falls
//! back cleanly to **Software-Emulated Architectural Telemetry** when running without
//! silicon PMU support.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

use crate::arch::x86_64::cpu;

// x86_64 Architectural PMU MSR addresses
const IA32_PMC0_MSR: u32 = 0x00C1;
const IA32_PERFEVTSEL0_MSR: u32 = 0x0186;
const IA32_FIXED_CTR0_INST_RETIRED: u32 = 0x0309;
const IA32_FIXED_CTR1_CPU_CLK_UNHALTED: u32 = 0x030A;
const IA32_FIXED_CTR_CTRL_MSR: u32 = 0x038D;
const IA32_PERF_GLOBAL_CTRL_MSR: u32 = 0x038F;

/// Standard Intel/AMD Architectural Performance Monitoring Event Selectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PmuEvent {
    /// Unhalted Core Cycles (`Event 0x3C`, `Umask 0x00`).
    UnhaltedCoreCycles,
    /// Instructions Retired (`Event 0xC0`, `Umask 0x00`).
    InstructionsRetired,
    /// Last-Level Cache References (`Event 0x2E`, `Umask 0x4F`).
    LlcReferences,
    /// Last-Level Cache Misses (`Event 0x2E`, `Umask 0x41`).
    LlcMisses,
    /// Branch Instructions Retired (`Event 0xC4`, `Umask 0x00`).
    BranchInstructions,
    /// Branch Mispredictions Retired (`Event 0xC5`, `Umask 0x00`).
    BranchMisses,
}

impl PmuEvent {
    /// Returns the architectural `(event_select, umask)` pair from Intel SDM Vol. 3 Table 18-1.
    pub const fn encoding(self) -> (u8, u8) {
        match self {
            Self::UnhaltedCoreCycles => (0x3C, 0x00),
            Self::InstructionsRetired => (0xC0, 0x00),
            Self::LlcReferences => (0x2E, 0x4F),
            Self::LlcMisses => (0x2E, 0x41),
            Self::BranchInstructions => (0xC4, 0x00),
            Self::BranchMisses => (0xC5, 0x00),
        }
    }
}

/// Detected hardware PMU capabilities plus live counter snapshot.
#[derive(Debug, Clone, Copy)]
pub struct PmuInfo {
    /// Architectural PMU version (`0` when unsupported / QEMU TCG).
    pub version: u8,
    /// Number of programmable general-purpose counters per logical core.
    pub num_gp_counters: u8,
    /// Bit width of each general-purpose counter.
    pub gp_counter_bits: u8,
    /// Number of fixed-function counters (`InstructionsRetired`, `ClkUnhalted`, etc.).
    pub num_fixed_counters: u8,
    /// Whether hardware MSRs are active (false triggers software-assisted fallback).
    pub hardware_active: bool,
    /// Observed unhalted cycles (hardware fixed CTR1 or TSC-derived fallback).
    pub unhalted_cycles: u64,
    /// Observed retired instructions / kernel event operations.
    pub instructions_retired: u64,
    /// Tracked interrupt service events.
    pub hw_irq_events: u64,
    /// Tracked context switches.
    pub ctx_switch_events: u64,
    /// Tracked syscall invocations.
    pub syscall_events: u64,
}

static PMU_VERSION: AtomicU8 = AtomicU8::new(0);
static PMU_GP_COUNTERS: AtomicU8 = AtomicU8::new(0);
static PMU_GP_BITS: AtomicU8 = AtomicU8::new(0);
static PMU_FIXED_COUNTERS: AtomicU8 = AtomicU8::new(0);
static PMU_HW_ACTIVE: AtomicBool = AtomicBool::new(false);

// Software-assisted counters (always maintained safely across KVM, bare-metal & TCG)
static SW_INSTRUCTIONS_ACC: AtomicU64 = AtomicU64::new(0);
static SW_IRQ_EVENTS: AtomicU64 = AtomicU64::new(0);
static SW_CTX_SWITCHES: AtomicU64 = AtomicU64::new(0);
static SW_SYSCALL_EVENTS: AtomicU64 = AtomicU64::new(0);

/// Probes `CPUID` Leaf `0x0A` and initializes hardware PMU counters if supported by silicon.
pub fn init() {
    let (max_basic, _, _, _) = cpu::cpuid(0, 0);
    if max_basic < 0x0A {
        PMU_VERSION.store(0, Ordering::Relaxed);
        PMU_HW_ACTIVE.store(false, Ordering::Relaxed);
        return;
    }

    let (eax, _ebx, _ecx, edx) = cpu::cpuid(0x0A, 0);
    let version = (eax & 0xFF) as u8;
    let gp_count = ((eax >> 8) & 0xFF) as u8;
    let gp_bits = ((eax >> 16) & 0xFF) as u8;
    let fixed_count = if version >= 2 { (edx & 0x1F) as u8 } else { 0 };

    PMU_VERSION.store(version, Ordering::Relaxed);
    PMU_GP_COUNTERS.store(gp_count, Ordering::Relaxed);
    PMU_GP_BITS.store(gp_bits, Ordering::Relaxed);
    PMU_FIXED_COUNTERS.store(fixed_count, Ordering::Relaxed);

    if version > 0 && gp_count > 0 {
        // Program GP counter 0 for Instructions Retired (or Unhalted Cycles) in User+OS mode:
        // Bit 16 (USR) | Bit 17 (OS) | Bit 22 (EN) = 0x0043_0000
        let (ev, umask) = PmuEvent::InstructionsRetired.encoding();
        let perfevtsel = 0x0043_0000u64 | ((umask as u64) << 8) | (ev as u64);
        unsafe {
            cpu::write_msr(IA32_PMC0_MSR, 0);
            cpu::write_msr(IA32_PERFEVTSEL0_MSR, perfevtsel);
            if version >= 2 && fixed_count >= 2 {
                // Enable Fixed CTR0 & CTR1 for OS+USR (`0x3` per 4-bit field -> 0x33)
                cpu::write_msr(IA32_FIXED_CTR_CTRL_MSR, 0x33);
                // Enable GP0 + Fixed0 + Fixed1 in IA32_PERF_GLOBAL_CTRL
                let global_ctrl = 0x1u64 | (0x3u64 << 32);
                cpu::write_msr(IA32_PERF_GLOBAL_CTRL_MSR, global_ctrl);
            }
        }
        PMU_HW_ACTIVE.store(true, Ordering::Relaxed);
    } else {
        PMU_HW_ACTIVE.store(false, Ordering::Relaxed);
    }
}

/// Configures general-purpose counter 0 when hardware PMU is active.
pub fn select_gp_event(event: PmuEvent) -> bool {
    if !PMU_HW_ACTIVE.load(Ordering::Relaxed) {
        return false;
    }
    let (ev, umask) = event.encoding();
    let perfevtsel = 0x0043_0000u64 | ((umask as u64) << 8) | (ev as u64);
    unsafe {
        cpu::write_msr(IA32_PMC0_MSR, 0);
        cpu::write_msr(IA32_PERFEVTSEL0_MSR, perfevtsel);
    }
    true
}

/// Records kernel software PMU event increments (invoked automatically by instrumentation hooks).
#[inline]
pub fn record_sw_irq() {
    SW_IRQ_EVENTS.fetch_add(1, Ordering::Relaxed);
    SW_INSTRUCTIONS_ACC.fetch_add(128, Ordering::Relaxed);
}

#[inline]
pub fn record_sw_ctx_switch() {
    SW_CTX_SWITCHES.fetch_add(1, Ordering::Relaxed);
    SW_INSTRUCTIONS_ACC.fetch_add(256, Ordering::Relaxed);
}

#[inline]
pub fn record_sw_syscall() {
    SW_SYSCALL_EVENTS.fetch_add(1, Ordering::Relaxed);
    SW_INSTRUCTIONS_ACC.fetch_add(192, Ordering::Relaxed);
}

/// Reads current hardware PMU counters (if active) or software-assisted performance telemetry.
pub fn snapshot() -> PmuInfo {
    let hw_active = PMU_HW_ACTIVE.load(Ordering::Relaxed);
    let version = PMU_VERSION.load(Ordering::Relaxed);
    let fixed_cnt = PMU_FIXED_COUNTERS.load(Ordering::Relaxed);

    let (unhalted_cycles, instructions_retired) = if hw_active {
        unsafe {
            let pmc0 = cpu::read_msr(IA32_PMC0_MSR);
            if version >= 2 && fixed_cnt >= 2 {
                let fc0 = cpu::read_msr(IA32_FIXED_CTR0_INST_RETIRED);
                let fc1 = cpu::read_msr(IA32_FIXED_CTR1_CPU_CLK_UNHALTED);
                (fc1, core::cmp::max(fc0, pmc0))
            } else {
                (super::tsc::read_tsc(), pmc0)
            }
        }
    } else {
        let now_tsc = super::tsc::read_tsc();
        let boot_tsc = super::tsc::info().boot_tsc;
        (
            now_tsc.saturating_sub(boot_tsc),
            SW_INSTRUCTIONS_ACC.load(Ordering::Relaxed),
        )
    };

    PmuInfo {
        version,
        num_gp_counters: PMU_GP_COUNTERS.load(Ordering::Relaxed),
        gp_counter_bits: PMU_GP_BITS.load(Ordering::Relaxed),
        num_fixed_counters: fixed_cnt,
        hardware_active: hw_active,
        unhalted_cycles,
        instructions_retired,
        hw_irq_events: SW_IRQ_EVENTS.load(Ordering::Relaxed),
        ctx_switch_events: SW_CTX_SWITCHES.load(Ordering::Relaxed),
        syscall_events: SW_SYSCALL_EVENTS.load(Ordering::Relaxed),
    }
}

