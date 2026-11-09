//! # Kernel Security, Memory Hardening, and Defense-in-Depth Subsystem (Phase 10 - M26)
//!
//! This module is the **central hub** for all kernel-level security mitigations in NullOS.
//! It aggregates and re-exports three specialized security submodules that together form a
//! layered defense-in-depth strategy against memory-corruption attacks, privilege escalation,
//! and unauthorized cross-boundary memory access.
//!
//! ## Why Security Matters in Kernel Code
//!
//! The kernel runs at the highest CPU privilege level (Ring 0 / CPL=0). Any memory corruption
//! bug in the kernel — a stack overflow, a use-after-free, an out-of-bounds write — can
//! immediately compromise the entire system: install rootkits, escalate unprivileged processes
//! to root, or cause unrecoverable data corruption. The mitigations in this module implement
//! the same hardening strategies used by Linux, Windows NT, and seL4 to reduce the attack
//! surface of an operating system kernel.
//!
//! ## Submodules Overview
//!
//! ### `guard` — Stack Canaries & Guard Pages
//! Provides two complementary stack overflow detection mechanisms:
//! - **Stack Canary**: A secret 64-bit random value placed at the base of every stack frame.
//!   Before a function returns, the compiler (or we manually) verify the canary is unmodified.
//!   A buffer overflow that smashes the return address must first overwrite the canary, which
//!   triggers an immediate fatal security alert, stopping the attack before it can redirect
//!   control flow.
//! - **Guard Pages**: Unmapped virtual memory pages placed adjacent to the top and/or bottom
//!   of kernel stacks. Any stack pointer that escapes the valid stack region will access the
//!   guard page, which has no physical page mapped behind it, causing a #PF (Page Fault)
//!   exception at CPL=0, which the fault handler detects as a fatal kernel stack overflow.
//!
//! ### `usercopy` — User-Kernel Boundary Validation
//! Provides validated memory transfer primitives between kernel space and userspace:
//! - **`copy_from_user`**: Copies bytes from a userspace pointer into a kernel buffer.
//!   Before copying, it validates that the source pointer and length are entirely within
//!   the lower canonical half of the virtual address space (< 0x0000_8000_0000_0000).
//!   This prevents a compromised syscall handler from being tricked into reading kernel
//!   memory by passing a kernel-space address as the "user" source.
//! - **`copy_to_user`**: Symmetric: validates destination is in user space before writing.
//! - **`copy_str_from_user`**: Byte-by-byte validated null-terminated string copy with
//!   UTF-8 sanitization, preventing kernel panics from invalid byte sequences.
//! - **SMAP integration**: On CPUs that support SMAP (Supervisor Mode Access Prevention,
//!   CR4 bit 21), the kernel cannot accidentally access user-mapped pages without explicitly
//!   executing `STAC` (Set AC flag, allows user access) around the copy and `CLAC`
//!   (Clear AC flag, re-enables SMAP protection) afterward.
//!
//! ### `hardening` — Hardware Security Flags, W^X, ASLR, SMEP/SMAP
//! Provides automated security auditing and self-tests:
//! - **ASLR** (Address Space Layout Randomization): Randomizes the virtual base addresses
//!   of kernel stacks, heap regions, and driver mappings at boot time. Even if an attacker
//!   knows the kernel binary layout (e.g., via an info leak), they cannot predict where
//!   the stack or heap reside, making return-oriented programming (ROP) attacks significantly
//!   harder.
//! - **SMEP** (Supervisor Mode Execution Prevention, CR4 bit 20): Prevents the CPU from
//!   executing code that resides in user-mapped pages while in Ring 0. Without SMEP, an
//!   attacker who can redirect kernel control flow to a user-space shellcode page can
//!   execute arbitrary code with kernel privileges. SMEP makes this impossible at the
//!   hardware level.
//! - **SMAP** (Supervisor Mode Access Prevention, CR4 bit 21): Prevents the kernel from
//!   accidentally reading or writing user-space memory without an explicit STAC/CLAC pair.
//!   This prevents an entire class of "confused deputy" bugs where a kernel function is
//!   tricked into dereferencing a user-supplied pointer.
//! - **NX / XD bit** (No-Execute, via EFER.NXE): Marks stack and heap pages as
//!   non-executable in the page table entries. Code injected into data pages (classic
//!   shellcode injection) cannot be executed. This is enforced by the hardware MMU on
//!   every memory access.
//! - **CR0.WP** (Write-Protect bit): When set, prevents the kernel itself from writing to
//!   read-only pages, even at Ring 0. This enforces the W^X (Write XOR Execute) invariant:
//!   no page should be simultaneously writable and executable.
//! - **`SecurityReport`**: A structured snapshot of the current state of all hardware and
//!   software security mitigations, queryable at runtime.
//! - **`run_self_tests`**: An automated test suite that exercises every boundary check,
//!   canary mechanism, and hardware flag to verify the security subsystem is functioning.
//!
//! ## Threat Model
//!
//! This subsystem defends against:
//! 1. **Stack buffer overflows**: Canaries + guard pages catch these at runtime.
//! 2. **Heap corruption**: W^X + NX prevent injected heap shellcode from executing.
//! 3. **Kernel-pointer leaks via syscalls**: `usercopy` validation prevents user processes
//!    from tricking the kernel into reading or writing kernel memory.
//! 4. **Control-flow hijacking via code injection**: SMEP + NX prevent execution of
//!    user-supplied code in kernel context.
//! 5. **Accidental user-page dereference**: SMAP + `usercopy` STAC/CLAC pairs enforce
//!    intentional user-memory access only.
//!
//! ## References
//! - Intel SDM Vol. 3A, §4.6 (Access Rights), §26.3 (SMEP), §26.4 (SMAP)
//! - Linux kernel: `arch/x86/mm/fault.c`, `lib/usercopy.c`
//! - "The Art of Exploitation" by Jon Erickson — stack smashing analysis
//! - PaX/grsecurity — original SMEP/SMAP/ASLR research
#![allow(unused_imports, dead_code)]

/// Stack guard pages and stack-smashing canary protection.
///
/// See [`guard`] module documentation for full details on how canaries and
/// guard pages complement each other to detect and halt stack overflow attacks.
pub mod guard;

/// Hardware hardening flags (SMEP, SMAP, NX, CR0.WP, ASLR) and self-tests.
///
/// See [`hardening`] module documentation for the complete security audit system.
pub mod hardening;

/// Safe user-kernel memory transfer primitives with address range validation.
///
/// See [`usercopy`] module documentation for SMAP integration and boundary checks.
pub mod usercopy;

/// Re-export canary functions for direct use from the kernel root.
///
/// - [`get_stack_canary`](guard::get_stack_canary): Returns the current global canary value.
/// - [`init_canary`](guard::init_canary): Seeds the canary from RDTSC hardware entropy.
/// - [`place_canary`](guard::place_canary): Writes the canary to a memory location.
/// - [`verify_canary`](guard::verify_canary): Checks a canary location for tampering.
/// - [`STACK_CANARY_MAGIC`](guard::STACK_CANARY_MAGIC): The compile-time default canary value.
pub use guard::{get_stack_canary, init_canary, place_canary, verify_canary, STACK_CANARY_MAGIC};

/// Re-export the security report and self-test runner for top-level access.
///
/// - [`get_security_report`](hardening::get_security_report): Snapshots all active mitigations.
/// - [`run_self_tests`](hardening::run_self_tests): Executes the full security test suite.
/// - [`SecurityReport`](hardening::SecurityReport): The structured mitigation status type.
pub use hardening::{get_security_report, run_self_tests, SecurityReport};

/// Re-export user-kernel copy primitives and validation types.
///
/// - [`copy_from_user`](usercopy::copy_from_user): Validated copy from userspace into kernel buffer.
/// - [`copy_str_from_user`](usercopy::copy_str_from_user): Validated null-terminated string copy.
/// - [`copy_to_user`](usercopy::copy_to_user): Validated copy from kernel buffer to userspace.
/// - [`validate_user_buffer`](usercopy::validate_user_buffer): Pure range check, no data transfer.
/// - [`SecurityError`](usercopy::SecurityError): Enumeration of boundary-check failure reasons.
/// - [`MAX_USER_PATH_LEN`](usercopy::MAX_USER_PATH_LEN): Maximum path string length (1024 bytes).
/// - [`USER_SPACE_CEILING`](usercopy::USER_SPACE_CEILING): Upper bound of valid user addresses.
pub use usercopy::{
    copy_from_user, copy_str_from_user, copy_to_user, validate_user_buffer, SecurityError,
    MAX_USER_PATH_LEN, USER_SPACE_CEILING,
};
