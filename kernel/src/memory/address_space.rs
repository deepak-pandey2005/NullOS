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

            // 2. Clone active kernel and identity mappings
            // Use the baseline kernel PML4 table so we never copy partially mutated user PML4 tables.
            let kernel_pml4_frame = paging::kernel_pml4_frame().unwrap_or_else(|| {
                let (current_cr3, _) = paging::x86::read_cr3();
                current_cr3
            });
            let kernel_pml4 = &*(kernel_pml4_frame.start_address().as_ptr::<PageTable>());

            // Share higher-half kernel entries (136..511) directly.
            // Note: Kernel heap is at 0x0000_4444_4444_0000 (PML4 entry 136).
            for i in 136..511 {
                if kernel_pml4[i].is_present() {
                    new_pml4[i] = kernel_pml4[i];
                }
            }

            // For PML4 entry 0 (covers 0..512 GiB, containing physical RAM identity mappings):
            // We must deep-clone PDPT and PD so user-space allocations at 0x400000+ do not share
            // intermediate Page Directories with the kernel!
            if kernel_pml4[0].is_present() {
                let user_pdpt_frame = super::alloc_frame().ok_or("Out of memory for user PDPT[0]")?;
                core::ptr::write_bytes(user_pdpt_frame.start_address().as_mut_ptr::<u8>(), 0, PAGE_SIZE_4K as usize);
                let user_pdpt = &mut *(user_pdpt_frame.start_address().as_mut_ptr::<PageTable>());

                let kern_pdpt_frame = kernel_pml4[0].frame().unwrap();
                let kern_pdpt = &*(kern_pdpt_frame.start_address().as_ptr::<PageTable>());

                // Copy entries 1..512 from kernel PDPT directly (above 1 GiB)
                for j in 1..512 {
                    if kern_pdpt[j].is_present() {
                        user_pdpt[j] = kern_pdpt[j];
                    }
                }

                // In PDPT entry 0 (covers 0..1 GiB, containing physical RAM identity maps):
                if kern_pdpt[0].is_present() {
                    let user_pd_frame = super::alloc_frame().ok_or("Out of memory for user PD[0]")?;
                    core::ptr::write_bytes(user_pd_frame.start_address().as_mut_ptr::<u8>(), 0, PAGE_SIZE_4K as usize);
                    let user_pd = &mut *(user_pd_frame.start_address().as_mut_ptr::<PageTable>());

                    let kern_pd_frame = kern_pdpt[0].frame().unwrap();
                    let kern_pd = &*(kern_pd_frame.start_address().as_ptr::<PageTable>());

                    // Entries 3..512 (6 MiB .. 1 GiB) can be shared directly
                    for k in 3..512 {
                        if kern_pd[k].is_present() {
                            user_pd[k] = kern_pd[k];
                        }
                    }

                    // For PD[0] (0..2 MiB, kernel code) and PD[1] (2..4 MiB, kernel text/rodata):
                    // Share them directly so kernel code/rodata remains executable in identity map.
                    if kern_pd[0].is_present() {
                        user_pd[0] = kern_pd[0];
                    }
                    if kern_pd[1].is_present() {
                        user_pd[1] = kern_pd[1];
                    }

                    // For PD[2] (4..6 MiB, contains kernel stack/BSS at 0x400000+):
                    // Deep-clone the Page Table for PD[2] so user mappings (at 0x400000..0x600000)
                    // have a private Page Table and do NOT overwrite the kernel's PT entries!
                    if kern_pd[2].is_present() {
                        let user_pt_frame = super::alloc_frame().ok_or("Out of memory for user PT[2]")?;
                        core::ptr::write_bytes(user_pt_frame.start_address().as_mut_ptr::<u8>(), 0, PAGE_SIZE_4K as usize);
                        let user_pt = &mut *(user_pt_frame.start_address().as_mut_ptr::<PageTable>());

                        let kern_pt_frame = kern_pd[2].frame().unwrap();
                        let kern_pt = &*(kern_pt_frame.start_address().as_ptr::<PageTable>());

                        // Copy all existing PT entries from kernel
                        for l in 0..512 {
                            if kern_pt[l].is_present() {
                                user_pt[l] = kern_pt[l];
                            }
                        }

                        user_pd[2].set_entry(
                            user_pt_frame,
                            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
                        );
                    }

                    user_pdpt[0].set_entry(
                        user_pd_frame,
                        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
                    );
                }

                new_pml4[0].set_entry(
                    user_pdpt_frame,
                    PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
                );
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
