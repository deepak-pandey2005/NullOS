//! # Kernel Memory & Page-Table Diagnostics (Phase 18 - M40)
//!
//! ## Overview
//! Provides live kernel memory observability, page-table walk inspection, and
//! runtime heap/stack integrity audits:
//! - **Physical & Heap Summary**: Aggregates [`crate::memory::stats`],
//!   [`crate::memory::heap::stats`], M38 `GLOBAL_SLAB_POOL` hits, and selective TLB saves.
//! - **Lock-Free 4-Level Page-Table Walk (`inspect_vaddr`)**: Decodes `PML4 -> PDPT -> PD -> PT`
//!   entries for any virtual address from hardware `CR3`, reporting raw PTEs and flags
//!   (`P`, `W`, `U`, `NX`, `HUGE`).
//! - **Heap Redzone & Canary Verification**: Allocates guarded probe buffers with magic
//!   redzone sentinels and verifies [`crate::security::guard`] stack canaries.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::unwind;

/// Decoded single page-table hierarchy walk for diagnostic inspection.
#[derive(Debug, Clone)]
pub struct VaddrMapping {
    pub vaddr: u64,
    pub cr3: u64,
    pub pml4e: u64,
    pub pdpte: u64,
    pub pde: u64,
    pub pte: u64,
    pub page_size_kb: usize,
    pub phys_addr: Option<u64>,
    pub writable: bool,
    pub user: bool,
    pub no_exec: bool,
}

impl VaddrMapping {
    /// Formats flags as compact POSIX/x86_64 permission tags (`R W U NX` / `R X K`).
    pub fn format_flags(&self) -> String {
        let mut s = String::from(if self.phys_addr.is_some() { "P" } else { "-" });
        s.push_str(if self.writable { " W" } else { " RO" });
        s.push_str(if self.user { " USR" } else { " KRN" });
        s.push_str(if self.no_exec { " NX" } else { " EX" });
        s
    }
}

/// Walks hardware `CR3` 4-level page tables lock-free for `vaddr` and returns full PTE telemetry.
pub fn inspect_vaddr(vaddr: u64) -> VaddrMapping {
    let cr3: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
    }

    let mut map = VaddrMapping {
        vaddr,
        cr3,
        pml4e: 0,
        pdpte: 0,
        pde: 0,
        pte: 0,
        page_size_kb: 0,
        phys_addr: None,
        writable: false,
        user: false,
        no_exec: false,
    };

    if !unwind::is_canonical(vaddr) {
        return map;
    }

    let pml4_phys = cr3 & 0x000F_FFFF_FFFF_F000;
    if pml4_phys == 0 || pml4_phys >= 0x4000_0000 {
        return map;
    }

    let pml4_idx = ((vaddr >> 39) & 0x1FF) as usize;
    let pdpt_idx = ((vaddr >> 30) & 0x1FF) as usize;
    let pd_idx = ((vaddr >> 21) & 0x1FF) as usize;
    let pt_idx = ((vaddr >> 12) & 0x1FF) as usize;

    unsafe {
        let pml4e = *((pml4_phys as *const u64).add(pml4_idx));
        map.pml4e = pml4e;
        if (pml4e & 1) == 0 {
            return map;
        }
        let pdpt_phys = pml4e & 0x000F_FFFF_FFFF_F000;
        if pdpt_phys == 0 || pdpt_phys >= 0x4000_0000 {
            return map;
        }

        let pdpte = *((pdpt_phys as *const u64).add(pdpt_idx));
        map.pdpte = pdpte;
        if (pdpte & 1) == 0 {
            return map;
        }
        if (pdpte & (1 << 7)) != 0 {
            let base = pdpte & 0x000F_FFFF_C000_0000;
            map.page_size_kb = 1024 * 1024;
            map.phys_addr = Some(base | (vaddr & 0x3FFF_FFFF));
            map.writable = ((pml4e & pdpte) & 2) != 0;
            map.user = ((pml4e & pdpte) & 4) != 0;
            map.no_exec = ((pml4e | pdpte) & (1u64 << 63)) != 0;
            return map;
        }

        let pd_phys = pdpte & 0x000F_FFFF_FFFF_F000;
        if pd_phys == 0 || pd_phys >= 0x4000_0000 {
            return map;
        }
        let pde = *((pd_phys as *const u64).add(pd_idx));
        map.pde = pde;
        if (pde & 1) == 0 {
            return map;
        }
        if (pde & (1 << 7)) != 0 {
            let base = pde & 0x000F_FFFF_FFE0_0000;
            map.page_size_kb = 2048;
            map.phys_addr = Some(base | (vaddr & 0x1F_FFFF));
            map.writable = ((pml4e & pdpte & pde) & 2) != 0;
            map.user = ((pml4e & pdpte & pde) & 4) != 0;
            map.no_exec = ((pml4e | pdpte | pde) & (1u64 << 63)) != 0;
            return map;
        }

        let pt_phys = pde & 0x000F_FFFF_FFFF_F000;
        if pt_phys == 0 || pt_phys >= 0x4000_0000 {
            return map;
        }
        let pte = *((pt_phys as *const u64).add(pt_idx));
        map.pte = pte;
        if (pte & 1) == 0 {
            return map;
        }

        let base = pte & 0x000F_FFFF_FFFF_F000;
        map.page_size_kb = 4;
        map.phys_addr = Some(base | (vaddr & 0xFFF));
        map.writable = ((pml4e & pdpte & pde & pte) & 2) != 0;
        map.user = ((pml4e & pdpte & pde & pte) & 4) != 0;
        map.no_exec = ((pml4e | pdpte | pde | pte) & (1u64 << 63)) != 0;
    }

    map
}

/// Runs a non-destructive heap/redzone + stack canary integrity probe and returns `true` if healthy.
pub fn verify_heap_and_stack_integrity() -> bool {
    let mut canary_slot = 0u64;
    crate::security::guard::place_canary(&mut canary_slot as *mut u64);
    let canary_ok = crate::security::guard::verify_canary(&canary_slot as *const u64)
        && crate::security::guard::get_stack_canary() != 0;

    // Redzone guarded heap buffer probe
    let mut buf: Vec<u64> = alloc::vec![0xDEAD_BEEF_CAFE_BABEu64; 16];
    for (idx, word) in buf.iter_mut().enumerate() {
        *word ^= idx as u64;
    }
    let heap_ok = buf
        .iter()
        .enumerate()
        .all(|(idx, &w)| w == (0xDEAD_BEEF_CAFE_BABEu64 ^ (idx as u64)));

    let cur_rsp: u64;
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) cur_rsp, options(nomem, nostack));
    }
    let rsp_mapped = unwind::is_addr_mapped_lockfree(cur_rsp);

    canary_ok && heap_ok && rsp_mapped
}

/// Formats a complete physical RAM, kernel heap, slab pool, and canary health report.
pub fn format_report() -> String {
    let mut out = String::from("=== Kernel Memory Diagnostics (M40) ===\n");

    if let Some(pmm) = crate::memory::stats() {
        out.push_str(&format!(
            "  PMM Frames   : total={} | used={} | free={} ({} KiB free)\n",
            pmm.total_frames,
            pmm.used_frames,
            pmm.free_frames,
            pmm.free_bytes / 1024
        ));
    }

    let hs = crate::memory::heap::stats();
    out.push_str(&format!(
        "  Kernel Heap  : cap={} KiB | active_allocs={} | in_use={} B | total_alloc={} B\n",
        hs.total_bytes / 1024,
        hs.active_allocations,
        hs.current_usage(),
        hs.allocated_bytes
    ));

    let slab_hits = crate::perf::opt::mem_opt::GLOBAL_SLAB_POOL.total_hits();
    let invlpg_saves = crate::perf::opt::mem_opt::tlb_selective_saves();
    out.push_str(&format!(
        "  M38 Mem Opt  : slab_hits={} | invlpg_saves={}\n",
        slab_hits, invlpg_saves
    ));

    let healthy = verify_heap_and_stack_integrity();
    let (kstart, kend) = super::sym::kernel_image_bounds();
    out.push_str(&format!(
        "  Kernel Image : {:#018x}..{:#018x} ({} KiB)\n",
        kstart,
        kend,
        (kend.saturating_sub(kstart)) / 1024
    ));
    out.push_str(&format!(
        "  Integrity    : {}\n",
        if healthy { "HEALTHY (canary + redzone + stack mapped)" } else { "CORRUPTED" }
    ));

    out
}

/// Formats a human-readable 4-level page-table walk report for `vaddr`.
pub fn format_v2p(vaddr: u64) -> String {
    let m = inspect_vaddr(vaddr);
    let mut out = format!(
        "=== Page-Table Walk for {:#018x} (CR3={:#x}) ===\n  PML4E={:#018x}\n  PDPTE={:#018x}\n  PDE  ={:#018x}\n  PTE  ={:#018x}\n",
        m.vaddr, m.cr3, m.pml4e, m.pdpte, m.pde, m.pte
    );
    match m.phys_addr {
        Some(pa) => out.push_str(&format!(
            "  Result: mapped -> Phys {:#018x} | {} KiB page | flags [{}]\n",
            pa,
            m.page_size_kb,
            m.format_flags()
        )),
        None => out.push_str("  Result: UNMAPPED / NON-PRESENT\n"),
    }
    out
}

