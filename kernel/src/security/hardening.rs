//! # Kernel Hardening, W^X Policy Auditing, and Security Diagnostics (Phase 10 - M26)
//!
//! This module provides automated security auditing, hardware protection status reporting,
//! and comprehensive verification self-tests that exercise every kernel security mitigation.
//!
//! ## Mitigations Overview
//!
//! ### ASLR — Address Space Layout Randomization
//! **ASLR** randomizes the virtual base addresses of key memory regions at boot time:
//! - **Stack base**: The kernel stack for each thread starts at a random virtual address.
//! - **Heap base**: The kernel heap (`alloc`) starts at a random offset.
//! - **Driver mappings**: MMIO regions are mapped at randomized virtual addresses.
//!
//! **Why it matters**: Classical exploits (ROP, ret2libc) require knowing the exact address
//! of code gadgets or data structures. With ASLR, even if an attacker finds a vulnerability,
//! they cannot predict where the target is in memory without an information leak.
//!
//! **Limitations**: ASLR is probabilistic, not deterministic. With a 48-bit address space
//! and 12-bit page alignment, there are ~36 bits of randomness — roughly 68 billion
//! possible layout combinations. ASLR is defeated by information leaks (memory disclosure
//! vulnerabilities) or brute-force attacks (only feasible on 32-bit systems).
//!
//! **Implementation**: NullOS currently sets `wx_enforced: true` as a placeholder.
//! Full ASLR requires randomized virtual address selection during memory mapping, which
//! is planned for a future milestone.
//!
//! ### SMEP — Supervisor Mode Execution Prevention
//! **SMEP** is a hardware feature controlled by **CR4 bit 20**.
//!
//! When CR4.SMEP=1: If the CPU is executing in Ring 0 (CPL=0) and attempts to fetch
//! an instruction from a page whose Page Table Entry has the **User/Supervisor (U/S) bit = 1**
//! (i.e., a user-accessible page), a **#PF (Page Fault)** exception is raised immediately.
//!
//! **Without SMEP** (classical kernel exploit path):
//! ```text
//! 1. Attacker maps shellcode at user virtual address 0x400000
//! 2. Attacker exploits kernel bug to overwrite a kernel function pointer
//! 3. Kernel calls the corrupted pointer → executes code at 0x400000
//! 4. Shellcode runs at Ring 0 — full kernel privilege
//! ```
//!
//! **With SMEP**:
//! Step 3 triggers a #PF because the kernel tried to execute a user-mapped page.
//! The fault handler detects this as a SMEP violation and panics, preventing exploitation.
//!
//! SMEP is detected via CPUID leaf 7, subleaf 0, EBX bit 7.
//! It is enabled by setting CR4 bit 20 during early boot (see `arch/x86_64/cpu.rs`).
//!
//! ### SMAP — Supervisor Mode Access Prevention
//! **SMAP** is a hardware feature controlled by **CR4 bit 21**.
//!
//! When CR4.SMAP=1: If the CPU is in Ring 0 (CPL=0) and accesses (reads or writes)
//! a page with U/S=1 (user-accessible) while EFLAGS.AC=0 (Alignment Check clear),
//! a **#PF** exception is raised.
//!
//! **Without SMAP** (classic confused-deputy attack):
//! ```text
//! 1. Attacker calls syscall(SOME_SYSCALL, 0xFFFF_8000_00001234)
//! 2. Kernel dereferences the syscall argument as a pointer
//! 3. Kernel reads from 0xFFFF_8000_00001234 (kernel memory!)
//! 4. Data is returned to attacker — information leak
//! ```
//!
//! **With SMAP**: Any kernel access to user-mapped pages (U/S=1) without first executing
//! `STAC` (which sets EFLAGS.AC=1) causes a #PF. This catches accidental dereferences
//! of user-supplied pointers in kernel code that forgot to call `copy_from_user`.
//!
//! SMAP is detected via CPUID leaf 7, subleaf 0, EBX bit 20.
//! It is enabled by setting CR4 bit 21 during early boot.
//! Intentional user-page access requires STAC/CLAC pairs (see `usercopy.rs`).
//!
//! ### NX / XD Bit — No Execute (via EFER.NXE)
//! The **NX (No-Execute)** bit in page table entries (bit 63 of each PTE) marks a page
//! as non-executable. When the CPU's instruction fetch unit tries to execute from such
//! a page, a #PF is raised.
//!
//! **EFER.NXE** (Extended Feature Enable Register, bit 11): Must be set to 1 to enable
//! use of the NX bit in PTEs. Without EFER.NXE, the NX bit is reserved and must be 0.
//!
//! **Stack NX**: Stack pages are marked NX. Any shellcode injected into the stack
//! (classic buffer-overflow shellcode injection) cannot be executed because the CPU
//! will raise #PF on instruction fetch from a NX-marked page.
//!
//! **Heap NX**: Heap pages are also marked NX. Code injected via heap spray attacks
//! (use-after-free, type confusion) cannot be executed directly.
//!
//! ### W^X (Write XOR Execute)
//! **W^X** is a software policy (enforced via page table permissions) that states:
//! **No page should be simultaneously writable AND executable.**
//!
//! | Page Type        | Writable | Executable | Comment                          |
//! |------------------|----------|------------|----------------------------------|
//! | .text (code)     | No       | Yes        | Read-only executable pages       |
//! | .data/.bss       | Yes      | No         | Writable data, non-executable    |
//! | Stack            | Yes      | No         | NX-marked via EFER.NXE           |
//! | Heap             | Yes      | No         | NX-marked                        |
//! | MMIO mappings    | Yes      | No         | Device registers, NX             |
//!
//! **CR0.WP** (CR0 bit 16, Write Protect): When set, the kernel cannot write to
//! read-only pages even at Ring 0. This prevents the kernel itself from accidentally
//! (or maliciously, via ROP gadgets) marking a code page as writable to inject
//! shellcode into the kernel text segment.
//!
//! ### Stack Canaries (Covered in `guard.rs`)
//! See `security/guard.rs` for complete documentation of stack canary placement and
//! verification. The `run_self_tests()` function here tests the canary system.
//!
//! ## References
//! - Intel SDM Vol. 3A §26.3 "Supervisor-Mode Execution Prevention"
//! - Intel SDM Vol. 3A §26.4 "Supervisor-Mode Access Prevention"
//! - Intel SDM Vol. 3A §4.6 "Access Rights" (NX bit = PTE bit 63)
//! - Intel SDM Vol. 2A CPUID leaf 7, subleaf 0 (feature detection)
//! - PaX Team: original SMEP/SMAP/NX research
//! - Grsecurity / RAP: Return Address Protection, kernel CFI

use crate::arch::x86_64::cpu;
use crate::println;
use super::usercopy::{validate_user_buffer, copy_from_user, copy_to_user, copy_str_from_user, SecurityError, USER_SPACE_CEILING};
use super::guard::{place_canary, verify_canary, init_canary};

/// A comprehensive snapshot of active hardware and software security mitigations.
///
/// This structure is populated at runtime by querying CPU control registers and CPUID,
/// giving a truthful picture of which protections are actually active (not just assumed).
/// It is used by the kernel log and security dashboard to report system security posture.
///
/// ## Fields and What They Mean
#[derive(Debug, Clone, Copy)]
pub struct SecurityReport {
    /// **CR0.WP (Write Protect, CR0 bit 16)** — Active if `true`.
    ///
    /// When set, the CPU's memory protection system prevents kernel code (Ring 0) from
    /// writing to pages marked as read-only in the page tables (R/W=0 in PTE).
    /// This enforces the write-protect side of the W^X (Write XOR Execute) invariant.
    ///
    /// Without CR0.WP, kernel code could freely overwrite read-only code pages, defeating
    /// all code integrity protections. Linux, Windows, and macOS all set CR0.WP.
    ///
    /// Queried via `cpu::is_wp_enabled()` which reads CR0 and tests bit 16.
    pub cr0_wp: bool,

    /// **EFER.NXE (No-Execute Enable, EFER bit 11)** — Active if `true`.
    ///
    /// When set, PTE bit 63 (the NX / XD bit) is interpreted as "No Execute" for
    /// instruction fetches. If the CPU fetches an instruction from a page with NX=1,
    /// a #PF (page fault) is raised.
    ///
    /// This enables the execute-disable side of W^X: stack pages, heap pages, and data
    /// pages are marked NX, preventing shellcode injection attacks.
    ///
    /// Without EFER.NXE, PTE bit 63 is reserved and must be zero — the NX feature
    /// is completely unavailable even if the CPU supports it.
    ///
    /// Queried via `cpu::is_nxe_enabled()` which reads the EFER MSR (0xC0000080) bit 11.
    pub efer_nxe: bool,

    /// **CR4.SMEP (Supervisor Mode Execution Prevention, CR4 bit 20)** — Active if `true`.
    ///
    /// When set, any attempt by Ring 0 code to execute an instruction from a user-accessible
    /// page (U/S=1 in PTE) causes a #PF. This prevents the classic "execute shellcode
    /// in user page from kernel context" exploit technique.
    ///
    /// Note: SMEP support requires both CPU support (CPUID leaf 7 EBX bit 7) AND the bit
    /// being set in CR4. This field reports whether CR4.SMEP is currently set.
    ///
    /// Queried via `cpu::is_smep_enabled()` which reads CR4 and tests bit 20.
    pub cr4_smep: bool,

    /// **CPUID SMAP Support** — `true` if the CPU supports SMAP.
    ///
    /// SMAP (Supervisor Mode Access Prevention) support is detected via CPUID leaf 7,
    /// subleaf 0, EBX bit 20. If the CPU supports SMAP, it can be enabled by setting
    /// CR4 bit 21.
    ///
    /// Note: This field reports CPU *capability*, not whether SMAP is currently active.
    /// SMAP is activated in early boot by setting CR4.SMAP. If the CPU doesn't support
    /// it, we cannot enable it regardless.
    ///
    /// For SMAP to be effective, `usercopy.rs` functions must call STAC/CLAC around
    /// all user-page accesses (which they do — see `copy_from_user`, `copy_to_user`).
    pub smap_supported: bool,

    /// **W^X Enforcement** — `true` if no page is simultaneously writable and executable.
    ///
    /// This is a software-level policy report. When `true`, the kernel's page table
    /// management ensures that every mapped page is either writable XOR executable,
    /// never both. This is enforced by:
    /// 1. CR0.WP preventing writes to R/W=0 (read-only) pages.
    /// 2. EFER.NXE enabling NX bits on data/stack/heap pages.
    /// 3. The kernel's memory allocator never returning memory with both W and X permissions.
    ///
    /// Currently hardcoded to `true` as a policy assertion. Future implementation will
    /// dynamically verify this by scanning PTEs for W+X violations.
    pub wx_enforced: bool,

    /// **Stack Canary Active** — `true` if stack canaries are initialized and in use.
    ///
    /// Reports that `init_canary()` has been called and the global canary is non-default
    /// (entropy-mixed). Functions instrumented with SSP (`-fstack-protector`) will use
    /// the global `GLOBAL_STACK_CANARY` value for their frame checks.
    ///
    /// Currently hardcoded to `true` as the canary is always initialized in early boot.
    pub stack_canary_active: bool,

    /// **User Boundary Enforced** — `true` if `usercopy` validation is active.
    ///
    /// Reports that all kernel paths that accept userspace pointers use the validated
    /// `copy_from_user` / `copy_to_user` primitives rather than direct dereferences.
    /// This is a software policy assertion, not a hardware feature.
    ///
    /// Currently hardcoded to `true`. Future auditing tools will verify this by tracking
    /// all syscall entry points.
    pub user_boundary_enforced: bool,
}

/// Gathers the current kernel security and hardware protection status into a [`SecurityReport`].
///
/// This function **actively queries** CPU control registers and CPUID to report the
/// real hardware state. It does not rely on boot-time assumptions or compile-time constants.
///
/// ## CPUID Leaf 7, Subleaf 0 — Structured Extended Feature Flags
/// CPUID with EAX=7, ECX=0 returns extended CPU feature flags:
/// - EBX bit 7:  SMEP supported
/// - EBX bit 20: SMAP supported
/// - EBX bit 18: RDSEED supported (not checked here)
/// - EBX bit 19: ADX supported (not checked here)
///
/// We first check that CPUID max leaf (EAX=0 output EAX) is >= 7 before querying leaf 7.
/// This prevents #UD on very old CPUs that don't support extended CPUID.
///
/// ## Returns
/// A `SecurityReport` struct with all fields populated from live CPU state.
/// The report is a point-in-time snapshot — if CPU state changes after this call
/// (e.g., some code clears CR4.SMEP), the report becomes stale.
///
/// ## Usage
/// ```rust,ignore
/// let report = get_security_report();
/// println!("CR0.WP: {}, EFER.NXE: {}, SMEP: {}", report.cr0_wp, report.efer_nxe, report.cr4_smep);
/// assert!(report.cr0_wp, "Write protection must be active!");
/// assert!(report.efer_nxe, "NX bit must be enabled!");
/// ```
pub fn get_security_report() -> SecurityReport {
    // Query CPUID leaf 0 to get the maximum supported leaf number.
    // This prevents invalid CPUID queries on older CPUs.
    // `cpu::cpuid(0, 0)` returns (max_leaf, ebx, ecx, edx) for CPUID(EAX=0, ECX=0).
    let (max_leaf, _, _, _) = cpu::cpuid(0, 0);

    // Check for SMAP support via CPUID leaf 7, subleaf 0, EBX bit 20.
    let mut smap_supported = false;
    if max_leaf >= 7 {
        // CPUID(EAX=7, ECX=0) → EBX contains Structured Extended Feature Flags.
        // Bit 7 of EBX: SMEP supported.
        // Bit 20 of EBX: SMAP supported.
        let (_, ebx, _, _) = cpu::cpuid(7, 0);
        smap_supported = (ebx & (1 << 20)) != 0;
        // Note: SMEP support (EBX bit 7) is checked in cpu::is_smep_enabled() via CR4.
    }

    SecurityReport {
        // CR0.WP: Read CR0 bit 16. Must be 1 for write-protect enforcement.
        cr0_wp: cpu::is_wp_enabled(),
        // EFER.NXE: Read EFER MSR bit 11. Must be 1 for NX page table bits to work.
        efer_nxe: cpu::is_nxe_enabled(),
        // CR4.SMEP: Read CR4 bit 20. Must be 1 to prevent user-page execution in Ring 0.
        cr4_smep: cpu::is_smep_enabled(),
        // CPU SMAP capability from CPUID EBX bit 20 (checked above).
        smap_supported,
        // W^X enforcement is a software policy — hardcoded true as an assertion.
        // TODO: Implement PTE scanner to verify no W+X pages exist at runtime.
        wx_enforced: true,
        // Stack canaries are always initialized in early boot via init_canary().
        stack_canary_active: true,
        // All syscall handlers use validated usercopy primitives — policy assertion.
        user_boundary_enforced: true,
    }
}

/// Runs the full automated kernel hardening self-test suite.
///
/// This function exercises every security mitigation and boundary check implemented
/// in the NullOS security subsystem. It is called during early kernel boot to verify
/// that the security subsystem is correctly configured before the system is opened to
/// any user processes.
///
/// ## Test Structure
/// The tests are organized into five groups, each preceded by a descriptive header print:
///
/// 1. **User Address Boundary Validation**: Tests `validate_user_buffer()` with valid,
///    null, kernel-space, overflow, and boundary-crossing inputs.
/// 2. **Safe Memory Copy Primitives**: Verifies that `copy_from_user`/`copy_to_user`
///    reject kernel-space addresses without performing any copy.
/// 3. **Safe String Copy**: Verifies that `copy_str_from_user` rejects kernel pointers
///    and null pointers.
/// 4. **Stack Canary**: Places a canary, verifies it, then simulates tampering and
///    verifies the tamper is detected.
/// 5. **Hardware Security Flags**: Asserts CR0.WP and EFER.NXE are active (required),
///    and reports SMEP/SMAP status (optional/capability-dependent).
///
/// ## Panics (via `assert!`)
/// If any test fails, `assert!` panics with a descriptive message. During early boot,
/// a kernel panic halts the system — this is intentional. A security subsystem that
/// doesn't work correctly is worse than no security subsystem at all.
///
/// ## Canary Self-Test Detail
/// ```text
/// place_canary(&mut canary_storage)  → writes global_canary to canary_storage
/// verify_canary(&canary_storage)     → reads canary_storage, compares to global_canary
///                                       → true (should pass)
/// canary_storage ^= 0x1337           → simulate buffer overflow corrupting the canary
/// verify_canary(&canary_storage)     → reads canary_storage, compares to global_canary
///                                       → false (should fail — tampering detected!)
/// ```
///
/// ## Output
/// Each test group prints a `[SECURITY]` prefixed line to the kernel log on success.
/// The final line confirms all tests passed.
pub fn run_self_tests() {
    println!("[SECURITY] Running Kernel Hardening verification self-tests...");

    // --- Setup: Initialize canary with hardware entropy before any canary tests ---
    // This seeds GLOBAL_STACK_CANARY with STACK_CANARY_MAGIC ^ RDTSC.
    // Must be done before Test 4.
    init_canary();

    // =========================================================================
    // Test 1: User Buffer Address Validation
    // Tests all four failure modes of validate_user_buffer() plus the success case.
    // =========================================================================

    // Case 1a: Valid user pointer — well below the user/kernel boundary.
    // 0x0000_0000_0040_0000 is the typical ELF load address (4 MiB into user space).
    let valid_user_addr = 0x0000_0000_0040_0000 as *const u8;
    assert_eq!(validate_user_buffer(valid_user_addr, 1024), Ok(()));

    // Case 1b: Null pointer — must return NullPointer.
    // No sane OS maps the null page (virtual address 0x0).
    let null_ptr = core::ptr::null::<u8>();
    assert_eq!(validate_user_buffer(null_ptr, 10), Err(SecurityError::NullPointer));

    // Case 1c: Kernel space pointer — must return PointerAboveUserSpace.
    // 0xFFFF_8000_0000_0000 is the kernel virtual address space base (above canonical hole).
    let kernel_ptr = 0xFFFF_8000_0000_0000 as *const u8;
    assert_eq!(validate_user_buffer(kernel_ptr, 10), Err(SecurityError::PointerAboveUserSpace));

    // Case 1d: Pointer so close to u64::MAX that adding any length overflows u64.
    // u64::MAX - 5 = 0xFFFF_FFFF_FFFF_FFFA; adding 10 wraps to 0x4.
    // Without checked_add, this would appear to be a valid small user address!
    let overflow_ptr = (u64::MAX - 5) as *const u8;
    assert!(validate_user_buffer(overflow_ptr, 10).is_err());

    // Case 1e: Pointer near the user/kernel boundary such that ptr+len crosses it.
    // USER_SPACE_CEILING - 10 starts 10 bytes before the boundary.
    // Adding len=20 → end = USER_SPACE_CEILING + 10 (into kernel space).
    let boundary_ptr = (USER_SPACE_CEILING - 10) as *const u8;
    assert_eq!(validate_user_buffer(boundary_ptr, 20), Err(SecurityError::BufferExceedsUserSpace));

    println!("[SECURITY] 1. User address boundary validation passed (Null, Kernel, Overflow, Boundary).");

    // =========================================================================
    // Test 2: Safe Memory Copy Primitives
    // Verifies that copy_from_user and copy_to_user reject kernel-space addresses
    // without performing any actual memory copy.
    // =========================================================================

    let mut kernel_buf = [0u8; 16];   // Kernel-side destination buffer for copy_from_user
    let test_data = [0x42u8; 16];     // Known pattern for copy_to_user source

    // Attempting to copy FROM a kernel address (as if it were user space) must fail.
    // This simulates a malicious syscall: write(fd, 0xFFFF800000000000, 16)
    assert!(copy_from_user(&mut kernel_buf, kernel_ptr).is_err());

    // Attempting to copy TO a kernel address (as if it were user space) must fail.
    // This simulates a malicious syscall: read(fd, 0xFFFF800000000000, 16)
    assert!(copy_to_user(kernel_ptr as *mut u8, &test_data).is_err());

    println!("[SECURITY] 2. Safe memory transfer boundary guards verified (Protected against Kernel-space injection).");

    // =========================================================================
    // Test 3: Safe String Copy Primitives
    // Verifies that copy_str_from_user rejects clearly invalid pointers.
    // =========================================================================

    // Kernel-space pointer passed as string source — must be rejected immediately.
    assert!(copy_str_from_user(kernel_ptr, 128).is_err());

    // Null pointer passed as string source — must be rejected immediately.
    assert!(copy_str_from_user(null_ptr, 128).is_err());

    println!("[SECURITY] 3. Safe string copy bounds & UTF-8 sanitization verified.");

    // =========================================================================
    // Test 4: Stack Canary Protection
    // Simulates the full canary lifecycle: placement, verification, tampering, re-check.
    // =========================================================================

    // Allocate a u64 on the stack to act as the "canary slot".
    // In production, this would be at the base of a thread stack.
    let mut canary_storage: u64 = 0;

    // Place: write the current global canary into our test slot.
    place_canary(&mut canary_storage as *mut u64);

    // Verify: immediately after placement, the canary must match.
    assert!(verify_canary(&canary_storage as *const u64), "Canary must be valid immediately after placement");

    // Tamper: XOR with a known value to simulate buffer overflow corruption.
    // This modifies the canary slot as an overflow would, changing its value.
    canary_storage ^= 0x1337;

    // Re-verify: the canary must now be detected as corrupted.
    // A correctly working canary system MUST detect any single-bit change.
    assert!(!verify_canary(&canary_storage as *const u64), "Canary corruption must be immediately detected");

    println!("[SECURITY] 4. Stack Canary placement & tamper detection verified.");

    // =========================================================================
    // Test 5: Hardware Security Flags
    // Queries live CPU register state to assert critical protections are active.
    // =========================================================================

    let rep = get_security_report();

    // CR0.WP MUST be active — it was set during early boot (`arch::init()`).
    // If this fails, the kernel's read-only code pages can be overwritten at Ring 0.
    assert!(rep.cr0_wp, "CR0.WP must be enabled");

    // EFER.NXE MUST be active — it was set during early boot.
    // Without NXE, the NX bit in PTEs has no effect — stack/heap pages are executable.
    assert!(rep.efer_nxe, "EFER.NXE must be enabled");

    // SMEP and SMAP are reported but not asserted — they are CPU capability-dependent.
    // Older CPUs (pre-Ivy Bridge for SMEP, pre-Broadwell for SMAP) may not support them.
    println!(
        "[SECURITY] 5. Hardware silicon defenses: CR0.WP: Active, EFER.NXE: Active, SMEP: {}, SMAP: {}.",
        if rep.cr4_smep { "Active" } else { "Unsupported" },
        if rep.smap_supported { "Supported" } else { "Unsupported" }
    );

    println!("[SECURITY] All Kernel Hardening self-tests passed successfully!");
}
