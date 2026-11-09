//! # UEFI Memory Map Parser, Descriptors, and Iterator
//!
//! ## Overview
//! When the bootloader invokes `ExitBootServices()`, UEFI firmware generates a contiguous
//! snapshot of the system's physical memory layout: the **UEFI Memory Map**.
//!
//! The buffer contains an array of variable-stride [`MemoryDescriptor`] records.
//! Because firmware implementations may add OEM-specific fields to descriptors beyond the
//! standard 40 or 48 bytes, the kernel must iterate using the firmware-specified `descriptor_size`
//! rather than `core::mem::size_of::<MemoryDescriptor>()`.
//!
//! ## Key UEFI Memory Classifications
//! - **`Conventional` (Type 7)**: Standard usable RAM available for operating system allocation.
//! - **`BootServicesCode` / `BootServicesData` (Types 3, 4)**: Memory allocated for UEFI bootloader
//!   execution. Once `ExitBootServices()` completes, these regions can be safely reclaimed.
//! - **`RuntimeServicesCode` / `RuntimeServicesData` (Types 5, 6)**: Firmware code/data required
//!   for UEFI runtime services calls (e.g. NVRAM variables, RTC time). Must be preserved.
//! - **`AcpiReclaim` (Type 9)**: ACPI tables (RSDT, XSDT, MADT, FADT). Can be reclaimed after
//!   the OS finishes copying or parsing all ACPI data.
//! - **`AcpiNvs` (Type 10)**: Non-volatile storage reserved by firmware. Must never be touched.
//! - **`Mmio` / `MmioPortSpace` (Types 11, 12)**: Memory-mapped hardware registers (Local APIC,
//!   I/O APIC, PCIe ECAM, GPU framebuffers).

#![allow(dead_code)]

use bootinfo::MemoryMapInfo;
use crate::println;

/// Standard UEFI Memory Types per Section 7.2 of the UEFI Specification.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UefiMemoryType {
    /// Not usable by firmware or OS.
    Reserved = 0,
    /// Loaded UEFI application code.
    LoaderCode = 1,
    /// Data structures allocated by UEFI loaders.
    LoaderData = 2,
    /// Code used by UEFI Boot Services (reclaimable after ExitBootServices).
    BootServicesCode = 3,
    /// Data used by UEFI Boot Services (reclaimable after ExitBootServices).
    BootServicesData = 4,
    /// Code used by UEFI Runtime Services (must be identity-mapped for runtime calls).
    RuntimeServicesCode = 5,
    /// Data used by UEFI Runtime Services.
    RuntimeServicesData = 6,
    /// Free, unallocated conventional system RAM.
    Conventional = 7,
    /// Memory containing persistent hardware errors.
    Unusable = 8,
    /// ACPI tables; reclaimable after ACPI parsing completes.
    AcpiReclaim = 9,
    /// ACPI non-volatile firmware memory.
    AcpiNvs = 10,
    /// Memory-mapped hardware I/O address space.
    Mmio = 11,
    /// System I/O port address space mapped to physical memory.
    MmioPortSpace = 12,
    /// Processor Auto-Configuration (Itanium/legacy).
    PalCode = 13,
    /// Byte-addressable persistent non-volatile RAM.
    PersistentMemory = 14,
    /// Unrecognized future or vendor-specific memory type.
    Unknown(u32),
}

impl From<u32> for UefiMemoryType {
    fn from(val: u32) -> Self {
        match val {
            0 => Self::Reserved,
            1 => Self::LoaderCode,
            2 => Self::LoaderData,
            3 => Self::BootServicesCode,
            4 => Self::BootServicesData,
            5 => Self::RuntimeServicesCode,
            6 => Self::RuntimeServicesData,
            7 => Self::Conventional,
            8 => Self::Unusable,
            9 => Self::AcpiReclaim,
            10 => Self::AcpiNvs,
            11 => Self::Mmio,
            12 => Self::MmioPortSpace,
            13 => Self::PalCode,
            14 => Self::PersistentMemory,
            other => Self::Unknown(other),
        }
    }
}

impl UefiMemoryType {
    /// Returns human-readable name of the memory type.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Reserved => "Reserved",
            Self::LoaderCode => "LoaderCode",
            Self::LoaderData => "LoaderData",
            Self::BootServicesCode => "BootServicesCode",
            Self::BootServicesData => "BootServicesData",
            Self::RuntimeServicesCode => "RuntimeServicesCode",
            Self::RuntimeServicesData => "RuntimeServicesData",
            Self::Conventional => "Conventional (Free RAM)",
            Self::Unusable => "Unusable",
            Self::AcpiReclaim => "AcpiReclaim",
            Self::AcpiNvs => "AcpiNVS",
            Self::Mmio => "MMIO",
            Self::MmioPortSpace => "MmioPortSpace",
            Self::PalCode => "PalCode",
            Self::PersistentMemory => "PersistentMemory",
            Self::Unknown(_) => "Unknown",
        }
    }

    /// Whether this memory region is usable RAM that can be allocated for general use.
    #[inline]
    pub const fn is_usable(&self) -> bool {
        matches!(self, Self::Conventional)
    }

    /// Whether this region belonged to UEFI boot services and can be reclaimed after ExitBootServices.
    #[inline]
    pub const fn is_reclaimable(&self) -> bool {
        matches!(self, Self::BootServicesCode | Self::BootServicesData)
    }
}

/// A single UEFI memory descriptor structure matching the EFI_MEMORY_DESCRIPTOR format.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MemoryDescriptor {
    /// Type of the memory region ([`UefiMemoryType`]).
    pub ty: u32,
    /// Physical starting address of the memory range.
    pub phys_start: u64,
    /// Virtual starting address assigned by UEFI firmware (if mapped).
    pub virt_start: u64,
    /// Number of 4096-byte (4 KiB) pages in the region.
    pub page_count: u64,
    /// Memory attributes (e.g. UC, WC, WT, WB, WP, RP, XP, NV).
    pub attribute: u64,
}

impl MemoryDescriptor {
    /// Returns the parsed [`UefiMemoryType`] of this descriptor.
    #[inline]
    pub fn memory_type(&self) -> UefiMemoryType {
        UefiMemoryType::from(self.ty)
    }

    /// Returns the total size of this memory region in bytes (`page_count * 4096`).
    #[inline]
    pub fn size_in_bytes(&self) -> u64 {
        self.page_count * 4096
    }

    /// Returns the non-inclusive physical end address of this region.
    #[inline]
    pub fn phys_end(&self) -> u64 {
        self.phys_start + self.size_in_bytes()
    }
}

/// An iterator yielding each [`MemoryDescriptor`] from the raw `MemoryMapInfo` buffer.
///
/// Accounts for variable descriptor strides (`descriptor_size`) emitted by different
/// UEFI firmware implementations.
pub struct MemoryMapIterator<'a> {
    mmap: &'a MemoryMapInfo,
    index: usize,
    entry_count: usize,
}

impl<'a> MemoryMapIterator<'a> {
    /// Creates a new iterator over the given UEFI memory map metadata.
    pub fn new(mmap: &'a MemoryMapInfo) -> Self {
        let entry_count = if mmap.descriptor_size > 0 {
            mmap.size / mmap.descriptor_size
        } else {
            0
        };
        Self {
            mmap,
            index: 0,
            entry_count,
        }
    }
}

impl<'a> Iterator for MemoryMapIterator<'a> {
    type Item = &'a MemoryDescriptor;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.entry_count {
            return None;
        }

        let offset = self.index * self.mmap.descriptor_size;
        let ptr = (self.mmap.address + offset as u64) as *const MemoryDescriptor;
        self.index += 1;
        unsafe { Some(&*ptr) }
    }
}

/// Print formatted memory map entries for debugging and architectural verification.
///
/// # Parameters
/// - `mmap`: Reference to the bootloader memory map buffer metadata.
pub fn print_memory_map(mmap: &MemoryMapInfo) {
    let entry_count = if mmap.descriptor_size > 0 {
        mmap.size / mmap.descriptor_size
    } else {
        0
    };

    println!(
        "[MEMORY] UEFI Memory Map at {:#x} ({} entries, desc size: {} bytes):",
        mmap.address, entry_count, mmap.descriptor_size
    );

    let mut printed = 0;
    for (i, desc) in MemoryMapIterator::new(mmap).enumerate() {
        if printed < 12 {
            let mtype = desc.memory_type();
            println!(
                "  [{:02}] {:<22} Start: {:#014x} End: {:#014x} Pages: {:6} ({:>4} KiB)",
                i,
                mtype.name(),
                desc.phys_start,
                desc.phys_end(),
                desc.page_count,
                desc.page_count * 4
            );
            printed += 1;
        }
    }

    if entry_count > 12 {
        println!("  ... ({} total entries truncated)", entry_count);
    }
}
