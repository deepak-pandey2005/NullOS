//! # ELF64 User Binary Loader
//!
//! ## Overview
//! The NullOS ELF loader reads Executable and Linkable Format (ELF) 64-bit binaries
//! compiled for x86_64, parsing program headers and establishing isolated process address
//! space mappings before transitioning the CPU to Ring 3 user mode.
//!
//! ## ELF Loading Pipeline:
//! 1. **Header Validation**:
//!    - Verifies 4-byte magic signature `[0x7F, 'E', 'L', 'F']`.
//!    - Ensures 64-bit class (`ELFCLASS64 = 2`) and little-endian data encoding (`ELFDATA2LSB = 1`).
//!    - Asserts AMD x86_64 machine architecture (`EM_X86_64 = 0x3E`).
//!    - Confirms executable or shared library object type (`ET_EXEC` or `ET_DYN`).
//! 2. **Segment Scanning (`PT_LOAD`)**:
//!    - Iterates over all program headers, selecting loadable segments (`p_type == PT_LOAD`).
//!    - Enforces user-space address boundary: `p_vaddr < 0x0000_8000_0000_0000` (128 TiB canonical user ceiling).
//! 3. **Page Frame Allocation & Virtual Mapping**:
//!    - For each 4 KiB page spanned by `[p_vaddr, p_vaddr + p_memsz)`:
//!      - Allocates a zeroed physical frame from the PMM.
//!      - Temporarily maps the page as writable.
//!      - Copies `p_filesz` bytes from the ELF file offset.
//!      - Zero-fills remaining `.bss` memory (`p_memsz - p_filesz`).
//!      - Remaps the page with strict final permissions (`PF_R`, `PF_W`, `PF_X` -> `WRITABLE`, `NO_EXECUTE`).
//! 4. **Entry Point Resolution**:
//!    - Returns `header.e_entry` for CPU execution.

use crate::memory::address::{Page, PhysFrame, VirtAddr, PAGE_SIZE_4K};
use crate::memory::PageTableFlags;
use alloc::vec::Vec;
use core::ptr;

/// ELF magic identifier bytes (`\x7fELF`).
pub const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
/// ELF 64-bit architecture class.
pub const ELFCLASS64: u8 = 2;
/// ELF 2's complement little-endian data encoding.
pub const ELFDATA2LSB: u8 = 1;
/// AMD x86_64 machine target architecture.
pub const EM_X86_64: u16 = 0x3E;
/// Executable ELF file type.
pub const ET_EXEC: u16 = 2;
/// Shared object / dynamic library ELF file type.
pub const ET_DYN: u16 = 3;

/// Loadable segment type identifier in program headers.
pub const PT_LOAD: u32 = 1;
/// Segment execute permission flag.
pub const PF_X: u32 = 1;
/// Segment write permission flag.
pub const PF_W: u32 = 2;
/// Segment read permission flag.
pub const PF_R: u32 = 4;

/// Standard 64-bit ELF file header (64 bytes).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Header {
    /// ELF identification bytes (magic, class, endianness, version).
    pub e_ident: [u8; 16],
    /// Object file type (ET_EXEC, ET_DYN, etc.).
    pub e_type: u16,
    /// Target CPU architecture.
    pub e_machine: u16,
    /// Object file version.
    pub e_version: u32,
    /// Virtual memory entry point address where execution begins.
    pub e_entry: u64,
    /// File offset to the program header table in bytes.
    pub e_phoff: u64,
    /// File offset to the section header table in bytes.
    pub e_shoff: u64,
    /// Processor-specific flags.
    pub e_flags: u32,
    /// Size of this ELF header in bytes (64 bytes).
    pub e_ehsize: u16,
    /// Size of a program header table entry in bytes (56 bytes).
    pub e_phentsize: u16,
    /// Number of entries in the program header table.
    pub e_phnum: u16,
    /// Size of a section header table entry in bytes.
    pub e_shentsize: u16,
    /// Number of entries in the section header table.
    pub e_shnum: u16,
    /// Section header table index of the section name string table.
    pub e_shstrndx: u16,
}

/// Standard 64-bit ELF program header (56 bytes).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64ProgramHeader {
    /// Segment type (PT_LOAD, PT_DYNAMIC, PT_INTERP, etc.).
    pub p_type: u32,
    /// Segment permission flags (PF_R, PF_W, PF_X).
    pub p_flags: u32,
    /// File offset where segment data resides.
    pub p_offset: u64,
    /// Target virtual address where the segment is to be loaded.
    pub p_vaddr: u64,
    /// Physical address (ignored on systems with paging).
    pub p_paddr: u64,
    /// Size of segment data in the file image.
    pub p_filesz: u64,
    /// Size of segment in memory (>= p_filesz; excess is zeroed .bss).
    pub p_memsz: u64,
    /// Required memory alignment boundary (e.g. 0x1000 for 4 KiB pages).
    pub p_align: u64,
}

/// Information returned upon successful loading of an ELF binary image.
#[derive(Debug, Clone, Copy)]
pub struct LoadedUserElf {
    /// Resolved virtual entry address where CPU execution starts.
    pub entry_point: u64,
    /// Lowest virtual load address among all mapped segments.
    pub load_base: u64,
    /// Total virtual memory span consumed by mapped segments.
    pub total_memory_size: u64,
}

/// Parses and loads a 64-bit ELF user binary into the active user address space.
///
/// # Parameters
/// - `elf_data`: Raw byte slice containing the complete ELF executable file image.
///
/// # Returns
/// - `Ok(LoadedUserElf)` containing entry point and boundary metadata.
/// - `Err(&'static str)` on format invalidation or mapping failure.
pub fn load_user_elf(elf_data: &[u8]) -> Result<LoadedUserElf, &'static str> {
    if elf_data.len() < core::mem::size_of::<Elf64Header>() {
        return Err("ELF file too small for 64-bit header");
    }

    let header_ptr = elf_data.as_ptr() as *const Elf64Header;
    let header = unsafe { ptr::read_unaligned(header_ptr) };

    if header.e_ident[0..4] != ELF_MAGIC {
        return Err("Invalid ELF magic identifier");
    }
    if header.e_ident[4] != ELFCLASS64 {
        return Err("Unsupported ELF class (must be 64-bit)");
    }
    if header.e_ident[5] != ELFDATA2LSB {
        return Err("Unsupported ELF endianness (must be little-endian)");
    }
    if header.e_machine != EM_X86_64 {
        return Err("Unsupported ELF target architecture (must be x86_64)");
    }
    if header.e_type != ET_EXEC && header.e_type != ET_DYN {
        return Err("Unsupported ELF object type (must be ET_EXEC or ET_DYN)");
    }

    let ph_offset = header.e_phoff as usize;
    let ph_count = header.e_phnum as usize;
    let ph_size = header.e_phentsize as usize;

    if ph_offset + ph_count * ph_size > elf_data.len() {
        return Err("ELF program header table exceeds file bounds");
    }

    let mut min_vaddr = u64::MAX;
    let mut max_vaddr = 0u64;

    for i in 0..ph_count {
        let entry_offset = ph_offset + i * ph_size;
        let ph_ptr = unsafe { elf_data.as_ptr().add(entry_offset) as *const Elf64ProgramHeader };
        let ph = unsafe { ptr::read_unaligned(ph_ptr) };

        if ph.p_type == PT_LOAD && ph.p_memsz > 0 {
            // Verify address is in canonical user space (< 128 TiB)
            if ph.p_vaddr >= 0x0000_8000_0000_0000 {
                return Err("User ELF segment attempted mapping into kernel address space");
            }

            let start_vaddr = ph.p_vaddr & !0xFFF; // Page-align
            let end_vaddr = (ph.p_vaddr + ph.p_memsz + PAGE_SIZE_4K - 1) & !0xFFF;

            let mut page_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
            if (ph.p_flags & PF_W) != 0 {
                page_flags |= PageTableFlags::WRITABLE;
            }
            if (ph.p_flags & PF_X) == 0 {
                page_flags |= PageTableFlags::NO_EXECUTE;
            }

            let mut mapped_pages: Vec<(Page, PhysFrame, PageTableFlags)> = Vec::new();

            // Map each virtual page for this segment (initially writable so kernel can copy file data)
            let mut curr = start_vaddr;
            while curr < end_vaddr {
                let page = Page::containing_address(VirtAddr::new(curr));
                let frame = crate::memory::alloc_frame().ok_or("Out of memory mapping ELF segment")?;

                // Zero out the frame initially
                unsafe {
                    core::ptr::write_bytes(frame.start_address().as_mut_ptr::<u8>(), 0, PAGE_SIZE_4K as usize);
                }

                // Map with writable so kernel can populate segment data
                crate::memory::paging::map_page(
                    page,
                    frame,
                    PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
                )?;
                mapped_pages.push((page, frame, page_flags));
                curr += PAGE_SIZE_4K;
            }

            // Copy file data into the mapped segment
            if ph.p_filesz > 0 {
                let file_offset = ph.p_offset as usize;
                let file_size = ph.p_filesz as usize;
                if file_offset + file_size > elf_data.len() {
                    return Err("ELF segment data exceeds file length");
                }

                unsafe {
                    core::ptr::copy_nonoverlapping(
                        elf_data.as_ptr().add(file_offset),
                        ph.p_vaddr as *mut u8,
                        file_size,
                    );
                }
            }

            // Zero any remaining .bss memory up to p_memsz
            if ph.p_memsz > ph.p_filesz {
                let bss_start = ph.p_vaddr + ph.p_filesz;
                let bss_size = (ph.p_memsz - ph.p_filesz) as usize;
                unsafe {
                    core::ptr::write_bytes(bss_start as *mut u8, 0, bss_size);
                }
            }

            // Apply final protection flags (e.g. read-only text / rodata)
            for (page, frame, final_flags) in mapped_pages {
                if !final_flags.contains(PageTableFlags::WRITABLE) {
                    crate::memory::paging::map_page(page, frame, final_flags)?;
                }
            }

            if ph.p_vaddr < min_vaddr {
                min_vaddr = ph.p_vaddr;
            }
            if ph.p_vaddr + ph.p_memsz > max_vaddr {
                max_vaddr = ph.p_vaddr + ph.p_memsz;
            }
        }
    }

    Ok(LoadedUserElf {
        entry_point: header.e_entry,
        load_base: if min_vaddr != u64::MAX { min_vaddr } else { 0 },
        total_memory_size: if max_vaddr > min_vaddr { max_vaddr - min_vaddr } else { 0 },
    })
}

/// Helper synthesizing a valid 64-bit ELF binary image in memory from code bytes for self-tests.
pub fn create_test_elf_image(code: &[u8], entry_vaddr: u64) -> Vec<u8> {
    let eh_size = core::mem::size_of::<Elf64Header>();
    let ph_size = core::mem::size_of::<Elf64ProgramHeader>();
    let header_total = eh_size + ph_size;
    let mut elf = Vec::with_capacity(header_total + code.len());

    let header = Elf64Header {
        e_ident: [0x7f, b'E', b'L', b'F', ELFCLASS64, ELFDATA2LSB, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        e_type: ET_EXEC,
        e_machine: EM_X86_64,
        e_version: 1,
        e_entry: entry_vaddr,
        e_phoff: eh_size as u64,
        e_shoff: 0,
        e_flags: 0,
        e_ehsize: eh_size as u16,
        e_phentsize: ph_size as u16,
        e_phnum: 1,
        e_shentsize: 0,
        e_shnum: 0,
        e_shstrndx: 0,
    };

    let pheader = Elf64ProgramHeader {
        p_type: PT_LOAD,
        p_flags: PF_R | PF_W | PF_X,
        p_offset: header_total as u64,
        p_vaddr: entry_vaddr,
        p_paddr: entry_vaddr,
        p_filesz: code.len() as u64,
        p_memsz: code.len() as u64,
        p_align: 0x1000,
    };

    unsafe {
        let eh_bytes = core::slice::from_raw_parts(&header as *const _ as *const u8, eh_size);
        elf.extend_from_slice(eh_bytes);
        let ph_bytes = core::slice::from_raw_parts(&pheader as *const _ as *const u8, ph_size);
        elf.extend_from_slice(ph_bytes);
    }

    elf.extend_from_slice(code);
    elf
}
