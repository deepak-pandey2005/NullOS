//! # Differentiated System Description Table (DSDT) & AML Bytecode Parser
//!
//! ## Overview
//! The Differentiated System Description Table (DSDT) is the primary ACPI definition block,
//! containing AML (ACPI Machine Language) bytecode compiled from ASL (ACPI Source Language).
//! AML describes the platform's hardware topology, devices, methods, power resources, and
//! sleep states.
//!
//! In NullOS, the DSDT parser performs targeted pattern matching across the AML byte stream
//! to extract the **`\_S5` (Soft Off / Poweroff)** sleep state object without requiring a full,
//! heavyweight AML interpreter.
//!
//! ## The `\_S5` Sleep State Object in AML
//! The `\_S5` state object is represented in AML as:
//! ```text
//! Name (_S5, Package (0x04) {
//!     0x00, // SLP_TYPa: Value written to PM1a_CNT to initiate S5
//!     0x00, // SLP_TYPb: Value written to PM1b_CNT to initiate S5
//!     0x00, // Reserved
//!     0x00  // Reserved
//! })
//! ```
//! In binary AML:
//! 1. **NameOp identifier**: ASCII sequence `_S5_` (bytes `0x5F, 0x53, 0x35, 0x5F`).
//! 2. **PackageOp**: Byte `0x12`.
//! 3. **PkgLength**: Variable-length lead byte encoding the size of the package.
//! 4. **NumElements**: Byte defining number of items in the package.
//! 5. **Element Bytes**: Values encoded as `ZeroOp` (0x00), `OneOp` (0x01), `BytePrefix` (0x0A + byte),
//!    `WordPrefix` (0x0B + 2 bytes), or `DWordPrefix` (0x0C + 4 bytes).
//!
//! When successfully extracted, `slp_typa` and `slp_typb` are shifted into bits 10..12 of the
//! `PM1_CNT` register alongside the `SLP_EN` bit (bit 13) to power off the machine.

use crate::arch::x86_64::acpi::tables::AcpiTableHeader;
use crate::println;

/// Information extracted from the DSDT AML bytecode.
#[derive(Debug, Clone, Copy)]
pub struct DsdtInfo {
    /// Physical address of the DSDT table.
    pub address: u64,
    /// Total length of the DSDT table in bytes.
    pub length: usize,
    /// Whether the `\_S5` sleep state package was successfully located and parsed.
    pub s5_supported: bool,
    /// Sleep Type A value for PM1a control register (`SLP_TYPa`).
    pub slp_typa: u8,
    /// Sleep Type B value for PM1b control register (`SLP_TYPb`).
    pub slp_typb: u8,
}

/// Parses the DSDT at `dsdt_phys` and locates the `\_S5` object in AML bytecode.
///
/// Validates the `b"DSDT"` header signature, isolates the AML bytecode payload,
/// and delegates to [`find_s5_object`].
///
/// # Parameters
/// - `dsdt_phys`: Physical address of the DSDT.
///
/// # Returns
/// - `Ok(DsdtInfo)` containing the parsed sleep state parameters.
/// - `Err(&'static str)` if physical address is null or signature is invalid.
pub fn parse_dsdt(dsdt_phys: u64) -> Result<DsdtInfo, &'static str> {
    if dsdt_phys == 0 {
        return Err("Null DSDT physical address");
    }

    let header = unsafe { &*(dsdt_phys as *const AcpiTableHeader) };
    if &header.signature != b"DSDT" {
        return Err("Invalid DSDT signature");
    }

    let total_len = header.length();
    let aml_bytes = unsafe {
        core::slice::from_raw_parts(
            (dsdt_phys + core::mem::size_of::<AcpiTableHeader>() as u64) as *const u8,
            total_len.saturating_sub(core::mem::size_of::<AcpiTableHeader>()),
        )
    };

    let (s5_found, slp_typa, slp_typb) = find_s5_object(aml_bytes);

    if s5_found {
        println!(
            "[ACPI] Discovered \\_S5 object in DSDT (SLP_TYPa: 0x{:02x}, SLP_TYPb: 0x{:02x})",
            slp_typa, slp_typb
        );
    } else {
        println!("[ACPI] Notice: \\_S5 object not found in DSDT, using QEMU/KVM fallback (0x00, 0x00).");
    }

    Ok(DsdtInfo {
        address: dsdt_phys,
        length: total_len,
        s5_supported: s5_found,
        slp_typa,
        slp_typb,
    })
}

/// Scans AML bytecode for the `_S5_` NameOp and extracts the sleep values.
///
/// Locates `b"_S5_"`, inspects the trailing window for `PackageOp` (`0x12`),
/// skips the AML `PkgLength` prefix, and decodes the first two integer elements
/// as `SLP_TYPa` and `SLP_TYPb`.
///
/// # Parameters
/// - `aml`: Slice of raw AML bytecode.
///
/// # Returns
/// A tuple `(s5_found, slp_typa, slp_typb)`. If not found, defaults to `(false, 0, 0)`
/// which matches QEMU/Bochs default ACPI S5 parameters.
fn find_s5_object(aml: &[u8]) -> (bool, u8, u8) {
    // Search for "_S5_" signature in AML
    let pattern = b"_S5_";
    let mut idx = 0;

    while idx + 4 <= aml.len() {
        if &aml[idx..idx + 4] == pattern {
            // Found "_S5_". Now search for PackageOp (0x12) within the next 8 bytes
            let start = idx + 4;
            let end = (start + 8).min(aml.len());

            for p_idx in start..end {
                if aml[p_idx] == 0x12 {
                    // PackageOp found! Decode AML PkgLength
                    let mut curr = p_idx + 1;
                    if curr >= aml.len() {
                        break;
                    }

                    let lead = aml[curr];
                    let byte_count = (lead >> 6) as usize;
                    curr += 1 + byte_count; // Skip length bytes

                    if curr >= aml.len() {
                        break;
                    }

                    // NumElements
                    let num_elements = aml[curr];
                    curr += 1;

                    if num_elements > 0 && curr < aml.len() {
                        // Element 1: SLP_TYPa
                        let (typa, next_curr) = parse_aml_integer(&aml[curr..]);
                        curr += next_curr;

                        let typb = if num_elements > 1 && curr < aml.len() {
                            let (tb, _) = parse_aml_integer(&aml[curr..]);
                            tb
                        } else {
                            0
                        };

                        return (true, typa, typb);
                    }
                }
            }
        }
        idx += 1;
    }

    // Default QEMU/Bochs S5 values (SLP_TYP = 0)
    (false, 0, 0)
}

/// Helper function to parse an AML integer element (BytePrefix, ZeroOp, OneOp, etc.).
///
/// Handles AML variable integer representations:
/// - `0x00`: `ZeroOp` -> 0
/// - `0x01`: `OneOp` -> 1
/// - `0x0A`: `BytePrefix` -> next byte
/// - `0x0B`: `WordPrefix` -> next 2 bytes (lower byte returned)
/// - `0x0C`: `DWordPrefix` -> next 4 bytes (lowest byte returned)
/// - Any other byte: Direct literal value
///
/// # Returns
/// `(value, bytes_consumed)`.
fn parse_aml_integer(bytes: &[u8]) -> (u8, usize) {
    if bytes.is_empty() {
        return (0, 0);
    }

    match bytes[0] {
        0x00 => (0, 1), // ZeroOp
        0x01 => (1, 1), // OneOp
        0x0A => {
            // BytePrefix
            if bytes.len() > 1 {
                (bytes[1], 2)
            } else {
                (0, 1)
            }
        }
        0x0B => {
            // WordPrefix
            if bytes.len() > 2 {
                (bytes[1], 3)
            } else {
                (0, 1)
            }
        }
        0x0C => {
            // DWordPrefix
            if bytes.len() > 4 {
                (bytes[1], 5)
            } else {
                (0, 1)
            }
        }
        val => (val, 1), // Direct small integer byte
    }
}

