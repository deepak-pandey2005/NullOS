//! # Multiple APIC Description Table (MADT / "APIC") Parser
//!
//! ## Overview
//! The Multiple APIC Description Table (MADT, signature `b"APIC"`) provides the kernel
//! with the hardware topology required for multi-core symmetric multiprocessing (SMP)
//! and advanced interrupt routing.
//!
//! Specifically, the MADT communicates:
//! 1. **Local APIC Base Address**: 32-bit physical address of the per-core Local APIC
//!    registers (typically `0xFEE00000`).
//! 2. **Processor Local APIC Records (Type 0)**:
//!    - One entry per logical or physical CPU core.
//!    - Identifies the ACPI Processor UID, hardware Local APIC ID, and enablement flags
//!      (bit 0: Enabled, bit 1: Online Capable).
//! 3. **I/O APIC Records (Type 1)**:
//!    - System I/O APIC chips responsible for routing external hardware IRQs (e.g., keyboard,
//!      PCI devices, timer) to Local APICs across the system bus.
//!    - Records base MMIO address (typically `0xFEC00000`) and the Global System Interrupt (GSI) base.
//! 4. **Interrupt Source Overrides (Type 2)** & **NMI Sources (Type 4)**:
//!    - Remappings of legacy ISA IRQs (such as IRQ 0 timer -> GSI 2).
//!
//! ## Multiprocessor Invariants
//! - The Bootstrap Processor (BSP) is identified by matching each discovered core's APIC ID
//!   with the BSP APIC ID reported by `CPUID` Leaf 1 `EBX[24..31]`.
//! - Only cores with the Enabled or Online-Capable flag set are scheduled for AP initialization.

use alloc::vec::Vec;
use crate::arch::x86_64::acpi::tables::AcpiTableHeader;
use crate::println;

/// Maximum number of CPUs supported concurrently by NullOS.
pub const MAX_CPUS: usize = 16;

/// MADT (Multiple APIC Description Table) main header structure.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MadtHeader {
    /// Standard ACPI table header with signature `b"APIC"`.
    pub header: AcpiTableHeader,
    /// 32-bit physical memory address of the Local APIC MMIO register aperture.
    pub local_apic_address: u32,
    /// Flags (bit 0: 1 = PC/AT dual-8259 PIC installed, 0 = no legacy PIC).
    pub flags: u32,
}

/// Discovered CPU Core Information.
#[derive(Debug, Clone, Copy)]
pub struct CpuCore {
    /// Zero-based logical CPU index within NullOS.
    pub cpu_id: usize,
    /// ACPI Processor Unique Identifier (from DSDT/SSDT).
    pub processor_uid: u8,
    /// Hardware Local APIC ID used for physical IPI targeting.
    pub apic_id: u8,
    /// Whether the processor is enabled and ready to receive code.
    pub enabled: bool,
    /// Whether this core is the current Bootstrap Processor (BSP).
    pub is_bsp: bool,
}

/// Discovered I/O APIC Information.
#[derive(Debug, Clone, Copy)]
pub struct IoApicInfo {
    /// Hardware I/O APIC ID.
    pub io_apic_id: u8,
    /// Physical MMIO base address for reading/writing I/O APIC registers.
    pub address: u32,
    /// Global System Interrupt (GSI) index corresponding to I/O APIC input pin 0.
    pub gsi_base: u32,
}

/// Parsed ACPI MADT Subsystem Configuration.
#[derive(Debug, Clone)]
pub struct MadtInfo {
    /// Physical address of the Local APIC MMIO region.
    pub local_apic_addr: u32,
    /// APIC ID of the Bootstrap Processor that started the kernel.
    pub bsp_apic_id: u8,
    /// List of all discovered CPU cores.
    pub cpus: Vec<CpuCore>,
    /// List of all discovered I/O APICs.
    pub io_apics: Vec<IoApicInfo>,
}

/// Reads the BSP Local APIC ID from CPUID (Leaf 1, EBX[24..31]).
///
/// Executes CPUID with `EAX = 1` and extracts the initial 8-bit Local APIC ID
/// assigned to this CPU core by hardware reset.
///
/// # Returns
/// 8-bit Local APIC ID of the executing core.
pub fn read_bsp_apic_id() -> u8 {
    let mut ebx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "mov {0:e}, ebx",
            "pop rbx",
            out(reg) ebx,
            lateout("eax") _,
            lateout("ecx") _,
            lateout("edx") _,
            options(nomem)
        );
    }
    ((ebx >> 24) & 0xFF) as u8
}

/// Parses the MADT entries and collects all CPU cores and I/O APICs.
///
/// Iterates through variable-length APIC structures following the [`MadtHeader`],
/// extracting Type 0 (Processor Local APIC) and Type 1 (I/O APIC) descriptors.
///
/// # Parameters
/// - `madt_addr`: Physical address of the MADT table.
/// - `bsp_apic_id`: Hardware APIC ID of the Bootstrap Processor.
///
/// # Returns
/// - `Ok(MadtInfo)` with populated CPU and I/O APIC lists.
/// - `Err(&'static str)` if address is null or signature is invalid.
pub fn parse_madt(madt_addr: u64, bsp_apic_id: u8) -> Result<MadtInfo, &'static str> {
    if madt_addr == 0 {
        return Err("Null MADT physical address");
    }

    let madt = unsafe { &*(madt_addr as *const MadtHeader) };
    if &madt.header.signature != b"APIC" {
        return Err("Invalid MADT signature");
    }

    let total_len = madt.header.length();
    let local_apic_addr = madt.local_apic_address;
    let madt_flags = madt.flags;

    println!(
        "[ACPI] Local APIC physical address: {:#010x}, Dual PIC flags: {:#x}",
        local_apic_addr, madt_flags
    );

    let mut cpus: Vec<CpuCore> = Vec::new();
    let mut io_apics: Vec<IoApicInfo> = Vec::new();

    let mut offset = core::mem::size_of::<MadtHeader>();
    let base_ptr = madt_addr as *const u8;

    while offset + 2 <= total_len {
        let entry_type = unsafe { *base_ptr.add(offset) };
        let entry_len = unsafe { *base_ptr.add(offset + 1) } as usize;

        if entry_len < 2 || offset + entry_len > total_len {
            break;
        }

        match entry_type {
            0 => {
                // Processor Local APIC Entry (Type 0, Length 8)
                if entry_len >= 8 {
                    let proc_uid = unsafe { *base_ptr.add(offset + 2) };
                    let apic_id = unsafe { *base_ptr.add(offset + 3) };
                    let flags = unsafe {
                        let ptr = base_ptr.add(offset + 4) as *const u32;
                        ptr.read_unaligned()
                    };

                    let enabled = (flags & 0x01) != 0 || (flags & 0x02) != 0;
                    let is_bsp = apic_id == bsp_apic_id;

                    let cpu_id = cpus.len();
                    cpus.push(CpuCore {
                        cpu_id,
                        processor_uid: proc_uid,
                        apic_id,
                        enabled,
                        is_bsp,
                    });
                }
            }
            1 => {
                // I/O APIC Entry (Type 1, Length 12)
                if entry_len >= 12 {
                    let io_apic_id = unsafe { *base_ptr.add(offset + 2) };
                    let io_apic_addr = unsafe {
                        let ptr = base_ptr.add(offset + 4) as *const u32;
                        ptr.read_unaligned()
                    };
                    let gsi_base = unsafe {
                        let ptr = base_ptr.add(offset + 8) as *const u32;
                        ptr.read_unaligned()
                    };

                    io_apics.push(IoApicInfo {
                        io_apic_id,
                        address: io_apic_addr,
                        gsi_base,
                    });
                }
            }
            _ => {
                // Ignore other entry types (Interrupt Source Override, NMI, etc.)
            }
        }

        offset += entry_len;
    }

    println!("[ACPI] Discovered {} CPU cores and {} I/O APIC(s):", cpus.len(), io_apics.len());
    for cpu in &cpus {
        println!(
            "  - CPU #{} (APIC ID: {}, Processor UID: {}, Status: {}{})",
            cpu.cpu_id,
            cpu.apic_id,
            cpu.processor_uid,
            if cpu.enabled { "Enabled" } else { "Disabled" },
            if cpu.is_bsp { " [BSP]" } else { " [AP]" }
        );
    }

    for ioapic in &io_apics {
        println!(
            "  - I/O APIC #{} (MMIO: {:#010x}, GSI Base: {})",
            ioapic.io_apic_id, ioapic.address, ioapic.gsi_base
        );
    }

    Ok(MadtInfo {
        local_apic_addr,
        bsp_apic_id,
        cpus,
        io_apics,
    })
}
