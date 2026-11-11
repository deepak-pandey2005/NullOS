//! # Live Kernel Stack Unwinder (Phase 18 - M40)
//!
//! ## Overview
//! Walks kernel execution call stacks safely without triggering recursive faults or
//! acquiring locks during `#PF` / `#GP` / `panic` contexts:
//! - **Lock-Free CR3 Page-Table Verification**: Validates every frame pointer (`RBP`)
//!   and stack probe directly against hardware `CR3` (supporting 4 KiB, 2 MiB huge,
//!   and 1 GiB huge pages) without touching any `Spinlock`.
//! - **Frame-Pointer Chain Walker**: Follows SysV AMD64 `[rbp] -> saved_rbp`,
//!   `[rbp + 8] -> return_rip` frames with monotonic growth and cycle guards.
//! - **Hybrid Stack-Slot Fallback**: When leaf or assembly frames omit `RBP`, scans
//!   bounded validated stack words for return addresses inside kernel `.text` bounds.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::sym::{self, ResolvedSymbol};

/// Maximum stack frames captured per unwind invocation.
pub const MAX_UNWIND_FRAMES: usize = 24;

/// Single unwound call-stack frame.
#[derive(Debug, Clone)]
pub struct StackFrame {
    /// Zero-based frame index (`0` = innermost/faulting site).
    pub index: usize,
    /// Instruction pointer (RIP) or caller return address.
    pub rip: u64,
    /// Frame base pointer (`RBP`) associated with this frame (0 if heuristic scan).
    pub rbp: u64,
    /// Symbolizer resolution for `rip`, if within kernel symbol bounds.
    pub symbol: Option<ResolvedSymbol>,
}

impl StackFrame {
    /// Formats this frame as `#NN 0x0000000000xxxxxx in <sym+offset>`.
    pub fn format_line(&self) -> String {
        match &self.symbol {
            Some(sym_info) => format!(
                "  #{:02} {:#018x} in {}",
                self.index,
                self.rip,
                sym_info.format_compact()
            ),
            None => format!("  #{:02} {:#018x} in <unknown>", self.index, self.rip),
        }
    }
}

/// Checks whether `vaddr` is canonical on x86_64 (`[0..0x0000_7FFF_FFFF_FFFF]` or `[0xFFFF_8000_0000_0000..]`).
#[inline]
pub fn is_canonical(vaddr: u64) -> bool {
    let top = vaddr >> 47;
    top == 0 || top == 0x1FFFF
}

/// Performs a completely lock-free 4-level CR3 walk verifying `vaddr` is mapped `PRESENT`.
///
/// Safe inside interrupt handlers, `#PF`/`#GP` exception context, and kernel `panic()`.
/// Assumes NullOS identity-maps low physical page-table frames.
pub fn is_addr_mapped_lockfree(vaddr: u64) -> bool {
    if vaddr < 0x1000 || !is_canonical(vaddr) {
        return false;
    }

    let cr3: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
    }
    let pml4_phys = cr3 & 0x000F_FFFF_FFFF_F000;
    if pml4_phys == 0 || pml4_phys >= 0x4000_0000 {
        return false;
    }

    let pml4_idx = ((vaddr >> 39) & 0x1FF) as usize;
    let pdpt_idx = ((vaddr >> 30) & 0x1FF) as usize;
    let pd_idx = ((vaddr >> 21) & 0x1FF) as usize;
    let pt_idx = ((vaddr >> 12) & 0x1FF) as usize;

    unsafe {
        let pml4e = *((pml4_phys as *const u64).add(pml4_idx));
        if (pml4e & 1) == 0 {
            return false;
        }
        let pdpt_phys = pml4e & 0x000F_FFFF_FFFF_F000;
        if pdpt_phys == 0 || pdpt_phys >= 0x4000_0000 {
            return false;
        }

        let pdpte = *((pdpt_phys as *const u64).add(pdpt_idx));
        if (pdpte & 1) == 0 {
            return false;
        }
        if (pdpte & (1 << 7)) != 0 {
            return true; // 1 GiB huge page
        }
        let pd_phys = pdpte & 0x000F_FFFF_FFFF_F000;
        if pd_phys == 0 || pd_phys >= 0x4000_0000 {
            return false;
        }

        let pde = *((pd_phys as *const u64).add(pd_idx));
        if (pde & 1) == 0 {
            return false;
        }
        if (pde & (1 << 7)) != 0 {
            return true; // 2 MiB huge page
        }
        let pt_phys = pde & 0x000F_FFFF_FFFF_F000;
        if pt_phys == 0 || pt_phys >= 0x4000_0000 {
            return false;
        }

        let pte = *((pt_phys as *const u64).add(pt_idx));
        (pte & 1) != 0
    }
}

/// Validates that a 16-byte stack frame slot `[rbp, rbp + 16)` is aligned and mapped.
#[inline]
fn is_readable_frame_slot(rbp: u64) -> bool {
    if rbp < 0x10_0000 || (rbp & 0x7) != 0 {
        return false;
    }
    is_addr_mapped_lockfree(rbp) && is_addr_mapped_lockfree(rbp.saturating_add(15))
}

/// Unwinds up to `max_frames` frames starting from explicit `(rbp, rsp, rip)` registers.
///
/// Ideal for exception & panic dumps where the interrupted/faulting context supplies exact
/// register state. Falls back to bounded stack-slot scanning if frame pointers are unavailable.
pub fn unwind_from(rbp: u64, rsp: u64, rip: u64, max_frames: usize) -> Vec<StackFrame> {
    let limit = max_frames.clamp(1, MAX_UNWIND_FRAMES);
    let mut frames = Vec::with_capacity(limit);

    if rip != 0 {
        frames.push(StackFrame {
            index: 0,
            rip,
            rbp,
            symbol: sym::resolve_addr(rip),
        });
    }

    // 1. Primary RBP-chain frame unwind
    let mut cur_rbp = rbp;
    for _ in 0..limit {
        if frames.len() >= limit || !is_readable_frame_slot(cur_rbp) {
            break;
        }

        let saved_rbp = unsafe { *(cur_rbp as *const u64) };
        let ret_rip = unsafe { *((cur_rbp + 8) as *const u64) };

        if !sym::is_kernel_addr(ret_rip) {
            break;
        }

        frames.push(StackFrame {
            index: frames.len(),
            rip: ret_rip,
            rbp: saved_rbp,
            symbol: sym::resolve_addr(ret_rip),
        });

        // Guard against circular or backwards stack frames.
        if saved_rbp <= cur_rbp || saved_rbp.saturating_sub(cur_rbp) > 256 * 1024 {
            break;
        }
        cur_rbp = saved_rbp;
    }

    // 2. Conservative stack scan fallback if frame pointer chain stopped early
    if frames.len() <= 1 && rsp >= 0x10_0000 && (rsp & 0x7) == 0 {
        let scan_words = 128usize; // Scan up to 1 KiB of top stack words
        for i in 0..scan_words {
            if frames.len() >= limit {
                break;
            }
            let slot_addr = rsp.saturating_add((i as u64) * 8);
            if !is_addr_mapped_lockfree(slot_addr) {
                break;
            }
            let cand = unsafe { *(slot_addr as *const u64) };
            if sym::is_kernel_addr(cand) && frames.last().map_or(true, |f| f.rip != cand) {
                frames.push(StackFrame {
                    index: frames.len(),
                    rip: cand,
                    rbp: 0,
                    symbol: sym::resolve_addr(cand),
                });
            }
        }
    }

    frames
}

/// Captures live `RIP`, `RSP`, and `RBP` from the current CPU call site and unwinds callers.
#[inline(never)]
pub fn capture_backtrace(max_frames: usize) -> Vec<StackFrame> {
    let cur_rip: u64;
    let cur_rbp: u64;
    let cur_rsp: u64;
    unsafe {
        core::arch::asm!(
            "lea {rip}, [rip + 0]",
            "mov {rbp}, rbp",
            "mov {rsp}, rsp",
            rip = out(reg) cur_rip,
            rbp = out(reg) cur_rbp,
            rsp = out(reg) cur_rsp,
            options(nomem, nostack)
        );
    }
    unwind_from(cur_rbp, cur_rsp, cur_rip, max_frames)
}

/// Formats a captured slice of [`StackFrame`] items as a printable multi-line call trace.
pub fn format_backtrace(frames: &[StackFrame]) -> String {
    let mut out = String::new();
    for frame in frames {
        out.push_str(&frame.format_line());
        out.push('\n');
    }
    out
}

