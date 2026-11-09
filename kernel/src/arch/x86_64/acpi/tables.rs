//! # ACPI Table Header, RSDP, RSDT, and XSDT Parsing Primitives
//!
//! ## Overview
//! The Advanced Configuration and Power Interface (ACPI) specification organizes hardware
//! description tables hierarchically. At the root of this structure is the **Root System
//! Description Pointer (RSDP)**, passed to the kernel from UEFI Boot Services or discovered
//! in the legacy Extended BIOS Data Area (EBDA).
//!
//! Depending on the ACPI version:
//! - **ACPI 1.0**: The RSDP points to the 32-bit **Root System Description Table (RSDT)**,
//!   which contains an array of 32-bit physical addresses pointing to description tables.
//! - **ACPI 2.0+**: The RSDP is extended to include a 64-bit pointer to the **Extended System
//!   Description Table (XSDT)**, which contains an array of 64-bit physical addresses.
//!
//! All ACPI tables share a common 36-byte header ([`AcpiTableHeader`]) detailing the table's
//! signature, total length, revision, checksum, and OEM provenance.
//!
//! ## Invariants & Checksum Rules
//! - Any valid ACPI table must satisfy: the 8-bit sum of all bytes in the table must equal 0 (`sum % 256 == 0`).
//! - All table addresses parsed from RSDT/XSDT are physical addresses identity-mapped during kernel bootstrap.

use alloc::vec::Vec;

/// Standard ACPI Table Header present on all System Description Tables (36 bytes).
///
/// Every standard ACPI table (FADT, MADT, HPET, DSDT, SSDT, etc.) begins with this identical
/// header structure defining identification, boundary, and validation data.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct AcpiTableHeader {
    /// 4-byte ASCII signature identifying table type (e.g., b"APIC", b"FACP", b"HPET").
    pub signature: [u8; 4],
    /// Total length of the table in bytes, including this 36-byte header.
    pub length: u32,
    /// ACPI specification revision supported by this table.
    pub revision: u8,
    /// 8-bit checksum; the sum of all bytes in the table including this byte must equal zero.
    pub checksum: u8,
    /// OEM identification string (6 ASCII characters).
    pub oem_id: [u8; 6],
    /// OEM table identification string (8 ASCII characters).
    pub oem_table_id: [u8; 8],
    /// OEM revision number.
    pub oem_revision: u32,
    /// Vendor ID of utility that created the table.
    pub creator_id: [u8; 4],
    /// Revision of utility that created the table.
    pub creator_revision: u32,
}

impl AcpiTableHeader {
    /// Returns the 4-byte signature as a UTF-8 string slice, or `????` if invalid UTF-8.
    pub fn signature_str(&self) -> &str {
        core::str::from_utf8(&self.signature).unwrap_or("????")
    }

    /// Returns the length of the table in bytes as `usize`.
    pub fn length(&self) -> usize {
        self.length as usize
    }

    /// Verifies the ACPI 8-bit checksum over the entire table length.
    ///
    /// # Parameters
    /// - `table_addr`: The physical (or identity-mapped virtual) address of the table start.
    ///
    /// # Safety Invariant
    /// The memory region `[table_addr, table_addr + self.length)` must be valid mapped memory.
    pub fn verify_checksum(&self, table_addr: u64) -> bool {
        let len = self.length as usize;
        if len < core::mem::size_of::<AcpiTableHeader>() {
            return false;
        }
        let bytes = unsafe { core::slice::from_raw_parts(table_addr as *const u8, len) };
        let sum: u8 = bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
        sum == 0
    }
}

/// Root System Description Pointer (RSDP) descriptor (ACPI 1.0 - 20 bytes).
///
/// Contains the signature `"RSD PTR "` and the 32-bit physical address of the RSDT.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct RsdpDescriptor {
    /// ACPI 1.0 signature: must be `*b"RSD PTR "`.
    pub signature: [u8; 8], // "RSD PTR "
    /// 8-bit checksum over the first 20 bytes (ACPI 1.0 fields).
    pub checksum: u8,
    /// OEM identification string.
    pub oem_id: [u8; 6],
    /// ACPI revision: `0` for ACPI 1.0, `2` for ACPI 2.0 through 6.x.
    pub revision: u8,
    /// 32-bit physical address of the Root System Description Table (RSDT).
    pub rsdt_address: u32,
}

/// Extended RSDP descriptor for ACPI 2.0+ (36 bytes).
///
/// Extends [`RsdpDescriptor`] with a 64-bit physical address to the XSDT and an extended checksum.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct RsdpExtendedDescriptor {
    /// Base ACPI 1.0 RSDP structure (20 bytes).
    pub base: RsdpDescriptor,
    /// Total length of the descriptor including the extended fields (36 bytes).
    pub length: u32,
    /// 64-bit physical address of the Extended System Description Table (XSDT).
    pub xsdt_address: u64,
    /// Checksum over all 36 bytes of the extended structure.
    pub extended_checksum: u8,
    /// Reserved bytes for future expansion.
    pub reserved: [u8; 3],
}

/// Information about a discovered ACPI System Description Table.
///
/// Cached metadata describing table location, boundary, revision, and OEM identifiers.
#[derive(Debug, Clone)]
pub struct DiscoveredTable {
    /// 4-character signature (e.g., `APIC`, `FACP`, `HPET`, `MCFG`).
    pub signature: [u8; 4],
    /// Physical address where the table resides in memory.
    pub address: u64,
    /// Total length of the table in bytes.
    pub length: usize,
    /// Table specification revision.
    pub revision: u8,
    /// OEM identifier string bytes.
    pub oem_id: [u8; 6],
    /// OEM table identifier string bytes.
    pub oem_table_id: [u8; 8],
}

impl DiscoveredTable {
    /// Returns the table signature as a readable ASCII/UTF-8 string.
    pub fn signature_str(&self) -> &str {
        core::str::from_utf8(&self.signature).unwrap_or("????")
    }

    /// Returns the OEM identifier as a readable ASCII/UTF-8 string.
    pub fn oem_id_str(&self) -> &str {
        core::str::from_utf8(&self.oem_id).unwrap_or("??????")
    }

    /// Returns the OEM table identifier as a readable ASCII/UTF-8 string.
    pub fn oem_table_id_str(&self) -> &str {
        core::str::from_utf8(&self.oem_table_id).unwrap_or("????????")
    }
}

/// Traverses 32-bit RSDT table pointers to collect all ACPI tables.
///
/// # Parameters
/// - `rsdt_addr`: Physical address of the RSDT table.
///
/// # Returns
/// A [`Vec`] of all valid [`DiscoveredTable`] entries referenced by the RSDT.
pub fn parse_rsdt(rsdt_addr: u64) -> Vec<DiscoveredTable> {
    let mut tables = Vec::new();
    if rsdt_addr == 0 {
        return tables;
    }

    let header = unsafe { &*(rsdt_addr as *const AcpiTableHeader) };
    if &header.signature != b"RSDT" {
        return tables;
    }

    let entry_count = (header.length() - core::mem::size_of::<AcpiTableHeader>()) / 4;
    let entries_ptr = (rsdt_addr + core::mem::size_of::<AcpiTableHeader>() as u64) as *const u32;

    for i in 0..entry_count {
        let table_ptr = unsafe { entries_ptr.add(i).read_unaligned() } as u64;
        if table_ptr == 0 {
            continue;
        }
        let tbl_hdr = unsafe { &*(table_ptr as *const AcpiTableHeader) };
        tables.push(DiscoveredTable {
            signature: tbl_hdr.signature,
            address: table_ptr,
            length: tbl_hdr.length(),
            revision: tbl_hdr.revision,
            oem_id: tbl_hdr.oem_id,
            oem_table_id: tbl_hdr.oem_table_id,
        });
    }

    tables
}

/// Traverses 64-bit XSDT table pointers to collect all ACPI tables.
///
/// # Parameters
/// - `xsdt_addr`: Physical address of the XSDT table.
///
/// # Returns
/// A [`Vec`] of all valid [`DiscoveredTable`] entries referenced by the XSDT.
pub fn parse_xsdt(xsdt_addr: u64) -> Vec<DiscoveredTable> {
    let mut tables = Vec::new();
    if xsdt_addr == 0 {
        return tables;
    }

    let header = unsafe { &*(xsdt_addr as *const AcpiTableHeader) };
    if &header.signature != b"XSDT" {
        return tables;
    }

    let entry_count = (header.length() - core::mem::size_of::<AcpiTableHeader>()) / 8;
    let entries_ptr = (xsdt_addr + core::mem::size_of::<AcpiTableHeader>() as u64) as *const u64;

    for i in 0..entry_count {
        let table_ptr = unsafe { entries_ptr.add(i).read_unaligned() };
        if table_ptr == 0 {
            continue;
        }
        let tbl_hdr = unsafe { &*(table_ptr as *const AcpiTableHeader) };
        tables.push(DiscoveredTable {
            signature: tbl_hdr.signature,
            address: table_ptr,
            length: tbl_hdr.length(),
            revision: tbl_hdr.revision,
            oem_id: tbl_hdr.oem_id,
            oem_table_id: tbl_hdr.oem_table_id,
        });
    }

    tables
}

/// Searches a list of discovered tables for a table matching `target_sig`.
///
/// # Parameters
/// - `tables`: Slice of discovered ACPI tables.
/// - `target_sig`: 4-byte signature to locate (e.g. `b"FACP"`, `b"APIC"`).
///
/// # Returns
/// `Some(u64)` physical address of the table if found, or `None`.
pub fn find_table(tables: &[DiscoveredTable], target_sig: &[u8; 4]) -> Option<u64> {
    for tbl in tables {
        if &tbl.signature == target_sig {
            return Some(tbl.address);
        }
    }
    None
}
