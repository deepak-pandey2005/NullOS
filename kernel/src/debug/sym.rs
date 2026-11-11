//! # Kernel ELF Symbolizer & Rust Symbol Demangler (Phase 18 - M40)
//!
//! ## Overview
//! Resolves runtime 64-bit instruction/data addresses to human-readable kernel symbols,
//! function base addresses, and byte offsets (`symbol + 0xoffset`):
//! - **Runtime `KSYM` v1 Symbol Table**: Loaded from `/disk/KERNEL.SYM` (generated at
//!   build time from `kernel.elf` via `nm`), indexed by sorted virtual address for
//!   `O(log N)` binary-search resolution.
//! - **Bootstrap Anchor Fallback**: Static table anchored to `__kernel_start`,
//!   `kernel_main`, `timer_handler`, `exception_handler`, `syscall_dispatcher`, and
//!   `__kernel_end` so symbolization succeeds even prior to VFS initialization.
//! - **Rust v0 & Legacy Demangler**: Translates `_RN...` (Rust v0) and `_ZN...E`
//!   (Itanium legacy) mangled names into clean readable Rust paths.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::fs::vfs::{OpenFlags, VFS};
use crate::sync::Spinlock;

/// Binary magic header bytes `"KSYM"` identifying NullOS symbol table blobs.
pub const KSYM_MAGIC: [u8; 4] = *b"KSYM";
/// Current supported `KSYM` binary format version.
pub const KSYM_VERSION: u32 = 1;
/// Path on the NullOS FAT32 disk where the build script places `KERNEL.SYM`.
pub const DEFAULT_KSYM_PATH: &str = "/disk/KERNEL.SYM";

unsafe extern "C" {
    static __kernel_start: u8;
    static __kernel_end: u8;
    fn kernel_main(boot_info: &'static bootinfo::BootInfo) -> !;
    fn timer_handler();
    fn exception_handler(ctx: &crate::arch::x86_64::idt::ExceptionContext);
    fn syscall_dispatcher(num: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> u64;
}

/// Single sorted symbol entry in the runtime kernel symbol table.
#[derive(Debug, Clone)]
pub struct SymbolEntry {
    /// Virtual address where the symbol begins.
    pub addr: u64,
    /// Size of the symbol in bytes (0 if unsized / assembly label).
    pub size: u64,
    /// Demangled symbol name.
    pub name: String,
}

/// Result of resolving an arbitrary 64-bit address against the symbol table.
#[derive(Debug, Clone)]
pub struct ResolvedSymbol {
    /// Demangled symbol name.
    pub name: String,
    /// Base virtual address of the matched symbol.
    pub base: u64,
    /// Reported ELF size of the symbol (or 0 if unknown).
    pub size: u64,
    /// Byte offset past `base` (`addr - base`).
    pub offset: u64,
    /// True if backed by full `KSYM` debug table, false if coarse anchor fallback.
    pub exact: bool,
}

impl ResolvedSymbol {
    /// Formats this resolved symbol as `name+0xoffset` (or exact `name` if `offset == 0`).
    pub fn format_compact(&self) -> String {
        if self.offset == 0 {
            self.name.clone()
        } else {
            format!("{}+{:#x}", self.name, self.offset)
        }
    }
}

struct SymbolStore {
    loaded_ksym: bool,
    entries: Vec<SymbolEntry>,
}

impl SymbolStore {
    const fn new() -> Self {
        Self {
            loaded_ksym: false,
            entries: Vec::new(),
        }
    }
}

static SYM_STORE: Spinlock<SymbolStore> = Spinlock::new(SymbolStore::new());
static VFS_LOAD_ATTEMPTED: AtomicBool = AtomicBool::new(false);
static KERNEL_TEXT_START: AtomicU64 = AtomicU64::new(0x0010_0000);
static KERNEL_TEXT_END: AtomicU64 = AtomicU64::new(0x0038_0000);

/// Returns `(kernel_text_start, kernel_text_end)` bounds of executable kernel `.text`.
#[inline]
pub fn kernel_image_bounds() -> (u64, u64) {
    (
        KERNEL_TEXT_START.load(Ordering::Relaxed),
        KERNEL_TEXT_END.load(Ordering::Relaxed),
    )
}

/// Returns `true` if `addr` falls within executable kernel `.text` bounds.
#[inline]
pub fn is_kernel_addr(addr: u64) -> bool {
    let (start, end) = kernel_image_bounds();
    addr >= start && addr < end
}

/// Initializes the symbolizer with built-in bootstrap anchor symbols.
pub fn init() {
    let mut anchors = Vec::new();
    let kstart = (core::ptr::addr_of!(__kernel_start) as u64).max(0x0010_0000);
    let fns = [
        kernel_main as *const () as usize as u64,
        timer_handler as *const () as usize as u64,
        exception_handler as *const () as usize as u64,
        syscall_dispatcher as *const () as usize as u64,
        resolve_addr as *const () as usize as u64,
    ];
    let kend_linker = core::ptr::addr_of!(__kernel_end) as u64;
    let max_fn = fns.iter().copied().max().unwrap_or(kstart);
    let estimated_end = if kend_linker > kstart + 0x1000 {
        kend_linker
    } else {
        (max_fn + 0x10_0000).max(0x0038_0000)
    };
    KERNEL_TEXT_START.store(kstart, Ordering::Relaxed);
    KERNEL_TEXT_END.store(estimated_end, Ordering::Relaxed);

    anchors.push(SymbolEntry {
        addr: kstart,
        size: 0,
        name: String::from("__kernel_start"),
    });
    anchors.push(SymbolEntry {
        addr: fns[0],
        size: 0,
        name: String::from("kernel_main"),
    });
    anchors.push(SymbolEntry {
        addr: fns[1],
        size: 0,
        name: String::from("timer_handler"),
    });
    anchors.push(SymbolEntry {
        addr: fns[2],
        size: 0,
        name: String::from("exception_handler"),
    });
    anchors.push(SymbolEntry {
        addr: fns[3],
        size: 0,
        name: String::from("syscall_dispatcher"),
    });
    anchors.sort_by_key(|e| e.addr);

    let mut store = SYM_STORE.lock();
    if !store.loaded_ksym {
        store.entries = anchors;
    }
}

/// Attempts once to load full symbols from `/disk/KERNEL.SYM` via VFS after disk mount.
///
/// Safe to call repeatedly; if `/disk/KERNEL.SYM` has already been loaded or attempted,
/// subsequent calls return immediately without re-reading disk.
pub fn ensure_loaded_from_vfs() -> bool {
    if VFS_LOAD_ATTEMPTED.swap(true, Ordering::SeqCst) {
        return SYM_STORE.lock().loaded_ksym;
    }
    load_from_vfs(DEFAULT_KSYM_PATH).is_ok()
}

/// Loads and parses a binary `KSYM` v1 symbol table file from VFS (`path`).
pub fn load_from_vfs(path: &str) -> Result<usize, &'static str> {
    let file = VFS
        .open(path, OpenFlags::READ_ONLY)
        .map_err(|_| "KERNEL.SYM not found on VFS")?;
    let size = file.stat().size;
    if size < 16 || size > 4 * 1024 * 1024 {
        return Err("Invalid KSYM file size");
    }

    let mut blob = alloc::vec![0u8; size];
    let n = file
        .read_at(0, &mut blob)
        .map_err(|_| "Failed reading KSYM file")?;
    if n < 16 {
        return Err("Short KSYM read");
    }

    load_ksym_bytes(&blob[..n])
}

/// Parses raw `KSYM` v1 bytes into the active runtime symbol store.
pub fn load_ksym_bytes(data: &[u8]) -> Result<usize, &'static str> {
    if data.len() < 16 || data[0..4] != KSYM_MAGIC {
        return Err("Bad KSYM magic");
    }
    let version = u32::from_le_bytes(data[4..8].try_into().unwrap());
    if version != KSYM_VERSION {
        return Err("Unsupported KSYM version");
    }
    let count = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;

    let mut entries = Vec::with_capacity(count.min(16384));
    let mut off = 16usize;

    for _ in 0..count {
        if off + 18 > data.len() {
            break;
        }
        let addr = u64::from_le_bytes(data[off..off + 8].try_into().unwrap());
        let sym_size = u64::from_le_bytes(data[off + 8..off + 16].try_into().unwrap());
        let name_len = u16::from_le_bytes(data[off + 16..off + 18].try_into().unwrap()) as usize;
        off += 18;
        if off + name_len > data.len() {
            break;
        }
        let raw_name = core::str::from_utf8(&data[off..off + name_len]).unwrap_or("<invalid>");
        off += name_len;

        entries.push(SymbolEntry {
            addr,
            size: sym_size,
            name: demangle_symbol(raw_name),
        });
    }

    if entries.is_empty() {
        return Err("No valid symbols decoded from KSYM");
    }

    entries.sort_by_key(|e| e.addr);
    let total = entries.len();
    if let (Some(first), Some(last)) = (entries.first(), entries.last()) {
        KERNEL_TEXT_START.store(first.addr, Ordering::Relaxed);
        KERNEL_TEXT_END.store(last.addr.saturating_add(last.size.max(64)), Ordering::Relaxed);
    }

    let mut store = SYM_STORE.lock();
    store.entries = entries;
    store.loaded_ksym = true;
    Ok(total)
}

/// Resolves an arbitrary virtual address to its nearest preceding kernel symbol via binary search.
pub fn resolve_addr(addr: u64) -> Option<ResolvedSymbol> {
    if !is_kernel_addr(addr) {
        return None;
    }

    let store = SYM_STORE.lock();
    if store.entries.is_empty() {
        return None;
    }

    // Find first entry strictly greater than `addr`, then step back one slot.
    let idx = store.entries.partition_point(|e| e.addr <= addr);
    if idx == 0 {
        return None;
    }

    let entry = &store.entries[idx - 1];
    let offset = addr - entry.addr;

    // Guard against stale matches way past known symbol boundaries when exact sizes are known.
    if entry.size > 0 && offset > entry.size.max(0x4000) {
        return Some(ResolvedSymbol {
            name: entry.name.clone(),
            base: entry.addr,
            size: entry.size,
            offset,
            exact: false,
        });
    }

    Some(ResolvedSymbol {
        name: entry.name.clone(),
        base: entry.addr,
        size: entry.size,
        offset,
        exact: store.loaded_ksym,
    })
}

/// Searches loaded kernel symbols whose demangled name contains `pattern` (up to `limit` hits).
pub fn search_symbols(pattern: &str, limit: usize) -> Vec<SymbolEntry> {
    let store = SYM_STORE.lock();
    let mut matches = Vec::new();
    for entry in &store.entries {
        if entry.name.contains(pattern) {
            matches.push(entry.clone());
            if matches.len() >= limit {
                break;
            }
        }
    }
    matches
}

/// Returns `(loaded_count, full_ksym_active)`.
pub fn stats() -> (usize, bool) {
    let store = SYM_STORE.lock();
    (store.entries.len(), store.loaded_ksym)
}

/// Demangles Rust v0 (`_RN...`), Rust legacy (`_ZN...E`), or returns unmangled identifiers intact.
pub fn demangle_symbol(mangled: &str) -> String {
    if mangled.starts_with("_RN") {
        if let Some(decoded) = demangle_rust_v0(mangled) {
            if !decoded.is_empty() {
                return decoded;
            }
        }
    } else if mangled.starts_with("_ZN") {
        if let Some(decoded) = demangle_legacy_itanium(mangled) {
            if !decoded.is_empty() {
                return decoded;
            }
        }
    }
    String::from(mangled)
}

/// Lightweight Rust v0 path decoder extracting hierarchical crate::mod::item components.
fn demangle_rust_v0(s: &str) -> Option<String> {
    let rest = s.strip_prefix("_R")?;
    let mut bytes = rest.as_bytes();
    let mut parts: Vec<String> = Vec::new();

    // Parse simple path chain (N.. / C.. / M..) stopping on unrecognized generics/backrefs.
    while !bytes.is_empty() && parts.len() < 16 {
        match bytes[0] {
            b'N' => {
                // Namespace tag (`N` + namespace char + parent path + identifier)
                if bytes.len() < 2 {
                    break;
                }
                bytes = &bytes[2..];
            }
            b'C' => {
                // Crate root identifier (`C` + optional disambiguator `s<base62>_` + ident)
                bytes = &bytes[1..];
                if bytes.first() == Some(&b's') {
                    bytes = skip_base62_sep(&bytes[1..]);
                }
                if let Some((ident, next)) = parse_v0_ident(bytes) {
                    parts.push(ident);
                    bytes = next;
                } else {
                    break;
                }
            }
            b'M' | b'X' => {
                // Inherent or trait impl; skip impl disambiguator tag and advance
                bytes = &bytes[1..];
                if bytes.first() == Some(&b's') {
                    bytes = skip_base62_sep(&bytes[1..]);
                }
            }
            b'0'..=b'9' | b'u' => {
                if let Some((ident, next)) = parse_v0_ident(bytes) {
                    parts.push(ident);
                    bytes = next;
                } else {
                    break;
                }
            }
            b's' => {
                bytes = skip_base62_sep(&bytes[1..]);
            }
            _ => {
                // Extract remaining readable length-prefixed identifiers heuristically
                if let Some((ident, next)) = find_next_v0_ident(bytes) {
                    parts.push(ident);
                    bytes = next;
                } else {
                    break;
                }
            }
        }
    }

    if parts.is_empty() {
        None
    } else {
        // v0 paths encode innermost last when parsed left-to-right or crates first depending on nesting;
        // reverse only if crate name ("kernel" / "core" / "alloc") appears at tail.
        if parts.last().map(|p| p.as_str()) == Some("kernel")
            || parts.last().map(|p| p.as_str()) == Some("core")
            || parts.last().map(|p| p.as_str()) == Some("alloc")
        {
            parts.reverse();
        }
        Some(parts.join("::"))
    }
}

fn skip_base62_sep(mut b: &[u8]) -> &[u8] {
    while let Some(&ch) = b.first() {
        b = &b[1..];
        if ch == b'_' {
            break;
        }
    }
    b
}

fn parse_v0_ident(mut b: &[u8]) -> Option<(String, &[u8])> {
    if b.first() == Some(&b'u') {
        b = &b[1..];
    }
    let mut len = 0usize;
    let mut digits = 0usize;
    while digits < b.len() && b[digits].is_ascii_digit() {
        len = len.saturating_mul(10).saturating_add((b[digits] - b'0') as usize);
        digits += 1;
    }
    if digits == 0 || len == 0 || len > 128 {
        return None;
    }
    b = &b[digits..];
    if b.first() == Some(&b'_') {
        b = &b[1..];
    }
    if b.len() < len {
        return None;
    }
    let ident_str = core::str::from_utf8(&b[..len]).ok()?;
    Some((String::from(ident_str), &b[len..]))
}

fn find_next_v0_ident(b: &[u8]) -> Option<(String, &[u8])> {
    for i in 0..b.len() {
        if b[i].is_ascii_digit() {
            if let Some(res) = parse_v0_ident(&b[i..]) {
                return Some(res);
            }
        }
    }
    None
}

/// Legacy Itanium `_ZN<len><name>...E` demangler.
fn demangle_legacy_itanium(s: &str) -> Option<String> {
    let mut b = s.strip_prefix("_ZN")?.as_bytes();
    let mut parts: Vec<String> = Vec::new();

    while !b.is_empty() && b[0] != b'E' {
        let mut len = 0usize;
        let mut i = 0usize;
        while i < b.len() && b[i].is_ascii_digit() {
            len = len.saturating_mul(10).saturating_add((b[i] - b'0') as usize);
            i += 1;
        }
        if i == 0 || len == 0 || i + len > b.len() {
            break;
        }
        let raw = core::str::from_utf8(&b[i..i + len]).ok()?;
        // Strip legacy Rust hash suffix (`17h0123456789abcdef`)
        if !(raw.len() == 17 && raw.starts_with('h') && raw[1..].bytes().all(|c| c.is_ascii_hexdigit())) {
            parts.push(String::from(raw));
        }
        b = &b[i + len..];
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("::"))
    }
}
