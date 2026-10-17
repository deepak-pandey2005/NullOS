# Boot and Execution Flow

This document details the exact execution lifecycle of **NullOS**, from machine power-on to the kernel's idle loop.

```
       +---------------------------------------------+
       |             Machine Power-On                |
       +---------------------------------------------+
                              |
                              v
       +---------------------------------------------+
       |          UEFI Firmware (OVMF/EDK2)          |
       |  - Discovers GPT partitioned disk image     |
       |  - Loads \EFI\BOOT\BOOTX64.EFI              |
       +---------------------------------------------+
                              |
                              v
       +---------------------------------------------+
       |        NullOS Bootloader (Phase 1)          |
       |  1. Initialize serial port & GOP display    |
       |  2. Locate ACPI RSDP via UEFI config table  |
       |  3. Read \kernel.elf from FAT32 ESP volume  |
       |  4. Parse ELF64 headers & load PT_LOAD segs |
       |  5. Allocate & populate BootInfo structure  |
       |  6. Call exit_boot_services()               |
       |  7. Jump to loaded_kernel.entry_point       |
       +---------------------------------------------+
                              |
                              v
       +---------------------------------------------+
       |     Kernel Entry: entry.S (_start)          |
       |  - Disable interrupts (cli)                 |
       |  - Setup 16 KiB initial stack (stack_top)   |
       |  - Call kernel_main(boot_info)              |
       +---------------------------------------------+
                              |
                              v
       +---------------------------------------------+
       |         Kernel Initialization               |
       |  1. drivers::console::init(&boot_info.fb)   |
       |  2. arch::x86_64::gdt::init()               |
       |  3. arch::x86_64::idt::init()               |
       |  4. arch::x86_64::apic::init()              |
       |  5. Enable interrupts (sti)                 |
       +---------------------------------------------+
                              |
                              v
       +---------------------------------------------+
       |             Kernel Idle Loop                |
       |  - Periodically tick via APIC Timer ISR     |
       |  - Sleep / halt CPU power saving (hlt)      |
       +---------------------------------------------+
```

---

## Detailed Step-by-Step Flow

### 1. UEFI Firmware Stage
1. Hardware starts up, firmware (OVMF in QEMU) initializes basic hardware, consoles, and PCI buses.
2. Firmware parses GPT partition tables and identifies the **EFI System Partition (ESP)** formatted with FAT32.
3. Firmware executes the default 64-bit boot application located at:
   `/EFI/BOOT/BOOTX64.EFI`

### 2. Bootloader Stage (`bootloader/src/main.rs`)
1. **GOP Framebuffer**: Locates the Graphics Output Protocol and queries native resolutions (e.g. 1280x800, 32bpp).
2. **ACPI RSDP**: Traverses the UEFI Configuration Table entries to find the GUID for ACPI 2.0 or ACPI 1.0 RSDP table.
3. **Filesystem & ELF Loading**:
   - Opens the boot filesystem root via `SimpleFileSystem`.
   - Reads `\kernel.elf` into memory.
   - Validates the ELF magic (`0x7F 'E' 'L' 'F'`), 64-bit architecture, and System V ABI.
   - Iterates through `PT_LOAD` segments, allocates destination pages, and copies segments to their load addresses (starting at physical `0x100000` = 1 MiB).
4. **BootInfo Preparation**:
   - Allocates memory for `BootInfo` and memory map descriptors using UEFI `allocate_pool`.
   - Records framebuffer metadata, ACPI pointer, and kernel command line.
5. **ExitBootServices**:
   - Fetches the final UEFI memory map.
   - Calls `exit_boot_services()`. At this precise instant, UEFI runtime ceases all text console and memory management services.
6. **Handoff**:
   - Loads `%rdi` (SysV AMD64 ABI) and `%rcx` (Win64 ABI) with the pointer to `BootInfo`.
   - Jumps directly to the ELF entry point (`0x10a000`).

### 3. Kernel Entry Stage (`kernel/src/arch/x86_64/entry.S`)
1. Executes `_start`:
   - Enforces `cli` (clears interrupts).
   - Loads `%rsp` with the top of the 16 KiB `.bss` stack (`stack_top`).
   - Calls `kernel_main` passing the `BootInfo` pointer in `%rdi`.

### 4. Kernel Main Stage (`kernel/src/main.rs`)
1. **Console Initialization (`drivers::console::init`)**:
   - Hooks into the raw GOP framebuffer.
   - Instantiates an 8x16 bitmap font engine (`noto-sans-mono-bitmap`).
   - Clears the display to dark slate blue and initializes cursor coordinates.
   - Now, all `println!` calls simultaneously output to COM1 serial and the graphical screen!
2. **GDT & TSS (`arch::x86_64::gdt::init`)**:
   - Loads a standard 64-bit GDT with Null, Kernel Code (0x08), Kernel Data (0x10), User Data (0x18), User Code (0x20), and TSS (0x28).
   - Configures an Interrupt Stack Table (IST1) for Double Fault exception recovery.
   - Performs far return (`lretq`) to reload `%cs` and reloads `%ds`, `%es`, `%ss`, `%fs`, `%gs`.
   - Loads Task Register with `ltr`.
3. **IDT & Exceptions (`arch::x86_64::idt::init`)**:
   - Loads a 256-entry IDT.
   - Installs assembly ISR stubs for vectors 0..31 (Divide-by-zero, Page Fault, GPF, Double Fault on IST1, etc.).
   - Connects assembly stubs to Rust `exception_handler` with full register state printing.
4. **APIC & Timers (`arch::x86_64::apic::init`)**:
   - Masks legacy 8259 PIC.
   - Enables Local APIC and installs spurious vector 255.
   - Configures periodic Local APIC Timer mapped to vector 32.
   - Executes `sti` (Set Interrupt Flag).
5. **Idle Loop**:
   - Enters `loop { hlt }` waiting for hardware interrupts.
