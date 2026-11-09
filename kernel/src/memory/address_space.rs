//! # Process Address Space Abstraction
//!
//! ## Overview
//! On x86_64, process memory isolation is enforced using distinct 4-level page table
//! hierarchies. Each user process possesses its own independent [`AddressSpace`] anchored
//! by a private Page Map Level 4 (PML4) physical frame.
//!
//! ## Canonical 48-bit Virtual Address Split
//! ```text
//! +-----------------------------------------+ 0xFFFF_FFFF_FFFF_FFFF
//! |   Higher Half: Shared Kernel Space      | (PML4 entries 256..511)
//! |   - Recursive Mapping (PML4[511])       |
//! |   - Direct Physical Map / Kernel Image  |
//! |   - Kernel Heap & APIC/MMIO Apertures   |
//! +-----------------------------------------+ 0xFFFF_8000_0000_0000
//! |            Non-Canonical Hole           | (Triggers #GP if accessed)
//! +-----------------------------------------+ 0x0000_7FFF_FFFF_FFFF
//! |   Lower Half: Private User Space        | (PML4 entries 0..255)
//! |   - Process Code (.text) & Data (.data) |
//! |   - Dynamic Shared Libraries / mmap     |
//! |   - User Stack (grows downwards)        |
//! +-----------------------------------------+ 0x0000_0000_0000_0000
//! ```
//!
//! ## Address Space Invariants
//! - Shared kernel mappings (higher-half and identity maps) are copied into each new PML4,
//!   ensuring uninterrupted kernel execution and interrupt dispatch across context switches.
//! - Lower-half user mappings are strictly private to each process.
//! - Entry 511 of the PML4 points back to the PML4 itself, enabling recursive page table manipulation.

use super::address::{Page, PhysAddr, PhysFrame, VirtAddr, PAGE_SIZE_4K};
use super::paging::{self, KernelPageTable, PageTable, PageTableFlags};

/// An isolated virtual address space for a user process.
///
/// Wraps a dedicated PML4 root table frame. Loading this frame into the CPU's `%cr3`
/// register activates the corresponding process's virtual memory mappings.
pub struct AddressSpace {
    pml4_frame: PhysFrame,
}

impl AddressSpace {
    /// Creates a new isolated user address space.
    ///
    /// # Mechanics:
    /// 1. Allocates a new physical frame from the PMM to serve as the PML4 root table.
    /// 2. Zero-initializes all 512 entries (4096 bytes).
    /// 3. Configures recursive paging by setting PML4 entry 511 to point back to `pml4_frame`.
    /// 4. Copies active kernel entries (including identity maps and high-half mappings)
    ///    from the current PML4 into the new PML4.
    ///
    /// # Returns
    /// - `Ok(AddressSpace)` on successful allocation.
    /// - `Err(&'static str)` if physical RAM is exhausted.
    pub fn new() -> Result<Self, &'static str> {
        let pml4_frame = super::alloc_frame().ok_or("Out of memory for process PML4")?;

        unsafe {
            // Zero the entire 4 KiB PML4 table
            core::ptr::write_bytes(pml4_frame.start_address().as_mut_ptr::<u8>(), 0, PAGE_SIZE_4K as usize);

            let new_pml4 = &mut *(pml4_frame.start_address().as_mut_ptr::<PageTable>());

            // 1. Recursive mapping at PML4 entry 511
            new_pml4[511].set_entry(
                pml4_frame,
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE,
            );

            // 2. Clone all active kernel and identity mappings from the current PML4
            let (current_cr3, _) = paging::x86::read_cr3();
            let current_pml4 = &*(current_cr3.start_address().as_ptr::<PageTable>());

            for i in 0..511 {
                if current_pml4[i].is_present() {
                    new_pml4[i] = current_pml4[i];
                }
            }
        }

        Ok(Self { pml4_frame })
    }

    /// Returns the underlying PML4 physical frame.
    #[inline]
    pub fn pml4_frame(&self) -> PhysFrame {
        self.pml4_frame
    }

    /// Activates this address space on the current CPU core by writing its PML4 address to `%cr3`.
    ///
    /// # Safety
    /// Flushes the Translation Lookaside Buffer (TLB). The newly loaded page tables must
    /// correctly map all kernel instructions and stack currently being executed.
    #[inline]
    pub unsafe fn activate(&self) {
        unsafe {
            paging::x86::write_cr3(self.pml4_frame);
        }
    }

    /// Creates a [`KernelPageTable`] manipulator view rooted at this address space.
    #[inline]
    pub fn page_table(&self) -> KernelPageTable {
        KernelPageTable::new(self.pml4_frame)
    }
}
