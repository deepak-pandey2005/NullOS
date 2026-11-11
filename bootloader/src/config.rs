//! NullOS Boot Configuration Parser (`nullos.cfg`, Phase 19 - M41).
//!
//! # Overview
//! Reads and parses `nullos.cfg` from the EFI System Partition (`\nullos.cfg`,
//! `\EFI\BOOT\nullos.cfg`, or `\EFI\nullos\nullos.cfg`) without heap allocation.
//!
//! # Supported Directives (`key=value`)
//! - `version`: Distribution release string (e.g. `0.1.0`)
//! - `kernel`: Path to the ELF64 kernel binary on the ESP (default `\kernel.elf`)
//! - `initrd`: Path to the USTAR initramfs archive on the ESP (default `\initramfs.tar`)
//! - `cmdline`: Kernel boot parameters passed via `BootInfo` (default `console=com1 loglevel=info`)
//! - `timeout`: Pre-boot delay in seconds (`0` for immediate boot)

use crate::fs;
use uefi::cstr16;

/// Parsed boot configuration extracted from `nullos.cfg` (or compiled defaults if absent).
#[derive(Debug, Clone, Copy)]
pub struct BootConfig {
    /// Human-readable release version string.
    pub version: &'static str,
    /// ESP path to the ELF64 kernel image.
    pub kernel_path: &'static str,
    /// ESP path to the optional initramfs USTAR archive.
    pub initramfs_path: &'static str,
    /// Kernel command line string forwarded into `BootInfo`.
    pub cmdline: &'static str,
    /// Optional countdown delay in seconds prior to loading the kernel.
    pub timeout_secs: u64,
    /// Indicates whether a `nullos.cfg` file was discovered and parsed from disk.
    pub loaded_from_file: bool,
}

impl Default for BootConfig {
    /// Returns safe default boot parameters used when `nullos.cfg` is not present on the ESP.
    fn default() -> Self {
        Self {
            version: "0.1.0",
            kernel_path: "\\kernel.elf",
            initramfs_path: "\\initramfs.tar",
            cmdline: "console=com1 loglevel=info",
            timeout_secs: 0,
            loaded_from_file: false,
        }
    }
}

/// Attempts to locate, read, and parse `nullos.cfg` from the EFI System Partition.
///
/// Search order:
/// 1. `\nullos.cfg`
/// 2. `\EFI\BOOT\nullos.cfg`
///
/// Unrecognized keys and malformed UTF-8 lines are ignored safely while preserving
/// defaults for unspecified fields.
pub fn load_boot_config() -> BootConfig {
    let raw = fs::load_file(cstr16!("nullos.cfg"))
        .or_else(|_| fs::load_file(cstr16!("EFI\\BOOT\\nullos.cfg")));

    let Ok(bytes) = raw else {
        return BootConfig::default();
    };

    let Ok(text) = core::str::from_utf8(bytes) else {
        return BootConfig::default();
    };

    let mut cfg = BootConfig::default();
    cfg.loaded_from_file = true;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some(eq_idx) = line.find('=') else {
            continue;
        };
        let key = line[..eq_idx].trim();
        let mut val = line[eq_idx + 1..].trim();
        if val.len() >= 2 && val.starts_with('"') && val.ends_with('"') {
            val = &val[1..val.len() - 1];
        }

        match key {
            "version" if !val.is_empty() => cfg.version = val,
            "kernel" if !val.is_empty() => cfg.kernel_path = val,
            "initrd" | "initramfs" if !val.is_empty() => cfg.initramfs_path = val,
            "cmdline" | "append" => cfg.cmdline = val,
            "timeout" => {
                if let Ok(secs) = val.parse::<u64>() {
                    cfg.timeout_secs = secs.min(30);
                }
            }
            _ => {}
        }
    }

    cfg
}

