//! Fixed ACPI Description Table (FADT / "FACP") Parser.
//!
//! Provides power management register blocks, ACPI hardware enable sequences,
//! and system reset register definitions.

use crate::arch::x86_64::acpi::tables::AcpiTableHeader;
use crate::println;

/// ACPI Generic Address Structure (GAS) (12 bytes).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GenericAddressStructure {
    pub address_space: u8, // 0 = System Memory, 1 = System I/O, 2 = PCI Config, etc.
    pub bit_width: u8,
    pub bit_offset: u8,
    pub access_size: u8,
    pub address: u64,
}

/// FADT Header (Fixed ACPI Description Table).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct FadtHeader {
    pub header: AcpiTableHeader, // Signature "FACP"
    pub firmware_ctrl: u32,      // 32-bit physical address of FACS
    pub dsdt: u32,               // 32-bit physical address of DSDT
    pub reserved1: u8,
    pub preferred_pm_profile: u8,
    pub sci_int: u16,            // SCI interrupt vector
    pub smi_cmd: u32,            // SMI command port (System I/O)
    pub acpi_enable: u8,         // Value to write to smi_cmd to enable ACPI
    pub acpi_disable: u8,        // Value to write to smi_cmd to disable ACPI
    pub s4bios_req: u8,
    pub pstate_cnt: u8,
    pub pm1a_evt_blk: u32,       // Port for PM1a Event Block
    pub pm1b_evt_blk: u32,       // Port for PM1b Event Block
    pub pm1a_cnt_blk: u32,       // Port for PM1a Control Block
    pub pm1b_cnt_blk: u32,       // Port for PM1b Control Block
    pub pm2_cnt_blk: u32,
    pub pm_tmr_blk: u32,         // Port for PM Timer Block
    pub gpe0_blk: u32,
    pub gpe1_blk: u32,
    pub pm1_evt_len: u8,
    pub pm1_cnt_len: u8,
    pub pm2_cnt_len: u8,
    pub pm_tmr_len: u8,
    pub gpe0_blk_len: u8,
    pub gpe1_blk_len: u8,
    pub gpe1_base: u8,
    pub cst_cnt: u8,
    pub p_lvl2_lat: u16,
    pub p_lvl3_lat: u16,
    pub flush_size: u16,
    pub flush_stride: u16,
    pub duty_offset: u8,
    pub duty_width: u8,
    pub day_alarm: u8,
    pub month_alarm: u8,
    pub century: u8,
    pub iapc_boot_arch: u16,
    pub reserved2: u8,
    pub flags: u32,              // Bit 10: RESET_REG_SUP
    pub reset_reg: GenericAddressStructure, // Reset Register
    pub reset_value: u8,         // Value to write to reset_reg
    pub arm_boot_arch: u16,
    pub fadt_minor_version: u8,
    pub x_firmware_ctrl: u64,    // 64-bit physical address of FACS
    pub x_dsdt: u64,             // 64-bit physical address of DSDT
    pub x_pm1a_evt_blk: GenericAddressStructure,
    pub x_pm1b_evt_blk: GenericAddressStructure,
    pub x_pm1a_cnt_blk: GenericAddressStructure,
    pub x_pm1b_cnt_blk: GenericAddressStructure,
    pub x_pm2_cnt_blk: GenericAddressStructure,
    pub x_pm_tmr_blk: GenericAddressStructure,
    pub x_gpe0_blk: GenericAddressStructure,
    pub x_gpe1_blk: GenericAddressStructure,
}

/// Parsed FADT configuration information.
#[derive(Debug, Clone)]
pub struct FadtInfo {
    pub dsdt_address: u64,
    pub pm1a_cnt_blk: u16,
    pub pm1b_cnt_blk: u16,
    pub smi_cmd: u16,
    pub acpi_enable: u8,
    pub acpi_disable: u8,
    pub pm1_cnt_len: u8,
    pub flags: u32,
    pub reset_reg: Option<GenericAddressStructure>,
    pub reset_value: u8,
    pub sci_int: u16,
}

impl FadtInfo {
    /// Returns true if the ACPI 2.0+ hardware reset register is supported.
    pub fn supports_reset_reg(&self) -> bool {
        (self.flags & (1 << 10)) != 0 && self.reset_reg.is_some()
    }

    /// Checks if ACPI hardware mode is currently enabled (bit 0 of PM1a_CNT == 1).
    pub fn is_acpi_enabled(&self) -> bool {
        if self.pm1a_cnt_blk == 0 {
            return false;
        }
        let val = unsafe { inw(self.pm1a_cnt_blk) };
        (val & 1) != 0
    }
}

/// Helper function to read a 16-bit word from an I/O port.
#[inline]
pub unsafe fn inw(port: u16) -> u16 {
    let value: u16;
    unsafe {
        core::arch::asm!("in ax, dx", in("dx") port, out("ax") value, options(nomem, nostack));
    }
    value
}

/// Helper function to write a 16-bit word to an I/O port.
#[inline]
pub unsafe fn outw(port: u16, value: u16) {
    unsafe {
        core::arch::asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack));
    }
}

/// Helper function to write an 8-bit byte to an I/O port.
#[inline]
pub unsafe fn outb(port: u16, value: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack));
    }
}

/// Helper function to read an 8-bit byte from an I/O port.
#[inline]
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack));
    }
    value
}

/// Parses the FADT table at `fadt_phys`.
pub fn parse_fadt(fadt_phys: u64) -> Result<FadtInfo, &'static str> {
    if fadt_phys == 0 {
        return Err("Null FADT physical address");
    }

    let fadt = unsafe { &*(fadt_phys as *const FadtHeader) };
    if &fadt.header.signature != b"FACP" {
        return Err("Invalid FADT signature");
    }

    // Determine DSDT address: prefer 64-bit X_DSDT if table length supports it and non-zero
    let dsdt_address = if fadt.header.length() >= 148 && fadt.x_dsdt != 0 {
        fadt.x_dsdt
    } else {
        fadt.dsdt as u64
    };

    // Determine PM1a_CNT_BLK port: prefer X_PM1a_CNT_BLK if available and in System I/O space (0x01)
    let pm1a_cnt_blk = if fadt.header.length() >= 172 && fadt.x_pm1a_cnt_blk.address != 0 {
        fadt.x_pm1a_cnt_blk.address as u16
    } else {
        fadt.pm1a_cnt_blk as u16
    };

    // Determine PM1b_CNT_BLK port
    let pm1b_cnt_blk = if fadt.header.length() >= 184 && fadt.x_pm1b_cnt_blk.address != 0 {
        fadt.x_pm1b_cnt_blk.address as u16
    } else {
        fadt.pm1b_cnt_blk as u16
    };

    let flags = fadt.flags;
    let reset_reg = if (flags & (1 << 10)) != 0 && fadt.header.length() >= 128 && fadt.reset_reg.address != 0 {
        Some(fadt.reset_reg)
    } else {
        None
    };

    let info = FadtInfo {
        dsdt_address,
        pm1a_cnt_blk,
        pm1b_cnt_blk,
        smi_cmd: fadt.smi_cmd as u16,
        acpi_enable: fadt.acpi_enable,
        acpi_disable: fadt.acpi_disable,
        pm1_cnt_len: fadt.pm1_cnt_len,
        flags,
        reset_reg,
        reset_value: fadt.reset_value,
        sci_int: fadt.sci_int,
    };

    Ok(info)
}

/// Enables ACPI hardware mode if currently disabled in legacy firmware mode.
pub fn enable_acpi(fadt: &FadtInfo) {
    if fadt.is_acpi_enabled() {
        return;
    }

    if fadt.smi_cmd != 0 && fadt.acpi_enable != 0 {
        println!("[ACPI] Enabling ACPI hardware mode (writing 0x{:02x} to SMI port 0x{:04x})...", fadt.acpi_enable, fadt.smi_cmd);
        unsafe {
            outb(fadt.smi_cmd, fadt.acpi_enable);
        }

        // Wait until bit 0 (SCI_EN) in PM1a_CNT becomes 1
        for _ in 0..10_000 {
            if fadt.is_acpi_enabled() {
                println!("[ACPI] ACPI hardware mode active (SCI_EN set).");
                return;
            }
            core::hint::spin_loop();
        }

        println!("[ACPI] Warning: Timed out waiting for SCI_EN after writing ACPI_ENABLE.");
    }
}
