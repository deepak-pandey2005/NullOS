# Chapter 5: Physical Memory Management
### *Managing Raw Silicon: Page Frames and the Bitmap Allocator*

Before an operating system can create threads, allocate virtual memory, or load programs, it must solve a fundamental physical problem:
**How do we know which physical RAM bytes are free, and which ones are holding critical hardware data?**

In this chapter, we explore **Layer 1** of the memory architecture: the **Physical Memory Manager (PMM)**, why memory is managed in **4 KiB Page Frames**, and how NullOS uses a **Bitmap Allocator** to manage hundreds of megabytes of RAM with near-zero overhead.

---

## 1. Physical vs. Virtual Memory

It is critical to distinguish between physical and virtual memory:

```
+-------------------------------------------------------------+
| VIRTUAL ADDRESS (What instructions use: e.g. 0x444444440000)|
+-------------------------------------------------------------+
                               |
                   [MMU 4-Level Page Table]
                               |
                               v
+-------------------------------------------------------------+
| PHYSICAL ADDRESS (Actual electrical pins on RAM chips)      |
| Pin 0x00000000 ............................ Pin 0x1E150000  |
+-------------------------------------------------------------+
```

- **Physical Memory**: The real, physical capacitor banks inside DDR4/DDR5 RAM sticks. Address `0x1000` corresponds to physical wire traces and capacitor rows on your motherboard.
- **Virtual Memory**: An artificial coordinate system created by page tables (Chapter 6).

The **Physical Memory Manager (PMM)** cares **only** about physical memory. It does not care about threads, processes, or virtual addresses. Its single responsibility is: *"Here is a 4 KiB block of real physical silicon. Is it free, or is someone using it?"*

---

## 2. Why 4 KiB (4,096 Bytes)?

Why doesn't the PMM allocate single bytes?
- A computer with 16 GB of RAM has $17,179,869,184$ individual bytes.
- If the operating system tracked the state (free or used) of every single byte using a 1-byte flag, the tracking data structure would take **16 Gigabytes of RAM** just to track 16 Gigabytes of RAM! You would have zero bytes left for any program!

To solve this, CPU hardware and operating systems group bytes into **Page Frames**:
$$\mathbf{1\text{ Page Frame} = 4,096\text{ Bytes (4 KiB)} = 0x1000\text{ bytes}}$$

Because a 4 KiB frame always starts at a multiple of 4,096, its lowest 12 bits are always zeroes:
```text
Frame 0: 0x0000_0000 ... 0x0000_0FFF
Frame 1: 0x0000_1000 ... 0x0000_1FFF
Frame 2: 0x0000_2000 ... 0x0000_2FFF
...
```
The lowest 12 bits ($2^{12} = 4096$) represent the offset inside the page!

---

## 3. Discovering RAM: The UEFI Memory Map

When the computer turns on, RAM is not a continuous empty field. Parts of RAM are used by motherboard firmware, ACPI power tables, and GPU framebuffers.

In NullOS ([kernel/src/memory/mod.rs](../../kernel/src/memory/mod.rs)), the kernel inspects the **UEFI Memory Map** passed by the bootloader:

```text
[00] Conventional (Free RAM) Start: 0x000000000000 End: 0x000000087000 Pages: 135 (540 KiB)
[01] BootServicesData        Start: 0x000000087000 End: 0x000000088000 Pages:   1 (  4 KiB)
[02] Conventional (Free RAM) Start: 0x000000088000 End: 0x0000000a0000 Pages:  24 ( 96 KiB)
[03] LoaderData (Kernel)     Start: 0x000000100000 End: 0x00000013c000 Pages:  60 (240 KiB)
[04] Conventional (Free RAM) Start: 0x00000013c000 End: 0x000000800000 Pages:1732 (6928 KiB)
[05] AcpiNVS                 Start: 0x000000800000 End: 0x000000808000 Pages:   8 ( 32 KiB)
...
```

The PMM iterates through this list:
1. It calculates the **highest physical address** of usable RAM (e.g. `0x1E15_0000` $\approx 481\text{ MiB}$).
2. It divides by 4,096 to determine the **total number of frames** in the system:
   $$\text{Total Frames} = \frac{0x1E15\_0000}{4096} = 123,216\text{ frames}$$

---

## 4. The Bitmap Allocator: Maximum Power, Minimum Space

How do we track whether each of those 123,216 frames is free or used?

We use a **Bitmap** ([kernel/src/memory/frame_allocator.rs](../../kernel/src/memory/frame_allocator.rs)):
- **Bit = `0`** $\rightarrow$ Frame is **FREE**!
- **Bit = `1`** $\rightarrow$ Frame is **ALLOCATED**!

```
Byte 0:   [ Bit 7 | Bit 6 | Bit 5 | Bit 4 | Bit 3 | Bit 2 | Bit 1 | Bit 0 ]
          Frame 7 Frame 6 Frame 5 Frame 4 Frame 3 Frame 2 Frame 1 Frame 0

Byte 1:   [ Bit 15| Bit 14| Bit 13| Bit 12| Bit 11| Bit 10| Bit 9 | Bit 8 ]
          Frame 15...                                             Frame 8
```

### The Math: Why a Bitmap is Unbeatable
To track 123,216 frames:
$$\text{Bitmap Size} = \frac{123,216\text{ bits}}{8\text{ bits/byte}} = 15,402\text{ bytes} \approx \mathbf{16\text{ KiB (4 frames)!}}$$

*Think about this:* Just **4 frames (16 KiB)** of memory is enough to manage **nearly 500 Megabytes of RAM**!

### Where do we put the Bitmap?
Before the PMM exists, you cannot call `malloc()` or `alloc()` to create the bitmap. It's a chicken-and-egg problem!
**The Solution:**
The kernel scans the UEFI memory map for the very first free `ConventionalMemory` chunk that is at least 16 KiB long (found at physical address `0x0013_C000`), reserves those 4 frames, places the bitmap there, and zeroes it out!

---

## 5. Reserving Critical Memory

We cannot let the allocator hand out just any physical address. Certain memory ranges must be **permanently reserved**:

1. **Lower 1 MiB (`0x0` to `0x100000`)**: Contains the Real Mode Interrupt Vector Table (IVT), BIOS Data Area (BDA), and video memory. We mark all 256 frames as `1` (used).
2. **The Kernel Image (`0x100000` to `0x13C000`)**: The physical frames where our kernel code and data reside. We mark them as `1` so the kernel does not overwrite itself!
3. **The PMM Bitmap itself (`0x13C000` to `0x140000`)**: Marked as `1`.
4. **Non-RAM Regions**: ACPI tables, MMIO registers, and reserved firmware memory are marked as `1`.

Everything else marked `ConventionalMemory` in the UEFI memory map is marked `0` (available for allocation).

---

## 6. How Frame Allocation Works in Rust

### A. Allocating a Single Frame: `alloc_frame()`
When paging or the heap needs a 4 KiB frame:
1. Scan the bitmap byte-by-byte. If a byte is `0xFF` (all 8 bits are 1), skip it!
2. When a byte $< 0xFF$ is found, find the first `0` bit using the CPU instruction `trailing_ones` or `tzcnt`:
   $$\text{Frame Index} = (\text{Byte Index} \times 8) + \text{Bit Index}$$
3. Set the bit to `1` (mark as used).
4. Decrement the free frame counter.
5. Compute the physical address:
   $$\text{Physical Address} = \text{Frame Index} \times 4096$$
6. Return `Some(PhysFrame)`.

```rust
pub fn alloc_frame(&mut self) -> Option<PhysFrame> {
    for byte_idx in 0..self.bitmap_bytes {
        if self.bitmap[byte_idx] != 0xFF {
            for bit_idx in 0..8 {
                let mask = 1 << bit_idx;
                if (self.bitmap[byte_idx] & mask) == 0 {
                    // Mark as allocated
                    self.bitmap[byte_idx] |= mask;
                    self.used_frames += 1;
                    let frame_idx = (byte_idx * 8) + bit_idx;
                    return Some(PhysFrame::from_index(frame_idx as u64));
                }
            }
        }
    }
    None // Out of physical memory!
}
```

### B. Freeing a Frame: `dealloc_frame()`
When a process exits or memory is released:
1. Convert the physical address to a frame index:
   $$\text{Frame Index} = \frac{\text{Physical Address}}{4096}$$
2. Compute the byte and bit:
   $$\text{Byte Index} = \text{Frame Index} / 8, \quad \text{Bit Index} = \text{Frame Index} \% 8$$
3. Clear the bit to `0`:
   ```rust
   self.bitmap[byte_idx] &= !(1 << bit_idx);
   self.used_frames -= 1;
   ```
4. The frame is instantly available for reuse!

---

## 🧠 Self-Test & Review Questions

1. **Why does the Physical Memory Manager care about 4 KiB alignment? What would happen if a physical frame started at address `0x1005`?**
   <details>
   <summary>Click to view answer</summary>
   The x86_64 CPU hardware MMU requires all page table base addresses and mapped physical frames to be aligned to 4 KiB boundaries (lowest 12 bits must be zero). If an address started at <code>0x1005</code>, the lowest 12 bits would be mistaken by the CPU hardware as Page Table Entry control flags (like Present, Writable, User) rather than address bits!
   </details>

2. **How does a bitmap allocator achieve $O(1)$ or very fast sequential allocation?**
   <details>
   <summary>Click to view answer</summary>
   By inspecting memory in 64-bit integer chunks (words), the allocator can check 64 frames at once. If a 64-bit word equals <code>u64::MAX</code> (<code>0xFFFF_FFFF_FFFF_FFFF</code>), all 64 frames are occupied and skipped in a single CPU instruction!
   </details>

3. **What is the purpose of preserving the Lower 1 MiB (`0x0 .. 0x100000`) of physical memory from allocation?**
   <details>
   <summary>Click to view answer</summary>
   The lower 1 MiB contains hardware-specific data structures, BIOS Data Area (BDA), AP startup trampoline code for waking secondary CPU cores (SMP), and VGA/video memory buffers. Overwriting it would corrupt hardware communication.
   </details>

---

*In [Chapter 6: Virtual Memory & 4-Level Paging](chapter06_virtual_memory_paging.md), we explore how the CPU's MMU uses page tables to map virtual addresses to these physical frames.*
