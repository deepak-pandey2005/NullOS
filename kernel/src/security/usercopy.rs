//! # Safe Userspace Memory Transfer and Boundary Sanitization (Phase 10 - M26)
//!
//! This module is the **kernel-userspace memory barrier** — it provides the only
//! sanctioned path through which the kernel may read from or write to userspace memory.
//!
//! ## Why This Module Exists: The Confused Deputy Problem
//!
//! The kernel runs at CPU privilege level Ring 0, with full access to all physical memory.
//! Userspace processes run at Ring 3, with access only to their own virtual address space.
//! When a userspace process makes a system call (syscall), it passes pointers to the kernel:
//!
//! ```text
//! User Process                    Kernel (Ring 0)
//! ───────────────                 ─────────────────────────────────────────
//! write(fd, buf_ptr, len)  ──→   sys_write(fd, buf_ptr: *const u8, len: usize)
//!                                  │
//!                                  │  WRONG (vulnerable):
//!                                  │  memcpy(kernel_dst, buf_ptr, len);
//!                                  │  // buf_ptr could be 0xFFFF8000_00000000
//!                                  │  // (kernel text/data!) — info leak!
//!                                  │
//!                                  │  CORRECT (this module):
//!                                  └→ copy_from_user(kernel_dst, buf_ptr, len)
//!                                       → validate_user_buffer(buf_ptr, len)?
//!                                       → stac()
//!                                       → memcpy(kernel_dst, buf_ptr, len)
//!                                       → clac()
//! ```
//!
//! Without validation, a malicious process can pass kernel-space addresses as syscall
//! arguments. If the kernel blindly dereferences them:
//! - **Info leak**: Reading from a kernel address leaks kernel secrets to userspace.
//! - **Kernel write-what-where**: Writing to a kernel address corrupts kernel data structures.
//! - **Privilege escalation**: Overwriting a kernel function pointer with a user-controlled
//!   value redirects Ring 0 execution to attacker-controlled code.
//!
//! ## x86-64 Virtual Address Space Layout
//!
//! On x86-64 with 48-bit virtual addresses (most common), the address space is split:
//! ```text
//! 0x0000_0000_0000_0000  ──────────────────────────────
//!         │                       User Virtual Space
//!         │                   (each process gets its own)
//! 0x0000_7FFF_FFFF_FFFF  ──────────────────────────────
//!                                 (canonical hole: non-canonical addresses,
//!                                  bits [63:48] must all be the same as bit 47)
//! 0xFFFF_8000_0000_0000  ──────────────────────────────
//!         │                       Kernel Virtual Space
//!         │                   (same mapping in every process, Ring 0 only)
//! 0xFFFF_FFFF_FFFF_FFFF  ──────────────────────────────
//! ```
//!
//! `USER_SPACE_CEILING = 0x0000_8000_0000_0000` is the first address that is NOT valid
//! userspace. Any pointer at or above this value is either canonical kernel space or
//! a non-canonical address (which the CPU will reject with #GP on dereference).
//!
//! ## SMAP: Supervisor Mode Access Prevention
//!
//! Modern Intel CPUs (Broadwell/2015+) and AMD CPUs support **SMAP**, controlled by
//! **CR4 bit 21**. When CR4.SMAP=1:
//! - If the kernel (CPL=0) attempts to load/store to a user-accessible page (U/S=1 in PTE)
//!   **without** having the EFLAGS.AC (Alignment Check) flag set, a #PF fault is raised.
//! - This prevents the kernel from accidentally reading/writing userspace memory due to a
//!   bug (e.g., following a stale pointer that points into user address space).
//!
//! To intentionally access userspace memory, the kernel must:
//! 1. Execute `STAC` (Set AC flag): sets EFLAGS.AC=1, temporarily bypassing SMAP.
//! 2. Perform the memory access to user pages.
//! 3. Execute `CLAC` (Clear AC flag): clears EFLAGS.AC=0, re-enabling SMAP protection.
//!
//! **This module wraps every user-space memory access in a STAC/CLAC pair** via
//! `cpu::stac()` and `cpu::clac()`, ensuring SMAP protection is active at all other times.
//!
//! ### SMAP and SMEP Together
//! - **SMEP** (CR4 bit 20): Prevents *executing* user-mapped code in Ring 0.
//! - **SMAP** (CR4 bit 21): Prevents *accessing* user-mapped data in Ring 0 accidentally.
//! Together they close the two most common kernel exploitation pathways:
//! "execute shellcode in user page" (SMEP) and "overwrite kernel data via user pointer" (SMAP).
//!
//! ## Address Validation: Why Not Just Check `ptr >= USER_SPACE_CEILING`?
//!
//! We also check for integer overflow in `addr + len`. Consider:
//! - `addr = 0x0000_7FFF_FFFF_FF00` (valid, near ceiling)
//! - `len = 0x2000` (8 KiB)
//! - `addr + len = 0x0000_8000_0000_0F00` (crosses into kernel space!)
//! - In 64-bit, this addition doesn't overflow `u64`, but it crosses the ceiling.
//!
//! Additionally:
//! - `addr = u64::MAX - 5 = 0xFFFF_FFFF_FFFF_FFFA`
//! - `len = 10`
//! - `addr + len` overflows `u64` → `checked_add` returns `None` → `AddressOverflow` error.
//!
//! All four failure modes are tested in `hardening::run_self_tests()`.
//!
//! ## References
//! - Intel SDM Vol. 3A §4.6 "Access Rights" (page table U/S bit)
//! - Intel SDM Vol. 3A §26.4 "Supervisor-Mode Access Prevention (SMAP)"
//! - Linux kernel: `arch/x86/include/asm/uaccess.h`, `lib/usercopy.c`
//! - CVE-2016-5195 "Dirty COW" — an example of kernel-userspace boundary violation
//! - Linus Torvalds: "Any user pointer that enters the kernel MUST be validated"

use alloc::string::String;
use alloc::vec::Vec;
use crate::arch::x86_64::cpu;

/// The canonical ceiling (exclusive upper bound) of the 48-bit user virtual address space.
///
/// User-mode addresses are in the **lower canonical half** of the 64-bit virtual address space:
/// `[0x0000_0000_0000_0000, 0x0000_7FFF_FFFF_FFFF]`
///
/// This constant is the first address that is **NOT** a valid user address:
/// `0x0000_8000_0000_0000 = 2^47 = 128 TiB`
///
/// Addresses from `USER_SPACE_CEILING` onwards are either:
/// - **Non-canonical** (bits [63:48] not all same as bit 47): causes #GP on access.
/// - **Kernel virtual space** (0xFFFF_8000_0000_0000 and above): privileged Ring 0 only.
///
/// ## Why 48 bits?
/// Current x86-64 CPUs implement 48-bit virtual addresses (the top 16 bits are sign-extended
/// from bit 47). This gives 256 TiB total address space: 128 TiB user, 128 TiB kernel.
/// Future CPUs with 5-level paging extend this to 57 bits (128 PiB user, 128 PiB kernel),
/// but this module targets the common 48-bit case.
pub const USER_SPACE_CEILING: u64 = 0x0000_8000_0000_0000;

/// The standard maximum byte length for null-terminated path strings copied from userspace.
///
/// This constant bounds the maximum number of bytes that `copy_str_from_user()` will
/// scan when looking for a null terminator. It prevents:
/// 1. **Infinite loops**: A user-provided string with no null terminator would cause
///    unbounded scanning. `MAX_USER_PATH_LEN` limits the scan to 1024 bytes.
/// 2. **Excessive memory allocation**: The resulting `Vec<u8>` is bounded at 1024 bytes.
///
/// Value: `1024` bytes, matching the `PATH_MAX`-equivalent common in many kernels.
/// Note: POSIX `PATH_MAX` is 4096 on Linux; we use a smaller value to be conservative.
pub const MAX_USER_PATH_LEN: usize = 1024;

/// Enumeration of all possible security boundary validation failures.
///
/// Returned by `validate_user_buffer()`, `copy_from_user()`, `copy_to_user()`,
/// and `copy_str_from_user()` when a validation check fails. Callers (syscall handlers)
/// should map these errors to appropriate userspace error codes (e.g., `EFAULT` in POSIX).
///
/// ## Design Note
/// Using a typed enum rather than a raw integer error code gives:
/// - Descriptive error messages in debug builds.
/// - Exhaustive match checking — the compiler warns if a new variant is added but not handled.
/// - Zero-cost: `Copy + Clone` means no heap allocation is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityError {
    /// The provided pointer is a null pointer (`0x0`).
    ///
    /// A null pointer is never a valid userspace buffer address. This check prevents
    /// the kernel from dereferencing NULL, which on most systems is an unmapped page
    /// (causes #PF). This is the most common defensive null-pointer check.
    NullPointer,

    /// The pointer's base address is at or above `USER_SPACE_CEILING`.
    ///
    /// The base address itself points into kernel virtual address space (or into the
    /// non-canonical hole). This is the most direct form of "kernel pointer injection":
    /// the user has explicitly passed a kernel-space address hoping the kernel
    /// will dereference it and leak kernel memory.
    PointerAboveUserSpace,

    /// Computing `ptr + len` would overflow a 64-bit unsigned integer.
    ///
    /// This occurs when both `ptr` and `len` are very large (e.g., both near `u64::MAX`).
    /// Without this check, the overflow would wrap around and the resulting "end address"
    /// would appear to be in user space even though the range spans the entire address space.
    AddressOverflow,

    /// The buffer range `[ptr, ptr + len)` starts in user space but ends in kernel space.
    ///
    /// Example: `ptr = 0x0000_7FFF_FFFF_F000`, `len = 0x2000`.
    /// The range straddles `USER_SPACE_CEILING`, meaning the last bytes of the buffer
    /// lie in kernel virtual address space. This is a cross-boundary injection attempt.
    BufferExceedsUserSpace,

    /// A string copy exceeded `max_len` bytes without finding a null terminator (`\0`).
    ///
    /// Either the user intentionally provided a non-null-terminated "string" (possibly
    /// to cause a kernel memory scan), or the string is legitimately longer than allowed.
    /// In either case, the copy is aborted. The caller should return `ENAMETOOLONG`.
    StringTooLong,

    /// The bytes copied from userspace are not valid UTF-8.
    ///
    /// NullOS kernel strings are `String` (Rust's UTF-8 string type). If a user provides
    /// a byte sequence that is not valid UTF-8 (e.g., arbitrary binary data), we reject it
    /// rather than allowing invalid byte sequences into kernel string processing.
    InvalidUtf8,
}

/// Validates that a user memory buffer `[ptr, ptr + len)` is entirely within userspace.
///
/// This is a **pure range check** — it does NOT copy any data, does NOT dereference `ptr`,
/// and does NOT require SMAP to be bypassed. It only verifies that the address range is
/// legal for a user-space buffer.
///
/// All four failure modes are checked in order:
/// 1. `ptr` is null → `NullPointer`
/// 2. `ptr` base ≥ `USER_SPACE_CEILING` → `PointerAboveUserSpace`
/// 3. `ptr + len` overflows `u64` → `AddressOverflow`
/// 4. `ptr + len` > `USER_SPACE_CEILING` → `BufferExceedsUserSpace`
///
/// ## Arguments
/// - `ptr`: The start of the user buffer. Must be non-null and within user address space.
/// - `len`: The number of bytes in the buffer. Must not cause `ptr + len` to overflow
///   or exceed `USER_SPACE_CEILING`.
///
/// ## Returns
/// - `Ok(())` if the entire buffer `[ptr, ptr + len)` lies within user address space.
/// - `Err(SecurityError::*)` describing the specific validation failure.
///
/// ## Usage
/// Call this before any `unsafe` dereference of a user-provided pointer:
/// ```rust,ignore
/// pub fn sys_read(fd: i32, buf: *mut u8, count: usize) -> isize {
///     if validate_user_buffer(buf as *const u8, count).is_err() {
///         return -EFAULT;
///     }
///     // ... safe to proceed (still need STAC/CLAC for actual access)
/// }
/// ```
///
/// ## Note
/// This function does NOT verify that the pages at `[ptr, ptr+len)` are actually mapped
/// and readable in the current process's page table. A later fault during `copy_from_user`
/// (caused by an unmapped page) must be handled separately via page fault recovery.
pub fn validate_user_buffer(ptr: *const u8, len: usize) -> Result<(), SecurityError> {
    // Check 1: Null pointer guard.
    // Null (address 0x0) is never a valid user buffer. On most systems, the page at
    // virtual address 0 is intentionally unmapped (to catch null dereferences).
    if ptr.is_null() {
        return Err(SecurityError::NullPointer);
    }

    let addr = ptr as u64;

    // Check 2: Base pointer must be in the lower half (user space).
    // If addr >= USER_SPACE_CEILING, the pointer itself refers to kernel memory.
    // This catches direct kernel-pointer injection by malicious syscall arguments.
    if addr >= USER_SPACE_CEILING {
        return Err(SecurityError::PointerAboveUserSpace);
    }

    // Check 3: Check for integer overflow in addr + len.
    // `checked_add` returns None if the addition would overflow u64.
    // Without this check: addr=u64::MAX-5, len=10 → (addr+len) wraps to 4,
    // appearing to be a valid user address even though the range is invalid.
    let end_addr = addr.checked_add(len as u64).ok_or(SecurityError::AddressOverflow)?;

    // Check 4: Ensure the entire buffer fits within user space.
    // The range [addr, end_addr) must be strictly below USER_SPACE_CEILING.
    // This catches "straddle" attacks where the buffer starts in user space but
    // extends into kernel space (e.g., addr near ceiling, large len).
    if end_addr > USER_SPACE_CEILING {
        return Err(SecurityError::BufferExceedsUserSpace);
    }

    Ok(())
}

/// Safely copies `dst.len()` bytes from the userspace pointer `src` into the kernel buffer `dst`.
///
/// This is the **primary** function for reading data from a userspace address in a syscall.
/// It is the equivalent of Linux's `copy_from_user()`.
///
/// ## Security Guarantees
/// 1. **Range validated**: `validate_user_buffer(src, dst.len())` is called first.
///    If validation fails, no memory access occurs and an error is returned.
/// 2. **SMAP-safe**: If SMAP is active (CR4.SMAP=1), the `cpu::stac()` call sets
///    EFLAGS.AC=1 to temporarily grant kernel access to user pages. After the copy,
///    `cpu::clac()` clears EFLAGS.AC=0 to re-enable SMAP protection.
/// 3. **Non-overlapping**: Uses `copy_nonoverlapping` (equivalent to `memcpy`, not `memmove`).
///    Since src is in user space and dst is in kernel space, they cannot overlap.
///
/// ## Arguments
/// - `dst`: A mutable kernel-side byte slice to receive the data.
///   Length determines how many bytes are copied.
/// - `src`: A raw const pointer to the start of the userspace source buffer.
///   Must point to at least `dst.len()` bytes of readable user memory.
///
/// ## Returns
/// - `Ok(())` on success — `dst` is now filled with the data from userspace.
/// - `Err(SecurityError::*)` if the userspace pointer fails validation.
///
/// ## Safety Contract (for callers)
/// - `src` must point to memory that the **current user process** owns and has mapped.
/// - The pointed-to memory must not be concurrently modified by another thread in a way
///   that invalidates the copy (Time-of-Check/Time-of-Use — TOCTOU — race conditions
///   are an inherent limitation of copy_from_user; use kernel-side copies to handle this).
/// - `dst` must be a valid, writable, non-null kernel slice.
///
/// ## SMAP Interaction Detail
/// ```text
/// CR4.SMAP = 1 (active)
/// EFLAGS.AC = 0 (default: SMAP blocking user page access)
///
/// cpu::stac()   →  sets EFLAGS.AC = 1  (now kernel CAN access user pages)
/// memcpy(...)   →  copies data from user pages to kernel buffer
/// cpu::clac()   →  clears EFLAGS.AC = 0  (SMAP blocks user page access again)
/// ```
pub fn copy_from_user(dst: &mut [u8], src: *const u8) -> Result<(), SecurityError> {
    // Step 1: Validate the source address range before touching any user memory.
    // This call is a pure range check — no dereferencing happens here.
    validate_user_buffer(src, dst.len())?;

    unsafe {
        // Step 2: Set EFLAGS.AC = 1 to bypass SMAP and allow user-page access.
        // On CPUs without SMAP, stac() is a no-op.
        // On CPUs with SMAP (CR4.SMAP=1), this is mandatory before accessing user pages.
        cpu::stac();

        // Step 3: Perform the actual memory copy.
        // `copy_nonoverlapping` is equivalent to `memcpy`: it copies `dst.len()` bytes
        // from `src` to `dst.as_mut_ptr()`. It assumes no overlap between source and
        // destination (guaranteed here: src is user space, dst is kernel space).
        core::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), dst.len());

        // Step 4: Clear EFLAGS.AC = 0 to re-enable SMAP protection.
        // This MUST be called in ALL code paths after stac(), including error paths.
        // Here, the error path (validate_user_buffer failure) returned early before stac(),
        // so clac() is only needed for the success path.
        cpu::clac();
    }

    Ok(())
}

/// Safely copies `src.len()` bytes from the kernel buffer `src` to the userspace pointer `dst`.
///
/// This is the **primary** function for writing kernel data back to a userspace address.
/// It is the equivalent of Linux's `copy_to_user()`.
///
/// ## Security Guarantees
/// 1. **Range validated**: `validate_user_buffer(dst, src.len())` is called first.
///    If validation fails, no memory access occurs and an error is returned.
/// 2. **SMAP-safe**: STAC/CLAC bracket the write to user pages.
/// 3. **Non-overlapping**: `copy_nonoverlapping` requires no src/dst overlap.
///    Kernel `src` and user `dst` are in different halves of the address space → safe.
///
/// ## Arguments
/// - `dst`: Raw mutable pointer to the userspace destination buffer.
///   Must point to at least `src.len()` bytes of writable user memory.
/// - `src`: A const kernel-side byte slice to copy from.
///
/// ## Returns
/// - `Ok(())` on success — `src.len()` bytes from the kernel have been written to userspace.
/// - `Err(SecurityError::*)` if `dst` fails validation.
///
/// ## Safety Contract
/// - `dst` must be mapped and writable by the current user process.
/// - `src` must be a valid, readable kernel slice (no kernel-side UAF/dangling).
/// - Caller is responsible for ensuring the user process cannot race with this write
///   to create TOCTOU vulnerabilities (e.g., verify the destination mapping before writing).
pub fn copy_to_user(dst: *mut u8, src: &[u8]) -> Result<(), SecurityError> {
    // Step 1: Validate the destination address range (cast to *const for read-only check).
    // We validate using *const u8 since validate_user_buffer only needs the range.
    validate_user_buffer(dst as *const u8, src.len())?;

    unsafe {
        // Step 2: Temporarily allow supervisor access to user pages (bypass SMAP).
        cpu::stac();

        // Step 3: Copy kernel buffer contents into the user-space destination.
        // Direction: kernel (src.as_ptr()) → user (dst).
        core::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len());

        // Step 4: Re-enable SMAP to block future accidental user-page access.
        cpu::clac();
    }

    Ok(())
}

/// Safely copies a null-terminated UTF-8 string from userspace into a kernel `String`.
///
/// This function performs a **byte-by-byte validated scan** of a user-provided C-style
/// string (null-terminated byte array). It is used for syscalls that accept string
/// arguments (e.g., `open(pathname, flags, mode)`, `exec(path, argv, envp)`).
///
/// ## Why Byte-by-Byte?
/// Unlike `copy_from_user` (which copies a known fixed length), strings have an unknown
/// length at call time. We must scan until we find a null terminator (`\0`), checking
/// address validity at each step to prevent scanning into kernel space.
///
/// ## Security Guarantees
/// 1. **Null pointer check**: Returns `NullPointer` immediately.
/// 2. **Base address check**: Returns `PointerAboveUserSpace` if `src >= USER_SPACE_CEILING`.
/// 3. **Per-byte address check**: Each byte's address is checked against `USER_SPACE_CEILING`
///    before reading. This handles strings that start in user space but would extend into
///    kernel space if `max_len` were not enforced.
/// 4. **Length limit**: Returns `StringTooLong` after reading `max_len` bytes without
///    finding a null terminator.
/// 5. **UTF-8 validation**: The final `String::from_utf8()` call validates the bytes.
///    Returns `InvalidUtf8` for non-UTF-8 byte sequences.
/// 6. **SMAP-safe**: STAC/CLAC bracket the entire scan loop.
///    Note: CLAC is called in ALL return paths from within the unsafe block.
///
/// ## Arguments
/// - `src`: A raw const pointer to the start of a null-terminated user-space string.
/// - `max_len`: Maximum number of bytes to scan (not including the null terminator).
///   Use `MAX_USER_PATH_LEN` for path strings.
///
/// ## Returns
/// - `Ok(String)` containing the validated UTF-8 string (without the null terminator).
/// - `Err(SecurityError::*)` on any validation failure.
///
/// ## Integer Overflow Safety
/// `curr_addr = src_addr + offset` is computed with `checked_add` to prevent overflow.
/// If the string is very long and the check would overflow u64, `AddressOverflow` is returned.
///
/// ## SMAP Note
/// The `cpu::clac()` call must be executed in every exit path from the `unsafe` block.
/// Notice the three `cpu::clac()` calls:
/// 1. When `curr_addr >= USER_SPACE_CEILING` (boundary crossed mid-string)
/// 2. When `byte == 0` (null terminator found — success path)
/// (The third is at the end of the unsafe block after the loop exits via `max_len`)
pub fn copy_str_from_user(src: *const u8, max_len: usize) -> Result<String, SecurityError> {
    // Check 1: Null pointer — fastest possible early exit.
    if src.is_null() {
        return Err(SecurityError::NullPointer);
    }

    let src_addr = src as u64;

    // Check 2: Base address is in kernel space — direct kernel-pointer injection attempt.
    if src_addr >= USER_SPACE_CEILING {
        return Err(SecurityError::PointerAboveUserSpace);
    }

    // Accumulate bytes into this Vec. We use a Vec rather than a fixed-size array
    // because we don't know the string length ahead of time.
    // The final `String::from_utf8()` will consume this Vec.
    let mut bytes = Vec::new();
    // `offset` tracks how many bytes we've read. Used to compute the current address
    // and to enforce the `max_len` limit.
    let mut offset = 0;

    unsafe {
        // Set EFLAGS.AC=1: temporarily allow kernel access to user-mapped pages (SMAP bypass).
        cpu::stac();

        while offset < max_len {
            // Per-byte address validation: compute current scan address with overflow check.
            // This prevents the scan from wrapping around and re-entering user space
            // at a lower address (integer overflow attack on offset).
            let curr_addr = src_addr.checked_add(offset as u64).ok_or(SecurityError::AddressOverflow)?;

            // Check whether the current byte's address has crossed into kernel space.
            // This handles strings that start in user space but would reach kernel space
            // if `max_len` were not enforced (e.g., src near USER_SPACE_CEILING).
            if curr_addr >= USER_SPACE_CEILING {
                // Re-enable SMAP before returning — mandatory in every exit path.
                cpu::clac();
                return Err(SecurityError::BufferExceedsUserSpace);
            }

            // Read one byte from user space using read_volatile.
            // `read_volatile` prevents the compiler from:
            // 1. Caching a previous read (user memory may be mapped copy-on-write).
            // 2. Reordering this read across the STAC/CLAC boundary.
            // Cast the computed u64 address to a *const u8 raw pointer for the read.
            let byte = core::ptr::read_volatile((src_addr + offset as u64) as *const u8);

            // Null terminator found: end of the C-style string.
            if byte == 0 {
                // Re-enable SMAP before returning — mandatory.
                cpu::clac();
                // Convert accumulated bytes to a Rust String, validating UTF-8.
                // `map_err` converts the `FromUtf8Error` into our `SecurityError::InvalidUtf8`.
                return String::from_utf8(bytes).map_err(|_| SecurityError::InvalidUtf8);
            }

            // Append valid byte to the accumulator and advance the offset.
            bytes.push(byte);
            offset += 1;
        }

        // Loop exhausted `max_len` bytes without finding a null terminator.
        // Re-enable SMAP before returning — mandatory.
        cpu::clac();
    }

    // The string was longer than `max_len` bytes — reject it.
    Err(SecurityError::StringTooLong)
}
