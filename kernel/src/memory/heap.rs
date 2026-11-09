//! # Kernel Dynamic Heap Allocator
//!
//! ## Architecture Overview
//! The NullOS dynamic heap implements the standard Rust [`core::alloc::GlobalAlloc`] interface,
//! unlocking the entire Rust standard allocation library (`extern crate alloc;`) in a `#![no_std]`
//! environment. This enables dynamic data structures across all kernel subsystems:
//! - [`alloc::boxed::Box`] for heap-allocated trait objects and isolated structures.
//! - [`alloc::vec::Vec`] for dynamic arrays (e.g. process lists, file descriptors, packet queues).
//! - [`alloc::string::String`] and [`alloc::format!`] for dynamic string manipulation.
//! - [`alloc::sync::Arc`] for atomically reference-counted shared structures.
//!
//! ## Implementation Design: Intrusive Linked-List Allocator
//! The heap is managed by [`LinkedListAllocator`] wrapped in a thread-safe [`LockedHeap`] spinlock:
//! 1. **Intrusive Free List**: Unallocated blocks store a [`ListNode`] header directly within
//!    their own memory bounds, consuming zero external heap tracking metadata.
//! 2. **First-Fit Allocation**: Traverses the free list to find the first node capable of
//!    satisfying both size and alignment constraints. If excess memory remains after alignment
//!    and allocation, the remainder is carved into a new smaller free node.
//! 3. **Dynamic On-Demand Expansion**:
//!    When an allocation request cannot be satisfied by the current free list, the allocator
//!    automatically invokes [`LinkedListAllocator::expand_heap`], which:
//!    - Allocates new physical frames from the Physical Memory Manager (PMM).
//!    - Maps them to virtual pages starting immediately at `heap_end`.
//!    - Adds the newly mapped range to the intrusive free list.
//!    - Retries the allocation seamlessly.

#![allow(dead_code)]

use super::address::{Page, PhysAddr, PhysFrame, VirtAddr, PAGE_SIZE_4K};
use super::paging::PageTableFlags;
use crate::println;
use crate::sync::Spinlock;
use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicBool, Ordering};

static HEAP_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Checks if the kernel heap allocator has completed initialization.
pub fn is_heap_initialized() -> bool {
    HEAP_INITIALIZED.load(Ordering::SeqCst)
}

/// Starting virtual address for the kernel heap (0x0000_4444_4444_0000).
pub const HEAP_START: u64 = 0x0000_4444_4444_0000;
/// Initial size of the kernel heap (1 MiB = 256 pages).
pub const HEAP_SIZE: usize = 1024 * 1024;

/// Heap usage and allocation statistics.
#[derive(Debug, Default, Clone, Copy)]
pub struct HeapStats {
    /// Total virtual heap capacity in bytes (initial capacity + all dynamic expansions).
    pub total_bytes: usize,
    /// Cumulative bytes allocated since boot.
    pub allocated_bytes: usize,
    /// Cumulative bytes freed since boot.
    pub freed_bytes: usize,
    /// Current count of active, un-freed heap allocations.
    pub active_allocations: usize,
}

impl HeapStats {
    /// Returns current active heap memory usage in bytes (`allocated_bytes - freed_bytes`).
    #[inline]
    pub fn current_usage(&self) -> usize {
        self.allocated_bytes.saturating_sub(self.freed_bytes)
    }
}

// ---------------------------------------------------------------------------
// 1. Linked-List Block Node
// ---------------------------------------------------------------------------

/// Intrusive header embedded directly inside every unallocated free memory block.
struct ListNode {
    size: usize,
    next: Option<&'static mut ListNode>,
}

impl ListNode {
    const fn new(size: usize) -> Self {
        Self { size, next: None }
    }

    fn start_address(&self) -> usize {
        self as *const Self as usize
    }

    fn end_address(&self) -> usize {
        self.start_address() + self.size
    }
}

// ---------------------------------------------------------------------------
// 2. Coalescing Linked-List Allocator
// ---------------------------------------------------------------------------

/// A First-Fit Intrusive Linked-List Allocator supporting dynamic heap expansion.
pub struct LinkedListAllocator {
    head: ListNode,
    stats: HeapStats,
    heap_end: usize,
}

impl LinkedListAllocator {
    /// Constructs an uninitialized linked list allocator.
    pub const fn new() -> Self {
        Self {
            head: ListNode::new(0),
            stats: HeapStats {
                total_bytes: 0,
                allocated_bytes: 0,
                freed_bytes: 0,
                active_allocations: 0,
            },
            heap_end: 0,
        }
    }

    /// Initializes the allocator with an initial virtual memory region.
    ///
    /// # Safety
    /// The memory region `[heap_start, heap_start + heap_size)` must be mapped
    /// and reserved exclusively for heap usage.
    pub unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        self.stats.total_bytes = heap_size;
        self.heap_end = heap_start + heap_size;
        unsafe {
            self.add_free_region(heap_start, heap_size);
        }
    }

    /// Dynamically expands the heap by allocating more physical frames and mapping new virtual pages.
    ///
    /// # Parameters
    /// - `required_size`: Minimum byte capacity needed to fulfill the pending allocation.
    pub fn expand_heap(&mut self, required_size: usize) -> Result<(), ()> {
        let chunk_size = align_up(required_size.max(128 * 1024), PAGE_SIZE_4K as usize);
        let pages = chunk_size / (PAGE_SIZE_4K as usize);
        let start = self.heap_end;

        for i in 0..pages {
            let vaddr = VirtAddr::new((start + i * (PAGE_SIZE_4K as usize)) as u64);
            let page = Page::containing_address(vaddr);
            let frame = super::alloc_frame().ok_or(())?;
            super::paging::map_page(
                page,
                frame,
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE,
            )
            .map_err(|_| ())?;
        }

        unsafe {
            self.add_free_region(start, chunk_size);
        }
        self.heap_end += chunk_size;
        self.stats.total_bytes += chunk_size;

        // Write directly to COM1 serial to avoid re-entrant locking of DMESG_RING while holding HEAP_ALLOCATOR
        crate::drivers::serial::write_str("[HEAP] Dynamic expansion completed.\n");

        Ok(())
    }

    /// Adds a freed region back into the intrusive free list.
    unsafe fn add_free_region(&mut self, addr: usize, size: usize) {
        // Alignment verification
        assert_eq!(
            align_up(addr, core::mem::align_of::<ListNode>()),
            addr,
            "Free region must be properly aligned"
        );
        assert!(size >= core::mem::size_of::<ListNode>());

        // Create new node in the freed memory
        let node = unsafe { &mut *(addr as *mut ListNode) };
        node.size = size;
        node.next = self.head.next.take();
        self.head.next = Some(node);
    }

    /// Finds a free region and allocates a block satisfying `layout`.
    fn find_region(&mut self, size: usize, align: usize) -> Option<(&'static mut ListNode, usize)> {
        let mut current = &mut self.head;

        while let Some(ref mut region) = current.next {
            if let Ok(alloc_start) = Self::alloc_from_region(region, size, align) {
                let next = region.next.take();
                let ret = Some((current.next.take().unwrap(), alloc_start));
                current.next = next;
                return ret;
            } else {
                current = current.next.as_mut().unwrap();
            }
        }
        None
    }

    /// Computes whether `region` can house `size` bytes with `align` alignment.
    fn alloc_from_region(region: &ListNode, size: usize, align: usize) -> Result<usize, ()> {
        let alloc_start = align_up(region.start_address(), align);
        let alloc_end = alloc_start.checked_add(size).ok_or(())?;

        if alloc_end > region.end_address() {
            return Err(());
        }

        let excess_size = region.end_address() - alloc_end;
        if excess_size > 0 && excess_size < core::mem::size_of::<ListNode>() {
            return Err(());
        }

        Ok(alloc_start)
    }

    /// Allocates memory for the given `Layout` using first-fit search, expanding the heap if needed.
    pub fn allocate_first_fit(&mut self, layout: Layout) -> Result<*mut u8, ()> {
        let (size, align) = Self::size_align(layout);

        if let Some((region, alloc_start)) = self.find_region(size, align) {
            let alloc_end = alloc_start.checked_add(size).ok_or(())?;
            let excess_size = region.end_address() - alloc_end;

            if excess_size > 0 {
                unsafe {
                    self.add_free_region(alloc_end, excess_size);
                }
            }

            self.stats.allocated_bytes += size;
            self.stats.active_allocations += 1;
            Ok(alloc_start as *mut u8)
        } else {
            // Attempt dynamic heap expansion
            if self.expand_heap(size).is_ok() {
                if let Some((region, alloc_start)) = self.find_region(size, align) {
                    let alloc_end = alloc_start.checked_add(size).ok_or(())?;
                    let excess_size = region.end_address() - alloc_end;

                    if excess_size > 0 {
                        unsafe {
                            self.add_free_region(alloc_end, excess_size);
                        }
                    }

                    self.stats.allocated_bytes += size;
                    self.stats.active_allocations += 1;
                    return Ok(alloc_start as *mut u8);
                }
            }
            Err(())
        }
    }

    /// Deallocates an active heap allocation, returning its bytes to the free list.
    pub fn deallocate_first_fit(&mut self, ptr: *mut u8, layout: Layout) {
        let (size, _) = Self::size_align(layout);
        unsafe {
            self.add_free_region(ptr as usize, size);
        }
        self.stats.freed_bytes += size;
        self.stats.active_allocations = self.stats.active_allocations.saturating_sub(1);
    }

    /// Normalizes layout size and alignment to ensure free node headers can always fit.
    fn size_align(layout: Layout) -> (usize, usize) {
        let layout = layout
            .align_to(core::mem::align_of::<ListNode>())
            .expect("Adjusting alignment failed")
            .pad_to_align();
        let size = layout.size().max(core::mem::size_of::<ListNode>());
        (size, layout.align())
    }

    /// Returns a copy of current heap statistics.
    pub fn stats(&self) -> HeapStats {
        self.stats
    }
}

// ---------------------------------------------------------------------------
// 3. Global Heap Wrapper & GlobalAlloc Implementation
// ---------------------------------------------------------------------------

/// Thread-safe wrapper around [`LinkedListAllocator`] protected by a Spinlock.
pub struct LockedHeap {
    inner: Spinlock<LinkedListAllocator>,
}

impl LockedHeap {
    /// Creates a new locked heap wrapper.
    pub const fn new() -> Self {
        Self {
            inner: Spinlock::new(LinkedListAllocator::new()),
        }
    }

    /// Initializes the underlying allocator with the given memory range.
    pub unsafe fn init(&self, heap_start: usize, heap_size: usize) {
        unsafe {
            self.inner.lock().init(heap_start, heap_size);
        }
    }

    /// Retrieves current heap statistics.
    pub fn stats(&self) -> HeapStats {
        self.inner.lock().stats()
    }
}

unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.inner
            .lock()
            .allocate_first_fit(layout)
            .unwrap_or(core::ptr::null_mut())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.inner.lock().deallocate_first_fit(ptr, layout);
    }
}

/// The global heap allocator instance used by Rust's `alloc` crate.
#[global_allocator]
pub static HEAP_ALLOCATOR: LockedHeap = LockedHeap::new();

// ---------------------------------------------------------------------------
// 4. Address Alignment Helper
// ---------------------------------------------------------------------------

#[inline]
pub fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

// ---------------------------------------------------------------------------
// 5. Heap Initialization & Verification
// ---------------------------------------------------------------------------

/// Maps memory pages and initializes the Kernel Heap Allocator.
pub fn init() {
    println!();
    println!("----------------------------------------");
    println!(" [HEAP] Initializing Kernel Heap        ");
    println!("----------------------------------------");

    let page_count = HEAP_SIZE / (PAGE_SIZE_4K as usize);
    println!(
        "[HEAP] Mapping {} heap pages ({} KiB) at virtual {:#014x}...",
        page_count,
        HEAP_SIZE / 1024,
        HEAP_START
    );

    // Map physical frames to backing heap pages
    for i in 0..page_count {
        let vaddr = VirtAddr::new(HEAP_START + (i as u64) * PAGE_SIZE_4K);
        let page = Page::containing_address(vaddr);
        let frame = super::alloc_frame().expect("[HEAP FATAL] Out of physical frames for heap backing memory");

        super::paging::map_page(
            page,
            frame,
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE,
        )
        .expect("[HEAP FATAL] Failed to map heap page in page tables");
    }

    // Initialize global allocator
    unsafe {
        HEAP_ALLOCATOR.init(HEAP_START as usize, HEAP_SIZE);
    }
    HEAP_INITIALIZED.store(true, Ordering::SeqCst);

    println!(
        "[HEAP] Kernel Heap active at {:#014x} - {:#014x} ({} MiB)",
        HEAP_START,
        HEAP_START + (HEAP_SIZE as u64),
        HEAP_SIZE / (1024 * 1024)
    );
}

/// Returns current kernel heap statistics.
pub fn stats() -> HeapStats {
    HEAP_ALLOCATOR.stats()
}

/// Runs comprehensive self-tests verifying dynamic allocation with `Box`, `Vec`, and `String`.
pub fn run_self_tests() {
    println!("[HEAP] Running Heap Allocator verification self-tests...");

    let initial_stats = stats();

    // Test 1: Box allocation
    {
        use alloc::boxed::Box;
        let heap_value = Box::new(0x1337_C0DE_CAFE_FEED_u64);
        assert_eq!(*heap_value, 0x1337_C0DE_CAFE_FEED_u64, "Box value mismatch!");

        let current_stats = stats();
        assert!(
            current_stats.active_allocations > initial_stats.active_allocations,
            "Active allocations must increase after Box::new"
        );
    }
    // Box is dropped here, deallocating memory

    // Test 2: Dynamic Vector expansion
    {
        use alloc::vec::Vec;
        let mut numbers = Vec::new();
        for i in 0..500 {
            numbers.push(i * 3);
        }

        assert_eq!(numbers.len(), 500);
        for i in 0..500 {
            assert_eq!(numbers[i], i * 3);
        }
    }
    // Vec is dropped here

    // Test 3: String formatting
    {
        use alloc::string::ToString;
        use alloc::format;

        let msg = format!("NullOS Heap: Version {}.{}.{}", 0, 1, 0);
        assert_eq!(msg, "NullOS Heap: Version 0.1.0");

        let dynamic_str = "Dynamic String Test".to_string();
        assert_eq!(dynamic_str.len(), 19);
    }

    // Test 4: Dynamic Heap Expansion on Demand
    {
        use alloc::vec::Vec;
        let pre_expansion_total = stats().total_bytes;

        // Allocate two large buffers totaling > 1.1 MiB (exceeding initial 1 MiB heap)
        let mut large_buffer: Vec<u8> = Vec::with_capacity(512 * 1024);
        for i in 0..(512 * 1024) {
            large_buffer.push((i % 256) as u8);
        }
        assert_eq!(large_buffer.len(), 512 * 1024);

        let mut second_buffer: Vec<u8> = Vec::with_capacity(600 * 1024);
        for i in 0..(600 * 1024) {
            second_buffer.push((i % 256) as u8);
        }
        assert_eq!(second_buffer.len(), 600 * 1024);

        let post_expansion_total = stats().total_bytes;
        assert!(
            post_expansion_total > pre_expansion_total,
            "Heap total capacity must expand when allocations exceed initial size"
        );
    }

    let final_stats = stats();
    println!(
        "[HEAP] All Heap self-tests passed successfully! (Box, Vec, String, Total alloc: {} bytes, Active: {})",
        final_stats.allocated_bytes,
        final_stats.active_allocations
    );
}
