# System Memory Layout

This document describes the physical and virtual address space partitioning in **NullOS**.

---

## Physical Address Space Layout

When UEFI hands control to the NullOS kernel after `ExitBootServices()`, physical memory is mapped 1:1 (identity-mapped) by the UEFI firmware environment.

```
+------------------------------------+ 0x0000_0000_0000_0000
| Lower 1 MiB (BIOS/IVT/BDA/Legacy)  |  (Reserved / Unusable)
+------------------------------------+ 0x0000_0000_0010_0000 (1 MiB)
| Kernel ELF (.text, .rodata, .data) |  Physical load address (PT_LOAD)
| Kernel Stack (16 KiB in BSS)       |  
+------------------------------------+ 0x0000_0000_0010_A000 (Kernel Entry)
| Kernel Image End                   |
+------------------------------------+
| BootInfo & Cmdline Buffer          |  Allocated via UEFI LOADER_DATA pool
| MemoryMap Buffer (32 KiB pool)     |
+------------------------------------+
| Conventional Memory (RAM)          |  Usable Physical Memory (Page Frame Allocator)
| ...                                |
+------------------------------------+
| GOP Framebuffer (MMIO)             |  Linear display video RAM (e.g., 0xFD00_0000)
+------------------------------------+
| Local APIC MMIO Registers          |  Fixed at 0x0000_0000_FEE0_0000
+------------------------------------+
| High System Memory / ACPI NVS      |
+------------------------------------+
```

---

## Memory Map Descriptor Layout

The UEFI Memory Map buffer passed to the kernel at `boot_info.memory_map.address` contains contiguous 48-byte UEFI descriptors formatted as:

```rust
#[repr(C)]
pub struct MemoryDescriptor {
    pub ty: u32,             // UEFI Memory Type (e.g., 7 = ConventionalMemory)
    pub phys_start: u64,     // Starting physical address
    pub virt_start: u64,     // Starting virtual address
    pub page_count: u64,     // Number of 4 KiB pages
    pub attribute: u64,      // Memory attribute flags (UC, WC, WT, WB, etc.)
}
```

### Key UEFI Memory Types

| Type Identifier | Name | PMM Treatment |
|---|---|---|
| `1` | `LOADER_CODE` | Reserved (Bootloader code) |
| `2` | `LOADER_DATA` | Reserved (Bootloader data / BootInfo) |
| `3` | `BOOT_SERVICES_CODE` | Reclaimable / Usable after ExitBootServices |
| `4` | `BOOT_SERVICES_DATA` | Reclaimable / Usable after ExitBootServices |
| `7` | `CONVENTIONAL_MEMORY` | **Usable Free RAM** (Added to physical frame allocator) |
| `9` | `ACPI_RECLAIM_MEMORY` | Reserved (ACPI tables) |
| `11` | `MEMORY_MAPPED_IO` | MMIO (Hardware registers / Framebuffer) |

---

## Framebuffer Mapping

- The Graphics Output Protocol (GOP) linear framebuffer resides in PCI MMIO physical space (e.g. `0xFD000000`).
- The kernel graphical text console driver (`drivers::console`) writes directly to this physical memory address using a 32-bit pixel offset formula:
  $$\text{Offset} = (y \times \text{stride} + x) \times 4$$
