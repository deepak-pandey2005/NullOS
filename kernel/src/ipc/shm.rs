//! # Shared Memory Subsystem (POSIX `shmget` / `shmat`)
//!
//! ## Overview
//! Shared memory represents the highest-throughput IPC mechanism in modern operating systems.
//! Instead of copying data across kernel space (like pipes or sockets), shared memory maps
//! the **exact same physical page frames** into the virtual address spaces of two or more
//! isolated processes:
//!
//! ```text
//! Process A Virtual Space                Physical Memory                Process B Virtual Space
//! +---------------------+             +-------------------+             +---------------------+
//! | Virtual Page 0x6000 | ----------> | Physical Frame #8 | <---------- | Virtual Page 0xA000 |
//! +---------------------+             +-------------------+             +---------------------+
//! ```
//!
//! ## Operational Pipeline:
//! 1. [`shmget`]: Looks up an existing segment by `key`, or allocates `num_pages` zeroed physical
//!    frames from the PMM and registers a new [`SharedMemorySegment`].
//! 2. [`shmat`]: Walks the calling process's page table, mapping the segment's physical frames
//!    at virtual address `vaddr` with `USER_ACCESSIBLE | WRITABLE` flags.
//! 3. Data written by Process A is instantly visible to Process B at CPU bus speed without
//!    issuing system calls or context switches.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::memory::address::{Page, PhysAddr, PhysFrame, VirtAddr, PAGE_SIZE_4K};
use crate::memory::paging::{self, PageTableFlags};
use crate::memory::AddressSpace;
use crate::sync::Spinlock;

/// Unique 64-bit Shared Memory Segment Identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShmId(pub u64);

/// Shared memory segment descriptor tracking allocated physical frames and attachment count.
pub struct SharedMemorySegment {
    /// Unique segment identifier.
    pub id: ShmId,
    /// User-supplied lookup key (ftok or explicit constant).
    pub key: u64,
    /// Total byte size of the segment (multiple of 4096).
    pub size: usize,
    /// Physical frames backing this shared segment.
    pub frames: Vec<PhysFrame>,
    /// Number of active process address space attachments.
    pub attachments: usize,
    /// POSIX file permission mode (e.g. 0o666).
    pub permissions: u32,
}

impl SharedMemorySegment {
    /// Maps the shared physical frames into the target process address space at `vaddr`.
    pub fn attach(&mut self, _address_space: &mut AddressSpace, vaddr: VirtAddr) -> Result<(), &'static str> {
        let flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE
            | PageTableFlags::NO_EXECUTE;

        for (i, frame) in self.frames.iter().enumerate() {
            let page = Page::containing_address(VirtAddr::new(vaddr.as_u64() + (i as u64) * PAGE_SIZE_4K));
            paging::map_page(page, *frame, flags)?;
        }

        self.attachments += 1;
        Ok(())
    }
}

/// Information summary of an active shared memory segment for diagnostics.
#[derive(Debug, Clone)]
pub struct ShmInfo {
    pub id: u64,
    pub key: u64,
    pub size: usize,
    pub pages: usize,
    pub attachments: usize,
}

/// Global Thread-Safe Shared Memory Registry.
pub struct ShmRegistry {
    segments: BTreeMap<ShmId, SharedMemorySegment>,
    key_to_id: BTreeMap<u64, ShmId>,
}

impl ShmRegistry {
    pub const fn new() -> Self {
        Self {
            segments: BTreeMap::new(),
            key_to_id: BTreeMap::new(),
        }
    }

    /// Creates or retrieves a shared memory segment with the specified key and size.
    pub fn get_or_create(&mut self, key: u64, size: usize, create: bool) -> Result<ShmId, &'static str> {
        if let Some(&id) = self.key_to_id.get(&key) {
            return Ok(id);
        }

        if !create {
            return Err("Shared memory segment not found");
        }

        let num_pages = (size + (PAGE_SIZE_4K as usize) - 1) / (PAGE_SIZE_4K as usize);
        if num_pages == 0 {
            return Err("Invalid shared memory size");
        }

        let mut frames = Vec::with_capacity(num_pages);
        for _ in 0..num_pages {
            let frame = crate::memory::alloc_frame().ok_or("Out of physical memory for shared segment")?;
            // Zero-fill physical memory
            unsafe {
                core::ptr::write_bytes(frame.start_address().as_u64() as *mut u8, 0, PAGE_SIZE_4K as usize);
            }
            frames.push(frame);
        }

        let id = ShmId(NEXT_SHM_ID.fetch_add(1, Ordering::SeqCst));
        let segment = SharedMemorySegment {
            id,
            key,
            size: num_pages * (PAGE_SIZE_4K as usize),
            frames,
            attachments: 0,
            permissions: 0o666,
        };

        self.segments.insert(id, segment);
        self.key_to_id.insert(key, id);

        Ok(id)
    }

    /// Attaches the specified shared memory segment to an address space.
    pub fn attach(&mut self, id: ShmId, address_space: &mut AddressSpace, vaddr: VirtAddr) -> Result<(), &'static str> {
        let segment = self.segments.get_mut(&id).ok_or("Invalid ShmId")?;
        segment.attach(address_space, vaddr)
    }

    /// Returns a list of all active shared memory segments.
    pub fn list(&self) -> Vec<ShmInfo> {
        self.segments
            .values()
            .map(|s| ShmInfo {
                id: s.id.0,
                key: s.key,
                size: s.size,
                pages: s.frames.len(),
                attachments: s.attachments,
            })
            .collect()
    }
}

static NEXT_SHM_ID: AtomicU64 = AtomicU64::new(1);
pub static SHM_REGISTRY: Spinlock<ShmRegistry> = Spinlock::new(ShmRegistry::new());

/// Creates or retrieves a shared memory segment.
pub fn shmget(key: u64, size: usize, create: bool) -> Result<ShmId, &'static str> {
    SHM_REGISTRY.lock().get_or_create(key, size, create)
}

/// Attaches a shared memory segment to a target address space.
pub fn shmat(id: ShmId, address_space: &mut AddressSpace, vaddr: VirtAddr) -> Result<(), &'static str> {
    SHM_REGISTRY.lock().attach(id, address_space, vaddr)
}

/// Attaches a shared memory segment to the currently active page table at `vaddr` and returns its size.
pub fn shmat_current(id: ShmId, vaddr: VirtAddr) -> Result<usize, &'static str> {
    let mut reg = SHM_REGISTRY.lock();
    let seg = reg.segments.get_mut(&id).ok_or("Invalid SHM ID")?;
    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_EXECUTE;
    for (i, frame) in seg.frames.iter().enumerate() {
        let page = Page::containing_address(VirtAddr::new(vaddr.as_u64() + (i as u64) * PAGE_SIZE_4K));
        paging::map_page(page, *frame, flags)?;
    }
    seg.attachments += 1;
    Ok(seg.size)
}

/// Returns diagnostic status of all active shared memory regions.
pub fn shm_list() -> Vec<ShmInfo> {
    SHM_REGISTRY.lock().list()
}
