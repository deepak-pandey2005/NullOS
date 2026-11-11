//! # NullOS Binary Package Format (`.npkg`) Loader & Verifier (Phase 18 - M39)
//!
//! ## Binary Wire Specification (Matches `sdk/include/nullos/pkg.h`)
//! - Offset `0x00` (4B): Magic `"NPKG"` (`0x474B504E` LE)
//! - Offset `0x04` (2B): Format version (`1`)
//! - Offset `0x06` (2B): Flags (bit 0 = executable ELF, bit 1 = dynamically linked)
//! - Offset `0x08` (24B): Package name (UTF-8, NUL-padded)
//! - Offset `0x20` (12B): Package SemVer version string (UTF-8, NUL-padded)
//! - Offset `0x2C` (4B): Header size (`64`)
//! - Offset `0x30` (8B): Embedded payload byte length
//! - Offset `0x38` (4B): IEEE 802.3 CRC-32 checksum over embedded payload bytes
//! - Offset `0x3C` (4B): Reserved (`0`)

#![allow(dead_code)]

use alloc::string::String;
use core::str;

use crate::println;
use crate::task::process::execute_user_elf;

/// 4-byte ASCII magic header (`"NPKG"`).
pub const NPKG_MAGIC: [u8; 4] = *b"NPKG";
/// Fixed binary package header size in bytes.
pub const NPKG_HEADER_SIZE: usize = 64;
/// Flag indicating payload is a Ring 3 ELF executable.
pub const NPKG_FLAG_EXEC_ELF: u16 = 1 << 0;
/// Flag indicating payload requires `/lib/ld.so` + `libc.so`.
pub const NPKG_FLAG_DYN_LIBC: u16 = 1 << 1;

/// Parsed metadata and payload view from an `.npkg` package buffer.
#[derive(Debug, Clone)]
pub struct ParsedNpkg<'a> {
    pub format_version: u16,
    pub flags: u16,
    pub name: String,
    pub version: String,
    pub header_size: u32,
    pub payload_size: u64,
    pub expected_crc32: u32,
    pub actual_crc32: u32,
    pub payload: &'a [u8],
}

/// Computes standard IEEE 802.3 CRC-32 checksum matching Python's `zlib.crc32`.
pub fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn read_u16_le(bytes: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([bytes[off], bytes[off + 1]])
}

fn read_u32_le(bytes: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
}

fn read_u64_le(bytes: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        bytes[off],
        bytes[off + 1],
        bytes[off + 2],
        bytes[off + 3],
        bytes[off + 4],
        bytes[off + 5],
        bytes[off + 6],
        bytes[off + 7],
    ])
}

fn parse_cstr(slice: &[u8]) -> String {
    let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
    String::from(str::from_utf8(&slice[..end]).unwrap_or("unknown"))
}

/// Parses and CRC-32 verifies an `.npkg` binary package slice.
pub fn parse_and_verify(data: &[u8]) -> Result<ParsedNpkg<'_>, &'static str> {
    if data.len() < NPKG_HEADER_SIZE {
        return Err("npkg: buffer smaller than 64-byte header");
    }
    if data[0..4] != NPKG_MAGIC {
        return Err("npkg: invalid magic bytes (expected NPKG)");
    }

    let format_version = read_u16_le(data, 0x04);
    let flags = read_u16_le(data, 0x06);
    let name = parse_cstr(&data[0x08..0x20]);
    let version = parse_cstr(&data[0x20..0x2C]);
    let header_size = read_u32_le(data, 0x2C);
    let payload_size = read_u64_le(data, 0x30);
    let expected_crc32 = read_u32_le(data, 0x38);

    if header_size as usize > data.len() {
        return Err("npkg: header_size exceeds file size");
    }
    let start = header_size as usize;
    let end = start
        .checked_add(payload_size as usize)
        .ok_or("npkg: payload_size overflow")?;
    if end > data.len() {
        return Err("npkg: truncated package payload");
    }

    let payload = &data[start..end];
    let actual_crc32 = crc32_ieee(payload);
    if actual_crc32 != expected_crc32 {
        return Err("npkg: payload CRC-32 mismatch");
    }

    Ok(ParsedNpkg {
        format_version,
        flags,
        name,
        version,
        header_size,
        payload_size,
        expected_crc32,
        actual_crc32,
        payload,
    })
}

/// Prints formatted diagnostics for a verified `.npkg` bundle.
pub fn print_package_info(pkg: &ParsedNpkg<'_>) {
    println!("=== NullOS Binary Package (.npkg) ===");
    println!("  Package Name   : {}", pkg.name);
    println!("  Version        : {}", pkg.version);
    println!("  Format Version : {} (hdr={}B)", pkg.format_version, pkg.header_size);
    println!(
        "  Flags          : {:#06x} (exec_elf={}, dyn_libc={})",
        pkg.flags,
        (pkg.flags & NPKG_FLAG_EXEC_ELF) != 0,
        (pkg.flags & NPKG_FLAG_DYN_LIBC) != 0
    );
    println!("  Payload Size   : {} bytes", pkg.payload_size);
    println!(
        "  CRC-32         : {:#010x} (VERIFIED)",
        pkg.actual_crc32
    );
}

/// Verifies an `.npkg` package and launches its embedded Ring 3 ELF executable.
pub fn execute_package(data: &[u8]) -> Result<i32, &'static str> {
    let pkg = parse_and_verify(data)?;
    if (pkg.flags & NPKG_FLAG_EXEC_ELF) == 0 {
        return Err("npkg: package does not contain an executable ELF");
    }
    println!(
        "[NPKG] Verified '{}' v{} (CRC-32={:#010x}, {} bytes) — launching ELF...",
        pkg.name, pkg.version, pkg.actual_crc32, pkg.payload_size
    );
    execute_user_elf(&pkg.name, pkg.payload)
}

/// Runs deterministic in-kernel self-tests covering CRC-32 & header parsing.
pub fn run_self_tests() {
    println!("[SDK/NPKG] Running M39 .npkg Package Format & CRC-32 self-tests...");
    // Standard IEEE 802.3 test vector: crc32("123456789") == 0xCBF43926
    assert_eq!(crc32_ieee(b"123456789"), 0xCBF4_3926);

    // Construct synthetic in-memory .npkg buffer and verify parsing
    let payload = b"\x7fELF_FAKE_TEST_PAYLOAD";
    let crc = crc32_ieee(payload);
    let mut pkg = alloc::vec![0u8; NPKG_HEADER_SIZE + payload.len()];
    pkg[0..4].copy_from_slice(&NPKG_MAGIC);
    pkg[0x04..0x06].copy_from_slice(&1u16.to_le_bytes());
    pkg[0x06..0x08].copy_from_slice(&NPKG_FLAG_EXEC_ELF.to_le_bytes());
    pkg[0x08..0x10].copy_from_slice(b"testpkg\0");
    pkg[0x20..0x26].copy_from_slice(b"1.2.3\0");
    pkg[0x2C..0x30].copy_from_slice(&(NPKG_HEADER_SIZE as u32).to_le_bytes());
    pkg[0x30..0x38].copy_from_slice(&(payload.len() as u64).to_le_bytes());
    pkg[0x38..0x3C].copy_from_slice(&crc.to_le_bytes());
    pkg[NPKG_HEADER_SIZE..].copy_from_slice(payload);

    let parsed = parse_and_verify(&pkg).expect("Synthetic .npkg should verify cleanly");
    assert_eq!(parsed.name, "testpkg");
    assert_eq!(parsed.version, "1.2.3");
    assert_eq!(parsed.actual_crc32, crc);

    // Corrupt a byte and verify CRC-32 rejects it
    pkg[NPKG_HEADER_SIZE] ^= 0xFF;
    assert!(parse_and_verify(&pkg).is_err(), "Corrupted payload must fail CRC-32 check");
    println!("  [OK] .npkg header validation & IEEE CRC-32 tamper detection verified.");
}

