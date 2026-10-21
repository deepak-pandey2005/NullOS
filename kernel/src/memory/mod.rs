//! NullOS Memory Management Subsystem (Phase 3).
//!
//! Subsystems:
//! - M7: Physical Memory Manager (PMM) with Bitmap Frame Allocator
//! - M8: Virtual Memory & 4-level x86_64 Paging (Prepared)
//! - M9: Kernel Heap Allocator & GlobalAlloc (Prepared)
#![allow(unused_imports, dead_code)]

pub mod address;
pub mod frame_allocator;
pub mod paging;
pub mod uefi_mmap;

pub use address::{Page, PhysAddr, PhysFrame, VirtAddr, PAGE_SIZE_4K};
pub use frame_allocator::{BitmapFrameAllocator, MemoryStats};
pub use paging::{KernelPageTable, PageTable, PageTableEntry, PageTableFlags};
pub use uefi_mmap::{MemoryDescriptor, MemoryMapIterator, UefiMemoryType};

use crate::println;
use crate::sync::Spinlock;
use bootinfo::BootInfo;

/// Global thread-safe physical frame allocator protected by spinlock.
static FRAME_ALLOCATOR: Spinlock<Option<BitmapFrameAllocator>> = Spinlock::new(None);

/// Initializes the Physical Memory Manager (PMM) using the UEFI Memory Map.
pub fn init(boot_info: &'static BootInfo) {
    println!();
    println!("----------------------------------------");
    println!(" [MEMORY] Initializing Memory Management ");
    println!("----------------------------------------");

    // 1. Diagnostics: Print memory map descriptors
    uefi_mmap::print_memory_map(&boot_info.memory_map);

    // 2. Discover memory bounds: Find highest physical address of conventional RAM
    let mut max_phys_addr = 0u64;
    let mut _total_conventional_bytes = 0u64;

    for desc in MemoryMapIterator::new(&boot_info.memory_map) {
        let end_addr = desc.phys_end();
        if desc.memory_type().is_usable() {
            _total_conventional_bytes += desc.size_in_bytes();
            if end_addr > max_phys_addr {
                max_phys_addr = end_addr;
            }
        }
    }

    if max_phys_addr == 0 {
        panic!("[MEMORY FATAL] No usable conventional memory detected in UEFI memory map!");
    }

    let total_frames = max_phys_addr / PAGE_SIZE_4K;
    let bitmap_bytes = ((total_frames + 7) / 8) as usize;

    println!(
        "[MEMORY] Physical space: max addr {:#x}, total frames: {}, bitmap needed: {} bytes ({} KiB)",
        max_phys_addr,
        total_frames,
        bitmap_bytes,
        (bitmap_bytes + 1023) / 1024
    );

    // 3. Find a placement for the PMM bitmap in conventional RAM above kernel
    let kernel_end = boot_info.kernel_image_start + boot_info.kernel_image_size;
    let mut bitmap_phys_addr: Option<u64> = None;

    for desc in MemoryMapIterator::new(&boot_info.memory_map) {
        if desc.memory_type().is_usable() {
            let start = desc.phys_start.max(kernel_end).max(0x100000); // Above kernel and lower 1 MiB
            let end = desc.phys_end();
            if end > start && (end - start) >= (bitmap_bytes as u64) {
                // Ensure address is 4 KiB aligned
                let aligned_start = (start + PAGE_SIZE_4K - 1) & !(PAGE_SIZE_4K - 1);
                if aligned_start + (bitmap_bytes as u64) <= end {
                    bitmap_phys_addr = Some(aligned_start);
                    break;
                }
            }
        }
    }

    let bitmap_addr = match bitmap_phys_addr {
        Some(addr) => addr,
        None => panic!("[MEMORY FATAL] Could not find sufficient contiguous RAM for PMM bitmap!"),
    };

    println!(
        "[MEMORY] Placed PMM bitmap at physical address {:#014x} ({} frames)",
        bitmap_addr,
        (bitmap_bytes as u64 + PAGE_SIZE_4K - 1) / PAGE_SIZE_4K
    );

    // 4. Construct the BitmapFrameAllocator
    let mut allocator = unsafe {
        BitmapFrameAllocator::new(
            bitmap_addr as *mut u8,
            bitmap_bytes,
            total_frames,
        )
    };

    // 5. Mark all Conventional Memory as free
    for desc in MemoryMapIterator::new(&boot_info.memory_map) {
        if desc.memory_type().is_usable() {
            allocator.free_range(PhysAddr::new(desc.phys_start), desc.size_in_bytes());
        }
    }

    // 6. Explicitly reserve essential hardware and kernel memory regions
    // a. Lower 1 MiB (BIOS IVT, BDA, legacy hardware)
    allocator.reserve_range(PhysAddr::new(0x0), 0x100000);

    // b. Kernel loaded ELF image (code, rodata, data, bss)
    allocator.reserve_range(
        PhysAddr::new(boot_info.kernel_image_start),
        boot_info.kernel_image_size,
    );

    // c. BootInfo structure
    allocator.reserve_range(
        PhysAddr::new(boot_info as *const _ as u64),
        core::mem::size_of::<BootInfo>() as u64,
    );

    // d. MemoryMap descriptors buffer
    allocator.reserve_range(
        PhysAddr::new(boot_info.memory_map.address),
        boot_info.memory_map.size as u64,
    );

    // e. Kernel command line string
    if boot_info.cmdline_addr != 0 && boot_info.cmdline_len > 0 {
        allocator.reserve_range(
            PhysAddr::new(boot_info.cmdline_addr),
            boot_info.cmdline_len as u64,
        );
    }

    // f. Framebuffer MMIO (if within physical memory limit)
    if boot_info.framebuffer.address != 0 && boot_info.framebuffer.address < max_phys_addr {
        allocator.reserve_range(
            PhysAddr::new(boot_info.framebuffer.address),
            boot_info.framebuffer.size as u64,
        );
    }

    // g. The PMM bitmap itself!
    allocator.reserve_range(PhysAddr::new(bitmap_addr), bitmap_bytes as u64);

    // 7. Store the initialized allocator into the global spinlock
    let stats = allocator.stats();
    *FRAME_ALLOCATOR.lock() = Some(allocator);

    println!(
        "[MEMORY] Physical Memory Manager active. Total: {} MiB, Free: {} MiB ({} frames), Used: {} MiB ({} frames)",
        stats.total_mib(),
        stats.free_mib(),
        stats.free_frames,
        stats.used_mib(),
        stats.used_frames
    );

    // 8. Initialize Virtual Memory & 4-level Paging (Phase 3 - M8)
    paging::init(boot_info);
}

/// Allocates a single 4 KiB physical page frame.
pub fn alloc_frame() -> Option<PhysFrame> {
    FRAME_ALLOCATOR
        .lock()
        .as_mut()
        .and_then(|allocator| allocator.alloc_frame())
}

/// Deallocates a 4 KiB physical page frame.
pub fn dealloc_frame(frame: PhysFrame) {
    if let Some(allocator) = FRAME_ALLOCATOR.lock().as_mut() {
        allocator.dealloc_frame(frame);
    }
}

/// Allocates `count` contiguous 4 KiB physical page frames.
pub fn alloc_contiguous_frames(count: usize) -> Option<PhysFrame> {
    FRAME_ALLOCATOR
        .lock()
        .as_mut()
        .and_then(|allocator| allocator.alloc_contiguous_frames(count))
}

/// Retrieves current physical memory statistics.
pub fn stats() -> Option<MemoryStats> {
    FRAME_ALLOCATOR.lock().as_ref().map(|a| a.stats())
}

/// Executes comprehensive self-tests verifying the Physical Memory Manager.
pub fn run_self_tests() {
    println!("[MEMORY] Running PMM verification self-tests...");

    let initial_free = stats().expect("PMM not initialized").free_frames;

    // Test 1: Single frame allocation & alignment
    let frame1 = alloc_frame().expect("Self-test: Failed to allocate single frame");
    assert!(
        frame1.start_address().is_aligned(PAGE_SIZE_4K),
        "Allocated frame address must be 4 KiB aligned!"
    );
    assert_eq!(
        stats().unwrap().free_frames,
        initial_free - 1,
        "Free frame count must decrement by 1"
    );

    // Test 2: Frame memory accessibility (identity-mapped by UEFI)
    unsafe {
        let ptr = frame1.start_address().as_mut_ptr::<u64>();
        core::ptr::write_volatile(ptr, 0xCAFE_BABE_DEAD_BEEF);
        assert_eq!(
            core::ptr::read_volatile(ptr),
            0xCAFE_BABE_DEAD_BEEF,
            "Memory read/write test failed on allocated frame"
        );
        core::ptr::write_bytes(frame1.start_address().as_mut_ptr::<u8>(), 0, PAGE_SIZE_4K as usize);
    }

    // Test 3: Contiguous allocation
    let contiguous_count = 4;
    let block = alloc_contiguous_frames(contiguous_count)
        .expect("Self-test: Failed to allocate 4 contiguous frames");
    assert_eq!(
        stats().unwrap().free_frames,
        initial_free - 1 - (contiguous_count as u64),
        "Free frame count must decrement by 4"
    );

    // Test 4: Deallocation
    dealloc_frame(frame1);
    assert_eq!(
        stats().unwrap().free_frames,
        initial_free - (contiguous_count as u64),
        "Free frame count must increment by 1 after dealloc"
    );

    for i in 0..contiguous_count {
        dealloc_frame(PhysFrame::from_index(block.index() + i as u64));
    }
    assert_eq!(
        stats().unwrap().free_frames,
        initial_free,
        "Free frame count must restore to initial free frames after all deallocations"
    );

    // Test 5: Re-allocation verification
    let recycled = alloc_frame().expect("Self-test: Failed to re-allocate frame");
    dealloc_frame(recycled);

    println!("[MEMORY] All PMM self-tests passed successfully! (Frame allocation, contiguous, read/write, dealloc, recycle)");

    // Run VMM Paging self-tests
    paging::run_self_tests();
}
