//! # Stack Guard Pages and Stack Smashing Protector (Canary) (Phase 10 - M26)
//!
//! This module implements two complementary mechanisms for detecting and halting
//! **stack-based memory corruption attacks** in the NullOS kernel:
//!
//! ## 1. Stack Canaries (Stack Smashing Protector / SSP)
//!
//! ### What is a Stack Canary?
//! A **stack canary** is a secret value placed on the stack between local variables and
//! the saved frame pointer / return address. The term "canary" comes from the coal-mining
//! practice of bringing a canary into a mine — if the canary dies (the value is modified),
//! you know something has gone wrong before it's too late.
//!
//! ### How Stack Smashing Works
//! Consider a vulnerable C function:
//! ```c
//! void vulnerable(char *user_input) {
//!     char buf[64];
//!     strcpy(buf, user_input);  // No bounds check!
//! }
//! ```
//! Stack layout during execution:
//! ```text
//! High addresses (top of stack frame)
//! +---------------------------+
//! |  Return address (RIP)     |  <-- attacker wants to overwrite this
//! +---------------------------+
//! |  Saved RBP                |
//! +---------------------------+
//! |  buf[63] ... buf[0]       |  <-- buffer overflow starts here
//! +---------------------------+
//! Low addresses
//! ```
//! If the attacker provides more than 64 bytes of input, `strcpy` will write beyond
//! `buf`, overwriting the saved RBP, then the saved return address. When the function
//! returns, the CPU jumps to the attacker-controlled address — arbitrary code execution.
//!
//! ### How Canaries Prevent This
//! With a canary, the stack layout becomes:
//! ```text
//! High addresses
//! +---------------------------+
//! |  Return address (RIP)     |
//! +---------------------------+
//! |  Saved RBP                |
//! +---------------------------+
//! |  CANARY VALUE (64-bit)    |  <-- placed here by prologue code
//! +---------------------------+
//! |  buf[63] ... buf[0]       |  <-- overflow starts here
//! +---------------------------+
//! Low addresses
//! ```
//! Before returning, the function epilogue checks: `if canary != global_canary { panic!() }`.
//! An overflow that reaches the return address **must** corrupt the canary first, which
//! the check catches. The function never returns to the attacker's address.
//!
//! ### Canary Randomization
//! The canary value is seeded from hardware entropy (the x86 `RDTSC` timestamp counter)
//! XOR-mixed with the compile-time magic constant `STACK_CANARY_MAGIC`. This makes it
//! infeasible to guess — an attacker cannot know the value and cannot craft an overflow
//! that "magically" preserves the correct canary while still overwriting the return address.
//!
//! ### Integration with GCC/LLVM SSP
//! When compiled with `-fstack-protector-strong` or `-fstack-protector-all`, the compiler
//! automatically inserts canary check/verify code in every function prologue/epilogue.
//! The compiler calls `__stack_chk_fail()` (defined here) if corruption is detected.
//! Our implementation of `__stack_chk_fail` halts the system with an emergency alert.
//!
//! ## 2. Guard Pages
//!
//! ### What is a Guard Page?
//! A **guard page** is an intentionally unmapped virtual memory page (a 4 KiB region with
//! no physical page mapped and no valid page table entry) placed immediately adjacent to
//! the top (or bottom) of a kernel stack.
//!
//! ### How Guard Pages Work
//! When a stack grows beyond its allocated region and accesses the guard page:
//! 1. The CPU's Memory Management Unit (MMU) finds no valid mapping in the page table.
//! 2. The CPU raises a **#PF (Page Fault, vector 14)** exception.
//! 3. The fault handler's `fault_address` (CR2 register) falls within the guard page range.
//! 4. The handler identifies this as a **kernel stack overflow** and panics.
//!
//! ### Why Both Mechanisms?
//! - Canaries catch **buffer overflows that target the return address** (attacker-controlled).
//! - Guard pages catch **deep recursion / runaway stack growth** (accidental exhaustion).
//! Together they cover the full threat landscape of stack-related memory corruption.
//!
//! ## Hardware Context
//! These mechanisms work at the software layer. For hardware-level stack protection,
//! see `hardening.rs` which configures SMEP (prevents kernel executing user-page shellcode)
//! and the NX bit (marks stack pages as non-executable).
//!
//! ## References
//! - Wagle & Cowan, "StackGuard: Automatic Adaptive Detection and Prevention of Buffer-Overflow Attacks"
//! - GCC documentation: `-fstack-protector`, `__stack_chk_fail`
//! - Linux: `arch/x86/kernel/process.c` (stack setup), `mm/mprotect.c` (guard pages)
//! - Intel SDM Vol. 3A §6.15 (Exception and Interrupt Reference — #PF)

use core::sync::atomic::{AtomicU64, Ordering};

/// The compile-time default stack canary magic value.
///
/// This constant serves as the **initial** (pre-randomization) canary value and is also
/// used as the XOR base when mixing in hardware entropy during `init_canary()`.
///
/// The value `0xDEADC0DE_FEEDFACE` was chosen because:
/// - It is non-zero (a zero canary is trivially guessable).
/// - It contains non-printable bytes, making it harder to include in a string overflow.
/// - The human-readable "words" make it recognizable in memory dumps.
/// - It is 64 bits wide, requiring an attacker to know/guess all 8 bytes exactly.
///
/// **Security Note**: This value alone is NOT secure — `init_canary()` MUST be called
/// at boot to XOR it with RDTSC entropy. The compile-time constant is only used if
/// `init_canary()` is never called (which should not happen in a correct boot sequence).
pub const STACK_CANARY_MAGIC: u64 = 0xDEADC0DE_FEEDFACE;

/// The global kernel stack canary value, shared across all kernel threads and ISRs.
///
/// This is an `AtomicU64` for two reasons:
/// 1. **Concurrency safety**: Multiple CPUs (SMP) or interrupt handlers may read it
///    simultaneously. `AtomicU64` ensures reads are atomic without a mutex.
/// 2. **No lock needed for reads**: `get_stack_canary()` uses `Ordering::Relaxed` because
///    the canary is only written once during boot (`init_canary()`) and then only read.
///    There is no data dependency on other memory operations that would require stronger
///    ordering guarantees.
///
/// The initial value is `STACK_CANARY_MAGIC` — it is replaced with the entropy-mixed
/// value during `init_canary()`.
static GLOBAL_STACK_CANARY: AtomicU64 = AtomicU64::new(STACK_CANARY_MAGIC);

/// Initializes the global stack canary with hardware-derived randomness.
///
/// This function should be called **once** during early kernel boot, before any untrusted
/// code paths are entered and before any stack-intensive operations.
///
/// ## Entropy Source: RDTSC
/// The `RDTSC` (Read Time-Stamp Counter) instruction reads the number of CPU cycles since
/// the last CPU reset. At boot time, this value depends on:
/// - The exact timing of firmware/BIOS initialization (variable across machines and boots).
/// - The speed of the UEFI bootloader execution path (also variable).
/// - CPU frequency and power-on characteristics.
///
/// While RDTSC is not a cryptographically secure random number generator (a sophisticated
/// attacker with physical access or precise boot timing knowledge might estimate it),
/// it provides sufficient entropy for a kernel-level canary where:
/// - The attacker cannot observe the canary value directly.
/// - The attacker cannot replay the boot sequence to get the same value.
///
/// ## Mixing Strategy
/// `canary = STACK_CANARY_MAGIC XOR rdtsc_at_init`
///
/// XOR mixing ensures:
/// - If RDTSC returns 0 (theoretical edge case on some CPUs), the canary is still
///   `STACK_CANARY_MAGIC` (non-zero, non-trivial).
/// - The mixing is bijective — the XOR is reversible but only if you know both values.
///
/// ## Production Enhancement (TODO)
/// In production, this should additionally incorporate:
/// - `RDRAND` / `RDSEED` instructions if available (hardware TRNG).
/// - Early boot memory layout randomization (physical address entropy).
/// - Multiple RDTSC samples with jitter.
///
/// ## Safety
/// - This function uses `unsafe` to call `rdtsc` via inline assembly.
/// - `options(nomem, nostack)` tells the compiler: this asm reads no memory and uses no
///   stack space, allowing the compiler to freely schedule it.
/// - `out("rax") rdtsc` captures the low 32 bits of TSC (the high 32 bits are in RDX,
///   which we ignore here for simplicity).
/// - `SeqCst` ordering on the store ensures the new canary value is visible to all
///   subsequent reads on all CPUs (memory fence on the store side).
pub fn init_canary() {
    // In production, this can incorporate TSC or hardware randomness
    let rdtsc: u64;
    unsafe {
        // RDTSC: Read Time Stamp Counter
        // Result: EDX:EAX = 64-bit TSC value
        //   EAX (rax) = low 32 bits  → captured into `rdtsc`
        //   EDX = high 32 bits       → ignored (clobbered implicitly)
        // options(nomem): this instruction reads no memory (safe to reorder w.r.t. memory ops)
        // options(nostack): this instruction does not modify RSP
        core::arch::asm!("rdtsc", out("rax") rdtsc, options(nomem, nostack));
    }
    // XOR the magic constant with the RDTSC value to produce a boot-unique canary.
    // If rdtsc == 0 (edge case), the result is still STACK_CANARY_MAGIC (non-trivial).
    let mixed = STACK_CANARY_MAGIC ^ rdtsc;
    // SeqCst: Sequentially consistent store — acts as a full memory barrier.
    // All CPUs will observe the new canary value after this point.
    GLOBAL_STACK_CANARY.store(mixed, Ordering::SeqCst);
}

/// Returns the current global stack canary value for comparison.
///
/// Used in `verify_canary()` to compare against the value stored on the stack.
/// Also used by `place_canary()` to know what value to write.
///
/// ## Ordering
/// Uses `Ordering::Relaxed` because:
/// - The canary is only written once at boot by `init_canary()`.
/// - After that, it is read-only for the lifetime of the kernel.
/// - There is no other synchronization that depends on this read ordering.
/// - `Relaxed` avoids unnecessary memory fences, keeping hot-path performance optimal.
///
/// ## Returns
/// The 64-bit canary value that was stored during `init_canary()`.
/// Returns `STACK_CANARY_MAGIC` if `init_canary()` has not been called yet.
///
/// ## Inline
/// Marked `#[inline]` because this is called in hot paths (function epilogues).
/// Inlining eliminates the function call overhead and allows the compiler to
/// optimize the comparison into a single `cmpq` instruction.
#[inline]
pub fn get_stack_canary() -> u64 {
    GLOBAL_STACK_CANARY.load(Ordering::Relaxed)
}

/// Places (writes) the current global stack canary value at the specified memory address.
///
/// This is used to **initialize** a canary slot at the bottom of a newly allocated stack
/// (or at a specific location within a stack frame). After calling this, `verify_canary`
/// should be called with the same pointer before the stack frame is unwound.
///
/// ## Arguments
/// - `canary_ptr`: A raw mutable pointer to a `u64` where the canary will be written.
///   This is typically the address at the base of a kernel thread stack, chosen such that
///   any linear overflow from local variables must corrupt this location before reaching
///   the saved return address.
///
/// ## Null Check
/// If `canary_ptr` is null, this function is a no-op. This is a defensive check — a null
/// pointer here would indicate a programming error (e.g., allocation failure not caught
/// upstream). We prefer a no-op over a panic in the security setup code.
///
/// ## Why `write_volatile`?
/// We use `core::ptr::write_volatile` rather than a plain dereference because:
/// 1. **Prevents compiler optimization removal**: The compiler might notice that the written
///    value is never read by kernel code (only by the security check) and "optimize" the
///    write away as dead code. `volatile` forces the write to actually occur.
/// 2. **Prevents reordering**: The volatile write creates a memory ordering point that
///    prevents the compiler from reordering other memory operations across it.
///
/// ## Safety Contract
/// - Caller must ensure `canary_ptr` is valid, properly aligned (8-byte aligned for u64),
///   and points to writable memory within the kernel's address space.
/// - The pointed-to memory must remain live (not freed/reused) until `verify_canary` is called.
pub fn place_canary(canary_ptr: *mut u64) {
    if !canary_ptr.is_null() {
        unsafe {
            // write_volatile: Prevents the compiler from eliminating this write as "dead code".
            // The canary MUST be written to memory even if no Rust code reads it back,
            // because the security check reads it at a later point.
            core::ptr::write_volatile(canary_ptr, get_stack_canary());
        }
    }
}

/// Verifies that the canary value at `canary_ptr` has not been corrupted.
///
/// Compares the value at `canary_ptr` against the global canary. If they match,
/// the stack frame is intact. If they differ, a stack smashing attack or accidental
/// overflow has occurred.
///
/// ## Arguments
/// - `canary_ptr`: A raw const pointer to the `u64` canary slot that was previously
///   initialized with `place_canary()`. Must be the **exact same address** passed to
///   `place_canary()` for a valid check.
///
/// ## Returns
/// - `true` if the canary is intact (value matches the global canary).
/// - `false` if the pointer is null, OR if the canary has been modified.
///
/// ## Null Check
/// Returns `false` for null pointers as a fail-safe. A null canary pointer should
/// never occur in a correct implementation, so returning `false` (triggering a security
/// alert at the call site) is the safest behavior.
///
/// ## Why `read_volatile`?
/// We use `core::ptr::read_volatile` to prevent the compiler from:
/// 1. Caching the previous read of this location in a register and comparing against that.
/// 2. Eliminating the read as "already known" if the compiler can prove no Rust code
///    wrote to this address (even though an overflow may have done so outside Rust's
///    alias analysis knowledge).
/// The volatile read forces a fresh load from memory at the actual address.
///
/// ## Safety Contract
/// - Caller must ensure `canary_ptr` is valid, properly aligned, and readable.
/// - The memory must have been initialized with `place_canary()` prior to this call.
/// - The pointed-to memory must not have been freed/reused between `place_canary()` and
///   this call (use-after-free would give a false negative).
///
/// ## Timing Side-Channel
/// Note: a constant-time comparison is not used here. For a stack canary, timing attacks
/// are not a concern because the canary check is performed locally in the kernel, not
/// in response to network-observable operations. If this were used for a password check,
/// constant-time comparison would be essential.
pub fn verify_canary(canary_ptr: *const u64) -> bool {
    if canary_ptr.is_null() {
        return false;
    }
    unsafe {
        // read_volatile: Forces a fresh load from the actual memory address.
        // Prevents the compiler from caching the value or proving it "can't have changed"
        // since place_canary() was called (a stack overflow wrote to it outside Rust's view).
        core::ptr::read_volatile(canary_ptr) == get_stack_canary()
    }
}

/// ABI-compatible stack smashing detection callback, called by the compiler's SSP runtime.
///
/// When the compiler's Stack Smashing Protector (`-fstack-protector-strong` or
/// `-fstack-protector-all`) detects that a function's local canary value does not match
/// the global canary at function return, it calls this function.
///
/// The function name `__stack_chk_fail` is part of the GCC/LLVM SSP ABI. By providing
/// our own implementation with `#[no_mangle]`, we override any libc/compiler-rt default
/// and ensure the kernel-specific behavior (halt the system with a security alert) is used.
///
/// ## Why `#[no_mangle]`?
/// Rust normally mangles function names for namespacing. `#[no_mangle]` exports the symbol
/// with its exact C name so the compiler-generated SSP code can find it at link time.
///
/// ## Why `extern "C"`?
/// The compiler generates a call to `__stack_chk_fail` using the C calling convention
/// (System V AMD64 ABI on x86-64 Linux/UEFI). Our function must use the same ABI.
///
/// ## Why `-> !` (Never Return)?
/// A stack smashing detection is a **fatal, unrecoverable** security event:
/// 1. The stack frame is already corrupted — we cannot safely unwind.
/// 2. The return address may have been modified — we cannot return safely.
/// 3. Any further execution may execute attacker-controlled code.
/// The correct response is to halt immediately with maximum prejudice.
///
/// ## Halt Mechanism
/// We use an infinite loop of `cli; hlt`:
/// - `cli` (Clear Interrupt Flag): Disables maskable hardware interrupts. This prevents
///   any interrupt handler from resuming execution after the halt. Without `cli`, an
///   interrupt could un-halt the CPU and resume the corrupted kernel.
/// - `hlt` (Halt): Puts the CPU into a low-power halt state until the next interrupt.
///   With interrupts disabled, this effectively stops the CPU permanently.
/// The loop ensures that even if the CPU somehow exits `hlt` (e.g., via NMI), it
/// immediately returns to `cli; hlt`.
///
/// ## Safety
/// The inline assembly is `unsafe` because:
/// 1. `cli` is a privileged instruction (Ring 0 only). Executing it in user mode (#GP).
/// 2. `hlt` is a privileged instruction. Executing it in user mode (#GP).
/// Both are valid here because this code runs exclusively in Ring 0 kernel context.
#[unsafe(no_mangle)]
pub extern "C" fn __stack_chk_fail() -> ! {
    // Print a prominent emergency banner to the serial console / VGA text buffer.
    // We use `crate::println!` which is interrupt-safe (uses a spinlock internally).
    crate::println!("\n!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
    crate::println!("   FATAL SECURITY ALERT: STACK SMASHING DETECTED! ");
    crate::println!("   Stack canary corruption detected in execution. ");
    crate::println!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
    loop {
        unsafe {
            // cli: Clear Interrupt Flag (IF=0).
            //   - Disables maskable external interrupts (INTR pin).
            //   - Does NOT disable NMIs (Non-Maskable Interrupts).
            //   - Prevents any interrupt handler from resuming after hlt.
            //   - Privileged: CPL must be 0 or IOPL >= CPL.
            //
            // hlt: Halt the CPU until next interrupt arrives.
            //   - CPU enters low-power idle state (C0 → C1 or deeper).
            //   - With IF=0, no maskable interrupt will wake it.
            //   - NMI can still wake it, but the outer `loop` returns to `cli`.
            //   - Privileged: must be in Ring 0.
            core::arch::asm!("cli; hlt");
        }
    }
}
