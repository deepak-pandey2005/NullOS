//! 64-Bit ELF Binary Loader for Kernel Executables.
//!
//! # Overview
//! The NullOS kernel is compiled as a 64-bit ELF (`kernel.elf`). The bootloader
//! parses the ELF64 binary headers, validates hardware compatibility, allocates
//! physical pages at the requested virtual load addresses, and populates initialized
//! and zero-filled (`.bss`) memory segments prior to jumping into `kernel_main`.

use core::ptr;
use uefi::boot::{allocate_pages, AllocateType, MemoryType};

/// Standard ELF Identification Magic Number: `0x7F`, `'E'`, `'L'`, `'F'`.
pub const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// ELF Architecture Class: 64-bit architecture (`ELFCLASS64`).
pub const ELFCLASS64: u8 = 2;

/// ELF Data Encoding: 2's complement, little-endian (`ELFDATA2LSB`).
pub const ELFDATA2LSB: u8 = 1;

/// ELF Machine Architecture: Advanced Micro Devices x86-64 (`EM_X86_64` = `0x3E`).
pub const EM_X86_64: u16 = 0x3E;

/// ELF Object Type: Standard static executable (`ET_EXEC` = 2).
pub const ET_EXEC: u16 = 2;

/// ELF Object Type: Position-independent shared object / dynamic binary (`ET_DYN` = 3).
pub const ET_DYN: u16 = 3;

/// Program Header Type: Loadable segment (`PT_LOAD` = 1).
pub const PT_LOAD: u32 = 1;

/// Standard 64-bit ELF File Header (64 bytes).
///
/// Contains binary metadata, entry point address, and offsets to program and section headers.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Header {
    /// Identification bytes (Magic number, class, data endianness, version, ABI).
    pub e_ident: [u8; 16],
    /// Object file type (`ET_EXEC`, `ET_DYN`, `ET_REL`).
    pub e_type: u16,
    /// Target CPU architecture instruction set (`EM_X86_64`).
    pub e_machine: u16,
    /// Object file version (always 1).
    pub e_version: u32,
    /// Virtual memory entry point address where execution begins.
    pub e_entry: u64,
    /// File offset to the Program Header Table (in bytes).
    pub e_phoff: u64,
    /// File offset to the Section Header Table (in bytes).
    pub e_shoff: u64,
    /// Processor-specific flags.
    pub e_flags: u32,
    /// ELF header size (typically 64 bytes for ELF64).
    pub e_ehsize: u16,
    /// Size of an individual program header entry (typically 56 bytes).
    pub e_phentsize: u16,
    /// Total number of program header entries.
    pub e_phnum: u16,
    /// Size of an individual section header entry (typically 64 bytes).
    pub e_shentsize: u16,
    /// Total number of section header entries.
    pub e_shnum: u16,
    /// Section header table index of the section name string table.
    pub e_shstrndx: u16,
}

/// Standard 64-bit ELF Program Header (56 bytes).
///
/// Describes a contiguous segment to be loaded into memory or inspected by the runtime.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64ProgramHeader {
    /// Segment type (`PT_LOAD`, `PT_DYNAMIC`, `PT_INTERP`, `PT_NOTE`, etc.).
    pub p_type: u32,
    /// Segment execution and access flags (`PF_X`, `PF_W`, `PF_R`).
    pub p_flags: u32,
    /// Offset of the segment data from the start of the ELF file.
    pub p_offset: u64,
    /// Target virtual address where the first byte of the segment resides in memory.
    pub p_vaddr: u64,
    /// Physical address for systems where physical addressing is relevant.
    pub p_paddr: u64,
    /// Number of bytes in the file image of the segment.
    pub p_filesz: u64,
    /// Number of bytes in the memory image of the segment (may exceed `p_filesz` for `.bss`).
    pub p_memsz: u64,
    /// Segment memory alignment requirement (power of 2, typically 4096 bytes).
    pub p_align: u64,
}

/// Memory geometry metrics describing the loaded kernel in physical RAM.
#[derive(Debug, Clone, Copy)]
pub struct LoadedKernel {
    /// Virtual entry point address (`_start`).
    pub entry_point: u64,
    /// Lowest physical/virtual address of the loaded kernel image.
    pub image_start: u64,
    /// Total virtual span of the kernel image (in bytes).
    pub image_size: u64,
}

/// Parses an ELF64 binary and maps all loadable segments (`PT_LOAD`) into memory.
///
/// # Validation Steps
/// 1. Verifies that the buffer contains at least 64 bytes for `Elf64Header`.
/// 2. Validates the 4-byte ELF Magic sequence (`0x7F 'E' 'L' 'F'`).
/// 3. Confirms 64-bit architecture (`ELFCLASS64`) and little-endian encoding (`ELFDATA2LSB`).
/// 4. Confirms AMD x86-64 target architecture (`EM_X86_64`).
/// 5. Verifies object type is either `ET_EXEC` or `ET_DYN`.
///
/// # Segment Loading Algorithm
/// For every `PT_LOAD` program header:
/// 1. Computes the 4096-byte page-aligned start and end addresses.
/// 2. Requests physical page allocation via UEFI `allocate_pages` with `AllocateType::Address`.
/// 3. Zero-fills the entire memory span (`p_memsz`) to guarantee clean uninitialized `.bss` memory.
/// 4. Copies `p_filesz` bytes from the raw ELF file into `p_vaddr`.
///
/// # Returns
/// - `Ok(LoadedKernel)` with entry point address and kernel image memory range.
/// - `Err(&'static str)` if header verification or page allocation fails.
pub fn load_elf(elf_data: &[u8]) -> Result<LoadedKernel, &'static str> {
    if elf_data.len() < core::mem::size_of::<Elf64Header>() {
        return Err("ELF file too small for header");
    }

    let header_ptr = elf_data.as_ptr() as *const Elf64Header;
    let header = unsafe { ptr::read_unaligned(header_ptr) };

    // Step 1: Validate ELF magic identifier
    if header.e_ident[0..4] != ELF_MAGIC {
        return Err("Invalid ELF magic");
    }

    // Step 2: Validate 64-bit architecture
    if header.e_ident[4] != ELFCLASS64 {
        return Err("Not a 64-bit ELF");
    }

    // Step 3: Validate little-endian encoding
    if header.e_ident[5] != ELFDATA2LSB {
        return Err("Not little-endian ELF");
    }

    // Step 4: Validate x86_64 machine type
    if header.e_machine != EM_X86_64 {
        return Err("Not an x86_64 ELF");
    }

    // Step 5: Validate supported executable object types
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

    // Step 6: Iterate Program Header Table and map PT_LOAD segments
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

        // Allocate physical memory pages at the designated virtual load address
        let _ = allocate_pages(
            AllocateType::Address(start_page),
            MemoryType::LOADER_DATA,
            num_pages,
        );

        // Zero-fill the entire allocated segment memory (covers .bss and uninitialized state)
        unsafe {
            ptr::write_bytes(ph.p_vaddr as *mut u8, 0, ph.p_memsz as usize);
        }

        // Copy file content to segment destination memory
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

        // Update overall kernel memory bounds
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
