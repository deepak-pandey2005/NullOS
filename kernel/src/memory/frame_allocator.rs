//! # Physical Memory Manager (PMM) Bitmap Frame Allocator
//!
//! ## Bitmap Allocation Algorithm
//! The Physical Memory Manager tracks all 4096-byte (4 KiB) physical frames across RAM
//! using a compact bitset where:
//! - **Bit 0**: Frame is free and available for allocation.
//! - **Bit 1**: Frame is used or reserved (hardware MMIO, kernel code, ACPI, etc.).
//!
//! ### Storage Efficiency
//! Each byte in the bitmap governs 8 page frames (32 KiB of physical RAM).
//! To track 4 GiB of RAM requires only:
//! $$\frac{4 \times 1024^3 \text{ bytes}}{4096 \text{ bytes/frame} \times 8 \text{ bits/byte}} = 131,072 \text{ bytes} = 128 \text{ KiB}$$
//!
//! ### Optimization Strategy
//! - **Next-Fit Search Pointer**: Maintains `last_search_index` to resume sequential allocation
//!   where the previous allocation completed, avoiding $O(N)$ rescanning of early allocated frames.
//! - **Fast Byte Skipping**: When `frame_idx % 8 == 0`, checks if `bitmap[byte_idx] == 0xFF`. If so,
//!   all 8 frames are occupied, allowing an immediate 8-frame jump.
//! - **Contiguous Allocations**: Linear scan finding uninterrupted sequences of free frames
//!   for device DMA buffers (e.g. virtio queue descriptors, audio PCM rings).

#![allow(dead_code)]

use super::address::{PhysAddr, PhysFrame, PAGE_SIZE_4K};

/// Memory statistics tracked by the Physical Memory Manager.
#[derive(Debug, Clone, Copy)]
pub struct MemoryStats {
    /// Total count of physical frames tracked by the allocator.
    pub total_frames: u64,
    /// Count of physical frames currently in use or reserved.
    pub used_frames: u64,
    /// Count of physical frames currently free and available.
    pub free_frames: u64,
    /// Total physical memory capacity in bytes.
    pub total_bytes: u64,
    /// Used physical memory capacity in bytes.
    pub used_bytes: u64,
    /// Free physical memory capacity in bytes.
    pub free_bytes: u64,
}

impl MemoryStats {
    /// Returns total tracked physical memory in Mebibytes (MiB).
    #[inline]
    pub fn total_mib(&self) -> u64 {
        self.total_bytes / (1024 * 1024)
    }

    /// Returns free physical memory in Mebibytes (MiB).
    #[inline]
    pub fn free_mib(&self) -> u64 {
        self.free_bytes / (1024 * 1024)
    }

    /// Returns used physical memory in Mebibytes (MiB).
    #[inline]
    pub fn used_mib(&self) -> u64 {
        self.used_bytes / (1024 * 1024)
    }
}

/// A bitmap-based Physical Page Frame Allocator tracking 4 KiB physical memory frames.
pub struct BitmapFrameAllocator {
    /// Pointer and length of the underlying bitset slice.
    bitmap: &'static mut [u8],
    /// Total frames within the physical address range up to `max_phys_addr`.
    total_frames: u64,
    /// Total frames currently free for allocation.
    free_frames: u64,
    /// Search index for next-fit frame allocation.
    last_search_index: u64,
}

impl BitmapFrameAllocator {
    /// Constructs a new `BitmapFrameAllocator` over the provided bitmap slice.
    ///
    /// Initially fills the entire bitmap with `0xFF` (all frames marked used/reserved)
    /// to ensure that unmapped or reserved holes in the memory map are never allocated.
    ///
    /// # Safety
    /// The caller must ensure that `bitmap_ptr` points to valid, exclusive memory of `bitmap_len` bytes.
    pub unsafe fn new(
        bitmap_ptr: *mut u8,
        bitmap_len: usize,
        total_frames: u64,
    ) -> Self {
        let bitmap = unsafe { core::slice::from_raw_parts_mut(bitmap_ptr, bitmap_len) };
        // Initially, assume all frames are used/reserved until explicitly freed
        bitmap.fill(0xFF);

        Self {
            bitmap,
            total_frames,
            free_frames: 0,
            last_search_index: 0,
        }
    }

    /// Returns total tracked frames.
    #[inline]
    pub fn total_frames(&self) -> u64 {
        self.total_frames
    }

    /// Returns current count of free frames.
    #[inline]
    pub fn free_frames(&self) -> u64 {
        self.free_frames
    }

    /// Returns current count of used frames.
    #[inline]
    pub fn used_frames(&self) -> u64 {
        self.total_frames.saturating_sub(self.free_frames)
    }

    /// Marks a frame index as used (bit = 1) and decrements free frame count.
    pub fn mark_used(&mut self, frame_idx: u64) {
        if frame_idx >= self.total_frames {
            return;
        }
        let byte_idx = (frame_idx / 8) as usize;
        let bit_idx = (frame_idx % 8) as u8;

        if byte_idx < self.bitmap.len() {
            let mask = 1 << bit_idx;
            if (self.bitmap[byte_idx] & mask) == 0 {
                self.bitmap[byte_idx] |= mask;
                self.free_frames = self.free_frames.saturating_sub(1);
            }
        }
    }

    /// Marks a frame index as free (bit = 0) and increments free frame count.
    pub fn mark_free(&mut self, frame_idx: u64) {
        if frame_idx >= self.total_frames {
            return;
        }
        let byte_idx = (frame_idx / 8) as usize;
        let bit_idx = (frame_idx % 8) as u8;

        if byte_idx < self.bitmap.len() {
            let mask = 1 << bit_idx;
            if (self.bitmap[byte_idx] & mask) != 0 {
                self.bitmap[byte_idx] &= !mask;
                self.free_frames += 1;
            }
        }
    }

    /// Checks if a frame index is currently marked as used (bit = 1).
    #[inline]
    pub fn is_used(&self, frame_idx: u64) -> bool {
        if frame_idx >= self.total_frames {
            return true;
        }
        let byte_idx = (frame_idx / 8) as usize;
        let bit_idx = (frame_idx % 8) as u8;
        if byte_idx < self.bitmap.len() {
            (self.bitmap[byte_idx] & (1 << bit_idx)) != 0
        } else {
            true
        }
    }

    /// Marks a contiguous physical range of memory as used / reserved.
    ///
    /// # Parameters
    /// - `start`: Base physical start address.
    /// - `size`: Length of the reserved region in bytes.
    pub fn reserve_range(&mut self, start: PhysAddr, size: u64) {
        if size == 0 {
            return;
        }
        let start_frame = start.0 / PAGE_SIZE_4K;
        let end_frame = (start.0.saturating_add(size) + PAGE_SIZE_4K - 1) / PAGE_SIZE_4K;
        let limit = end_frame.min(self.total_frames);

        for frame_idx in start_frame..limit {
            self.mark_used(frame_idx);
        }
    }

    /// Marks a physical range of memory as free / available for allocation.
    ///
    /// Rounds up `start` and rounds down `end` to prevent partial page boundary hazards.
    ///
    /// # Parameters
    /// - `start`: Base physical start address.
    /// - `size`: Length of the free region in bytes.
    pub fn free_range(&mut self, start: PhysAddr, size: u64) {
        if size == 0 {
            return;
        }
        // Round up start, round down end to avoid partial page boundary hazards
        let start_frame = (start.0 + PAGE_SIZE_4K - 1) / PAGE_SIZE_4K;
        let end_frame = (start.0 + size) / PAGE_SIZE_4K;
        if start_frame >= end_frame {
            return;
        }
        let limit = end_frame.min(self.total_frames);

        for frame_idx in start_frame..limit {
            self.mark_free(frame_idx);
        }
    }

    /// Allocates a single 4 KiB physical page frame using next-fit search.
    ///
    /// Searches from `last_search_index` to the end of memory, wrapping back to 0
    /// if necessary.
    ///
    /// # Returns
    /// `Some(PhysFrame)` on success, or `None` if physical memory is exhausted.
    pub fn alloc_frame(&mut self) -> Option<PhysFrame> {
        if self.free_frames == 0 {
            return None;
        }

        let total_frames = self.total_frames;
        let start_frame = self.last_search_index;

        // Search from last index to end
        if let Some(frame) = self.find_and_allocate_from(start_frame, total_frames) {
            return Some(frame);
        }

        // Wrap around from 0 to start
        if start_frame > 0 {
            if let Some(frame) = self.find_and_allocate_from(0, start_frame) {
                return Some(frame);
            }
        }

        None
    }

    /// Helper searching for the first 0-bit within range `[start_frame, end_frame)`.
    fn find_and_allocate_from(&mut self, start_frame: u64, end_frame: u64) -> Option<PhysFrame> {
        let mut frame_idx = start_frame;

        while frame_idx < end_frame {
            let byte_idx = (frame_idx / 8) as usize;
            if byte_idx >= self.bitmap.len() {
                break;
            }

            // Quick skip fully occupied bytes if aligned on byte boundary
            if frame_idx % 8 == 0 && self.bitmap[byte_idx] == 0xFF {
                frame_idx += 8;
                continue;
            }

            let bit_idx = (frame_idx % 8) as u8;
            if (self.bitmap[byte_idx] & (1 << bit_idx)) == 0 {
                // Found free frame
                self.bitmap[byte_idx] |= 1 << bit_idx;
                self.free_frames = self.free_frames.saturating_sub(1);
                self.last_search_index = frame_idx + 1;
                return Some(PhysFrame::from_index(frame_idx));
            }

            frame_idx += 1;
        }

        None
    }

    /// Deallocates a previously allocated physical frame, resetting its bit to 0.
    ///
    /// # Parameters
    /// - `frame`: The physical frame to free.
    pub fn dealloc_frame(&mut self, frame: PhysFrame) {
        let frame_idx = frame.index();
        if frame_idx < self.total_frames {
            self.mark_free(frame_idx);
            if frame_idx < self.last_search_index {
                self.last_search_index = frame_idx;
            }
        }
    }

    /// Allocates `count` contiguous 4 KiB physical page frames.
    ///
    /// Required for hardware device DMA buffers (VirtIO queues, Intel HD Audio buffers).
    ///
    /// # Parameters
    /// - `count`: Number of contiguous 4 KiB frames required.
    ///
    /// # Returns
    /// `Some(PhysFrame)` containing the first frame of the run, or `None` if
    /// no contiguous block of that size exists.
    pub fn alloc_contiguous_frames(&mut self, count: usize) -> Option<PhysFrame> {
        if count == 0 {
            return None;
        }
        if count == 1 {
            return self.alloc_frame();
        }
        if (count as u64) > self.free_frames {
            return None;
        }

        let mut run_start = 0u64;
        let mut run_len = 0usize;

        for frame_idx in 0..self.total_frames {
            if !self.is_used(frame_idx) {
                if run_len == 0 {
                    run_start = frame_idx;
                }
                run_len += 1;
                if run_len == count {
                    // Mark all frames in the run as used
                    for i in 0..count {
                        self.mark_used(run_start + i as u64);
                    }
                    self.last_search_index = run_start + count as u64;
                    return Some(PhysFrame::from_index(run_start));
                }
            } else {
                run_len = 0;
            }
        }

        None
    }

    /// Retrieves a snapshot of current physical memory statistics.
    pub fn stats(&self) -> MemoryStats {
        let total_frames = self.total_frames;
        let free_frames = self.free_frames;
        let used_frames = total_frames.saturating_sub(free_frames);

        MemoryStats {
            total_frames,
            used_frames,
            free_frames,
            total_bytes: total_frames * PAGE_SIZE_4K,
            used_bytes: used_frames * PAGE_SIZE_4K,
            free_bytes: free_frames * PAGE_SIZE_4K,
        }
    }
}
