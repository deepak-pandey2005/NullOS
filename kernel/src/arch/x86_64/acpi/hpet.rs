//! High Precision Event Timer (HPET) Table Parser.

use crate::arch::x86_64::acpi::fadt::GenericAddressStructure;
use crate::arch::x86_64::acpi::tables::AcpiTableHeader;

/// HPET Description Table Header (56 bytes).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct HpetHeader {
    pub header: AcpiTableHeader, // "HPET"
    pub event_timer_block_id: u32,
    pub base_address: GenericAddressStructure,
    pub hpet_number: u8,
    pub main_counter_min_clock_tick: u16,
    pub page_protection_and_oem: u8,
}

/// Parsed HPET configuration information.
#[derive(Debug, Clone, Copy)]
pub struct HpetInfo {
    pub address: u64,
    pub hpet_number: u8,
    pub block_id: u32,
    pub min_tick: u16,
}

/// Parses the HPET table at `hpet_phys`.
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

