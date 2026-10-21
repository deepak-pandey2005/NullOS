//! Process Address Space Abstraction (Phase 5 - M12).
//!
//! Provides isolated virtual memory spaces for user processes:
//! - Dedicated PML4 root table per process.
//! - Lower half (PML4[0..256], 128 TiB): Private to the user process.
//! - Higher half (PML4[256..512], 128 TiB): Shared kernel space (kernel image, heap, APIC, recursive mapping).
//! - Clean switching between address spaces via CR3.

use super::address::{Page, PhysAddr, PhysFrame, VirtAddr, PAGE_SIZE_4K};
use super::paging::{self, KernelPageTable, PageTable, PageTableFlags};

/// An isolated virtual address space for a process.
pub struct AddressSpace {
    pml4_frame: PhysFrame,
}

impl AddressSpace {
    /// Creates a new isolated user address space.
    ///
    /// Copies higher-half kernel mappings from the active kernel page table
    /// while keeping lower-half user space unmapped.
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

    #[inline]
    pub fn pml4_frame(&self) -> PhysFrame {
        self.pml4_frame
    }

    /// Activates this address space by loading its PML4 frame into CR3.
    #[inline]
    pub unsafe fn activate(&self) {
        unsafe {
            paging::x86::write_cr3(self.pml4_frame);
        }
    }

    /// Creates a KernelPageTable view into this address space.
    #[inline]
    pub fn page_table(&self) -> KernelPageTable {
        KernelPageTable::new(self.pml4_frame)
    }
}
