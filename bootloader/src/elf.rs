use core::ptr;
use uefi::boot::{allocate_pages, AllocateType, MemoryType};

pub const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
pub const ELFCLASS64: u8 = 2;
pub const ELFDATA2LSB: u8 = 1;
pub const EM_X86_64: u16 = 0x3E;
pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;
pub const PT_LOAD: u32 = 1;

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
pub struct LoadedKernel {
    pub entry_point: u64,
    pub image_start: u64,
    pub image_size: u64,
}

pub fn load_elf(elf_data: &[u8]) -> Result<LoadedKernel, &'static str> {
    if elf_data.len() < core::mem::size_of::<Elf64Header>() {
        return Err("ELF file too small for header");
    }

    let header_ptr = elf_data.as_ptr() as *const Elf64Header;
    let header = unsafe { ptr::read_unaligned(header_ptr) };

    if header.e_ident[0..4] != ELF_MAGIC {
        return Err("Invalid ELF magic");
    }

    if header.e_ident[4] != ELFCLASS64 {
        return Err("Not a 64-bit ELF");
    }

    if header.e_ident[5] != ELFDATA2LSB {
        return Err("Not little-endian ELF");
    }

    if header.e_machine != EM_X86_64 {
        return Err("Not an x86_64 ELF");
    }

    if header.e_type != ET_EXEC && header.e_type != ET_DYN {
        return Err("Unsupported ELF type (expected ET_EXEC or ET_DYN)");
    }

    let ph_offset = header.e_phoff as usize;
    let ph_entsize = header.e_phentsize as usize;
    let ph_num = header.e_phnum as usize;

    if ph_entsize < core::mem::size_of::<Elf64ProgramHeader>() {
        return Err("Invalid program header entry size");
    }

    let mut min_addr: u64 = u64::MAX;
    let mut max_addr: u64 = 0;

    for i in 0..ph_num {
        let entry_offset = ph_offset + i * ph_entsize;
        if entry_offset + core::mem::size_of::<Elf64ProgramHeader>() > elf_data.len() {
            return Err("Program header exceeds file bounds");
        }

        let ph_ptr = unsafe { elf_data.as_ptr().add(entry_offset) } as *const Elf64ProgramHeader;
        let ph = unsafe { ptr::read_unaligned(ph_ptr) };

        if ph.p_type != PT_LOAD || ph.p_memsz == 0 {
            continue;
        }

        let start_page = (ph.p_vaddr / 4096) * 4096;
        let end_page = ((ph.p_vaddr + ph.p_memsz + 4095) / 4096) * 4096;
        let num_pages = ((end_page - start_page) / 4096) as usize;

        // Allocate physical memory pages for this PT_LOAD segment
        let _ = allocate_pages(
            AllocateType::Address(start_page),
            MemoryType::LOADER_DATA,
            num_pages,
        );

        // Zero destination memory
        unsafe {
            ptr::write_bytes(ph.p_vaddr as *mut u8, 0, ph.p_memsz as usize);
        }

        // Copy file content to segment destination
        if ph.p_filesz > 0 {
            let file_offset = ph.p_offset as usize;
            let file_size = ph.p_filesz as usize;
            if file_offset + file_size > elf_data.len() {
                return Err("Segment file data exceeds ELF file length");
            }
            unsafe {
                let src = elf_data.as_ptr().add(file_offset);
                let dst = ph.p_vaddr as *mut u8;
                ptr::copy_nonoverlapping(src, dst, file_size);
            }
        }

        if ph.p_vaddr < min_addr {
            min_addr = ph.p_vaddr;
        }
        let segment_end = ph.p_vaddr + ph.p_memsz;
        if segment_end > max_addr {
            max_addr = segment_end;
        }
    }

    if min_addr == u64::MAX {
        return Err("No PT_LOAD segments found");
    }

    Ok(LoadedKernel {
        entry_point: header.e_entry,
        image_start: min_addr,
        image_size: max_addr - min_addr,
    })
}
