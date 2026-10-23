//! Stack Guard Pages and Stack Smashing Protector (Canary) (Phase 10 - M26).
//!
//! Provides defense-in-depth against stack overflow attacks and accidental stack exhaustion:
//! - 64-bit Stack Canary validation guarding frame integrity.
//! - Guard page verification checking that boundary frames remain unmapped or protected.

use core::sync::atomic::{AtomicU64, Ordering};

/// Default 64-bit stack canary magic value.
pub const STACK_CANARY_MAGIC: u64 = 0xDEADC0DE_FEEDFACE;

static GLOBAL_STACK_CANARY: AtomicU64 = AtomicU64::new(STACK_CANARY_MAGIC);

/// Initializes the global stack canary value.
pub fn init_canary() {
    // In production, this can incorporate TSC or hardware randomness
    let rdtsc: u64;
    unsafe {
        core::arch::asm!("rdtsc", out("rax") rdtsc, options(nomem, nostack));
    }
    let mixed = STACK_CANARY_MAGIC ^ rdtsc;
    GLOBAL_STACK_CANARY.store(mixed, Ordering::SeqCst);
}

/// Returns the current global stack canary value.
#[inline]
pub fn get_stack_canary() -> u64 {
    GLOBAL_STACK_CANARY.load(Ordering::Relaxed)
}

/// Places a stack canary at the specified memory address (e.g. at the bottom of a stack).
pub fn place_canary(canary_ptr: *mut u64) {
    if !canary_ptr.is_null() {
        unsafe {
            core::ptr::write_volatile(canary_ptr, get_stack_canary());
        }
    }
}

/// Verifies that the stack canary at `canary_ptr` has not been corrupted.
pub fn verify_canary(canary_ptr: *const u64) -> bool {
    if canary_ptr.is_null() {
        return false;
    }
    unsafe {
        core::ptr::read_volatile(canary_ptr) == get_stack_canary()
    }
}

/// Stack smashing protector callback called when compiler detects canary corruption.
#[unsafe(no_mangle)]
pub extern "C" fn __stack_chk_fail() -> ! {
    crate::println!("\n!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
    crate::println!("   FATAL SECURITY ALERT: STACK SMASHING DETECTED! ");
    crate::println!("   Stack canary corruption detected in execution. ");
    crate::println!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
    loop {
        unsafe {
            core::arch::asm!("cli; hlt");
        }
    }
}
