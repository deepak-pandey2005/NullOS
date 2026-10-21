//! CPU control registers and modern architectural features for x86_64.
//!
//! Enables:
//! - CR0.WP: Kernel Write Protection for read-only pages (prevents kernel corruption).
//! - EFER.NXE: No-Execute bit in page tables (prevents code execution on stack/heap/MMIO).
//! - CR4.PGE: Page Global Enable (preserves global kernel pages across CR3 switches).
//! - CR4.OSFXSR: SSE/AVX instructions and XMM register support.
//! - CR4.OSXMMEXCPT: Unmasked SIMD floating-point exceptions (#XM).
#![allow(dead_code)]

use crate::println;

const IA32_EFER_MSR: u32 = 0xC000_0080;

#[inline]
unsafe fn read_msr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        core::arch::asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high, options(nomem, nostack));
    }
    ((high as u64) << 32) | (low as u64)
}

#[inline]
unsafe fn write_msr(msr: u32, val: u64) {
    let low = val as u32;
    let high = (val >> 32) as u32;
    unsafe {
        core::arch::asm!("wrmsr", in("ecx") msr, in("eax") low, in("edx") high, options(nomem, nostack));
    }
}

#[inline]
unsafe fn read_cr0() -> u64 {
    let val: u64;
    unsafe {
        core::arch::asm!("mov {}, cr0", out(reg) val, options(nomem, nostack));
    }
    val
}

#[inline]
unsafe fn write_cr0(val: u64) {
    unsafe {
        core::arch::asm!("mov cr0, {}", in(reg) val, options(nomem, nostack));
    }
}

#[inline]
unsafe fn read_cr4() -> u64 {
    let val: u64;
    unsafe {
        core::arch::asm!("mov {}, cr4", out(reg) val, options(nomem, nostack));
    }
    val
}

#[inline]
unsafe fn write_cr4(val: u64) {
    unsafe {
        core::arch::asm!("mov cr4, {}", in(reg) val, options(nomem, nostack));
    }
}

/// Initializes modern x86_64 CPU hardware features and security flags.
pub fn init() {
    unsafe {
        // 1. Enable CR0.WP (Bit 16 - Write Protect in Ring 0)
        let cr0 = read_cr0();
        let wp_flag = 1 << 16;
        if (cr0 & wp_flag) == 0 {
            write_cr0(cr0 | wp_flag);
        }

        // 2. Enable EFER.NXE (Bit 11 - No-Execute / Execute-Disable Enable)
        let efer = read_msr(IA32_EFER_MSR);
        let nxe_flag = 1 << 11;
        if (efer & nxe_flag) == 0 {
            write_msr(IA32_EFER_MSR, efer | nxe_flag);
        }

        // 3. Enable CR4 features:
        // - Bit 7: PGE (Page Global Enable)
        // - Bit 9: OSFXSR (Operating System FXSAVE/FXRSTOR Support - SSE)
        // - Bit 10: OSXMMEXCPT (Operating System Unmasked SIMD Exception Support)
        let mut cr4 = read_cr4();
        cr4 |= (1 << 7) | (1 << 9) | (1 << 10);
        write_cr4(cr4);
    }

    println!(
        "[CPU] Modern CPU features activated: CR0.WP (Write-Protect), EFER.NXE (No-Execute), CR4.PGE (Global Pages), CR4.OSFXSR (SSE/AVX)."
    );
}
