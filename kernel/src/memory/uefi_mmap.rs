//! UEFI Memory Map parser, descriptors, and iterator.
#![allow(dead_code)]

use bootinfo::MemoryMapInfo;
use crate::println;

/// Standard UEFI Memory Types per the UEFI Specification.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UefiMemoryType {
    Reserved = 0,
    LoaderCode = 1,
    LoaderData = 2,
    BootServicesCode = 3,
    BootServicesData = 4,
    RuntimeServicesCode = 5,
    RuntimeServicesData = 6,
    Conventional = 7,
    Unusable = 8,
    AcpiReclaim = 9,
    AcpiNvs = 10,
    Mmio = 11,
    MmioPortSpace = 12,
    PalCode = 13,
    PersistentMemory = 14,
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
    pub ty: u32,
    pub phys_start: u64,
    pub virt_start: u64,
    pub page_count: u64,
    pub attribute: u64,
}

impl MemoryDescriptor {
    #[inline]
    pub fn memory_type(&self) -> UefiMemoryType {
        UefiMemoryType::from(self.ty)
    }

    #[inline]
    pub fn size_in_bytes(&self) -> u64 {
        self.page_count * 4096
    }

    #[inline]
    pub fn phys_end(&self) -> u64 {
        self.phys_start + self.size_in_bytes()
    }
}

/// An iterator yielding each `MemoryDescriptor` from the raw `MemoryMapInfo` buffer.
pub struct MemoryMapIterator<'a> {
    mmap: &'a MemoryMapInfo,
    index: usize,
    entry_count: usize,
}

impl<'a> MemoryMapIterator<'a> {
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
