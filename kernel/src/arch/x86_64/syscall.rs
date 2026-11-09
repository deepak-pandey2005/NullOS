//! Fast System Call Hardware ABI (`SYSCALL` / `SYSRET`) for x86_64.
//!
//! # Architecture & Silicon Overview
//! Traditional 32-bit x86 operating systems executed system calls via software interrupt gates
//! (e.g. `int 0x80`), which suffered heavy pipeline serialization penalties: pushing full 5-word
//! interrupt frames to memory, performing IDT gate lookups, and checking segment descriptor limits.
//!
//! AMD64 introduced the `SYSCALL` and `SYSRET` instructions to achieve near-zero-overhead Ring 3 $\rightarrow$ Ring 0
//! privilege transitions in hardware:
//! 1. **Instruction Execution**: The user-mode application invokes `syscall`.
//! 2. **Hardware State Saving**: The CPU copies `%rip` into `%rcx` and `RFLAGS` into `%r11`.
//! 3. **Segment & Target Resolution**: The CPU loads `%cs` and `%ss` from `IA32_STAR_MSR`,
//!    masks `RFLAGS` with `IA32_FMASK_MSR`, and loads `%rip` from `IA32_LSTAR_MSR` (`syscall_entry`).
//! 4. **Stack Pivoting**: In `syscall.S`, the kernel uses `swapgs` to obtain the CPU's dedicated
//!    kernel stack pointer, stores the user stack pointer, and dispatches the call into Rust.
//! 5. **Fast Return**: The kernel restores user registers and invokes `sysretq`, which atomics
//!    reloads user segments from `IA32_STAR_MSR`, restores `%rip` from `%rcx`, and restores `RFLAGS` from `%r11`.

use crate::println;

/// IA32_EFER MSR: Extended Feature Enable Register (`0xC000_0080`).
pub const IA32_EFER_MSR: u32 = 0xC000_0080;

/// IA32_STAR MSR: Target segment selectors for `SYSCALL` and `SYSRET` (`0xC000_0081`).
pub const IA32_STAR_MSR: u32 = 0xC000_0081;

/// IA32_LSTAR MSR: Long Mode System Target Address Register (`0xC000_0082`).
pub const IA32_LSTAR_MSR: u32 = 0xC000_0082;

/// IA32_FMASK MSR: RFLAGS Mask Register (`0xC000_0084`).
pub const IA32_FMASK_MSR: u32 = 0xC000_0084;

/// IA32_FS_BASE MSR: Thread-Local Storage (TLS) FS Base Address (`0xC000_0100`).
pub const IA32_FS_BASE_MSR: u32 = 0xC000_0100;

// Low-level assembly function stubs implemented in `syscall.S`
unsafe extern "C" {
    /// Assembly entry point pointed to by `IA32_LSTAR_MSR`.
    fn syscall_entry();

    /// Performs the initial Ring 0 -> Ring 3 privilege transition via `iretq`.
    pub fn enter_user_mode(entry_point: u64, user_stack_top: u64) -> !;

    /// Transitions a new user thread into Ring 3, placing initial argument in `%rdi`.
    pub fn enter_user_mode_thread(entry_point: u64, user_stack_top: u64, arg: u64) -> !;

    /// Test payload markers used by kernel user-mode self tests.
    pub fn user_test_program_start();
    pub fn user_test_program_end();
    pub fn user_fault_program_start();
    pub fn user_fault_program_end();

    /// Top of the static fallback kernel syscall stack.
    pub static kernel_syscall_stack_top: u8;

    /// Dynamic per-thread kernel stack pointer loaded upon `syscall` entry.
    pub static mut current_kernel_rsp: u64;
}

/// Sets the active thread's dedicated kernel stack pointer loaded on `syscall` entry.
#[inline]
pub fn set_kernel_rsp(rsp: u64) {
    unsafe {
        current_kernel_rsp = rsp;
    }
}

/// Reads a 64-bit Model-Specific Register (MSR) via `rdmsr`.
#[inline]
pub unsafe fn read_msr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        core::arch::asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high, options(nomem, nostack));
    }
    ((high as u64) << 32) | (low as u64)
}

/// Writes a 64-bit value to a Model-Specific Register (MSR) via `wrmsr`.
#[inline]
pub unsafe fn write_msr(msr: u32, val: u64) {
    let low = val as u32;
    let high = (val >> 32) as u32;
    unsafe {
        core::arch::asm!("wrmsr", in("ecx") msr, in("eax") low, in("edx") high, options(nomem, nostack));
    }
}

/// Configures CPU hardware Model-Specific Registers for fast `SYSCALL` and `SYSRET`.
///
/// # Hardware Configuration Steps
/// 1. **Enable SCE (Bit 0) in `IA32_EFER`**: Activates `SYSCALL` and `SYSRET` instructions.
/// 2. **Configure `IA32_STAR` Segment Bases**:
///    - Bits `47:32` = Kernel Code Selector (`0x08`). (Kernel SS is implicitly `0x10`).
///    - Bits `63:48` = User Segment Base (`0x10 | 3 = 0x13`).
///      On 64-bit `sysretq`, silicon computes User SS = `Base + 8 = 0x1B` and User CS = `Base + 16 = 0x23`.
/// 3. **Configure `IA32_LSTAR`**: Sets target address to `syscall_entry` in `syscall.S`.
/// 4. **Configure `IA32_FMASK`**: Masks `RFLAGS.IF` (bit 9 = `0x200`) and `RFLAGS.TF` (bit 8 = `0x100`)
///    to guarantee interrupts are disabled until the kernel pivots `%rsp` to a safe stack.
pub fn init() {
    unsafe {
        // Step 1: Enable SCE (System Call Extension) bit in IA32_EFER (Bit 0)
        let efer = read_msr(IA32_EFER_MSR);
        write_msr(IA32_EFER_MSR, efer | 1);

        // Step 2: Configure STAR MSR selectors for kernel entry and user return
        let star_val: u64 = ((0x0010u64 | 3) << 48) | (0x0008u64 << 32);
        write_msr(IA32_STAR_MSR, star_val);

        // Step 3: Configure LSTAR MSR with address of assembly syscall entry point
        let lstar_val = syscall_entry as *const () as usize as u64;
        write_msr(IA32_LSTAR_MSR, lstar_val);

        // Step 4: Configure FMASK MSR to mask RFLAGS.IF (Bit 9 = 0x200) and RFLAGS.TF (Bit 8 = 0x100)
        write_msr(IA32_FMASK_MSR, 0x300);
    }

    println!("[SYSCALL] Fast SYSCALL / SYSRET ABI initialized (LSTAR set, STAR configured).");
}
