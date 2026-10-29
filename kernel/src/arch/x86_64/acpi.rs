//! Advanced Configuration and Power Interface (ACPI) & MADT Parser.
//!
//! Discovers motherboard ACPI tables starting from the RSDP, locates the
//! Multiple APIC Description Table (MADT / "APIC"), and enumerates all
//! physical and logical CPU cores (Processor Local APICs) and I/O APICs.

#![allow(dead_code)]

use alloc::vec::Vec;
use crate::println;
use crate::sync::Spinlock;

/// Maximum number of CPUs supported by NullOS.
pub const MAX_CPUS: usize = 16;

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

/// Root System Description Pointer (RSDP) descriptor.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct RsdpDescriptor {
    pub signature: [u8; 8], // "RSD PTR "
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub revision: u8,       // 0 = ACPI 1.0, 2 = ACPI 2.0+
    pub rsdt_address: u32,  // 32-bit physical address of RSDT
}

/// Extended RSDP descriptor for ACPI 2.0+.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct RsdpExtendedDescriptor {
    pub base: RsdpDescriptor,
    pub length: u32,
    pub xsdt_address: u64,  // 64-bit physical address of XSDT
    pub extended_checksum: u8,
    pub reserved: [u8; 3],
}

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

/// Global discovered ACPI MADT information.
pub static MADT_INFO: Spinlock<Option<MadtInfo>> = Spinlock::new(None);

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

/// Parses ACPI tables from the RSDP physical address passed by firmware.
pub fn init(rsdp_addr: u64) {
    println!();
    println!("----------------------------------------");
    println!("  [ACPI] Discovering Hardware Topology  ");
    println!("----------------------------------------");

    if rsdp_addr == 0 {
        println!("[ACPI] Warning: No RSDP physical address provided by firmware!");
        return;
    }

    let rsdp = unsafe { &*(rsdp_addr as *const RsdpDescriptor) };
    if &rsdp.signature != b"RSD PTR " {
        println!("[ACPI] Error: Invalid RSDP signature: {:?}", rsdp.signature);
        return;
    }

    let bsp_apic_id = read_bsp_apic_id();
    let revision = rsdp.revision;
    let rsdt_address = rsdp.rsdt_address;
    println!(
        "[ACPI] RSDP verified at {:#018x} (Revision: {}, OEM: {:?})",
        rsdp_addr,
        revision,
        core::str::from_utf8(&rsdp.oem_id).unwrap_or("???")
    );

    // Look for XSDT (64-bit pointers) if revision >= 2, otherwise RSDT (32-bit pointers)
    let madt_addr = if revision >= 2 {
        let ext_rsdp = unsafe { &*(rsdp_addr as *const RsdpExtendedDescriptor) };
        let xsdt_addr = ext_rsdp.xsdt_address;
        if xsdt_addr != 0 {
            println!("[ACPI] Probing 64-bit XSDT at {:#018x}...", xsdt_addr);
            find_table_in_xsdt(xsdt_addr, b"APIC")
        } else {
            find_table_in_rsdt(rsdt_address as u64, b"APIC")
        }
    } else {
        println!("[ACPI] Probing 32-bit RSDT at {:#010x}...", rsdt_address);
        find_table_in_rsdt(rsdt_address as u64, b"APIC")
    };

    let madt_phys = match madt_addr {
        Some(addr) => addr,
        None => {
            println!("[ACPI] Warning: MADT ('APIC') table not found!");
            return;
        }
    };

    println!("[ACPI] Found Multiple APIC Description Table (MADT) at {:#018x}", madt_phys);
    parse_madt(madt_phys, bsp_apic_id);
}

/// Traverses 32-bit RSDT table pointers to find an ACPI table with `target_sig`.
fn find_table_in_rsdt(rsdt_addr: u64, target_sig: &[u8; 4]) -> Option<u64> {
    if rsdt_addr == 0 {
        return None;
    }
    let header = unsafe { &*(rsdt_addr as *const AcpiTableHeader) };
    if &header.signature != b"RSDT" {
        return None;
    }

    let entry_count = (header.length() - core::mem::size_of::<AcpiTableHeader>()) / 4;
    let entries_ptr = (rsdt_addr + core::mem::size_of::<AcpiTableHeader>() as u64) as *const u32;

    for i in 0..entry_count {
        let table_ptr = unsafe { entries_ptr.add(i).read_unaligned() } as u64;
        if table_ptr == 0 {
            continue;
        }
        let tbl_hdr = unsafe { &*(table_ptr as *const AcpiTableHeader) };
        if &tbl_hdr.signature == target_sig {
            return Some(table_ptr);
        }
    }

    None
}

/// Traverses 64-bit XSDT table pointers to find an ACPI table with `target_sig`.
fn find_table_in_xsdt(xsdt_addr: u64, target_sig: &[u8; 4]) -> Option<u64> {
    if xsdt_addr == 0 {
        return None;
    }
    let header = unsafe { &*(xsdt_addr as *const AcpiTableHeader) };
    if &header.signature != b"XSDT" {
        return None;
    }

    let entry_count = (header.length() - core::mem::size_of::<AcpiTableHeader>()) / 8;
    let entries_ptr = (rsdt_addr_to_ptr(xsdt_addr)) as *const u64;

    for i in 0..entry_count {
        let table_ptr = unsafe { entries_ptr.add(i).read_unaligned() };
        if table_ptr == 0 {
            continue;
        }
        let tbl_hdr = unsafe { &*(table_ptr as *const AcpiTableHeader) };
        if &tbl_hdr.signature == target_sig {
            return Some(table_ptr);
        }
    }

    None
}

fn rsdt_addr_to_ptr(addr: u64) -> u64 {
    addr + core::mem::size_of::<AcpiTableHeader>() as u64
}

/// Parses the MADT entries and collects all CPU cores and I/O APICs.
fn parse_madt(madt_addr: u64, bsp_apic_id: u8) {
    let madt = unsafe { &*(madt_addr as *const MadtHeader) };
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

    let info = MadtInfo {
        local_apic_addr,
        bsp_apic_id,
        cpus,
        io_apics,
    };

    *MADT_INFO.lock() = Some(info);
}

/// Returns the total number of discovered CPU cores.
pub fn cpu_count() -> usize {
    MADT_INFO.lock().as_ref().map(|info| info.cpus.len()).unwrap_or(1)
}

/// Returns true if the system has multiple CPU cores discovered via ACPI.
pub fn is_smp_active() -> bool {
    cpu_count() > 1
}

/// Runs automated self-tests verifying ACPI MADT parsing and CPU core enumeration.
pub fn run_self_tests() {
    let guard = MADT_INFO.lock();
    let info = guard.as_ref().expect("ACPI MADT subsystem not initialized");

    assert!(!info.cpus.is_empty(), "MADT should discover at least 1 CPU (BSP)");
    assert!(info.cpus.iter().any(|c| c.is_bsp), "MADT must identify the Bootstrap Processor (BSP)");

    println!(
        "[ACPI] ACPI MADT verification passed: {} cores enumerated, BSP APIC ID: {}.",
        info.cpus.len(),
        info.bsp_apic_id
    );
}

// TODO(phase13-m33-acpi-power): Implement ACPI Power Management & Extended Tables:
// 1. FADT (Fixed ACPI Description Table) parser: Locate PM1a_CNT_BLK, PM1b_CNT_BLK, and DSDT physical address.
// 2. DSDT / AML parser: Parse the `\_S5` object to discover SLP_TYPa and SLP_TYPb values.
// 3. `poweroff()`: Write `(SLP_TYPa << 10) | (1 << 13)` to PM1a_CNT for clean hardware and VM shutdown.
// 4. `reboot()`: Pulse CPU reset line via 8042 keyboard controller (port 0x64, command 0xFE) or ACPI RESET_REG.
// 5. HPET (High Precision Event Timer) table parser for sub-microsecond hardware timestamping.
