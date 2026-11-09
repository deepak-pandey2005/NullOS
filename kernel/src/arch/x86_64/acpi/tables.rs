//! ACPI Table Header, RSDP, RSDT, and XSDT Parsing Primitives.

use alloc::vec::Vec;

/// Standard ACPI Table Header present on all System Description Tables (36 bytes).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct AcpiTableHeader {
    pub signature: [u8; 4],
    pub length: u32,
    pub revision: u8,
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub oem_table_id: [u8; 8],
    pub oem_revision: u32,
    pub creator_id: [u8; 4],
    pub creator_revision: u32,
}

impl AcpiTableHeader {
    pub fn signature_str(&self) -> &str {
        core::str::from_utf8(&self.signature).unwrap_or("????")
    }

    pub fn length(&self) -> usize {
        self.length as usize
    }

    /// Verifies the ACPI 8-bit checksum over the entire table length.
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
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct RsdpDescriptor {
    pub signature: [u8; 8], // "RSD PTR "
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub revision: u8,       // 0 = ACPI 1.0, 2 = ACPI 2.0+
    pub rsdt_address: u32,  // 32-bit physical address of RSDT
}

/// Extended RSDP descriptor for ACPI 2.0+ (36 bytes).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct RsdpExtendedDescriptor {
    pub base: RsdpDescriptor,
    pub length: u32,
    pub xsdt_address: u64,  // 64-bit physical address of XSDT
    pub extended_checksum: u8,
    pub reserved: [u8; 3],
}

/// Information about a discovered ACPI System Description Table.
#[derive(Debug, Clone)]
pub struct DiscoveredTable {
    pub signature: [u8; 4],
    pub address: u64,
    pub length: usize,
    pub revision: u8,
    pub oem_id: [u8; 6],
    pub oem_table_id: [u8; 8],
}

impl DiscoveredTable {
    pub fn signature_str(&self) -> &str {
        core::str::from_utf8(&self.signature).unwrap_or("????")
    }

    pub fn oem_id_str(&self) -> &str {
        core::str::from_utf8(&self.oem_id).unwrap_or("??????")
    }

    pub fn oem_table_id_str(&self) -> &str {
        core::str::from_utf8(&self.oem_table_id).unwrap_or("????????")
    }
}

/// Traverses 32-bit RSDT table pointers to collect all ACPI tables.
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
pub fn find_table(tables: &[DiscoveredTable], target_sig: &[u8; 4]) -> Option<u64> {
    for tbl in tables {
        if &tbl.signature == target_sig {
            return Some(tbl.address);
        }
    }
    None
}
