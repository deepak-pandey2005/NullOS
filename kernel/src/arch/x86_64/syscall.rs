//! Fast System Call (SYSCALL / SYSRET) MSR configuration and Ring 3 entry for x86_64.

use crate::println;

const IA32_EFER_MSR: u32 = 0xC000_0080;
const IA32_STAR_MSR: u32 = 0xC000_0081;
const IA32_LSTAR_MSR: u32 = 0xC000_0082;
const IA32_FMASK_MSR: u32 = 0xC000_0084;

unsafe extern "C" {
    fn syscall_entry();
    pub fn enter_user_mode(entry_point: u64, user_stack_top: u64) -> !;
    pub fn user_test_program_start();
    pub fn user_test_program_end();
    pub fn user_fault_program_start();
    pub fn user_fault_program_end();
    pub static kernel_syscall_stack_top: u8;
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

/// Initializes hardware MSRs for the fast `SYSCALL` and `SYSRET` instructions.
pub fn init() {
    unsafe {
        // 1. Enable SCE (System Call Extension) bit in IA32_EFER (Bit 0)
        let efer = read_msr(IA32_EFER_MSR);
        write_msr(IA32_EFER_MSR, efer | 1);

        // 2. Configure STAR MSR:
        // Bits 47:32 = Kernel CS (0x08)
        // Bits 63:48 = User CS/SS Base:
        //   sysretq loads User CS = (Base + 16) | 3, User SS = (Base + 8) | 3
        //   Base = 0x10 -> User SS = 0x18 | 3 = 0x1B, User CS = 0x20 | 3 = 0x23
        let star_val: u64 = (0x0010u64 << 48) | (0x0008u64 << 32);
        write_msr(IA32_STAR_MSR, star_val);

        // 3. Configure LSTAR MSR with address of our assembly syscall entry point
        let lstar_val = syscall_entry as *const () as usize as u64;
        write_msr(IA32_LSTAR_MSR, lstar_val);

        // 4. Configure FMASK MSR to mask RFLAGS.IF (Bit 9 = 0x200) and RFLAGS.TF (Bit 8 = 0x100)
        write_msr(IA32_FMASK_MSR, 0x300);
    }

    println!("[SYSCALL] Fast SYSCALL / SYSRET ABI initialized (LSTAR set, STAR configured).");
}
