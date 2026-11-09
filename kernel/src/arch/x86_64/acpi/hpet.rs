//! # High Precision Event Timer (HPET) Table Parser
//!
//! ## Overview
//! The High Precision Event Timer (HPET) specification defines an accurate, monotonic
//! hardware clock source designed by Intel and Microsoft to replace the legacy 8254
//! Programmable Interval Timer (PIT) and Real-Time Clock (RTC) periodic interrupts.
//!
//! The ACPI HPET table (signature `b"HPET"`) defines:
//! 1. **Base MMIO Address**: The 64-bit physical memory address of the HPET registers
//!    (standard base: `0xFED00000`).
//! 2. **Block ID**: Capabilities bitmask indicating vendor ID, number of comparator timers
//!    (at least 3, up to 32), and counter size (32-bit or 64-bit).
//! 3. **Main Counter Tick Period**: Minimum clock tick count in periodic mode.
//!
//! ## Hardware Characteristics
//! - The HPET main counter operates at a minimum frequency of 10 MHz (<= 100 ns per tick),
//!   typically around 14.31818 MHz.
//! - The main counter increments monotonically from system start, providing high-resolution
//!   nanosecond-scale timestamps for task profiling and preemptive scheduling.

use crate::arch::x86_64::acpi::fadt::GenericAddressStructure;
use crate::arch::x86_64::acpi::tables::AcpiTableHeader;

/// HPET Description Table Header (56 bytes).
///
/// Complies with the IA-PC HPET Specification revision 1.0a.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct HpetHeader {
    /// Standard ACPI header with signature `b"HPET"`.
    pub header: AcpiTableHeader,
    /// Hardware identification: Timer block ID, vendor ID, comparator count, 64-bit flag.
    pub event_timer_block_id: u32,
    /// Generic Address Structure specifying MMIO physical address of HPET registers.
    pub base_address: GenericAddressStructure,
    /// HPET sequence number (0 for primary HPET).
    pub hpet_number: u8,
    /// Main counter minimum clock tick in periodic mode.
    pub main_counter_min_clock_tick: u16,
    /// Page protection guarantee flags (0 = none, 1 = 4KB, 2 = 64KB) and OEM attributes.
    pub page_protection_and_oem: u8,
}

/// Parsed HPET configuration information.
#[derive(Debug, Clone, Copy)]
pub struct HpetInfo {
    /// 64-bit physical address of the HPET MMIO register space.
    pub address: u64,
    /// HPET controller sequence number (0 is the primary platform timer).
    pub hpet_number: u8,
    /// Raw Event Timer Block ID register value.
    pub block_id: u32,
    /// Main counter minimum clock tick count.
    pub min_tick: u16,
}

/// Parses the HPET table at physical address `hpet_phys`.
///
/// Validates the `b"HPET"` table signature and extracts the MMIO base address
/// and hardware capabilities.
///
/// # Parameters
/// - `hpet_phys`: Physical memory address of the HPET description table.
///
/// # Returns
/// - `Ok(HpetInfo)` on successful validation.
/// - `Err(&'static str)` if address is null or signature does not match `b"HPET"`.
pub fn parse_hpet(hpet_phys: u64) -> Result<HpetInfo, &'static str> {
    if hpet_phys == 0 {
        return Err("Null HPET address");
    }

    let hpet = unsafe { &*(hpet_phys as *const HpetHeader) };
    if &hpet.header.signature != b"HPET" {
        return Err("Invalid HPET signature");
    }

    Ok(HpetInfo {
        address: hpet.base_address.address,
        hpet_number: hpet.hpet_number,
        block_id: hpet.event_timer_block_id,
        min_tick: hpet.main_counter_min_clock_tick,
    })
}

