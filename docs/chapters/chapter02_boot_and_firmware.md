# Chapter 2: Boot & Firmware Protocols
### *From Motherboard Reset to the Kernel Handoff*

When you press the power button on your computer, how does code stored on a flash drive or SSD actually start executing?

In this chapter, we trace the boot journey step-by-step: why modern computers abandoned the legacy 1980s PC BIOS in favor of **UEFI**, how partition tables and filesystems work at boot, how the **ELF64** executable format is parsed, and how our bootloader safely hands control to the NullOS kernel.

---

## 1. The Startup Sequence: The Electrical Reality

The instant power reaches the motherboard:
1. **Power Good Signal**: The Power Supply Unit (PSU) checks that its $+12\text{V}$, $+5\text{V}$, and $+3.3\text{V}$ electrical rails are stable. Once stable, it asserts the `Power_Good` pin to the motherboard chipset.
2. **CPU Reset Pin Released**: The chipset releases the CPU's hardware `RESET#` electrical line.
3. **The Reset Vector**: The CPU initializes its internal registers to known default values. 
   - Instruction Pointer (`%rip`): Initialized to `0x0000_FFF0`.
   - Code Segment (`%cs`): Initialized with base address `0xFFFF_0000`.
   - The physical address fetched is: $\text{Base} + \text{Offset} = \text{0xFFFF\_0000} + \text{0xFFF0} = \mathbf{\text{0xFFFF\_FFF0}}$.

This physical address sits just 16 bytes below the 4 GiB mark. The motherboard chipset maps this specific memory region directly to a non-volatile flash memory chip soldered onto the motherboard: the **Firmware ROM**.

The CPU executes a jump instruction in this ROM, beginning firmware execution.

---

## 2. Legacy BIOS vs. Modern UEFI

For nearly 30 years (from the 1981 IBM PC until around 2011), computers used **BIOS** (Basic Input/Output System). Modern PCs use **UEFI** (Unified Extensible Firmware Interface).

```
+-----------------------------------+-----------------------------------+
|            Legacy BIOS            |            Modern UEFI            |
+-----------------------------------+-----------------------------------+
| CPU starts in 16-bit Real Mode    | CPU initializes into native       |
| (1 MB memory limit, no paging).   | 64-bit Long Mode.                 |
+-----------------------------------+-----------------------------------+
| Boots from MBR (Master Boot Rec): | Boots from GPT (GUID Partition    |
| Only 512 bytes in Sector 0!       | Table) supporting disks > 2 TB.   |
+-----------------------------------+-----------------------------------+
| Uses slow, clunky BIOS interrupts | Clean C/Rust function API tables  |
| (`int 0x10`, `int 0x13`).         | (`BootServices`, `RuntimeServices`)|
+-----------------------------------+-----------------------------------+
| Low-resolution VGA text (80x25).  | High-resolution native GOP screen |
|                                   | (1080p, 1440p, 4K graphics).      |
+-----------------------------------+-----------------------------------+
```

### Why NullOS Uses UEFI:
In BIOS, an operating system had to spend hundreds of lines of fragile assembly switching the CPU from 16-bit Real Mode into 32-bit Protected Mode, then setting up page tables and switching into 64-bit Long Mode.

With UEFI, the firmware **already has the CPU running in 64-bit Long Mode with paging enabled**! Furthermore, UEFI understands standard filesystems (FAT32), meaning our bootloader can simply read files by name rather than reading raw disk sectors.

---

## 3. Storage Architecture: GPT & The EFI System Partition

How does UEFI know where our operating system is on a USB drive or SSD?

```
+=============================================================================+
|                          PHYSICAL STORAGE DRIVE                             |
|                                                                             |
|  [LBA 0]: Protective MBR (prevents older tools from wiping GPT)             |
|  [LBA 1]: Primary GPT Header (Partition Table Header)                       |
|  [LBA 2-33]: Partition Entries (GUIDs, Start LBA, End LBA)                  |
|                                                                             |
|  +-----------------------------------------------------------------------+  |
|  | Partition 1: EFI System Partition (ESP) - Type GUID: C12A7328-...      |  |
|  | Filesystem: FAT32                                                     |  |
|  |                                                                       |  |
|  | Files:                                                                |  |
|  |   └── EFI/                                                            |  |
|  |       └── BOOT/                                                       |  |
|  |           └── BOOTX64.EFI   <-- Our compiled UEFI Bootloader!         |  |
|  |   └── kernel.elf             <-- Our 64-bit Rust Kernel!               |  |
|  |   └── startup.nsh            <-- Startup script                       |  |
|  +-----------------------------------------------------------------------+  |
+=============================================================================+
```

### The Standard Boot Path:
When the computer turns on:
1. UEFI scans connected disks looking for an **EFI System Partition (ESP)**.
2. On x86_64 machines, UEFI firmware specifically looks for the default fallback executable:
   ```text
   \EFI\BOOT\BOOTX64.EFI
   ```
3. UEFI loads this file into RAM and invokes its entry point.

---

## 4. Inside the NullOS Bootloader (`bootloader/src/main.rs`)

Our bootloader is a compiled UEFI application written in Rust using the `uefi` crate. When UEFI executes our bootloader, it performs six critical steps:

### Step 1: Initialize the Screen via GOP (Graphics Output Protocol)
In legacy systems, text was drawn in an ancient 80-column text buffer at address `0xB8000`. 
UEFI replaces this with **GOP (Graphics Output Protocol)**:
- We ask UEFI for the active display mode.
- UEFI gives us a **Framebuffer** struct:
  - `address`: The physical 64-bit address in RAM/VRAM where screen pixels live (e.g. `0x8000_0000`).
  - `width`: Number of horizontal pixels (e.g. 1280).
  - `height`: Number of vertical pixels (e.g. 800).
  - `stride`: Pixels per scanline (often equals width, but may include padding for hardware alignment).
  - `pixel_format`: Usually 32-bit RGB or BGR (4 bytes per pixel: Red, Green, Blue, Alpha/Reserved).

```
Pixel Buffer in Memory:
 Address: 0x80000000  0x80000004  0x80000008  ...
 Pixels:  [ B G R A ] [ B G R A ] [ B G R A ] ...
```
To paint a pixel white at coordinates $(X, Y)$:
$$\text{Pixel Address} = \text{Base} + (Y \times \text{Stride} + X) \times 4$$
Writing `0x00FF_FFFF` to that address immediately illuminates that pixel!

---

### Step 2: ACPI Discovery (Root System Description Pointer)
The **ACPI (Advanced Configuration and Power Interface)** tables tell the OS what physical hardware is installed on the motherboard (how many CPU cores exist, where the interrupt controllers are, and how to control power).
Our bootloader queries UEFI for the **`ACPI 2.0 RSDP`** (Root System Description Pointer) table address and saves it in `BootInfo`.

---

### Step 3: Loading and Parsing `kernel.elf`
The bootloader opens the FAT32 volume, finds `kernel.elf`, loads its raw bytes into memory, and parses it.

#### What is an ELF64 File?
An **ELF (Executable and Linkable Format)** binary is structured like this:

```
+-------------------------------------------------------------+
| ELF64 Header (64 bytes)                                     |
|  - Magic: 0x7F, 'E', 'L', 'F'                               |
|  - Architecture: EM_X86_64 (0x3E)                           |
|  - Type: ET_EXEC (Executable)                               |
|  - Entry Point: Virtual address where code starts (0x110000)|
|  - Program Header Table Offset                              |
+-------------------------------------------------------------+
| Program Header 1 (PT_LOAD - Code / .text)                   |
|  - File Offset: 0x1000                                      |
|  - Virtual Target Address: 0x100000                         |
|  - File Size: 45 KB (p_filesz)                              |
|  - Memory Size: 45 KB (p_memsz)                             |
|  - Flags: Read + Execute (PF_R | PF_X)                      |
+-------------------------------------------------------------+
| Program Header 2 (PT_LOAD - Data & BSS / .data & .bss)      |
|  - File Offset: 0xC000                                      |
|  - Virtual Target Address: 0x10B000                         |
|  - File Size: 10 KB (p_filesz)                              |
|  - Memory Size: 25 KB (p_memsz)  <-- NOTICE! memsz > filesz!|
|  - Flags: Read + Write (PF_R | PF_W)                        |
+-------------------------------------------------------------+
```

#### Why is `p_memsz > p_filesz`? (The `.bss` Section)
In programs, global variables initialized to zero (or uninitialized) live in the **`.bss` section**.
- If your operating system declares an array: `static mut BUFFER: [u8; 1024 * 1024] = [0; 1024 * 1024];`
- It would be ridiculous to waste 1 MB of disk space storing 1 million zeroes in the file!
- Instead, the ELF file simply records: `p_filesz = 0`, but `p_memsz = 1048576`.
- The loader copies the file bytes up to `p_filesz`, and then **zeroes out the remaining memory** up to `p_memsz`!

---

### Step 4: Acquiring the UEFI Physical Memory Map
Before shutting down UEFI, we must know what physical RAM exists.
We call UEFI's `get_memory_map()`.
It returns an array of memory descriptors:
```rust
pub struct MemoryDescriptor {
    pub memory_type: UefiMemoryType, // Conventional, BootServices, Acpi, etc.
    pub phys_start: u64,             // Starting physical address
    pub virt_start: u64,
    pub number_of_pages: u64,        // Number of 4 KiB pages
    pub attribute: u64,
}
```
Any region marked **`ConventionalMemory`** is completely free RAM ready for our Physical Memory Manager to allocate!

---

### Step 5: `ExitBootServices()` (The Point of No Return)
UEFI has been providing helper functions (file access, console printing, memory map querying).
Now we call:
```rust
boot_services.exit_boot_services(image_handle, memory_map_key);
```
**This is the most critical microsecond in the boot process:**
- UEFI disables its firmware timers and interrupt handlers.
- UEFI frees its bootloader memory.
- UEFI firmware functions become illegal to call.
- The CPU is now running purely on raw silicon. If the kernel does not have its own screen driver, interrupt table, and memory manager, the computer is blind and deaf.

---

### Step 6: The Handoff to `kernel_main`
Our bootloader stores all discovered hardware info in a shared data structure:
```rust
pub struct BootInfo {
    pub framebuffer: FramebufferInfo,
    pub memory_map: MemoryMapBuffer,
    pub acpi_rsdp: u64,
    pub kernel_image_start: u64,
    pub kernel_image_size: u64,
    pub cmdline_addr: u64,
    pub cmdline_len: usize,
}
```
In the System V AMD64 calling convention, function arguments are passed in register `%rdi`.
The bootloader places the pointer to `BootInfo` in `%rdi`, and jumps to the kernel's entry address:
```assembly
mov %rdi, boot_info_ptr
jmp kernel_entry_address
```
Control is officially transferred to NullOS!

---

## 🧠 Self-Test & Review Questions

1. **Why does UEFI firmware mandate that the boot partition be formatted specifically as FAT32?**
   <details>
   <summary>Click to view answer</summary>
   FAT32 is an open, universal, and simple filesystem specification. Mandating FAT32 ensures that firmware implementations across every motherboard vendor can reliably read bootloaders without needing complex drivers for NTFS, ext4, or APFS.
   </details>

2. **What would happen if an ELF loader copied `p_filesz` bytes into memory but forgot to zero the difference between `p_memsz` and `p_filesz`?**
   <details>
   <summary>Click to view answer</summary>
   The <code>.bss</code> section would contain leftover garbage data from whatever was previously in that RAM physical frame instead of clean zeroes. This would cause random, subtle bugs or crashes whenever global variables or buffers were accessed.
   </details>

3. **What is the significance of calling `ExitBootServices()`? Can the kernel call UEFI file-loading functions after this point?**
   <details>
   <summary>Click to view answer</summary>
   <code>ExitBootServices()</code> shuts down all UEFI boot services, reclaims firmware memory, and terminates firmware interrupts. After this call, the kernel <strong>cannot</strong> call any UEFI boot functions; it must provide its own native device drivers for disk, keyboard, and display.
   </details>

---

*In [Chapter 3: CPU Segmentation, Protection & Privilege (GDT & TSS)](chapter03_cpu_gdt_and_tss.md), we explore how the kernel configures the GDT, establishes CPU privilege rings, and protects stack security using the TSS.*
