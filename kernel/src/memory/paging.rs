//! Virtual Memory Management (VMM) and 4-Level x86_64 Paging.
//!
//! Handles PML4, PDPT, PD, and PT structures, page table entry flags,
//! address translation, page mapping/unmapping, recursive mapping, and CR3 management.
#![allow(dead_code)]

use super::address::{Page, PhysAddr, PhysFrame, VirtAddr, PAGE_SIZE_4K};
use crate::println;
use bootinfo::BootInfo;
use core::ops::{Index, IndexMut};

/// Flags for x86_64 Page Table Entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageTableFlags(pub u64);

impl PageTableFlags {
    pub const PRESENT: Self = Self(1 << 0);
    pub const WRITABLE: Self = Self(1 << 1);
    pub const USER_ACCESSIBLE: Self = Self(1 << 2);
    pub const WRITE_THROUGH: Self = Self(1 << 3);
    pub const NO_CACHE: Self = Self(1 << 4);
    pub const ACCESSED: Self = Self(1 << 5);
    pub const DIRTY: Self = Self(1 << 6);
    pub const HUGE_PAGE: Self = Self(1 << 7);
    pub const GLOBAL: Self = Self(1 << 8);
    pub const NO_EXECUTE: Self = Self(1 << 63);

    #[inline]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[inline]
    pub const fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    #[inline]
    pub const fn bits(&self) -> u64 {
        self.0
    }
}

impl core::ops::BitOr for PageTableFlags {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for PageTableFlags {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl core::ops::BitAnd for PageTableFlags {
    type Output = Self;
    #[inline]
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

/// A 64-bit entry in an x86_64 page table.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct PageTableEntry(pub u64);

const PHYSICAL_ADDRESS_MASK: u64 = 0x000F_FFFF_FFFF_F000;

impl PageTableEntry {
    #[inline]
    pub const fn new() -> Self {
        Self(0)
    }

    #[inline]
    pub const fn is_unused(&self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub const fn is_present(&self) -> bool {
        (self.0 & PageTableFlags::PRESENT.0) != 0
    }

    #[inline]
    pub const fn is_huge(&self) -> bool {
        (self.0 & PageTableFlags::HUGE_PAGE.0) != 0
    }

    #[inline]
    pub const fn flags(&self) -> PageTableFlags {
        PageTableFlags(self.0 & !PHYSICAL_ADDRESS_MASK)
    }

    #[inline]
    pub const fn phys_addr(&self) -> PhysAddr {
        PhysAddr::new(self.0 & PHYSICAL_ADDRESS_MASK)
    }

    #[inline]
    pub const fn frame(&self) -> Option<PhysFrame> {
        if self.is_present() {
            Some(PhysFrame::containing_address(self.phys_addr()))
        } else {
            None
        }
    }

    #[inline]
    pub fn set_entry(&mut self, frame: PhysFrame, flags: PageTableFlags) {
        self.0 = (frame.start_address().as_u64() & PHYSICAL_ADDRESS_MASK) | flags.bits();
    }

    #[inline]
    pub fn set_flags(&mut self, flags: PageTableFlags) {
        let addr = self.0 & PHYSICAL_ADDRESS_MASK;
        self.0 = addr | flags.bits();
    }

    #[inline]
    pub fn add_flags(&mut self, flags: PageTableFlags) {
        self.0 |= flags.bits();
    }

    #[inline]
    pub fn clear(&mut self) {
        self.0 = 0;
    }
}

impl core::fmt::Debug for PageTableEntry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "PageTableEntry({:#018x} -> {:#018x}, flags: {:#x})",
            self.0,
            self.phys_addr().as_u64(),
            self.flags().bits()
        )
    }
}

/// A 4 KiB x86_64 Page Table containing 512 entries.
#[repr(C, align(4096))]
pub struct PageTable {
    entries: [PageTableEntry; 512],
}

impl PageTable {
    #[inline]
    pub const fn new() -> Self {
        Self {
            entries: [PageTableEntry::new(); 512],
        }
    }

    #[inline]
    pub fn zero(&mut self) {
        for entry in self.entries.iter_mut() {
            entry.clear();
        }
    }

    #[inline]
    pub fn entry(&self, index: usize) -> &PageTableEntry {
        &self.entries[index]
    }

    #[inline]
    pub fn entry_mut(&mut self, index: usize) -> &mut PageTableEntry {
        &mut self.entries[index]
    }
}

impl Index<usize> for PageTable {
    type Output = PageTableEntry;
    #[inline]
    fn index(&self, index: usize) -> &Self::Output {
        &self.entries[index]
    }
}

impl IndexMut<usize> for PageTable {
    #[inline]
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.entries[index]
    }
}

/// Assembly helpers for CR3 and TLB management.
pub mod x86 {
    use super::*;

    /// Read the current physical frame of the active PML4 table from CR3.
    #[inline]
    pub fn read_cr3() -> (PhysFrame, u16) {
        let value: u64;
        unsafe {
            core::arch::asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags));
        }
        let frame = PhysFrame::containing_address(PhysAddr::new(value & 0x000F_FFFF_FFFF_F000));
        let flags = (value & 0xFFF) as u16;
        (frame, flags)
    }

    /// Load a new PML4 physical frame into CR3, activating a new address space.
    #[inline]
    pub unsafe fn write_cr3(pml4_frame: PhysFrame) {
        let value = pml4_frame.start_address().as_u64();
        unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) value, options(nostack, preserves_flags));
        }
    }

    /// Invalidate the TLB entry for a single virtual address via the `invlpg` instruction.
    #[inline]
    pub fn tlb_flush(vaddr: VirtAddr) {
        unsafe {
            core::arch::asm!("invlpg [{}]", in(reg) vaddr.as_u64(), options(nostack, preserves_flags));
        }
    }

    /// Reload the active CR3 to flush the entire TLB (except global pages).
    #[inline]
    pub fn tlb_flush_all() {
        let (frame, _) = read_cr3();
        unsafe {
            write_cr3(frame);
        }
    }
}

/// The Kernel Page Table manager controlling 4-level x86_64 address translation and mappings.
pub struct KernelPageTable {
    pml4_frame: PhysFrame,
}

impl KernelPageTable {
    /// Creates a new `KernelPageTable` managing the given PML4 root table frame.
    pub const fn new(pml4_frame: PhysFrame) -> Self {
        Self { pml4_frame }
    }

    #[inline]
    pub fn pml4_frame(&self) -> PhysFrame {
        self.pml4_frame
    }

    /// Helper returning a mutable reference to the PML4 table.
    ///
    /// # Safety
    /// Valid in identity-mapped physical memory space.
    #[inline]
    unsafe fn pml4_mut(&self) -> &'static mut PageTable {
        unsafe { &mut *(self.pml4_frame.start_address().as_mut_ptr::<PageTable>()) }
    }

    /// Translates a virtual address into its corresponding physical address by walking page tables.
    pub fn translate_addr(&self, vaddr: VirtAddr) -> Option<PhysAddr> {
        let pml4 = unsafe { self.pml4_mut() };
        let pml4_idx = ((vaddr.as_u64() >> 39) & 0x1FF) as usize;
        let pml4_entry = &pml4[pml4_idx];
        if !pml4_entry.is_present() {
            return None;
        }

        let pdpt = unsafe { &*(pml4_entry.phys_addr().as_ptr::<PageTable>()) };
        let pdpt_idx = ((vaddr.as_u64() >> 30) & 0x1FF) as usize;
        let pdpt_entry = &pdpt[pdpt_idx];
        if !pdpt_entry.is_present() {
            return None;
        }
        // 1 GiB huge page check
        if pdpt_entry.is_huge() {
            let offset = vaddr.as_u64() & 0x3FFF_FFFF;
            return Some(pdpt_entry.phys_addr() + offset);
        }

        let pd = unsafe { &*(pdpt_entry.phys_addr().as_ptr::<PageTable>()) };
        let pd_idx = ((vaddr.as_u64() >> 21) & 0x1FF) as usize;
        let pd_entry = &pd[pd_idx];
        if !pd_entry.is_present() {
            return None;
        }
        // 2 MiB huge page check
        if pd_entry.is_huge() {
            let offset = vaddr.as_u64() & 0x1F_FFFF;
            return Some(pd_entry.phys_addr() + offset);
        }

        let pt = unsafe { &*(pd_entry.phys_addr().as_ptr::<PageTable>()) };
        let pt_idx = ((vaddr.as_u64() >> 12) & 0x1FF) as usize;
        let pt_entry = &pt[pt_idx];
        if !pt_entry.is_present() {
            return None;
        }

        let offset = vaddr.as_u64() & 0xFFF;
        Some(pt_entry.phys_addr() + offset)
    }

    /// Maps a 4 KiB virtual page to a physical frame with specified flags.
    /// Allocates intermediate page tables from the PMM if not already present.
    pub fn map_page(
        &mut self,
        page: Page,
        frame: PhysFrame,
        flags: PageTableFlags,
    ) -> Result<(), &'static str> {
        let vaddr = page.start_address();
        let pml4_idx = ((vaddr.as_u64() >> 39) & 0x1FF) as usize;
        let pdpt_idx = ((vaddr.as_u64() >> 30) & 0x1FF) as usize;
        let pd_idx = ((vaddr.as_u64() >> 21) & 0x1FF) as usize;
        let pt_idx = ((vaddr.as_u64() >> 12) & 0x1FF) as usize;

        let pml4 = unsafe { self.pml4_mut() };

        // 1. Traverse or allocate PDPT
        let pdpt_frame = if pml4[pml4_idx].is_present() {
            if flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                pml4[pml4_idx].add_flags(PageTableFlags::USER_ACCESSIBLE);
            }
            pml4[pml4_idx].frame().unwrap()
        } else {
            let new_frame = super::alloc_frame().ok_or("Out of memory for PDPT")?;
            unsafe {
                core::ptr::write_bytes(new_frame.start_address().as_mut_ptr::<u8>(), 0, PAGE_SIZE_4K as usize);
            }
            pml4[pml4_idx].set_entry(
                new_frame,
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | (flags & PageTableFlags::USER_ACCESSIBLE),
            );
            new_frame
        };

        // 2. Traverse or allocate PD
        let pdpt = unsafe { &mut *(pdpt_frame.start_address().as_mut_ptr::<PageTable>()) };
        let pd_frame = if pdpt[pdpt_idx].is_present() {
            if flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                pdpt[pdpt_idx].add_flags(PageTableFlags::USER_ACCESSIBLE);
            }
            pdpt[pdpt_idx].frame().unwrap()
        } else {
            let new_frame = super::alloc_frame().ok_or("Out of memory for PD")?;
            unsafe {
                core::ptr::write_bytes(new_frame.start_address().as_mut_ptr::<u8>(), 0, PAGE_SIZE_4K as usize);
            }
            pdpt[pdpt_idx].set_entry(
                new_frame,
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | (flags & PageTableFlags::USER_ACCESSIBLE),
            );
            new_frame
        };

        // 3. Traverse or allocate PT
        let pd = unsafe { &mut *(pd_frame.start_address().as_mut_ptr::<PageTable>()) };
        let pt_frame = if pd[pd_idx].is_present() {
            if flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                pd[pd_idx].add_flags(PageTableFlags::USER_ACCESSIBLE);
            }
            pd[pd_idx].frame().unwrap()
        } else {
            let new_frame = super::alloc_frame().ok_or("Out of memory for PT")?;
            unsafe {
                core::ptr::write_bytes(new_frame.start_address().as_mut_ptr::<u8>(), 0, PAGE_SIZE_4K as usize);
            }
            pd[pd_idx].set_entry(
                new_frame,
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | (flags & PageTableFlags::USER_ACCESSIBLE),
            );
            new_frame
        };

        // 4. Map final 4 KiB frame into PT
        let pt = unsafe { &mut *(pt_frame.start_address().as_mut_ptr::<PageTable>()) };
        pt[pt_idx].set_entry(frame, flags | PageTableFlags::PRESENT);

        // 5. Invalidate TLB entry for this virtual address
        x86::tlb_flush(vaddr);

        Ok(())
    }

    /// Unmaps a 4 KiB virtual page, flushing its TLB entry and returning the previous physical frame.
    pub fn unmap_page(&mut self, page: Page) -> Result<PhysFrame, &'static str> {
        let vaddr = page.start_address();
        let pml4_idx = ((vaddr.as_u64() >> 39) & 0x1FF) as usize;
        let pdpt_idx = ((vaddr.as_u64() >> 30) & 0x1FF) as usize;
        let pd_idx = ((vaddr.as_u64() >> 21) & 0x1FF) as usize;
        let pt_idx = ((vaddr.as_u64() >> 12) & 0x1FF) as usize;

        let pml4 = unsafe { self.pml4_mut() };
        if !pml4[pml4_idx].is_present() {
            return Err("Page not mapped (PML4 missing)");
        }

        let pdpt = unsafe { &mut *(pml4[pml4_idx].phys_addr().as_mut_ptr::<PageTable>()) };
        if !pdpt[pdpt_idx].is_present() {
            return Err("Page not mapped (PDPT missing)");
        }

        let pd = unsafe { &mut *(pdpt[pdpt_idx].phys_addr().as_mut_ptr::<PageTable>()) };
        if !pd[pd_idx].is_present() {
            return Err("Page not mapped (PD missing)");
        }

        let pt = unsafe { &mut *(pd[pd_idx].phys_addr().as_mut_ptr::<PageTable>()) };
        let entry = &mut pt[pt_idx];
        if !entry.is_present() {
            return Err("Page not mapped (PT entry not present)");
        }

        let frame = entry.frame().ok_or("Invalid frame in present entry")?;
        entry.clear();
        x86::tlb_flush(vaddr);

        Ok(frame)
    }

    /// Identity-maps a physical memory range 1:1 (`virt_addr == phys_addr`).
    pub fn identity_map_range(
        &mut self,
        start: PhysAddr,
        size: u64,
        flags: PageTableFlags,
    ) -> Result<(), &'static str> {
        if size == 0 {
            return Ok(());
        }

        let start_addr = start.align_down(PAGE_SIZE_4K);
        let end_addr = (start.as_u64() + size + PAGE_SIZE_4K - 1) & !(PAGE_SIZE_4K - 1);
        let page_count = (end_addr - start_addr.as_u64()) / PAGE_SIZE_4K;

        for i in 0..page_count {
            let addr = start_addr.as_u64() + i * PAGE_SIZE_4K;
            let frame = PhysFrame::containing_address(PhysAddr::new(addr));
            let page = Page::containing_address(VirtAddr::new(addr));
            self.map_page(page, frame, flags)?;
        }

        Ok(())
    }
}

/// Global Kernel Page Table instance protected by Spinlock.
static KERNEL_PAGE_TABLE: crate::sync::Spinlock<Option<KernelPageTable>> =
    crate::sync::Spinlock::new(None);

/// Initializes 4-level x86_64 Virtual Memory Paging (M8).
///
/// 1. Allocates a new physical frame for the Kernel PML4 table.
/// 2. Sets up recursive page table mapping at PML4 entry 511.
/// 3. Identity-maps:
///    - Lower 1 MiB (0x0 .. 0x100000)
///    - Kernel ELF segments
///    - Conventional physical RAM (detected via UEFI MemoryMap)
///    - BootInfo, MemoryMap, and cmdline buffers
///    - GOP Framebuffer (with Write-Through / Uncached flags)
///    - Local APIC MMIO (0xFEE0_0000)
/// 4. Switches CR3 to the new kernel-managed PML4 table.
pub fn init(boot_info: &'static BootInfo) {
    println!();
    println!("----------------------------------------");
    println!(" [PAGING] Initializing Virtual Memory   ");
    println!("----------------------------------------");

    let (old_cr3_frame, _) = x86::read_cr3();
    println!(
        "[PAGING] Active firmware PML4 table at {:#x}",
        old_cr3_frame.start_address().as_u64()
    );

    // 1. Allocate root PML4 frame
    let pml4_frame = super::alloc_frame().expect("[PAGING FATAL] Out of memory allocating PML4 root table");
    unsafe {
        core::ptr::write_bytes(pml4_frame.start_address().as_mut_ptr::<u8>(), 0, PAGE_SIZE_4K as usize);
    }

    println!(
        "[PAGING] New Kernel PML4 table allocated at {:#x}",
        pml4_frame.start_address().as_u64()
    );

    let mut kpt = KernelPageTable::new(pml4_frame);

    // 2. Set up Recursive Page Table Mapping at PML4[511]
    // Pointing PML4[511] back to the PML4 itself enables accessing page tables in high virtual memory.
    {
        let pml4 = unsafe { kpt.pml4_mut() };
        pml4[511].set_entry(
            pml4_frame,
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE,
        );
    }
    println!("[PAGING] Recursive page table mapping established at PML4 entry 511.");

    // 3. Identity-map physical memory
    // a. Lower 1 MiB (0x0 .. 0x100000)
    kpt.identity_map_range(
        PhysAddr::new(0x0),
        0x100000,
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
    )
    .expect("Failed to identity-map lower 1 MiB");

    // b. Kernel ELF image segments
    kpt.identity_map_range(
        PhysAddr::new(boot_info.kernel_image_start),
        boot_info.kernel_image_size,
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
    )
    .expect("Failed to identity-map kernel image");

    // c. Conventional Memory (RAM) and ACPI/Runtime data regions
    for desc in super::uefi_mmap::MemoryMapIterator::new(&boot_info.memory_map) {
        let mtype = desc.memory_type();
        if mtype.is_usable()
            || mtype.is_reclaimable()
            || matches!(mtype, super::uefi_mmap::UefiMemoryType::AcpiReclaim | super::uefi_mmap::UefiMemoryType::AcpiNvs | super::uefi_mmap::UefiMemoryType::RuntimeServicesData)
        {
            let start = PhysAddr::new(desc.phys_start);
            let size = desc.size_in_bytes();
            kpt.identity_map_range(
                start,
                size,
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
            )
            .expect("Failed to map conventional/ACPI memory");
        }
    }

    // d. BootInfo, MemoryMap, and command line buffers
    kpt.identity_map_range(
        PhysAddr::new(boot_info as *const _ as u64),
        core::mem::size_of::<BootInfo>() as u64,
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
    )
    .expect("Failed to map BootInfo");

    kpt.identity_map_range(
        PhysAddr::new(boot_info.memory_map.address),
        boot_info.memory_map.size as u64,
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
    )
    .expect("Failed to map MemoryMap buffer");

    if boot_info.cmdline_addr != 0 && boot_info.cmdline_len > 0 {
        kpt.identity_map_range(
            PhysAddr::new(boot_info.cmdline_addr),
            boot_info.cmdline_len as u64,
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
        )
        .expect("Failed to map cmdline buffer");
    }

    // e. Framebuffer MMIO (with Cache-Disable / Write-Through)
    if boot_info.framebuffer.address != 0 && boot_info.framebuffer.size > 0 {
        kpt.identity_map_range(
            PhysAddr::new(boot_info.framebuffer.address),
            boot_info.framebuffer.size as u64,
            PageTableFlags::PRESENT
                | PageTableFlags::WRITABLE
                | PageTableFlags::WRITE_THROUGH
                | PageTableFlags::NO_EXECUTE,
        )
        .expect("Failed to map Framebuffer MMIO");
        println!(
            "[PAGING] Framebuffer mapped: {:#x} ({} KiB, Write-Through, NX)",
            boot_info.framebuffer.address,
            boot_info.framebuffer.size / 1024
        );
    }

    // f. Local APIC MMIO (Fixed at 0xFEE00000)
    kpt.identity_map_range(
        PhysAddr::new(0x0000_0000_FEE0_0000),
        0x1000,
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::NO_CACHE
            | PageTableFlags::NO_EXECUTE,
    )
    .expect("Failed to map Local APIC MMIO");
    println!("[PAGING] Local APIC MMIO mapped: 0xFEE00000 (No-Cache, NX)");

    // 4. Activate Kernel Page Table by updating CR3!
    unsafe {
        x86::write_cr3(pml4_frame);
    }

    let (active_cr3, _) = x86::read_cr3();
    println!(
        "[PAGING] CPU CR3 switched to Kernel PML4: {:#x}",
        active_cr3.start_address().as_u64()
    );

    // Store in global instance
    *KERNEL_PAGE_TABLE.lock() = Some(kpt);
    println!("[PAGING] 4-level x86_64 Virtual Memory Paging active.");
}

/// Translates a virtual address into its physical address using the active kernel page table.
pub fn translate_addr(vaddr: VirtAddr) -> Option<PhysAddr> {
    KERNEL_PAGE_TABLE
        .lock()
        .as_ref()
        .and_then(|kpt| kpt.translate_addr(vaddr))
}

/// Maps a page using the active kernel page table.
pub fn map_page(page: Page, frame: PhysFrame, flags: PageTableFlags) -> Result<(), &'static str> {
    KERNEL_PAGE_TABLE
        .lock()
        .as_mut()
        .ok_or("Kernel page table not initialized")?
        .map_page(page, frame, flags)
}

/// Unmaps a page using the active kernel page table.
pub fn unmap_page(page: Page) -> Result<PhysFrame, &'static str> {
    KERNEL_PAGE_TABLE
        .lock()
        .as_mut()
        .ok_or("Kernel page table not initialized")?
        .unmap_page(page)
}

/// Executes comprehensive self-tests verifying virtual memory mapping, translation, and unmapping.
pub fn run_self_tests() {
    println!("[PAGING] Running Virtual Memory verification self-tests...");

    // Test 1: Translation of kernel identity mapping
    let kernel_vaddr = VirtAddr::new(0x100000);
    let phys = translate_addr(kernel_vaddr).expect("Self-test: Failed to translate kernel load address");
    assert_eq!(
        phys.as_u64(),
        0x100000,
        "Identity-mapped kernel address translation mismatch"
    );

    // Test 2: Dynamic Page Mapping & Access
    // Allocate a free physical frame and map it to an arbitrary unmapped test page (e.g. 0x0000_7000_0000_0000)
    let test_frame = super::alloc_frame().expect("Self-test: Failed to allocate test frame");
    let test_page = Page::containing_address(VirtAddr::new(0x0000_7000_0000_0000));

    // Map test page
    map_page(test_page, test_frame, PageTableFlags::PRESENT | PageTableFlags::WRITABLE)
        .expect("Self-test: Failed to map test page");

    // Verify translation matches
    let translated = translate_addr(test_page.start_address())
        .expect("Self-test: Failed to translate mapped test page");
    assert_eq!(
        translated.as_u64(),
        test_frame.start_address().as_u64(),
        "Translated address must match allocated frame address"
    );

    // Test memory read & write through virtual mapping
    unsafe {
        let ptr = test_page.start_address().as_mut_ptr::<u64>();
        core::ptr::write_volatile(ptr, 0x1234_5678_9ABC_DEF0);
        assert_eq!(
            core::ptr::read_volatile(ptr),
            0x1234_5678_9ABC_DEF0,
            "Memory read/write test failed through newly mapped virtual page"
        );
    }

    // Test 3: Page Unmapping
    let unmapped_frame = unmap_page(test_page).expect("Self-test: Failed to unmap test page");
    assert_eq!(
        unmapped_frame.start_address().as_u64(),
        test_frame.start_address().as_u64(),
        "Unmapped frame must match original test frame"
    );

    // Verify translation now returns None
    assert!(
        translate_addr(test_page.start_address()).is_none(),
        "Unmapped page must return None on translation"
    );

    // Free test frame back to PMM
    super::dealloc_frame(unmapped_frame);

    println!("[PAGING] All PMM/VMM self-tests passed successfully! (Translation, Dynamic Map, Read/Write, Unmap, TLB flush)");
}
