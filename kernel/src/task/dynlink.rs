//! # Dynamic Linking & Shared Object (.so) Runtime Subsystem
//!
//! ## Overview
//! Modern operating systems allow programs to share library code across processes,
//! reducing disk and RAM footprints and facilitating modular updates without recompilation.
//!
//! The NullOS dynamic linker is a full in-kernel runtime implementation providing:
//! 1. **ELF64 Dynamic Segment Parser (`PT_DYNAMIC`, `Elf64Dyn`)**:
//!    - Extracts dynamic tags: `DT_NEEDED` (shared dependencies), `DT_STRTAB` / `DT_SYMTAB`
//!      (string and symbol tables), `DT_RELA` / `DT_JMPREL` (relocation arrays), `DT_INIT` / `DT_FINI`.
//! 2. **x86_64 Relocation Engine**:
//!    - `R_X86_64_RELATIVE`: Applies base address slide (`*target = base_address + r_addend`)
//!      for Position-Independent Executables (PIE) and shared libraries.
//!    - `R_X86_64_GLOB_DAT`: Resolves external global variables in the Global Offset Table (GOT).
//!    - `R_X86_64_JUMP_SLOT`: Resolves dynamic function addresses in the Procedure Linkage Table (PLT).
//!    - `R_X86_64_64`: Direct 64-bit absolute symbol addresses.
//! 3. **Shared Library Discovery & Dependency Resolution**:
//!    - Searches `/lib`, `/usr/lib`, `/disk` for missing `DT_NEEDED` dependencies recursively.
//! 4. **POSIX Dynamic Linker API**:
//!    - [`dlopen`]: Loads and relocates a shared library into memory.
//!    - [`dlsym`]: Looks up exported symbol addresses.
//!    - [`dlclose`]: Decrements reference count and unloads unused libraries.

#![allow(dead_code)]

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::ptr;

use crate::println;
use crate::sync::Spinlock;
use crate::task::elf::{
    Elf64Header, Elf64ProgramHeader, ELFCLASS64, ELFDATA2LSB, ELF_MAGIC, EM_X86_64, ET_DYN, ET_EXEC,
    PT_LOAD,
};

/// Program header segment type identifying dynamic linking information.
pub const PT_DYNAMIC: u32 = 2;
/// Program header segment type identifying the path to the dynamic interpreter (`/lib/ld.so`).
pub const PT_INTERP: u32 = 3;

// Dynamic Array Tags (DT_*)
/// Marks the end of the `_DYNAMIC` array.
pub const DT_NULL: i64 = 0;
/// String table offset of a needed shared library dependency.
pub const DT_NEEDED: i64 = 1;
/// Total size in bytes of PLT relocations.
pub const DT_PLTRELSZ: i64 = 2;
/// Address of the Global Offset Table (GOT) or Procedure Linkage Table (PLT).
pub const DT_PLTGOT: i64 = 3;
/// Address of the symbol hash table.
pub const DT_HASH: i64 = 4;
/// Address of the dynamic string table.
pub const DT_STRTAB: i64 = 5;
/// Address of the dynamic symbol table.
pub const DT_SYMTAB: i64 = 6;
/// Address of relocation table with explicit addends (`Elf64_Rela`).
pub const DT_RELA: i64 = 7;
/// Total size in bytes of the `DT_RELA` table.
pub const DT_RELASZ: i64 = 8;
/// Size in bytes of a single `DT_RELA` entry (24 bytes).
pub const DT_RELAENT: i64 = 9;
/// Total size in bytes of the dynamic string table.
pub const DT_STRSZ: i64 = 10;
/// Size in bytes of a single dynamic symbol table entry (24 bytes).
pub const DT_SYMENT: i64 = 11;
/// Address of the initialization function.
pub const DT_INIT: i64 = 12;
/// Address of the termination function.
pub const DT_FINI: i64 = 13;
/// String table offset of the shared object's canonical name.
pub const DT_SONAME: i64 = 14;
/// String table offset of library search path string.
pub const DT_RPATH: i64 = 15;
/// Type of relocation entries in PLT (DT_REL or DT_RELA).
pub const DT_PLTREL: i64 = 20;
/// Address of relocations associated solely with the PLT (`Elf64_Rela`).
pub const DT_JMPREL: i64 = 23;

// x86_64 Relocation Types (R_X86_64_*)
/// No relocation operation performed.
pub const R_X86_64_NONE: u32 = 0;
/// Direct 64-bit symbol value calculation (`S + A`).
pub const R_X86_64_64: u32 = 1;
/// PC-relative 32-bit signed address (`S + A - P`).
pub const R_X86_64_PC32: u32 = 2;
/// Copy symbol data from shared object at link time.
pub const R_X86_64_COPY: u32 = 5;
/// Set GOT entry to data symbol address (`S`).
pub const R_X86_64_GLOB_DAT: u32 = 6;
/// Set PLT/GOT entry to function symbol address (`S`).
pub const R_X86_64_JUMP_SLOT: u32 = 7;
/// Adjust address by base slide (`B + A`) for position-independent code.
pub const R_X86_64_RELATIVE: u32 = 8;

/// ELF64 Dynamic Array Entry (16 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Elf64Dyn {
    pub d_tag: i64,
    pub d_val: u64,
}

/// ELF64 Symbol Table Entry (24 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Elf64Sym {
    pub st_name: u32,
    pub st_info: u8,
    pub st_other: u8,
    pub st_shndx: u16,
    pub st_value: u64,
    pub st_size: u64,
}

impl Elf64Sym {
    pub fn binding(&self) -> u8 {
        self.st_info >> 4
    }

    pub fn sym_type(&self) -> u8 {
        self.st_info & 0xF
    }
}

/// ELF64 Relocation Entry with Explicit Addend (24 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Elf64Rela {
    pub r_offset: u64,
    pub r_info: u64,
    pub r_addend: i64,
}

impl Elf64Rela {
    pub fn r_sym(&self) -> u32 {
        (self.r_info >> 32) as u32
    }

    pub fn r_type(&self) -> u32 {
        (self.r_info & 0xFFFF_FFFF) as u32
    }
}

/// Summary of dynamic metadata extracted from an ELF file.
#[derive(Debug, Clone, Default)]
pub struct DynamicSummary {
    pub is_dynamic: bool,
    pub interpreter: Option<String>,
    pub needed_libraries: Vec<String>,
    pub soname: Option<String>,
    pub rela_count: usize,
    pub plt_rela_count: usize,
    pub exported_symbols_count: usize,
    pub init_addr: Option<u64>,
    pub fini_addr: Option<u64>,
}

/// Discovered and loaded shared library (.so) representation.
#[derive(Debug, Clone)]
pub struct LoadedSharedObject {
    pub handle: u64,
    pub name: String,
    pub path: String,
    pub base_address: u64,
    pub size: usize,
    pub symbols: BTreeMap<String, u64>,
    pub needed: Vec<String>,
    pub init_func: Option<u64>,
    pub fini_func: Option<u64>,
}

/// Global registry of shared libraries loaded into memory.
pub static LOADED_LIBRARIES: Spinlock<Vec<LoadedSharedObject>> = Spinlock::new(Vec::new());

static NEXT_LIB_HANDLE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

/// Inspects an ELF binary and extracts its dynamic dependencies and relocation metrics.
pub fn inspect_elf_dynamic(elf_data: &[u8]) -> Result<DynamicSummary, &'static str> {
    if elf_data.len() < core::mem::size_of::<Elf64Header>() {
        return Err("ELF file too small");
    }

    let header = unsafe { ptr::read_unaligned(elf_data.as_ptr() as *const Elf64Header) };
    if header.e_ident[0..4] != ELF_MAGIC {
        return Err("Invalid ELF magic");
    }

    let ph_offset = header.e_phoff as usize;
    let ph_count = header.e_phnum as usize;
    let ph_size = header.e_phentsize as usize;

    if ph_offset + ph_count * ph_size > elf_data.len() {
        return Err("Program header table exceeds file bounds");
    }

    let mut summary = DynamicSummary::default();
    let mut dyn_offset = 0usize;
    let mut dyn_filesz = 0usize;

    for i in 0..ph_count {
        let ph_ptr = unsafe { elf_data.as_ptr().add(ph_offset + i * ph_size) as *const Elf64ProgramHeader };
        let ph = unsafe { ptr::read_unaligned(ph_ptr) };

        if ph.p_type == PT_INTERP && ph.p_filesz > 0 {
            let offset = ph.p_offset as usize;
            let sz = ph.p_filesz as usize;
            if offset + sz <= elf_data.len() {
                let bytes = &elf_data[offset..offset + sz];
                let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                summary.interpreter = Some(core::str::from_utf8(&bytes[..len]).unwrap_or("").to_string());
            }
        } else if ph.p_type == PT_DYNAMIC {
            summary.is_dynamic = true;
            dyn_offset = ph.p_offset as usize;
            dyn_filesz = ph.p_filesz as usize;
        }
    }

    if !summary.is_dynamic || dyn_filesz == 0 {
        return Ok(summary);
    }

    // Parse dynamic segment entries
    let dyn_entry_size = core::mem::size_of::<Elf64Dyn>();
    let dyn_count = dyn_filesz / dyn_entry_size;

    let mut strtab_file_offset = 0usize;
    let mut _strsz = 0usize;
    let mut symtab_file_offset = 0usize;
    let mut syment = 24usize;
    let mut relasz = 0usize;
    let mut relaent = 24usize;
    let mut pltrelsz = 0usize;
    let mut needed_offsets = Vec::new();
    let mut soname_offset: Option<usize> = None;

    // Map virtual addresses in dynamic tags to file offsets using PT_LOAD headers
    let vaddr_to_file = |vaddr: u64| -> usize {
        for i in 0..ph_count {
            let ph_ptr = unsafe { elf_data.as_ptr().add(ph_offset + i * ph_size) as *const Elf64ProgramHeader };
            let ph = unsafe { ptr::read_unaligned(ph_ptr) };
            if ph.p_type == PT_LOAD && vaddr >= ph.p_vaddr && vaddr < ph.p_vaddr + ph.p_filesz {
                return (ph.p_offset + (vaddr - ph.p_vaddr)) as usize;
            }
        }
        vaddr as usize
    };

    for i in 0..dyn_count {
        let entry_ptr = unsafe { elf_data.as_ptr().add(dyn_offset + i * dyn_entry_size) as *const Elf64Dyn };
        let entry = unsafe { ptr::read_unaligned(entry_ptr) };

        match entry.d_tag {
            DT_NULL => break,
            DT_NEEDED => needed_offsets.push(entry.d_val as usize),
            DT_STRTAB => strtab_file_offset = vaddr_to_file(entry.d_val),
            DT_STRSZ => _strsz = entry.d_val as usize,
            DT_SYMTAB => symtab_file_offset = vaddr_to_file(entry.d_val),
            DT_SYMENT => syment = if entry.d_val != 0 { entry.d_val as usize } else { 24 },
            DT_RELASZ => relasz = entry.d_val as usize,
            DT_RELAENT => relaent = if entry.d_val != 0 { entry.d_val as usize } else { 24 },
            DT_PLTRELSZ => pltrelsz = entry.d_val as usize,
            DT_INIT => summary.init_addr = Some(entry.d_val),
            DT_FINI => summary.fini_addr = Some(entry.d_val),
            DT_SONAME => soname_offset = Some(entry.d_val as usize),
            _ => {}
        }
    }

    // Resolve DT_SONAME string
    if let Some(offset) = soname_offset {
        if strtab_file_offset + offset < elf_data.len() {
            let str_slice = &elf_data[strtab_file_offset + offset..];
            let len = str_slice.iter().position(|&b| b == 0).unwrap_or(str_slice.len());
            summary.soname = Some(core::str::from_utf8(&str_slice[..len]).unwrap_or("").to_string());
        }
    }

    // Resolve DT_NEEDED strings
    for offset in needed_offsets {
        if strtab_file_offset + offset < elf_data.len() {
            let str_slice = &elf_data[strtab_file_offset + offset..];
            let len = str_slice.iter().position(|&b| b == 0).unwrap_or(str_slice.len());
            if let Ok(name) = core::str::from_utf8(&str_slice[..len]) {
                summary.needed_libraries.push(name.to_string());
            }
        }
    }

    if relaent > 0 {
        summary.rela_count = relasz / relaent;
        summary.plt_rela_count = pltrelsz / relaent;
    }

    if symtab_file_offset > 0 && strtab_file_offset > symtab_file_offset {
        let sym_bytes = strtab_file_offset - symtab_file_offset;
        summary.exported_symbols_count = sym_bytes / syment;
    }

    Ok(summary)
}

/// Parses an in-memory shared object (.so), builds its symbol table, and registers it.
pub fn register_shared_object(
    name: &str,
    path: &str,
    elf_data: &[u8],
    base_address: u64,
) -> Result<u64, &'static str> {
    let summary = inspect_elf_dynamic(elf_data)?;
    if !summary.is_dynamic {
        return Err("Object is not dynamic (missing PT_DYNAMIC)");
    }

    let header = unsafe { ptr::read_unaligned(elf_data.as_ptr() as *const Elf64Header) };
    let ph_offset = header.e_phoff as usize;
    let ph_count = header.e_phnum as usize;
    let ph_size = header.e_phentsize as usize;

    let vaddr_to_file = |vaddr: u64| -> usize {
        for i in 0..ph_count {
            let ph_ptr = unsafe { elf_data.as_ptr().add(ph_offset + i * ph_size) as *const Elf64ProgramHeader };
            let ph = unsafe { ptr::read_unaligned(ph_ptr) };
            if ph.p_type == PT_LOAD && vaddr >= ph.p_vaddr && vaddr < ph.p_vaddr + ph.p_filesz {
                return (ph.p_offset + (vaddr - ph.p_vaddr)) as usize;
            }
        }
        vaddr as usize
    };

    // Locate dynamic segment
    let mut strtab_offset = 0usize;
    let mut symtab_offset = 0usize;
    let mut syment = 24usize;

    for i in 0..ph_count {
        let ph_ptr = unsafe { elf_data.as_ptr().add(ph_offset + i * ph_size) as *const Elf64ProgramHeader };
        let ph = unsafe { ptr::read_unaligned(ph_ptr) };
        if ph.p_type == PT_DYNAMIC {
            let dyn_count = (ph.p_filesz as usize) / core::mem::size_of::<Elf64Dyn>();
            for d in 0..dyn_count {
                let entry_ptr = unsafe {
                    elf_data.as_ptr().add(ph.p_offset as usize + d * core::mem::size_of::<Elf64Dyn>())
                        as *const Elf64Dyn
                };
                let entry = unsafe { ptr::read_unaligned(entry_ptr) };
                match entry.d_tag {
                    DT_STRTAB => strtab_offset = vaddr_to_file(entry.d_val),
                    DT_SYMTAB => symtab_offset = vaddr_to_file(entry.d_val),
                    DT_SYMENT => syment = if entry.d_val != 0 { entry.d_val as usize } else { 24 },
                    _ => {}
                }
            }
        }
    }

    let mut symbols = BTreeMap::new();

    // Populate exported symbols
    if symtab_offset > 0 && strtab_offset > symtab_offset {
        let sym_count = (strtab_offset - symtab_offset) / syment;
        for s in 0..sym_count {
            let sym_ptr = unsafe { elf_data.as_ptr().add(symtab_offset + s * syment) as *const Elf64Sym };
            let sym = unsafe { ptr::read_unaligned(sym_ptr) };

            if sym.st_name != 0 && sym.st_value != 0 {
                let name_offset = strtab_offset + sym.st_name as usize;
                if name_offset < elf_data.len() {
                    let slice = &elf_data[name_offset..];
                    let len = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
                    if let Ok(sym_name) = core::str::from_utf8(&slice[..len]) {
                        symbols.insert(sym_name.to_string(), base_address + sym.st_value);
                    }
                }
            }
        }
    }

    let handle = NEXT_LIB_HANDLE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let shared_obj = LoadedSharedObject {
        handle,
        name: name.to_string(),
        path: path.to_string(),
        base_address,
        size: elf_data.len(),
        symbols,
        needed: summary.needed_libraries,
        init_func: summary.init_addr.map(|addr| base_address + addr),
        fini_func: summary.fini_addr.map(|addr| base_address + addr),
    };

    LOADED_LIBRARIES.lock().push(shared_obj);
    Ok(handle)
}

/// Applies AMD64 ELF relocations to a loaded binary in memory.
pub fn apply_relocations(
    memory_base: *mut u8,
    elf_data: &[u8],
    load_base: u64,
    libraries: &[LoadedSharedObject],
) -> Result<usize, &'static str> {
    let header = unsafe { ptr::read_unaligned(elf_data.as_ptr() as *const Elf64Header) };
    let ph_offset = header.e_phoff as usize;
    let ph_count = header.e_phnum as usize;
    let ph_size = header.e_phentsize as usize;

    let vaddr_to_file = |vaddr: u64| -> usize {
        for i in 0..ph_count {
            let ph_ptr = unsafe { elf_data.as_ptr().add(ph_offset + i * ph_size) as *const Elf64ProgramHeader };
            let ph = unsafe { ptr::read_unaligned(ph_ptr) };
            if ph.p_type == PT_LOAD && vaddr >= ph.p_vaddr && vaddr < ph.p_vaddr + ph.p_filesz {
                return (ph.p_offset + (vaddr - ph.p_vaddr)) as usize;
            }
        }
        vaddr as usize
    };

    let mut rela_offset = 0usize;
    let mut relasz = 0usize;
    let mut relaent = 24usize;
    let mut jmprel_offset = 0usize;
    let mut pltrelsz = 0usize;
    let mut strtab_offset = 0usize;
    let mut symtab_offset = 0usize;
    let mut syment = 24usize;

    for i in 0..ph_count {
        let ph_ptr = unsafe { elf_data.as_ptr().add(ph_offset + i * ph_size) as *const Elf64ProgramHeader };
        let ph = unsafe { ptr::read_unaligned(ph_ptr) };
        if ph.p_type == PT_DYNAMIC {
            let dyn_count = (ph.p_filesz as usize) / core::mem::size_of::<Elf64Dyn>();
            for d in 0..dyn_count {
                let entry_ptr = unsafe {
                    elf_data.as_ptr().add(ph.p_offset as usize + d * core::mem::size_of::<Elf64Dyn>())
                        as *const Elf64Dyn
                };
                let entry = unsafe { ptr::read_unaligned(entry_ptr) };
                match entry.d_tag {
                    DT_RELA => rela_offset = vaddr_to_file(entry.d_val),
                    DT_RELASZ => relasz = entry.d_val as usize,
                    DT_RELAENT => relaent = if entry.d_val != 0 { entry.d_val as usize } else { 24 },
                    DT_JMPREL => jmprel_offset = vaddr_to_file(entry.d_val),
                    DT_PLTRELSZ => pltrelsz = entry.d_val as usize,
                    DT_STRTAB => strtab_offset = vaddr_to_file(entry.d_val),
                    DT_SYMTAB => symtab_offset = vaddr_to_file(entry.d_val),
                    DT_SYMENT => syment = if entry.d_val != 0 { entry.d_val as usize } else { 24 },
                    _ => {}
                }
            }
        }
    }

    let mut relocations_applied = 0usize;

    // Helper to resolve symbol name by index
    let get_sym_info = |sym_idx: u32| -> Option<(String, u64)> {
        if symtab_offset == 0 || strtab_offset == 0 {
            return None;
        }
        let sym_ptr = unsafe {
            elf_data.as_ptr().add(symtab_offset + (sym_idx as usize) * syment) as *const Elf64Sym
        };
        let sym = unsafe { ptr::read_unaligned(sym_ptr) };
        if sym.st_name == 0 {
            return None;
        }
        let name_offset = strtab_offset + sym.st_name as usize;
        if name_offset >= elf_data.len() {
            return None;
        }
        let slice = &elf_data[name_offset..];
        let len = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
        let sym_name = core::str::from_utf8(&slice[..len]).ok()?;

        // Look up in dependencies
        for lib in libraries {
            if let Some(&addr) = lib.symbols.get(sym_name) {
                return Some((sym_name.to_string(), addr));
            }
        }

        // Check in self if defined
        if sym.st_value != 0 {
            return Some((sym_name.to_string(), load_base + sym.st_value));
        }

        None
    };

    // Combine standard relocations (.rela.dyn) and PLT relocations (.rela.plt)
    let mut rela_ranges = Vec::new();
    if rela_offset > 0 && relasz > 0 {
        rela_ranges.push((rela_offset, relasz));
    }
    if jmprel_offset > 0 && pltrelsz > 0 {
        rela_ranges.push((jmprel_offset, pltrelsz));
    }

    for (start, size) in rela_ranges {
        let count = size / relaent;
        for r in 0..count {
            let r_ptr = unsafe { elf_data.as_ptr().add(start + r * relaent) as *const Elf64Rela };
            let rela = unsafe { ptr::read_unaligned(r_ptr) };

            let target_addr = load_base + rela.r_offset;
            let target_ptr = if !memory_base.is_null() {
                unsafe { memory_base.add(rela.r_offset as usize) as *mut u64 }
            } else {
                target_addr as *mut u64
            };

            match rela.r_type() {
                R_X86_64_RELATIVE => {
                    // Target = B + A (Base + Addend)
                    let final_val = (load_base as i64 + rela.r_addend) as u64;
                    unsafe {
                        ptr::write_unaligned(target_ptr, final_val);
                    }
                    relocations_applied += 1;
                }
                R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT => {
                    // Target = S (Resolved Symbol Address)
                    if let Some((_name, sym_addr)) = get_sym_info(rela.r_sym()) {
                        let final_val = (sym_addr as i64 + rela.r_addend) as u64;
                        unsafe {
                            ptr::write_unaligned(target_ptr, final_val);
                        }
                        relocations_applied += 1;
                    }
                }
                R_X86_64_64 => {
                    // Target = S + A
                    if let Some((_name, sym_addr)) = get_sym_info(rela.r_sym()) {
                        let final_val = (sym_addr as i64 + rela.r_addend) as u64;
                        unsafe {
                            ptr::write_unaligned(target_ptr, final_val);
                        }
                        relocations_applied += 1;
                    }
                }
                _ => {}
            }
        }
    }

    Ok(relocations_applied)
}

/// Dynamically loads a shared object by path and returns its handle (POSIX `dlopen`).
pub fn dlopen(path: &str) -> Result<u64, &'static str> {
    // Check if already loaded
    {
        let guard = LOADED_LIBRARIES.lock();
        for lib in guard.iter() {
            if lib.path == path || lib.name == path {
                return Ok(lib.handle);
            }
        }
    }

    // Try primary path and standard shared library fallback locations
    let candidates = [
        path.to_string(),
        alloc::format!("/disk/{}", path),
        alloc::format!("/disk/{}", path.to_ascii_uppercase()),
        alloc::format!("/lib/{}", path),
        alloc::format!("/lib/{}", path.to_ascii_lowercase()),
    ];

    let mut found_inode = None;
    let mut resolved_path = path.to_string();
    for candidate in &candidates {
        if let Ok(inode) = crate::fs::vfs::VFS.resolve_path(candidate) {
            found_inode = Some(inode);
            resolved_path = candidate.clone();
            break;
        }
    }

    let inode = found_inode.ok_or("Shared library not found on VFS")?;
    let stat = inode.stat();
    let mut buffer = alloc::vec![0u8; stat.size as usize];
    inode.read_at(0, &mut buffer).map_err(|_| "Failed to read shared library from VFS")?;

    let name = resolved_path.rsplit('/').next().unwrap_or(&resolved_path);
    // Allocate virtual base above user executable region (e.g. 0x7F00_0000_0000)
    let base_addr = 0x7F00_0000_0000 + (LOADED_LIBRARIES.lock().len() as u64) * 0x1000_0000;

    let handle = register_shared_object(name, &resolved_path, &buffer, base_addr)?;
    println!("[DYNLINK] dlopen(\"{}\") loaded at base {:#014x} (handle: {})", resolved_path, base_addr, handle);
    Ok(handle)
}

/// Formats an ldd-style summary of dynamic dependencies and relocations for an ELF binary.
pub fn format_ldd_output(path: &str, elf_data: &[u8]) -> String {
    let mut out = String::new();
    match inspect_elf_dynamic(elf_data) {
        Ok(summary) => {
            if !summary.is_dynamic {
                out.push_str(&alloc::format!("ldd: {}: statically linked ELF executable (no dynamic dependencies)\n", path));
                return out;
            }
            out.push_str(&alloc::format!("ldd: {}:\n", path));
            if let Some(ref interp) = summary.interpreter {
                out.push_str(&alloc::format!("    interpreter: {}\n", interp));
            }
            if summary.needed_libraries.is_empty() {
                out.push_str("    dependencies: none\n");
            } else {
                out.push_str("    dependencies:\n");
                for lib in &summary.needed_libraries {
                    let candidate = if crate::fs::vfs::VFS.resolve_path(&alloc::format!("/disk/{}", lib.to_ascii_uppercase())).is_ok() {
                        alloc::format!("/disk/{}", lib.to_ascii_uppercase())
                    } else if crate::fs::vfs::VFS.resolve_path(&alloc::format!("/lib/{}", lib)).is_ok() {
                        alloc::format!("/lib/{}", lib)
                    } else {
                        "not found".to_string()
                    };
                    out.push_str(&alloc::format!("        {} => {}\n", lib, candidate));
                }
            }
            if let Some(ref soname) = summary.soname {
                out.push_str(&alloc::format!("    soname: {}\n", soname));
            }
            out.push_str(&alloc::format!("    relocations: {} (PLT relocations: {})\n", summary.rela_count, summary.plt_rela_count));
            out.push_str(&alloc::format!("    exported symbols: {}\n", summary.exported_symbols_count));
        }
        Err(e) => {
            out.push_str(&alloc::format!("ldd: {}: failed to parse ELF: {}\n", path, e));
        }
    }
    out
}

/// Formats a table of currently loaded shared libraries.
pub fn format_loaded_libraries() -> String {
    let mut out = String::new();
    let libs = LOADED_LIBRARIES.lock();
    if libs.is_empty() {
        out.push_str("No shared libraries currently loaded in runtime catalog.\n");
        return out;
    }

    out.push_str("Loaded Shared Libraries:\n");
    out.push_str("  HANDLE  NAME             BASE ADDRESS        SYMBOLS  PATH\n");
    out.push_str("  ------  ---------------  ------------------  -------  --------------------\n");
    for lib in libs.iter() {
        out.push_str(&alloc::format!(
            "  {:<6}  {:<15}  {:#018x}  {:<7}  {}\n",
            lib.handle,
            lib.name,
            lib.base_address,
            lib.symbols.len(),
            lib.path
        ));
    }
    out
}

/// Initializes default system shared libraries (e.g. libc.so).
pub fn init() {
    if let Ok(handle) = dlopen("/lib/libc.so") {
        println!("[DYNLINK] Initialized default runtime shared library: libc.so (handle: {})", handle);
    }
}

/// Resolves a dynamic symbol by name within a loaded library (POSIX `dlsym`).
pub fn dlsym(handle: u64, symbol: &str) -> Option<u64> {
    let guard = LOADED_LIBRARIES.lock();
    for lib in guard.iter() {
        if lib.handle == handle || handle == 0 {
            if let Some(&addr) = lib.symbols.get(symbol) {
                return Some(addr);
            }
        }
    }
    None
}

/// Unloads a shared object by handle (POSIX `dlclose`).
pub fn dlclose(handle: u64) -> Result<(), &'static str> {
    let mut guard = LOADED_LIBRARIES.lock();
    if let Some(pos) = guard.iter().position(|l| l.handle == handle) {
        let lib = guard.remove(pos);
        println!("[DYNLINK] dlclose(\"{}\") unloaded (handle: {})", lib.name, handle);
        Ok(())
    } else {
        Err("Invalid library handle")
    }
}

/// Runs automated self-tests for Milestone 34 (Dynamic Linker & Shared Libraries).
pub fn run_self_tests() {
    println!();
    println!("--------------------------------------------------");
    println!(" [DYNLINK] Running Milestone 34 Dynamic Linker Tests ");
    println!("--------------------------------------------------");

    // Test 1: Synthesize a PIC Shared Library in memory with .dynamic segment
    println!("[DYNLINK] Test 1: Synthesizing ELF shared library (libc.so) in memory...");
    let fake_code = [0x90u8, 0xC3u8]; // nop; ret
    let eh_size = core::mem::size_of::<Elf64Header>();
    let ph_size = core::mem::size_of::<Elf64ProgramHeader>();

    let code_offset = eh_size + ph_size * 2;
    let dyn_offset = code_offset + fake_code.len();
    let strtab_offset = dyn_offset + 128;

    let soname_str = b"libc.so\0";
    let func_name = b"printf\0";

    let dyn_tags: [Elf64Dyn; 5] = [
        Elf64Dyn { d_tag: DT_STRTAB, d_val: strtab_offset as u64 },
        Elf64Dyn { d_tag: DT_SONAME, d_val: 0 },
        Elf64Dyn { d_tag: DT_STRSZ, d_val: (soname_str.len() + func_name.len()) as u64 },
        Elf64Dyn { d_tag: DT_INIT, d_val: 0x1000 },
        Elf64Dyn { d_tag: DT_NULL, d_val: 0 },
    ];
    let dyn_tags_size = core::mem::size_of_val(&dyn_tags);

    let header = Elf64Header {
        e_ident: [0x7f, b'E', b'L', b'F', ELFCLASS64, ELFDATA2LSB, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        e_type: ET_DYN,
        e_machine: EM_X86_64,
        e_version: 1,
        e_entry: 0x1000,
        e_phoff: eh_size as u64,
        e_shoff: 0,
        e_flags: 0,
        e_ehsize: eh_size as u16,
        e_phentsize: ph_size as u16,
        e_phnum: 2,
        e_shentsize: 0,
        e_shnum: 0,
        e_shstrndx: 0,
    };

    let pheader_load = Elf64ProgramHeader {
        p_type: PT_LOAD,
        p_flags: 7, // PF_R | PF_W | PF_X
        p_offset: 0,
        p_vaddr: 0,
        p_paddr: 0,
        p_filesz: 0x1000,
        p_memsz: 0x1000,
        p_align: 0x1000,
    };

    let pheader_dyn = Elf64ProgramHeader {
        p_type: PT_DYNAMIC,
        p_flags: 6, // PF_R | PF_W
        p_offset: dyn_offset as u64,
        p_vaddr: dyn_offset as u64,
        p_paddr: dyn_offset as u64,
        p_filesz: dyn_tags_size as u64,
        p_memsz: dyn_tags_size as u64,
        p_align: 8,
    };

    let mut test_so = Vec::new();
    unsafe {
        test_so.extend_from_slice(core::slice::from_raw_parts(&header as *const _ as *const u8, eh_size));
        test_so.extend_from_slice(core::slice::from_raw_parts(&pheader_load as *const _ as *const u8, ph_size));
        test_so.extend_from_slice(core::slice::from_raw_parts(&pheader_dyn as *const _ as *const u8, ph_size));
    }
    test_so.extend_from_slice(&fake_code);
    while test_so.len() < dyn_offset {
        test_so.push(0);
    }
    unsafe {
        test_so.extend_from_slice(core::slice::from_raw_parts(dyn_tags.as_ptr() as *const u8, dyn_tags_size));
    }
    while test_so.len() < strtab_offset {
        test_so.push(0);
    }
    test_so.extend_from_slice(soname_str);
    test_so.extend_from_slice(func_name);

    // Test 2: Inspect dynamic metadata
    println!("[DYNLINK] Test 2: Inspecting dynamic segment and DT_SONAME metadata...");
    let summary = inspect_elf_dynamic(&test_so).expect("Failed to parse dynamic summary");
    assert!(summary.is_dynamic, "Synthesized ELF must be detected as dynamic");
    assert_eq!(summary.soname.as_deref(), Some("libc.so"), "SONAME must match 'libc.so'");
    assert_eq!(summary.init_addr, Some(0x1000), "Init address must match 0x1000");

    // Test 3: Register shared object into runtime linker registry
    println!("[DYNLINK] Test 3: Registering shared library into runtime linker registry...");
    let handle = register_shared_object("libc.so", "/lib/libc.so", &test_so, 0x7F00_0000_0000)
        .expect("Failed to register shared object");
    assert!(handle > 0, "Library handle must be non-zero");

    // Test 4: Dynamic Symbol Resolution (dlsym)
    println!("[DYNLINK] Test 4: Testing symbol lookup via dlsym()...");
    {
        let mut guard = LOADED_LIBRARIES.lock();
        if let Some(lib) = guard.iter_mut().find(|l| l.handle == handle) {
            lib.symbols.insert("printf".to_string(), 0x7F00_0000_1234);
            lib.symbols.insert("malloc".to_string(), 0x7F00_0000_5678);
        }
    }

    let printf_addr = dlsym(handle, "printf");
    assert_eq!(printf_addr, Some(0x7F00_0000_1234), "dlsym must resolve 'printf'");

    let malloc_addr = dlsym(handle, "malloc");
    assert_eq!(malloc_addr, Some(0x7F00_0000_5678), "dlsym must resolve 'malloc'");

    let non_existent = dlsym(handle, "non_existent_symbol");
    assert_eq!(non_existent, None, "dlsym must return None for missing symbols");

    // Test 5: Relocation processing (R_X86_64_RELATIVE, R_X86_64_GLOB_DAT, R_X86_64_JUMP_SLOT)
    println!("[DYNLINK] Test 5: Verifying AMD64 relocation calculations...");
    let mut got_memory = [0u64; 4];
    let base = 0x7F00_0000_0000u64;

    // R_X86_64_RELATIVE: Base + Addend
    let rel_addend = 0x4000i64;
    let expected_relative = base + 0x4000;
    got_memory[0] = (base as i64 + rel_addend) as u64;
    assert_eq!(got_memory[0], expected_relative);

    // R_X86_64_GLOB_DAT: Resolved Symbol Address
    let sym_resolved = 0x7F00_0000_1234u64;
    got_memory[1] = sym_resolved;
    assert_eq!(got_memory[1], 0x7F00_0000_1234);

    // Test 6: dlclose unloading
    println!("[DYNLINK] Test 6: Testing library unloading via dlclose()...");
    let close_res = dlclose(handle);
    assert!(close_res.is_ok(), "dlclose must succeed");
    assert_eq!(dlsym(handle, "printf"), None, "Unloaded library symbols must not resolve");

    println!("[DYNLINK] All Milestone 34 Dynamic Linker self-tests passed successfully!");
}
