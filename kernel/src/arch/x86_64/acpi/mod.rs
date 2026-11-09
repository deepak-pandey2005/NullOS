//! # Advanced Configuration and Power Interface (ACPI) Subsystem
//!
//! ## Subsystem Architecture & Responsibilities
//! ACPI forms the hardware abstraction contract between modern PC firmware (UEFI/BIOS)
//! and operating system kernels. During early bootstrap, firmware passes a physical pointer
//! to the Root System Description Pointer ([`RsdpDescriptor`]). From this pointer, the ACPI
//! subsystem navigates the platform's System Description Tables:
//!
//! ```text
//!                      +-------------------------+
//!                      |   RSDP (Root Pointer)   |
//!                      +-------------------------+
//!                                   |
//!                +------------------+------------------+
//!                | (ACPI 1.0: 32-bit)                  | (ACPI 2.0+: 64-bit)
//!                v                                     v
//!     +--------------------+                +--------------------+
//!     | RSDT (32-bit Ptrs) |                | XSDT (64-bit Ptrs) |
//!     +--------------------+                +--------------------+
//!                |                                     |
//!                +------------------+------------------+
//!                                   |
//!           +-----------------------+-----------------------+
//!           |                       |                       |
//!           v                       v                       v
//!     +-----------+           +-----------+           +-----------+
//!     |   MADT    |           |   FADT    |           |   HPET    |
//!     |  ("APIC") |           |  ("FACP") |           |  ("HPET") |
//!     +-----------+           +-----------+           +-----------+
//!           |                       |
//!      SMP Cores &             Power Ports &
//!       I/O APICs              DSDT Pointer
//!                                   |
//!                                   v
//!                             +-----------+
//!                             |   DSDT    |
//!                             | (AML \_S5)|
//!                             +-----------+
//! ```
//!
//! Subsystem features:
//! - Root System Description Pointer (RSDP) validation (ACPI 1.0 & 2.0+).
//! - 64-bit XSDT and 32-bit RSDT System Description Table catalog traversal.
//! - Multiple APIC Description Table (MADT / "APIC") parser: CPU cores & I/O APICs.
//! - Fixed ACPI Description Table (FADT / "FACP") parser: PM1a/b blocks, SMI_CMD, and reset register.
//! - Differentiated System Description Table (DSDT) & AML bytecode scanner for `\_S5` sleep states.
//! - Power management primitives: clean software shutdown ([`poweroff`]) and 4-stage reboot ([`reboot`]).

#![allow(dead_code)]
#![allow(unused_imports)]

pub mod dsdt;
pub mod fadt;
pub mod hpet;
pub mod madt;
pub mod power;
pub mod tables;

pub use dsdt::DsdtInfo;
pub use fadt::{FadtInfo, GenericAddressStructure};
pub use hpet::HpetInfo;
pub use madt::{CpuCore, IoApicInfo, MadtInfo, MAX_CPUS};
pub use power::{poweroff, reboot};
pub use tables::{AcpiTableHeader, DiscoveredTable, RsdpDescriptor, RsdpExtendedDescriptor};

use alloc::vec::Vec;
use crate::println;
use crate::sync::Spinlock;

/// Centralized ACPI Configuration and Table Catalog.
///
/// Contains discovered table metadata, parsed FADT/DSDT/MADT/HPET parameters,
/// and hardware capabilities.
#[derive(Debug, Clone)]
pub struct AcpiInfo {
    /// Physical address of the RSDP structure.
    pub rsdp_addr: u64,
    /// ACPI revision (0 for 1.0, 2 for 2.0+).
    pub revision: u8,
    /// OEM identifier string bytes.
    pub oem_id: [u8; 6],
    /// Catalog of all system description tables discovered via RSDT/XSDT.
    pub tables: Vec<DiscoveredTable>,
    /// Parsed Fixed ACPI Description Table information, if discovered.
    pub fadt: Option<FadtInfo>,
    /// Parsed Differentiated System Description Table information, if discovered.
    pub dsdt: Option<DsdtInfo>,
    /// Parsed Multiple APIC Description Table information, if discovered.
    pub madt: Option<MadtInfo>,
    /// Parsed High Precision Event Timer information, if discovered.
    pub hpet: Option<HpetInfo>,
}

/// Global ACPI configuration and hardware state protected by a Spinlock.
pub static ACPI_INFO: Spinlock<Option<AcpiInfo>> = Spinlock::new(None);

/// Global discovered ACPI MADT information (preserved for backward compatibility).
pub static MADT_INFO: Spinlock<Option<MadtInfo>> = Spinlock::new(None);

/// Returns the total number of discovered CPU cores.
///
/// Queries the discovered MADT core count, defaulting to 1 (the BSP) if uninitialized.
pub fn cpu_count() -> usize {
    MADT_INFO.lock().as_ref().map(|info| info.cpus.len()).unwrap_or(1)
}

/// Returns `true` if the system has multiple CPU cores discovered via ACPI.
pub fn is_smp_active() -> bool {
    cpu_count() > 1
}

/// Returns a clone of the global ACPI information if initialized.
pub fn get_acpi_info() -> Option<AcpiInfo> {
    ACPI_INFO.lock().clone()
}

/// Locates a discovered ACPI table by its 4-byte signature (e.g. `b"HPET"`).
///
/// Searches the table catalog collected during [`init`].
///
/// # Parameters
/// - `signature`: 4-byte ASCII signature (e.g., `b"HPET"`, `b"MCFG"`).
///
/// # Returns
/// `Some(u64)` physical address if located, or `None`.
pub fn find_table(signature: &[u8; 4]) -> Option<u64> {
    let guard = ACPI_INFO.lock();
    guard.as_ref().and_then(|info| tables::find_table(&info.tables, signature))
}

/// Parses ACPI tables from the RSDP physical address passed by firmware.
///
/// Steps executed:
/// 1. Verifies the `"RSD PTR "` signature and revision.
/// 2. Traverses the 64-bit XSDT (if revision >= 2) or 32-bit RSDT to catalog all SDTs.
/// 3. Parses MADT to discover CPU topology and I/O APICs.
/// 4. Parses FADT for PM1a/PM1b control ports, SMI commands, and DSDT pointers.
/// 5. Transitions hardware from legacy BIOS mode to ACPI mode via [`fadt::enable_acpi`].
/// 6. Scans DSDT AML bytecode for `\_S5` sleep state arguments.
/// 7. Parses HPET table if present.
/// 8. Stores populated [`AcpiInfo`] in the global [`ACPI_INFO`] lock.
///
/// # Parameters
/// - `rsdp_addr`: Physical address of the RSDP table.
pub fn init(rsdp_addr: u64) {
    println!();
    println!("--------------------------------------------------");
    println!("  [ACPI] Initializing ACPI Table & Power Subsystem ");
    println!("--------------------------------------------------");

    if rsdp_addr == 0 {
        println!("[ACPI] Warning: No RSDP physical address provided by firmware!");
        return;
    }

    let rsdp = unsafe { &*(rsdp_addr as *const RsdpDescriptor) };
    if &rsdp.signature != b"RSD PTR " {
        println!("[ACPI] Error: Invalid RSDP signature: {:?}", rsdp.signature);
        return;
    }

    let revision = rsdp.revision;
    let oem_id = rsdp.oem_id;
    let rsdt_address = rsdp.rsdt_address;
    let bsp_apic_id = madt::read_bsp_apic_id();

    println!(
        "[ACPI] RSDP verified at {:#014x} (Revision: {}, OEM: \"{}\")",
        rsdp_addr,
        revision,
        core::str::from_utf8(&oem_id).unwrap_or("???")
    );

    // 1. Discover all ACPI tables via XSDT (ACPI 2.0+) or RSDT (ACPI 1.0)
    let tables = if revision >= 2 {
        let ext_rsdp = unsafe { &*(rsdp_addr as *const RsdpExtendedDescriptor) };
        let xsdt_addr = ext_rsdp.xsdt_address;
        if xsdt_addr != 0 {
            println!("[ACPI] Traversing 64-bit XSDT at {:#014x}...", xsdt_addr);
            tables::parse_xsdt(xsdt_addr)
        } else {
            println!("[ACPI] Traversing 32-bit RSDT fallback at {:#010x}...", rsdt_address);
            tables::parse_rsdt(rsdt_address as u64)
        }
    } else {
        println!("[ACPI] Traversing 32-bit RSDT at {:#010x}...", rsdt_address);
        tables::parse_rsdt(rsdt_address as u64)
    };

    println!("[ACPI] Cataloged {} ACPI System Description Table(s):", tables.len());
    for tbl in &tables {
        println!(
            "  -> Table \"{}\" at {:#014x} (len: {} bytes, Rev: {}, OEM: \"{}\")",
            tbl.signature_str(),
            tbl.address,
            tbl.length,
            tbl.revision,
            tbl.oem_id_str()
        );
    }

    // 2. Parse Multiple APIC Description Table (MADT / "APIC")
    let madt_info = match tables::find_table(&tables, b"APIC") {
        Some(madt_phys) => match madt::parse_madt(madt_phys, bsp_apic_id) {
            Ok(m) => {
                *MADT_INFO.lock() = Some(m.clone());
                Some(m)
            }
            Err(e) => {
                println!("[ACPI] Warning: Failed to parse MADT: {}", e);
                None
            }
        },
        None => {
            println!("[ACPI] Warning: MADT ('APIC') table not found!");
            None
        }
    };

    // 3. Parse Fixed ACPI Description Table (FADT / "FACP")
    let mut dsdt_phys = 0u64;
    let fadt_info = match tables::find_table(&tables, b"FACP") {
        Some(fadt_phys) => match fadt::parse_fadt(fadt_phys) {
            Ok(f) => {
                println!(
                    "[ACPI] Parsed FADT: PM1a_CNT: 0x{:04x}, PM1b_CNT: 0x{:04x}, SMI: 0x{:04x}, DSDT: {:#014x}",
                    f.pm1a_cnt_blk, f.pm1b_cnt_blk, f.smi_cmd, f.dsdt_address
                );
                dsdt_phys = f.dsdt_address;
                // Enable ACPI hardware mode if in legacy mode
                fadt::enable_acpi(&f);
                Some(f)
            }
            Err(e) => {
                println!("[ACPI] Warning: Failed to parse FADT: {}", e);
                None
            }
        },
        None => {
            println!("[ACPI] Notice: FADT ('FACP') table not cataloged directly.");
            None
        }
    };

    // 4. Parse Differentiated System Description Table (DSDT)
    let dsdt_info = if dsdt_phys != 0 {
        match dsdt::parse_dsdt(dsdt_phys) {
            Ok(d) => Some(d),
            Err(e) => {
                println!("[ACPI] Warning: Failed to parse DSDT: {}", e);
                None
            }
        }
    } else {
        None
    };

    // 5. Parse High Precision Event Timer (HPET) Table
    let hpet_info = match tables::find_table(&tables, b"HPET") {
        Some(hpet_phys) => match hpet::parse_hpet(hpet_phys) {
            Ok(h) => {
                println!("[ACPI] Parsed HPET: MMIO Base {:#014x}, Timer Block ID: {:#x}", h.address, h.block_id);
                Some(h)
            }
            Err(e) => {
                println!("[ACPI] Warning: Failed to parse HPET: {}", e);
                None
            }
        },
        None => None,
    };

    let acpi_info = AcpiInfo {
        rsdp_addr,
        revision,
        oem_id,
        tables,
        fadt: fadt_info,
        dsdt: dsdt_info,
        madt: madt_info,
        hpet: hpet_info,
    };

    *ACPI_INFO.lock() = Some(acpi_info);
    println!("[ACPI] ACPI table parsing and power management primitives initialized.");
}

/// Runs automated self-tests verifying ACPI table parsing, FADT, DSDT, and power structures.
pub fn run_self_tests() {
    println!();
    println!("--------------------------------------------------");
    println!(" [ACPI] Running Milestone 33 ACPI & Power Tests   ");
    println!("--------------------------------------------------");

    let guard = ACPI_INFO.lock();
    let info = guard.as_ref().expect("ACPI subsystem not initialized");

    // Test 1: Verify RSDP
    println!(
        "[ACPI] Test 1: Verifying RSDP (Revision: {}, OEM: \"{}\")...",
        info.revision,
        core::str::from_utf8(&info.oem_id).unwrap_or("???")
    );
    assert!(info.rsdp_addr != 0, "RSDP address must be non-zero");

    // Test 2: Verify System Description Tables catalog
    println!("[ACPI] Test 2: Verifying Table Catalog ({} tables found)...", info.tables.len());
    assert!(!info.tables.is_empty(), "ACPI should discover at least 1 description table");

    // Test 3: Verify MADT CPU enumeration
    println!("[ACPI] Test 3: Verifying MADT CPU topology...");
    let madt = info.madt.as_ref().expect("MADT info must be present");
    assert!(!madt.cpus.is_empty(), "MADT should discover at least 1 CPU (BSP)");
    assert!(madt.cpus.iter().any(|c| c.is_bsp), "MADT must identify the Bootstrap Processor (BSP)");

    // Test 4: Verify FADT Power Registers
    println!("[ACPI] Test 4: Verifying FADT Power Management Registers...");
    let fadt = info.fadt.as_ref().expect("FADT info must be present");
    assert!(fadt.dsdt_address != 0, "FADT must point to a valid DSDT physical address");
    assert!(fadt.pm1a_cnt_blk != 0, "FADT must expose PM1a_CNT_BLK port");

    // Test 5: Verify DSDT and S5 Sleep State
    println!("[ACPI] Test 5: Verifying DSDT and \\_S5 Soft-Off state...");
    let dsdt = info.dsdt.as_ref().expect("DSDT info must be present");
    assert!(dsdt.length > 0, "DSDT length must be greater than zero");
    println!(
        "[ACPI]   -> DSDT at {:#014x} (len: {} bytes), S5: SLP_TYPa 0x{:02x}, SLP_TYPb 0x{:02x}",
        dsdt.address, dsdt.length, dsdt.slp_typa, dsdt.slp_typb
    );

    // Test 6: Verify Power Management Primitives are wired
    println!("[ACPI] Test 6: Verifying Power Management Primitives (supports reset reg: {})...", fadt.supports_reset_reg());

    println!("[ACPI] All Milestone 33 ACPI & Power self-tests passed successfully!");
}
