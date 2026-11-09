//! Multiple APIC Description Table (MADT / "APIC") Parser.
//!
//! Enumerates physical and logical CPU cores (Processor Local APICs) and I/O APICs.

use alloc::vec::Vec;
use crate::arch::x86_64::acpi::tables::AcpiTableHeader;
use crate::println;

/// Maximum number of CPUs supported by NullOS.
pub const MAX_CPUS: usize = 16;

/// MADT (Multiple APIC Description Table) main header.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MadtHeader {
    pub header: AcpiTableHeader, // Signature "APIC"
    pub local_apic_address: u32, // Physical base address of Local APIC MMIO
    pub flags: u32,              // PC/AT Dual-PIC compatibility flags
}

/// Discovered CPU Core Information.
#[derive(Debug, Clone, Copy)]
pub struct CpuCore {
    pub cpu_id: usize,
    pub processor_uid: u8,
    pub apic_id: u8,
    pub enabled: bool,
    pub is_bsp: bool,
}

/// Discovered I/O APIC Information.
#[derive(Debug, Clone, Copy)]
pub struct IoApicInfo {
    pub io_apic_id: u8,
    pub address: u32,
    pub gsi_base: u32,
}

/// Parsed ACPI MADT Subsystem Configuration.
#[derive(Debug, Clone)]
pub struct MadtInfo {
    pub local_apic_addr: u32,
    pub bsp_apic_id: u8,
    pub cpus: Vec<CpuCore>,
    pub io_apics: Vec<IoApicInfo>,
}

/// Reads the BSP Local APIC ID from CPUID (Leaf 1, EBX[24..31]).
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
