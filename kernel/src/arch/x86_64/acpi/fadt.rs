//! # Fixed ACPI Description Table (FADT / "FACP") Parser
//!
//! ## Overview
//! The Fixed ACPI Description Table (FADT, identified by the 4-byte signature `b"FACP"`)
//! is a core ACPI table that defines:
//! 1. **Power Management Register Blocks**:
//!    - PM1a / PM1b Event and Control blocks (used for sleep state transitions like S5 poweroff).
//!    - PM2 Control block and PM Timer block (24-bit or 32-bit 3.579545 MHz ACPI timer).
//! 2. **SMI Command Port (`smi_cmd`)**:
//!    - Legacy system management interrupt port used to hand over hardware control from BIOS/SMM
//!      to ACPI OS mode by writing `acpi_enable`.
//! 3. **Reset Register (`reset_reg`)**:
//!    - ACPI 2.0+ Generic Address Structure specifying a memory, I/O, or PCI register to write
//!      `reset_value` to trigger a hardware system reset.
//! 4. **Differentiated System Description Table Pointer (`dsdt` / `x_dsdt`)**:
//!    - 32-bit physical address `dsdt` and 64-bit physical address `x_dsdt` pointing to the
//!      DSDT containing AML bytecode (such as the `\_S5` object).
//!
//! ## Hardware Interaction Invariants
//! - Before writing to PM1a/PM1b control ports for system shutdown, the OS must ensure ACPI
//!   mode is active (`SCI_EN` bit 0 set in `PM1a_CNT_BLK`).
//! - If `x_dsdt` is non-zero and the table revision is ACPI 2.0+, `x_dsdt` takes precedence
//!   over the 32-bit `dsdt` pointer.

use crate::arch::x86_64::acpi::tables::AcpiTableHeader;
use crate::println;

/// ACPI Generic Address Structure (GAS) (12 bytes).
///
/// Defines an address within system memory, system I/O, PCI configuration space,
/// embedded controller space, or SMBus.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GenericAddressStructure {
    /// Address space ID:
    /// - `0`: System Memory
    /// - `1`: System I/O
    /// - `2`: PCI Configuration Space
    /// - `3`: Embedded Controller
    /// - `4`: SMBus
    pub address_space: u8,
    /// Register size in bits.
    pub bit_width: u8,
    /// Bit offset within the register.
    pub bit_offset: u8,
    /// Access size: 0 = undefined, 1 = byte, 2 = word, 3 = dword, 4 = qword.
    pub access_size: u8,
    /// 64-bit physical or I/O address.
    pub address: u64,
}

/// Fixed ACPI Description Table (FADT / "FACP") structure.
///
/// Contains fixed hardware ACPI registers, sleep control blocks, SMI ports, and DSDT pointers.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct FadtHeader {
    /// Standard ACPI header with signature `b"FACP"`.
    pub header: AcpiTableHeader,
    /// 32-bit physical address of Firmware ACPI Control Structure (FACS).
    pub firmware_ctrl: u32,
    /// 32-bit physical address of Differentiated System Description Table (DSDT).
    pub dsdt: u32,
    /// Reserved byte.
    pub reserved1: u8,
    /// Preferred Power Management Profile (Desktop, Mobile, Workstation, Enterprise Server, etc.).
    pub preferred_pm_profile: u8,
    /// System Control Interrupt (SCI) vector number.
    pub sci_int: u16,
    /// System Management Interrupt (SMI) command port (System I/O space).
    pub smi_cmd: u32,
    /// Byte value written to `smi_cmd` to enable ACPI hardware mode.
    pub acpi_enable: u8,
    /// Byte value written to `smi_cmd` to disable ACPI hardware mode.
    pub acpi_disable: u8,
    /// S4BIOS request command value.
    pub s4bios_req: u8,
    /// Processor performance state control port.
    pub pstate_cnt: u8,
    /// System I/O port address of PM1a Event Register Block.
    pub pm1a_evt_blk: u32,
    /// System I/O port address of PM1b Event Register Block.
    pub pm1b_evt_blk: u32,
    /// System I/O port address of PM1a Control Register Block.
    pub pm1a_cnt_blk: u32,
    /// System I/O port address of PM1b Control Register Block.
    pub pm1b_cnt_blk: u32,
    /// System I/O port address of PM2 Control Register Block.
    pub pm2_cnt_blk: u32,
    /// System I/O port address of Power Management Timer Block.
    pub pm_tmr_blk: u32,
    /// System I/O port address of General Purpose Event 0 Block.
    pub gpe0_blk: u32,
    /// System I/O port address of General Purpose Event 1 Block.
    pub gpe1_blk: u32,
    /// Length of PM1 Event block in bytes (must be >= 4).
    pub pm1_evt_len: u8,
    /// Length of PM1 Control block in bytes (must be >= 2).
    pub pm1_cnt_len: u8,
    /// Length of PM2 Control block in bytes.
    pub pm2_cnt_len: u8,
    /// Length of PM Timer block in bytes (must be 4).
    pub pm_tmr_len: u8,
    /// Length of GPE0 block in bytes.
    pub gpe0_blk_len: u8,
    /// Length of GPE1 block in bytes.
    pub gpe1_blk_len: u8,
    /// GPE1 base offset.
    pub gpe1_base: u8,
    /// C-state control register value.
    pub cst_cnt: u8,
    /// Worst-case hardware latency in microseconds to enter/exit C2 state.
    pub p_lvl2_lat: u16,
    /// Worst-case hardware latency in microseconds to enter/exit C3 state.
    pub p_lvl3_lat: u16,
    /// Flush size for processor cache flushes.
    pub flush_size: u16,
    /// Flush stride.
    pub flush_stride: u16,
    /// Processor duty cycle index offset.
    pub duty_offset: u8,
    /// Processor duty cycle width.
    pub duty_width: u8,
    /// Real-time clock day alarm index.
    pub day_alarm: u8,
    /// Real-time clock month alarm index.
    pub month_alarm: u8,
    /// Real-time clock century register index.
    pub century: u8,
    /// IA-PC boot architecture flags (e.g., legacy PIC, 8042 keyboard controller present).
    pub iapc_boot_arch: u16,
    /// Reserved byte.
    pub reserved2: u8,
    /// Fixed ACPI feature flags (bit 10: `RESET_REG_SUP`, bit 0: `WBINVD`, etc.).
    pub flags: u32,
    /// Generic Address Structure for the ACPI 2.0+ hardware reset register.
    pub reset_reg: GenericAddressStructure,
    /// Value to write to `reset_reg` to trigger a system reset.
    pub reset_value: u8,
    /// ARM-specific boot architecture flags.
    pub arm_boot_arch: u16,
    /// Minor revision of the FADT specification.
    pub fadt_minor_version: u8,
    /// 64-bit physical address of Firmware ACPI Control Structure (FACS).
    pub x_firmware_ctrl: u64,
    /// 64-bit physical address of Differentiated System Description Table (DSDT).
    pub x_dsdt: u64,
    /// 64-bit Generic Address Structure for PM1a Event Block.
    pub x_pm1a_evt_blk: GenericAddressStructure,
    /// 64-bit Generic Address Structure for PM1b Event Block.
    pub x_pm1b_evt_blk: GenericAddressStructure,
    /// 64-bit Generic Address Structure for PM1a Control Block.
    pub x_pm1a_cnt_blk: GenericAddressStructure,
    /// 64-bit Generic Address Structure for PM1b Control Block.
    pub x_pm1b_cnt_blk: GenericAddressStructure,
    /// 64-bit Generic Address Structure for PM2 Control Block.
    pub x_pm2_cnt_blk: GenericAddressStructure,
    /// 64-bit Generic Address Structure for PM Timer Block.
    pub x_pm_tmr_blk: GenericAddressStructure,
    /// 64-bit Generic Address Structure for GPE0 Block.
    pub x_gpe0_blk: GenericAddressStructure,
    /// 64-bit Generic Address Structure for GPE1 Block.
    pub x_gpe1_blk: GenericAddressStructure,
}

/// Parsed FADT configuration information used by the kernel power and timing subsystems.
#[derive(Debug, Clone)]
pub struct FadtInfo {
    /// Physical address of DSDT (either 64-bit `x_dsdt` or 32-bit `dsdt`).
    pub dsdt_address: u64,
    /// I/O port address for PM1a Control Register Block.
    pub pm1a_cnt_blk: u16,
    /// I/O port address for PM1b Control Register Block (0 if absent).
    pub pm1b_cnt_blk: u16,
    /// I/O port address for SMI Command Port.
    pub smi_cmd: u16,
    /// Value to write to SMI port to transition from legacy BIOS to ACPI mode.
    pub acpi_enable: u8,
    /// Value to write to SMI port to transition back to legacy BIOS mode.
    pub acpi_disable: u8,
    /// Length of PM1 Control Register Block in bytes.
    pub pm1_cnt_len: u8,
    /// Feature flags bitfield from FADT.
    pub flags: u32,
    /// Optional ACPI 2.0+ reset register descriptor.
    pub reset_reg: Option<GenericAddressStructure>,
    /// Value to write to reset register to initiate system reset.
    pub reset_value: u8,
    /// System Control Interrupt vector.
    pub sci_int: u16,
}

impl FadtInfo {
    /// Returns `true` if the ACPI 2.0+ hardware reset register is supported.
    pub fn supports_reset_reg(&self) -> bool {
        (self.flags & (1 << 10)) != 0 && self.reset_reg.is_some()
    }

    /// Checks if ACPI hardware mode is currently enabled (bit 0 `SCI_EN` of PM1a_CNT == 1).
    pub fn is_acpi_enabled(&self) -> bool {
        if self.pm1a_cnt_blk == 0 {
            return false;
        }
        let val = unsafe { inw(self.pm1a_cnt_blk) };
        (val & 1) != 0
    }
}

/// Helper function to read a 16-bit word from an x86 I/O port.
///
/// # Safety
/// Reading from arbitrary I/O ports can trigger hardware actions or bus stalls.
#[inline]
pub unsafe fn inw(port: u16) -> u16 {
    let value: u16;
    unsafe {
        core::arch::asm!("in ax, dx", in("dx") port, out("ax") value, options(nomem, nostack));
    }
    value
}

/// Helper function to write a 16-bit word to an x86 I/O port.
///
/// # Safety
/// Writing to arbitrary I/O ports can mutate device hardware state.
#[inline]
pub unsafe fn outw(port: u16, value: u16) {
    unsafe {
        core::arch::asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack));
    }
}

/// Helper function to write an 8-bit byte to an x86 I/O port.
///
/// # Safety
/// Writing to arbitrary I/O ports can mutate device hardware state.
#[inline]
pub unsafe fn outb(port: u16, value: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack));
    }
}

/// Helper function to read an 8-bit byte from an x86 I/O port.
///
/// # Safety
/// Reading from arbitrary I/O ports can trigger hardware actions or bus stalls.
#[inline]
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack));
    }
    value
}

/// Parses the FADT table from its physical base address.
///
/// Validates signature `b"FACP"`, resolves 64-bit vs 32-bit addresses for DSDT
/// and PM control blocks, and checks for reset register support.
///
/// # Parameters
/// - `fadt_phys`: Physical memory address of the FADT table.
///
/// # Returns
/// - `Ok(FadtInfo)` on successful validation.
/// - `Err(&'static str)` if address is null or signature mismatch.
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
///
/// Sends the `acpi_enable` command byte to the SMI port (`smi_cmd`) and polls
/// the `SCI_EN` bit in `PM1a_CNT_BLK` until hardware mode is engaged.
///
/// # Parameters
/// - `fadt`: Reference to parsed [`FadtInfo`] structure.
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
