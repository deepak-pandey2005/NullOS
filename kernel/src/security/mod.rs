//! Kernel Security, Memory Hardening, and Defense-in-Depth Subsystem (Phase 10 - M26).
//!
//! Submodules:
//! - `usercopy`: Safe user-kernel boundary pointer sanitization and memory copying.
//! - `guard`: Stack guard pages and stack smashing protector canaries.
//! - `hardening`: Hardware security flags, W^X policy enforcement, and self-tests.
#![allow(unused_imports, dead_code)]

pub mod guard;
pub mod hardening;
pub mod usercopy;

pub use guard::{get_stack_canary, init_canary, place_canary, verify_canary, STACK_CANARY_MAGIC};
pub use hardening::{get_security_report, run_self_tests, SecurityReport};
pub use usercopy::{
    copy_from_user, copy_str_from_user, copy_to_user, validate_user_buffer, SecurityError,
    MAX_USER_PATH_LEN, USER_SPACE_CEILING,
};
