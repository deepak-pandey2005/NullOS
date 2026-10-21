//! Physical Memory Manager (PMM) Bitmap Frame Allocator.
#![allow(dead_code)]

use super::address::{PhysAddr, PhysFrame, PAGE_SIZE_4K};

/// Memory statistics tracked by the Physical Memory Manager.
#[derive(Debug, Clone, Copy)]
pub struct MemoryStats {
    pub total_frames: u64,
    pub used_frames: u64,
    pub free_frames: u64,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
}

impl MemoryStats {
    #[inline]
    pub fn total_mib(&self) -> u64 {
        self.total_bytes / (1024 * 1024)
    }

    #[inline]
    pub fn free_mib(&self) -> u64 {
        self.free_bytes / (1024 * 1024)
    }

    #[inline]
    pub fn used_mib(&self) -> u64 {
        self.used_bytes / (1024 * 1024)
    }
}

/// A bitmap-based Physical Page Frame Allocator tracking 4 KiB physical memory frames.
pub struct BitmapFrameAllocator {
    bitmap: &'static mut [u8],
    total_frames: u64,
    free_frames: u64,
    last_search_index: u64,
}

impl BitmapFrameAllocator {
    /// Constructs a new `BitmapFrameAllocator` over the provided bitmap slice.
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

    #[inline]
    pub fn total_frames(&self) -> u64 {
        self.total_frames
    }

    #[inline]
    pub fn free_frames(&self) -> u64 {
        self.free_frames
    }

    #[inline]
    pub fn used_frames(&self) -> u64 {
        self.total_frames.saturating_sub(self.free_frames)
    }

    /// Mark a frame index as used (bit = 1).
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

    /// Mark a frame index as free (bit = 0).
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

    /// Checks if a frame index is marked as used.
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

    /// Mark a physical range of memory as used / reserved.
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

    /// Mark a physical range of memory as free / available for allocation.
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

    /// Allocates a single 4 KiB physical page frame.
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

    /// Deallocates a previously allocated physical frame.
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

    /// Get current memory statistics.
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
