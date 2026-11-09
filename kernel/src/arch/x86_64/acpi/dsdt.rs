//! Differentiated System Description Table (DSDT) & AML Bytecode Parser.
//!
//! Maps the DSDT definition block and extracts the `\_S5` sleep state package
//! to resolve `SLP_TYPa` and `SLP_TYPb` values for hardware poweroff.

use crate::arch::x86_64::acpi::tables::AcpiTableHeader;
use crate::println;

/// Information extracted from the DSDT.
#[derive(Debug, Clone, Copy)]
pub struct DsdtInfo {
    pub address: u64,
    pub length: usize,
    pub s5_supported: bool,
    pub slp_typa: u8,
    pub slp_typb: u8,
}

/// Parses the DSDT at `dsdt_phys` and locates the `\_S5` object in AML bytecode.
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
