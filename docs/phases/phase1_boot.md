# Phase 1 — Bootloader & Firmware Protocol (Milestones M1 - M3)

This phase establishes the UEFI bootloader, Graphics Output Protocol (GOP) initialization, ACPI discovery, 64-bit ELF kernel loading, memory map acquisition, and safe handoff to the kernel.

---

## Completed Milestones

### M1 — UEFI Bootloader & GOP Framebuffer
- [x] **UEFI Target Entry**: Built using `x86_64-unknown-uefi` target with standard UEFI System Table entry point.
- [x] **GOP Framebuffer**: Located Graphics Output Protocol via Boot Services, queried native display modes, and configured high-resolution graphical framebuffer (1280x800).
- [x] **ACPI Discovery**: Located ACPI Root System Description Pointer (RSDP) in EFI Configuration Tables (`ACPI_20_TABLE_GUID` / `ACPI_10_TABLE_GUID`).

### M2 — 64-Bit ELF Kernel Loading
- [x] **EFI Simple File System Protocol**: Opened volume containing EFI System Partition (ESP).
- [x] **Kernel ELF Loading**: Read `\kernel.elf` into temporary boot memory buffer.
- [x] **ELF Header & Program Header Parsing**: Verified ELF magic bytes (`0x7F 'E' 'L' 'F'`), 64-bit architecture, and loaded all `PT_LOAD` segments to physical entry address `0x100000`.
- [x] **BSS Zeroing**: Zero-filled memory ranges where segment `memsz > filesz`.

### M3 — Handoff & Boot Services Exit
- [x] **Memory Map Acquisition**: Retrieved UEFI memory map buffer with descriptor sizes and versioning.
- [x] **ExitBootServices()**: Irreversibly transitioned from UEFI firmware execution to bare-metal control.
- [x] **BootInfo Contract**: Passed validated `BootInfo` structure containing GOP framebuffer, memory map, ACPI RSDP, and command line to `kernel_main`.

