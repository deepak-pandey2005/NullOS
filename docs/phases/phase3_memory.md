# Phase 3 — Memory Management (Milestones M7 - M9)

This phase establishes the memory management subsystem for NullOS, responsible for physical memory allocation, virtual address translation, and dynamic kernel heap management.

---

## Subsystem Architecture (`kernel/src/memory/`)

```
kernel/src/memory/
├── mod.rs             # Subsystem facade, init, memory statistics, self-tests
├── address.rs         # Strongly-typed PhysAddr, VirtAddr, PhysFrame, Page abstractions
├── uefi_mmap.rs       # UEFI memory map descriptor parser, UefiMemoryType, iterator
├── frame_allocator.rs # Bitmap physical frame allocator, range reservation, alloc/dealloc
├── paging.rs          # (M8) 4-level x86_64 paging & page tables
└── heap.rs            # (M9) Kernel dynamic heap allocator & GlobalAlloc
```

---

## Completed Milestones

### M7 — Physical Memory Manager (PMM)
- [x] **UEFI Memory Map Parsing**: Type-safe parser (`uefi_mmap::MemoryMapIterator`) reading 40-byte / 48-byte UEFI descriptors passed by the bootloader.
- [x] **Usable RAM Detection**: Identified all `ConventionalMemory` regions and computed total physical address space bounds.
- [x] **Hardware & Kernel Memory Reservation**:
  - Reserved Lower 1 MiB (`0x0000_0000 .. 0x0010_0000`) for BIOS IVT, BDA, and legacy hardware.
  - Reserved Kernel ELF segments (`0x100000 ..`).
  - Reserved Bootloader metadata, `BootInfo`, and command line buffer.
  - Reserved UEFI MemoryMap descriptor buffer.
  - Reserved GOP Framebuffer MMIO region.
- [x] **Dynamic Bitmap Allocator**:
  - Evaluated required bitmap size dynamically based on physical memory span (16 KiB for 512 MiB RAM).
  - Dynamically allocated the bitmap buffer in usable conventional memory above the kernel image.
  - Marked the PMM bitmap's own physical frames as reserved.
- [x] **4 KiB Frame Allocation Primitives**:
  - `memory::alloc_frame() -> Option<PhysFrame>`
  - `memory::dealloc_frame(frame: PhysFrame)`
  - `memory::alloc_contiguous_frames(count: usize) -> Option<PhysFrame>`
- [x] **Thread-Safe Synchronization**:
  - Protected the global `FRAME_ALLOCATOR` with `crate::sync::Spinlock`.
- [x] **Memory Statistics**:
  - Live tracking of total frames, used frames, free frames, and total/free/used MiB.
- [x] **Built-in Self-Tests**:
  - 4 KiB address alignment verification.
  - Memory write/read test pattern on allocated physical frames.
  - Multi-frame contiguous allocation test.
  - Free frame counter integrity verification across allocations and deallocations.
  - Frame recycling verification.

---

### M8 — Virtual Memory & 4-Level Paging
- [x] **Page Table Structures**: 512-entry aligned `PageTable`, `PageTableEntry`, and comprehensive `PageTableFlags` (Present, Writable, User, WriteThrough, NoCache, HugePage, Global, NoExecute).
- [x] **4-Level Page Table Hierarchy**: PML4, PDPT, PD, and PT indexing and traversal logic.
- [x] **Dedicated Kernel PML4**: Allocated a root PML4 physical frame and established a clean kernel address space.
- [x] **Recursive Page Table Mapping**: Bound PML4 entry 511 to the PML4 itself (`pml4_frame`), enabling transparent higher-half access to the page table hierarchy.
- [x] **Identity Mapping of Critical Hardware**:
  - Identity-mapped lower 1 MiB physical space.
  - Identity-mapped kernel executable ELF segments (`.text`, `.rodata`, `.data`, `.bss`).
  - Identity-mapped conventional RAM.
  - Identity-mapped `BootInfo`, `MemoryMap`, and command line buffers.
  - Identity-mapped GOP Framebuffer MMIO with `Write-Through` and `No-Execute` attributes.
  - Identity-mapped Local APIC MMIO (`0xFEE0_0000`) with `Cache-Disable` and `No-Execute` attributes.
- [x] **CR3 Switch & TLB Management**: Switched `%cr3` to the Kernel PML4, supported single-page invalidation (`invlpg`), and full TLB flushing.
- [x] **Dynamic Page Mapping & Unmapping**: Safe `map_page()` dynamically allocating intermediate tables from the PMM, and `unmap_page()`.
- [x] **Virtual-to-Physical Address Translation**: `translate_addr()` resolving arbitrary virtual addresses through the 4-level hierarchy.
- [x] **Enhanced Page Fault Diagnostics**: Connected `#PF` exception handler to decode error codes (present, write, user, reserved bit, instruction fetch / NX).
- [x] **Built-in VMM Self-Tests**: Verified identity translation, dynamic virtual mapping at `0x0000_7000_0000_0000`, memory read/write across page tables, unmapping, and translation failure on unmapped pages.

---

## Upcoming Milestones

### M9 — Kernel Heap Allocator
- Linked-list / Slab / Buddy allocator implementation.
- `#[global_allocator]` implementation of `core::alloc::GlobalAlloc`.
- Enable Rust standard collection types via `extern crate alloc;` (`Vec`, `Box`, `String`).

---

## Verification

Run the automated headless test:
```bash
./scripts/run.sh --headless
```

Expected output:
```
----------------------------------------
 [MEMORY] Initializing Memory Management 
----------------------------------------
[MEMORY] UEFI Memory Map at 0x1e057018 (110 entries, desc size: 40 bytes):
  [00] Conventional (Free RAM) Start: 0x000000000000 End: 0x000000087000 Pages:    135 ( 540 KiB)
  ...
[MEMORY] Physical space: max addr 0x1e150000, total frames: 123216, bitmap needed: 15402 bytes (16 KiB)
[MEMORY] Placed PMM bitmap at physical address 0x000000122000 (4 frames)
[MEMORY] Physical Memory Manager active. Total: 481 MiB, Free: 462 MiB (118375 frames), Used: 18 MiB (4841 frames)
[MEMORY] Running PMM verification self-tests...
[MEMORY] All PMM self-tests passed successfully! (Frame allocation, contiguous, read/write, dealloc, recycle)
```
