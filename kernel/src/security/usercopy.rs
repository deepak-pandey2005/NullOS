//! Safe Userspace Memory Transfer and Boundary Sanitization (Phase 10 - M26).
//!
//! Provides memory boundary checks and safe transfer primitives (`copy_from_user`,
//! `copy_to_user`, `copy_str_from_user`) ensuring Ring 0 kernel execution is insulated
//! from invalid, malicious, or out-of-bounds user-mode pointers.

use alloc::string::String;
use alloc::vec::Vec;
use crate::arch::x86_64::cpu;

/// Canonical ceiling of the 48-bit user virtual address space.
/// User-mode addresses must reside strictly in the lower half (< 0x0000_8000_0000_0000).
pub const USER_SPACE_CEILING: u64 = 0x0000_8000_0000_0000;

/// Standard maximum path length for string copies.
pub const MAX_USER_PATH_LEN: usize = 1024;

/// Security and boundary validation errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityError {
    /// Pointer is null.
    NullPointer,
    /// Pointer resides outside the legal user-space address range.
    PointerAboveUserSpace,
    /// Memory range calculation produced an integer overflow.
    AddressOverflow,
    /// User buffer extends across the user-kernel boundary.
    BufferExceedsUserSpace,
    /// String exceeded the maximum allowable length without a null terminator.
    StringTooLong,
    /// String contains invalid UTF-8 byte sequences.
    InvalidUtf8,
}

/// Validates that a user memory buffer `[ptr, ptr + len)` is strictly contained
/// within the lower canonical user virtual address space.
pub fn validate_user_buffer(ptr: *const u8, len: usize) -> Result<(), SecurityError> {
    if ptr.is_null() {
        return Err(SecurityError::NullPointer);
    }

    let addr = ptr as u64;

    // Check if base pointer is in kernel space
    if addr >= USER_SPACE_CEILING {
        return Err(SecurityError::PointerAboveUserSpace);
    }

    // Check for integer overflow in addr + len
    let end_addr = addr.checked_add(len as u64).ok_or(SecurityError::AddressOverflow)?;

    // Check if end of buffer crosses into kernel space
    if end_addr > USER_SPACE_CEILING {
        return Err(SecurityError::BufferExceedsUserSpace);
    }

    Ok(())
}

/// Safely copies `dst.len()` bytes from user-space pointer `src` into kernel buffer `dst`.
pub fn copy_from_user(dst: &mut [u8], src: *const u8) -> Result<(), SecurityError> {
    validate_user_buffer(src, dst.len())?;

    unsafe {
        // Temporarily allow supervisor access to user pages if SMAP is active
        cpu::stac();

        core::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), dst.len());

        cpu::clac();
    }

    Ok(())
}

/// Safely copies `src.len()` bytes from kernel buffer `src` to user-space pointer `dst`.
pub fn copy_to_user(dst: *mut u8, src: &[u8]) -> Result<(), SecurityError> {
    validate_user_buffer(dst as *const u8, src.len())?;

    unsafe {
        // Temporarily allow supervisor access to user pages if SMAP is active
        cpu::stac();

        core::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len());

        cpu::clac();
    }

    Ok(())
}

/// Safely copies a null-terminated UTF-8 string from user-space pointer `src` up to `max_len`.
pub fn copy_str_from_user(src: *const u8, max_len: usize) -> Result<String, SecurityError> {
    if src.is_null() {
        return Err(SecurityError::NullPointer);
    }

    let src_addr = src as u64;
    if src_addr >= USER_SPACE_CEILING {
        return Err(SecurityError::PointerAboveUserSpace);
    }

    let mut bytes = Vec::new();
    let mut offset = 0;

    unsafe {
        cpu::stac();

        while offset < max_len {
            let curr_addr = src_addr.checked_add(offset as u64).ok_or(SecurityError::AddressOverflow)?;
            if curr_addr >= USER_SPACE_CEILING {
                cpu::clac();
                return Err(SecurityError::BufferExceedsUserSpace);
            }

            let byte = core::ptr::read_volatile((src_addr + offset as u64) as *const u8);
            if byte == 0 {
                cpu::clac();
                return String::from_utf8(bytes).map_err(|_| SecurityError::InvalidUtf8);
            }

            bytes.push(byte);
            offset += 1;
        }

        cpu::clac();
    }

    Err(SecurityError::StringTooLong)
}
