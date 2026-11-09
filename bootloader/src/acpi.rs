//! UEFI ACPI Root System Description Pointer (RSDP) Discovery.
//!
//! # ACPI Discovery Protocol
//! Under legacy 16-bit BIOS systems, locating ACPI tables required scanning physical
//! memory byte-by-byte across the Extended BIOS Data Area (EBDA at `0x40E`) or the
//! main BIOS ROM memory window (`0xE0000..0xFFFFF`) searching for the 8-byte ASCII
//! signature `"RSD PTR "`.
//!
//! Under modern UEFI systems, memory scanning is obsolete. The UEFI firmware
//! publishes pointers to hardware description structures directly inside the
//! **UEFI Configuration Table**. This module iterates the firmware configuration
//! entries to identify the ACPI RSDP physical address.

use uefi::system::with_config_table;
use uefi::table::cfg::{ACPI_GUID, ACPI2_GUID};

/// Searches UEFI Configuration Tables for the ACPI Root System Description Pointer (RSDP).
///
/// # Search Priority
/// 1. **ACPI 2.0+ Specification (`ACPI2_GUID`)**:
///    GUID `8868e871-e4f1-11d3-bc22-0080c73c8881`. Points to an extended 36-byte
///    RSDP structure featuring 64-bit physical pointers to the Extended System
///    Description Table (XSDT).
/// 2. **ACPI 1.0 Specification (`ACPI_GUID`)**:
///    GUID `eb9d2d30-2d88-11d3-9a16-0090273fc14d`. Points to a legacy 20-byte
///    RSDP structure containing 32-bit pointers to the Root System Description
///    Table (RSDT). Used as a backward-compatibility fallback.
///
/// # Returns
/// - `Some(u64)`: Physical 64-bit memory address of the RSDP table.
/// - `None`: If the motherboard firmware does not publish ACPI configuration tables.
pub fn find_acpi_rsdp() -> Option<u64> {
    with_config_table(|entries| {
        // Step 1: Scan for modern ACPI 2.0+ Table Pointer (64-bit XSDT support)
        for entry in entries {
            if entry.guid == ACPI2_GUID {
                return Some(entry.address as u64);
            }
        }
        // Step 2: Fallback scan for legacy ACPI 1.0 Table Pointer (32-bit RSDT support)
        for entry in entries {
            if entry.guid == ACPI_GUID {
                return Some(entry.address as u64);
            }
        }
        None
    })
}
