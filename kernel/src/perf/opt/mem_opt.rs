//! # Memory Allocator & Page-Table Optimizations (Phase 17 - M38)
//!
//! ## Overview
//! 1. **Segregated Slab Object Caches (`SlabCache` / `SegregatedSlabPool`)**:
//!    Replaces linear free-list traversal in the global kernel heap for common kernel
//!    object sizes (`32, 64, 128, 256, 512, 1024, 2048` bytes) with O(1) free-index stacks.
//! 2. **Per-CPU Physical Frame Magazine (`FrameMagazine`)**:
//!    Caches batches of `4 KiB` physical [`PhysFrame`]s per core, serving allocations locally
//!    without acquiring the global PMM spinlock until batch refill/drain thresholds hit.
//! 3. **Page-Table Optimizations (`PageTableFeatures`, `TlbShootdownBatch`, 2 MiB HugePage helpers)**:
//!    - Probes hardware **PCID** (`CR4.PCIDE`, `CPUID.01H:ECX[17]`), **INVPCID**, and **1 GiB HugePages**.
//!    - Encodes `2 MiB` HugePage (`PS` bit 7) directory entries increasing TLB coverage **512x** per entry.
//!    - Implements smart **Batched TLB Shootdown**: uses single-page `invlpg` when `<= 16` pages
//!      were modified, collapsing to a single global TLB flush beyond 16 pages.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::cpu;
use crate::memory;
use crate::memory::address::{PhysFrame, VirtAddr};
use crate::memory::paging;
use crate::perf::opt::locks::CacheAligned;
use crate::sync::Spinlock;

/// Size classes served in O(1) by [`SegregatedSlabPool`].
pub const SLAB_CLASSES: [usize; 7] = [32, 64, 128, 256, 512, 1024, 2048];

/// Number of pre-allocated slots per slab cache class.
pub const SLOTS_PER_CLASS: usize = 64;

/// Single fixed-object size-class slab pool with O(1) index free-stack.
pub struct SlabClassPool {
    obj_size: usize,
    initialized: bool,
    backing: Vec<u8>,
    free_stack: Vec<u16>,
    pub alloc_hits: u64,
    pub frees: u64,
}

impl SlabClassPool {
    pub const fn new(obj_size: usize) -> Self {
        Self {
            obj_size,
            initialized: false,
            backing: Vec::new(),
            free_stack: Vec::new(),
            alloc_hits: 0,
            frees: 0,
        }
    }

    fn ensure_init(&mut self) {
        if self.initialized {
            return;
        }
        let total = self.obj_size * SLOTS_PER_CLASS;
        self.backing = alloc::vec![0u8; total];
        self.free_stack = Vec::with_capacity(SLOTS_PER_CLASS);
        for i in (0..SLOTS_PER_CLASS).rev() {
            self.free_stack.push(i as u16);
        }
        self.initialized = true;
    }

    /// Allocates a slot from this size class in O(1) time, returning `(slot_id, slice_ptr)`.
    #[inline]
    pub fn alloc(&mut self) -> Option<(u16, *mut u8)> {
        self.ensure_init();
        let idx = self.free_stack.pop()?;
        self.alloc_hits += 1;
        let ptr = unsafe { self.backing.as_mut_ptr().add((idx as usize) * self.obj_size) };
        Some((idx, ptr))
    }

    /// Returns a slot back to the O(1) free-stack.
    #[inline]
    pub fn free(&mut self, slot: u16) {
        if (slot as usize) < SLOTS_PER_CLASS {
            self.free_stack.push(slot);
            self.frees += 1;
        }
    }

    pub fn free_count(&self) -> usize {
        if !self.initialized {
            SLOTS_PER_CLASS
        } else {
            self.free_stack.len()
        }
    }
}

/// Global segregated O(1) slab allocator for common kernel object sizes (`32B..2048B`).
pub struct SegregatedSlabPool {
    classes: [Spinlock<SlabClassPool>; 7],
}

impl SegregatedSlabPool {
    pub const fn new() -> Self {
        Self {
            classes: [
                Spinlock::new(SlabClassPool::new(32)),
                Spinlock::new(SlabClassPool::new(64)),
                Spinlock::new(SlabClassPool::new(128)),
                Spinlock::new(SlabClassPool::new(256)),
                Spinlock::new(SlabClassPool::new(512)),
                Spinlock::new(SlabClassPool::new(1024)),
                Spinlock::new(SlabClassPool::new(2048)),
            ],
        }
    }

    /// Maps an arbitrary byte size to the smallest fitting slab class index (`0..7`).
    #[inline]
    pub fn class_index_for(size: usize) -> Option<usize> {
        for (idx, &cls) in SLAB_CLASSES.iter().enumerate() {
            if size <= cls {
                return Some(idx);
            }
        }
        None
    }

    /// Pre-warms all 7 segregated slab classes at boot or benchmark setup.
    pub fn warm_all(&self) {
        for cls in self.classes.iter() {
            cls.lock().ensure_init();
        }
    }

    /// Allocates an object of `size` bytes from the matching slab cache.
    #[inline]
    pub fn alloc(&self, size: usize) -> Option<(usize, u16, *mut u8)> {
        let idx = Self::class_index_for(size)?;
        let (slot, ptr) = self.classes[idx].lock().alloc()?;
        Some((idx, slot, ptr))
    }

    /// Frees an object back into its slab class.
    #[inline]
    pub fn free(&self, class_idx: usize, slot: u16) {
        if class_idx < self.classes.len() {
            self.classes[class_idx].lock().free(slot);
        }
    }

    pub fn total_hits(&self) -> u64 {
        let mut sum = 0;
        for c in self.classes.iter() {
            sum += c.lock().alloc_hits;
        }
        sum
    }
}

pub static GLOBAL_SLAB_POOL: SegregatedSlabPool = SegregatedSlabPool::new();

// ============================================================================
// Per-CPU Physical Frame Magazine Cache (PMM Lock Elimination)
// ============================================================================

/// Maximum physical 4 KiB frames cached per CPU core inside its local magazine.
pub const FRAME_MAGAZINE_CAP: usize = 32;

/// Cache-line-isolated Per-CPU physical page frame cache eliminating global PMM lock trips.
pub struct FrameMagazine {
    frames: [Option<PhysFrame>; FRAME_MAGAZINE_CAP],
    count: usize,
    pub fast_hits: u64,
    pub refills: u64,
}

impl FrameMagazine {
    pub const fn new() -> Self {
        Self {
            frames: [None; FRAME_MAGAZINE_CAP],
            count: 0,
            fast_hits: 0,
            refills: 0,
        }
    }

    /// Allocates a 4 KiB physical frame from this CPU's local magazine, refilling a batch
    /// of 16 frames from the global PMM only when empty.
    #[inline]
    pub fn alloc_frame_fast(&mut self) -> Option<PhysFrame> {
        if self.count == 0 {
            self.refill(16);
        }
        if self.count > 0 {
            self.count -= 1;
            self.fast_hits += 1;
            return self.frames[self.count].take();
        }
        None
    }

    /// Returns a 4 KiB physical frame back to the local magazine, spilling half back to PMM if full.
    #[inline]
    pub fn dealloc_frame_fast(&mut self, frame: PhysFrame) {
        if self.count >= FRAME_MAGAZINE_CAP {
            self.drain(16);
        }
        self.frames[self.count] = Some(frame);
        self.count += 1;
    }

    fn refill(&mut self, batch: usize) {
        self.refills += 1;
        let want = core::cmp::min(batch, FRAME_MAGAZINE_CAP - self.count);
        for _ in 0..want {
            if let Some(f) = memory::alloc_frame() {
                self.frames[self.count] = Some(f);
                self.count += 1;
            } else {
                break;
            }
        }
    }

    fn drain(&mut self, batch: usize) {
        let drop_cnt = core::cmp::min(batch, self.count);
        for _ in 0..drop_cnt {
            self.count -= 1;
            if let Some(f) = self.frames[self.count].take() {
                memory::dealloc_frame(f);
            }
        }
    }

    /// Returns all cached frames cleanly back to global PMM (zero memory leaks).
    pub fn flush_all(&mut self) {
        self.drain(self.count);
    }

    pub fn cached_count(&self) -> usize {
        self.count
    }
}

pub static PER_CPU_MAGAZINES: [CacheAligned<Spinlock<FrameMagazine>>; 8] = [
    CacheAligned::new(Spinlock::new(FrameMagazine::new())),
    CacheAligned::new(Spinlock::new(FrameMagazine::new())),
    CacheAligned::new(Spinlock::new(FrameMagazine::new())),
    CacheAligned::new(Spinlock::new(FrameMagazine::new())),
    CacheAligned::new(Spinlock::new(FrameMagazine::new())),
    CacheAligned::new(Spinlock::new(FrameMagazine::new())),
    CacheAligned::new(Spinlock::new(FrameMagazine::new())),
    CacheAligned::new(Spinlock::new(FrameMagazine::new())),
];

// ============================================================================
// Page-Table Optimizations (PCID, INVPCID, 2MiB HugePages, Batched TLB)
// ============================================================================

/// Hardware paging extension flags detected via CPUID.
#[derive(Debug, Clone, Copy)]
pub struct PageTableOptCaps {
    /// Process-Context Identifiers (`CPUID.01H:ECX[17]`) allow CR3 switches without full TLB flush.
    pub pcid_supported: bool,
    /// Fine-grained `INVPCID` instruction (`CPUID.07H:EBX[10]`).
    pub invpcid_supported: bool,
    /// 1 GiB HugePages (`CPUID.80000001H:EDX[26]`).
    pub huge_1gb_supported: bool,
    /// 2 MiB HugePage bit mask (`Present | Writable | PageSize`).
    pub huge_2mb_flags: u64,
}

static INVLPG_SAVED_FULL_FLUSHES: AtomicU64 = AtomicU64::new(0);

/// Probes CPUID for hardware paging acceleration features (PCID, INVPCID, 1GB pages).
pub fn detect_paging_caps() -> PageTableOptCaps {
    let (max_basic, _, ecx1, _) = cpu::cpuid(1, 0);
    let pcid_supported = (ecx1 & (1 << 17)) != 0;

    let invpcid_supported = if max_basic >= 7 {
        let (_, ebx7, _, _) = cpu::cpuid(7, 0);
        (ebx7 & (1 << 10)) != 0
    } else {
        false
    };

    let (max_ext, _, _, _) = cpu::cpuid(0x8000_0000, 0);
    let huge_1gb_supported = if max_ext >= 0x8000_0001 {
        let (_, _, _, edx_ext) = cpu::cpuid(0x8000_0001, 0);
        (edx_ext & (1 << 26)) != 0
    } else {
        false
    };

    PageTableOptCaps {
        pcid_supported,
        invpcid_supported,
        huge_1gb_supported,
        huge_2mb_flags: (1 << 0) | (1 << 1) | (1 << 7), // PRESENT | WRITABLE | HUGE_PAGE (PS)
    }
}

/// Encodes a 2 MiB aligned Page Directory Entry (`PDE`) with `PS` (`1 << 7`) enabled.
#[inline]
pub const fn encode_2mb_huge_pde(phys_2mb_aligned: u64, writable: bool, nx: bool) -> u64 {
    let mut entry = (phys_2mb_aligned & 0x000F_FFFF_FFE0_0000) | (1 << 0) | (1 << 7);
    if writable {
        entry |= 1 << 1;
    }
    if nx {
        entry |= 1 << 63;
    }
    entry
}

/// Maximum number of virtual pages selectively invalidated with `invlpg` before collapsing
/// into a single full CR3 TLB flush.
pub const TLB_INVLPG_THRESHOLD: usize = 16;

/// Accumulates modified virtual addresses and selects optimal hardware invalidation strategy.
pub struct TlbShootdownBatch {
    addrs: [u64; TLB_INVLPG_THRESHOLD],
    count: usize,
    overflowed: bool,
}

impl TlbShootdownBatch {
    pub const fn new() -> Self {
        Self {
            addrs: [0u64; TLB_INVLPG_THRESHOLD],
            count: 0,
            overflowed: false,
        }
    }

    /// Queues a modified page address for deferred TLB invalidation.
    #[inline]
    pub fn push(&mut self, vaddr: u64) {
        if self.count < TLB_INVLPG_THRESHOLD {
            self.addrs[self.count] = vaddr;
            self.count += 1;
        } else {
            self.overflowed = true;
        }
    }

    /// Flushes queued TLB entries selectively (`invlpg` per page if `<= 16`, else single full flush).
    /// Returns `true` if selective `invlpg` avoided a full CR3 flush.
    pub fn flush(&mut self) -> bool {
        if self.count == 0 {
            return true;
        }
        if self.overflowed {
            paging::x86::tlb_flush_all();
            self.count = 0;
            self.overflowed = false;
            false
        } else {
            for i in 0..self.count {
                paging::x86::tlb_flush(VirtAddr::new(self.addrs[i]));
            }
            INVLPG_SAVED_FULL_FLUSHES.fetch_add(1, Ordering::Relaxed);
            self.count = 0;
            true
        }
    }
}

pub fn tlb_selective_saves() -> u64 {
    INVLPG_SAVED_FULL_FLUSHES.load(Ordering::Relaxed)
}
