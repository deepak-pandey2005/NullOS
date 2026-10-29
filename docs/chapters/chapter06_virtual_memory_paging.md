# Chapter 6: Virtual Memory & 4-Level Paging
### *The Architectural Miracle: 48-Bit Addresses, the MMU, and CR3*

If an operating system had only physical memory, every process would have to know where other processes lived in RAM. If Process A wrote to address `0x2000`, it could corrupt Process B's memory. If a program needed 10 contiguous megabytes, but RAM was fragmented, the allocation would fail.

**Virtual Memory** solves this by giving every single program the illusion that it owns a pristine, private, contiguous 128 Terabyte address space starting at `0x0`.

In this chapter, we explore how x86_64 **4-Level Paging** turns this illusion into physical reality.

---

## 1. Why Virtual Memory?

Virtual memory provides four superpowers:

1. **Total Process Isolation**: Process A at virtual address `0x400000` maps to physical frame `0x150000`. Process B at virtual address `0x400000` maps to physical frame `0x890000`. They share the same numbers, but access completely different physical silicon chips!
2. **Memory Protection**: A virtual page can be marked:
   - **Read-Only**: Attempting to write to it causes an instant Page Fault (`#PF`).
   - **Kernel-Only (`U/S = 0`)**: User mode (Ring 3) cannot read or write it.
   - **No-Execute (`NX = 1`)**: Attempting to execute code from it (e.g. stack or heap) crashes the program, preventing buffer-overflow exploits!
3. **Contiguous Illusion from Fragmented RAM**: A program can request 1 GB of contiguous virtual memory, even if physical RAM is scattered in thousands of fragmented pieces across the motherboard.
4. **Hardware MMIO Protection**: Physical device registers (like the GPU Framebuffer or Local APIC) can be mapped into high virtual memory with special caching rules (e.g. Write-Through or Uncacheable).

---

## 2. The 48-Bit Virtual Address Breakdown

In 64-bit x86_64 CPUs, physical and virtual addresses do not yet use all 64 bits (implementing 64 full bits in hardware would require too many transistors and wires). Modern CPUs use **48-bit Canonical Addressing**:

```
63             48 47       39 38       30 29       21 20       12 11            0
+----------------+-----------+-----------+-----------+-----------+---------------+
| Sign Extension | PML4 (L4) | PDPT (L3) |  PD (L2)  |  PT (L1)  |  Page Offset  |
|  (16 bits)     | (9 bits)  | (9 bits)  | (9 bits)  | (9 bits)  |   (12 bits)   |
+----------------+-----------+-----------+-----------+-----------+---------------+
```

### 1. Sign Extension (Bits 48..63)
Bits 48 through 63 must be identical copies of Bit 47!
- If Bit 47 is `0`: Bits 48..63 must be all `0` (Lower Half: `0x0000_0000_0000_0000` to `0x0000_7FFF_FFFF_FFFF` = 128 TiB User Space).
- If Bit 47 is `1`: Bits 48..63 must be all `1` (Higher Half: `0xFFFF_8000_0000_0000` to `0xFFFF_FFFF_FFFF_FFFF` = 128 TiB Kernel Space).
Addresses that violate this rule are non-canonical; using them triggers a General Protection Fault.

### 2. The Four 9-Bit Indices
Notice that $2^9 = 512$.
Each level of the tree has exactly **512 entries**:
- **PML4 Index** (Bits 39..47): Selects 1 of 512 entries in the Page Map Level 4 table.
- **PDPT Index** (Bits 30..38): Selects 1 of 512 entries in the Page Directory Pointer Table.
- **PD Index** (Bits 21..29): Selects 1 of 512 entries in the Page Directory.
- **PT Index** (Bits 12..20): Selects 1 of 512 entries in the Page Table.
- **Offset** (Bits 0..11): Selects 1 of 4,096 exact bytes ($2^{12} = 4096$) inside the physical page frame!

---

## 3. How the Hardware MMU Walks the Tree

When your CPU executes an instruction like `mov (%rax), %rbx`:

```
               CR3 Register (Physical Address of PML4 Table)
                                   |
                                   v
                             [PML4 Table]
                             [Entry 0   ]
                             [Entry ... ] ---> Points to PDPT Frame
                             [Entry 511 ]
                                   |
                                   v
                             [PDPT Table]
                             [Entry 0   ]
                             [Entry ... ] ---> Points to PD Frame
                             [Entry 511 ]
                                   |
                                   v
                             [PD Table]
                             [Entry 0   ]
                             [Entry ... ] ---> Points to PT Frame
                             [Entry 511 ]
                                   |
                                   v
                             [PT Table]
                             [Entry 0   ]
                             [Entry ... ] ---> Points to Final Physical Frame!
                             [Entry 511 ]
                                   |
                                   v
                         [PHYSICAL RAM FRAME] + Offset (0..4095)
```

Every single table (`PML4`, `PDPT`, `PD`, `PT`) is exactly **one 4 KiB frame (4,096 bytes)** containing **512 entries of 8 bytes each** ($512 \times 8 = 4096$).

---

## 4. Anatomy of a Page Table Entry (PTE - 64 Bits)

Every entry in every page table has this exact bit layout:

```
63  62       52 51                                         12 11 9 8 7 6 5 4 3 2 1 0
+--+-----------+---------------------------------------------+---+-+-+-+-+-+-+-+-+-+-+
|NX| Available |          Physical Frame Address             |AVL|G|H|D|A|C|W|U|W|P|
|  |           |                (40 bits)                    |   | | | | |D|T|S|R| |
+--+-----------+---------------------------------------------+---+-+-+-+-+-+-+-+-+-+-+
```

### The Control Flags:
- **Bit 0 (`P` - Present)**: Must be `1` if the mapped page or next-level table is in memory. If `0`, the CPU stops and raises a **Page Fault (`#PF`)**!
- **Bit 1 (`W/R` - Writable / Read-Only)**: `1` = Read and Write; `0` = Read-Only.
- **Bit 2 (`U/S` - User / Supervisor)**:
  - `0` = Supervisor (Ring 0 Kernel only).
  - `1` = User Mode (Ring 3 allowed).
- **Bit 3 (`PWT` - Page Write-Through)**: Controls memory write-through caching.
- **Bit 4 (`PCD` - Page Cache Disable)**: When `1`, memory caching is disabled (used for MMIO like the APIC or GPU).
- **Bit 5 (`A` - Accessed)**: Hardware sets this to `1` whenever this page is read or written.
- **Bit 6 (`D` - Dirty)**: Hardware sets this to `1` whenever this page is written to (vital for disk swapping!).
- **Bit 7 (`PS` - Page Size / Huge Page)**: In `PDPT` (1 GiB page) or `PD` (2 MiB page), setting this bypasses lower table levels.
- **Bit 8 (`G` - Global)**: Prevents the TLB from invalidating this translation when `CR3` is reloaded.
- **Bits 12..51 (`Physical Frame Address`)**: The physical base address of the next table or final page frame (must be 4 KiB aligned!).
- **Bit 63 (`NX` - No-Execute / Execute-Disable XD)**: When set, the CPU refuses to fetch instructions from this page!

---

## 5. The Critical Rule of Intermediate Table Flags

There is a subtle rule in x86_64 paging that causes countless bugs for OS developers:
> **Access rights are determined by the most restrictive setting across ALL 4 levels of the tree!**

Suppose you want to map a user page at virtual address `0x400000` with `USER_ACCESSIBLE` (`U/S = 1`).
- The leaf Page Table entry has `U/S = 1`.
- **BUT**, what if the `PML4` entry, `PDPT` entry, or `PD` entry has `U/S = 0`?
- **Result:** The CPU denies user access and raises a Page Fault!

In NullOS ([kernel/src/memory/paging.rs](../../kernel/src/memory/paging.rs)), when `map_page` traverses existing parent tables, if the page being mapped requires `USER_ACCESSIBLE`, we automatically propagate that flag up the tree:
```rust
if flags.contains(PageTableFlags::USER_ACCESSIBLE) {
    pml4[pml4_idx].add_flags(PageTableFlags::USER_ACCESSIBLE);
    pdpt[pdpt_idx].add_flags(PageTableFlags::USER_ACCESSIBLE);
    pd[pd_idx].add_flags(PageTableFlags::USER_ACCESSIBLE);
}
```

---

## 6. The `CR3` Register & Address Space Switching

How does the CPU know where the root `PML4` table is?
The CPU has a hardware register called **`CR3`** (Control Register 3).

```assembly
# To read the current PML4 physical frame:
mov %cr3, %rax

# To switch to a completely new process address space:
mov %rax, %cr3
```

When you write to `CR3`:
1. The CPU points its MMU to the new PML4 root table.
2. The entire virtual address space switches instantaneously!
3. The CPU automatically invalidates all non-global cached translations in the TLB.

---

## 7. The Translation Lookaside Buffer (TLB) & `invlpg`

If the CPU had to perform 4 memory reads (PML4 $\rightarrow$ PDPT $\rightarrow$ PD $\rightarrow$ PT) for *every single instruction and variable read*, the computer would run **5 times slower**.

To make paging blazing fast, the CPU includes an ultra-fast on-chip cache called the **TLB (Translation Lookaside Buffer)**.
- The TLB caches recently used `(Virtual Address -> Physical Address)` translations.
- Next time the CPU needs that virtual address, translation takes **0.5 nanoseconds** instead of walking RAM!

### The TLB Invalidation Problem:
What if the operating system unmaps a page or changes its permissions in RAM?
The CPU's TLB **does not know** you modified the page table! It will continue using the stale cached translation!
To fix this, we must explicitly tell the CPU to invalidate that translation using the **`invlpg`** instruction:
```rust
pub fn tlb_flush(vaddr: VirtAddr) {
    unsafe {
        core::arch::asm!("invlpg [{}]", in(reg) vaddr.as_u64(), options(nostack, preserves_flags));
    }
}
```

---

## 8. Recursive Page Table Mapping (The OS Developer's "Cheat Code")

Here is a chicken-and-egg dilemma in operating systems:
- Paging is enabled. You can only read and write to **virtual addresses**.
- But Page Tables are physical frames! How does the kernel modify a Page Table entry when it doesn't have a virtual address for it?

### The Recursive Mapping Solution:
During initialization in NullOS ([paging.rs](../../kernel/src/memory/paging.rs#L480-L487)):
We point the **very last entry of the PML4 table (`PML4[511]`) back to the PML4 table itself!**

```
PML4[511] ---> Points directly to PML4 physical frame!
```

Because `PML4[511]` points to the PML4 itself, any virtual address with `PML4 Index = 511` makes the MMU treat the PML4 as a PDPT, and so on!
This maps the entire 4-level page table hierarchy into a fixed, high virtual address range (`0xFFFF_FFFF_FFFF_0000` to `0xFFFF_FFFF_FFFF_FFFF`), allowing the kernel to read and edit any page table entry without identity-mapping physical RAM!

---

## 9. Anatomy of a Page Fault (`#PF`, Vector 14)

When memory translation fails, the CPU raises Exception 14:
1. **The Faulting Address**: The CPU saves the exact address that caused the fault in register **`CR2`**.
2. **The Error Code**: The CPU pushes a 32-bit error code onto the stack:
   - **Bit 0 (`P` - Present)**:
     - `0` = Page was not present (unmapped memory / null pointer dereference).
     - `1` = Page was present, but you violated permissions (e.g. wrote to read-only page or user touched kernel page).
   - **Bit 1 (`W/R` - Write)**: `1` if caused by a write operation; `0` if caused by a read.
   - **Bit 2 (`U/S` - User)**: `1` if the CPU was in Ring 3 User Mode; `0` if in Ring 0 Kernel Mode.
   - **Bit 4 (`I/D` - Instruction Fetch)**: `1` if the CPU attempted to fetch and execute an instruction from a page marked with **No-Execute (`NX`)**!

---

## 🧠 Self-Test & Review Questions

1. **Why does 64-bit x86_64 use 48 bits for virtual addresses instead of all 64 bits?**
   <details>
   <summary>Click to view answer</summary>
   48 bits provides a massive 256 Terabytes of address space, which is far beyond the physical RAM capacity of modern computers. Implementing all 64 bits would require adding a 5th and 6th level of page tables, dramatically increasing MMU transistor count and adding latency to every memory access.
   </details>

2. **If a page table entry has `Present = 1` and `Writable = 1`, but its parent Page Directory entry has `Writable = 0`, can software write to that page?**
   <details>
   <summary>Click to view answer</summary>
   <strong>No!</strong> The CPU MMU enforces the most restrictive permissions found across all 4 levels of the tree. Because the parent Page Directory entry is read-only, any write attempt will trigger a Page Fault (#PF) with error code bit 0 (Present) and bit 1 (Write) set.
   </details>

3. **What happens if an operating system updates a Page Table entry in RAM but fails to call `invlpg`?**
   <details>
   <summary>Click to view answer</summary>
   The CPU core's Translation Lookaside Buffer (TLB) may continue using the old, cached translation. The CPU might access the wrong physical frame, fail to notice new permissions, or read stale data until an unrelated CR3 reload happens to flush the cache.
   </details>

---

*In [Chapter 7: Dynamic Kernel Heap Management](chapter07_heap_allocator.md), we explore how the kernel uses virtual memory pages to build a dynamic heap supporting `Box`, `Vec`, and `String`.*
