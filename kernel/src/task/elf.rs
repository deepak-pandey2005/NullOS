//! ELF64 User Binary Loader (Phase 5 - M12).
//!
//! Features:
//! - Validates 64-bit x86_64 ELF headers (magic bytes, endianness, machine type).
//! - Iterates over `PT_LOAD` segments.
//! - Maps user pages with appropriate permissions (Read, Write, Execute).
//! - Zero-fills BSS uninitialized data sections.
//! - Returns the executable's entry point (`e_entry`).

use crate::memory::address::{Page, PhysFrame, VirtAddr, PAGE_SIZE_4K};
use crate::memory::PageTableFlags;
use alloc::vec::Vec;
use core::ptr;

pub const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
pub const ELFCLASS64: u8 = 2;
pub const ELFDATA2LSB: u8 = 1;
pub const EM_X86_64: u16 = 0x3E;
pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;

pub const PT_LOAD: u32 = 1;
pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Header {
    pub e_ident: [u8; 16],
    pub e_type: u16,
    pub e_machine: u16,
    pub e_version: u32,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_shoff: u64,
    pub e_flags: u32,
    pub e_ehsize: u16,
    pub e_phentsize: u16,
    pub e_phnum: u16,
    pub e_shentsize: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64ProgramHeader {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct LoadedUserElf {
    pub entry_point: u64,
    pub load_base: u64,
    pub total_memory_size: u64,
}

/// Parses and loads a 64-bit ELF user binary into the active user address space.
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

            // Map each virtual page for this segment
            let mut curr = start_vaddr;
            while curr < end_vaddr {
                let page = Page::containing_address(VirtAddr::new(curr));
                let frame = crate::memory::alloc_frame().ok_or("Out of memory mapping ELF segment")?;

                // Zero out the frame initially
                unsafe {
                    core::ptr::write_bytes(frame.start_address().as_mut_ptr::<u8>(), 0, PAGE_SIZE_4K as usize);
                }

                crate::memory::paging::map_page(page, frame, page_flags)?;
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
