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

/// Queries the x86 CPUID instruction for the specified leaf and subleaf.
#[inline]
pub fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let mut eax: u32;
    let ebx: u32;
    let mut ecx: u32;
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {tmp:e}, ebx",
            "pop rbx",
            inout("eax") leaf => eax,
            inout("ecx") subleaf => ecx,
            tmp = out(reg) ebx,
            out("edx") edx,
            options(nomem)
        );
    }
    (eax, ebx, ecx, edx)
}

/// Temporarily enables supervisor mode access to user memory (sets EFLAGS.AC = 1).
#[inline(always)]
pub unsafe fn stac() {
    unsafe {
        core::arch::asm!("stac", options(nomem, nostack));
    }
}

/// Disables supervisor mode access to user memory (clears EFLAGS.AC = 0).
#[inline(always)]
pub unsafe fn clac() {
    unsafe {
        core::arch::asm!("clac", options(nomem, nostack));
    }
}

#[inline]
pub fn is_wp_enabled() -> bool {
    (read_cr0() & (1 << 16)) != 0
}

#[inline]
pub fn is_nxe_enabled() -> bool {
    unsafe { (read_msr(IA32_EFER_MSR) & (1 << 11)) != 0 }
}

#[inline]
pub fn is_smep_enabled() -> bool {
    (read_cr4() & (1 << 20)) != 0
}

#[inline]
pub fn is_smap_enabled() -> bool {
    (read_cr4() & (1 << 21)) != 0
}

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
pub fn read_cr0() -> u64 {
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
pub fn read_cr4() -> u64 {
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
        // 1. Configure CR0:
        // - Bit 1: MP (Monitor Coprocessor) -> 1
        // - Bit 2: EM (Emulation) -> 0 (Clear EM to allow CPU hardware execution of x87/SSE/AVX without #UD)
        // - Bit 3: TS (Task Switched) -> 0 (Clear TS to prevent #NM exceptions on SSE instructions)
        // - Bit 16: WP (Write Protect) -> 1 (Enforce Ring 0 write protection on read-only pages)
        let mut cr0 = read_cr0();
        cr0 |= (1 << 16) | (1 << 1); // Enable WP and MP
        cr0 &= !(1 << 2);            // Clear EM (prevents #UD on FPU/SSE/AVX)
        cr0 &= !(1 << 3);            // Clear TS (prevents #NM on FPU/SSE/AVX)
        write_cr0(cr0);

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

        // 4. Query Extended Features via CPUID Leaf 7 Subleaf 0
        let (max_leaf, _, _, _) = cpuid(0, 0);
        let mut smep_supported = false;
        let mut smap_supported = false;

        if max_leaf >= 7 {
            let (_, ebx, _, _) = cpuid(7, 0);
            smep_supported = (ebx & (1 << 7)) != 0;
            smap_supported = (ebx & (1 << 20)) != 0;
        }

        // Enable SMEP (Supervisor Mode Execution Prevention, Bit 20 in CR4) if supported
        if smep_supported {
            cr4 |= 1 << 20;
        }

        write_cr4(cr4);

        println!(
            "[CPU] Modern CPU features activated: CR0.WP, EFER.NXE, CR4.PGE, CR4.OSFXSR (SMEP: {}, SMAP: {}).",
            if smep_supported { "Enabled" } else { "Unsupported" },
            if smap_supported { "Supported" } else { "Unsupported" }
        );
    }
}
