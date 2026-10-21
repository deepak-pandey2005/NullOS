# UEFI Bootloader Subsystem (`bootloader/`)

The **NullOS Bootloader** (`bootloader/`) is a custom UEFI 64-bit application written in Rust using the `uefi` crate. It compiles into an EFI executable (`BOOTX64.EFI`) placed on the FAT32 EFI System Partition (ESP).

---

## Key Responsibilities

1. **Serial Port Initialization (`serial.rs`)**:
   - Initializes COM1 (`0x3F8`) UART early for early boot debugging output.
2. **GOP Framebuffer Setup (`gop.rs`)**:
   - Queries UEFI `GraphicsOutput` protocol.
   - Sets a high-resolution video mode (1280x800 or best available 32bpp format).
   - Extracts physical frame buffer base address, size, width, height, stride, and pixel format.
3. **ACPI Discovery (`acpi.rs`)**:
   - Traverses the UEFI Configuration Table entries.
   - Discovers ACPI 2.0 (`EFI_ACPI_20_TABLE_GUID`) or ACPI 1.0 (`ACPI_TABLE_GUID`) Root System Description Pointer (RSDP).
4. **FAT32 Kernel Loader (`fs.rs`)**:
   - Opens `SimpleFileSystem` on the boot volume.
   - Locates and reads `\kernel.elf` into memory pool.
5. **ELF64 Loader (`elf.rs`)**:
   - Parses ELF64 headers and program headers (`PT_LOAD`).
   - Verifies 64-bit architecture, SysV ABI, and executable target.
   - Allocates memory at physical load address (`0x100000` / 1 MiB) and copies ELF segments.
6. **Kernel Command Line Setup**:
   - Pre-allocates buffer for default kernel command line (`console=com1 loglevel=info`).
7. **UEFI Memory Map & ExitBootServices**:
   - Pre-allocates buffer for memory descriptors.
   - Calls `uefi::boot::exit_boot_services()`.
   - Copies final memory map descriptors into kernel-accessible memory.
8. **Kernel Handoff (`main.rs`)**:
   - Populates `BootInfo` structure.
   - Loads `%rdi` (SysV ABI) and `%rcx` (Win64 ABI) with `BootInfo` pointer.
   - Jumps directly to kernel entry point.

---

## File Structure

| File | Description |
|---|---|
| `bootloader/src/main.rs` | Main UEFI entry point, execution orchestrator, handoff assembly. |
| `bootloader/src/gop.rs` | Graphics Output Protocol (GOP) initialization and framebuffer metadata collection. |
| `bootloader/src/acpi.rs` | ACPI RSDP table discovery via UEFI system configuration tables. |
| `bootloader/src/fs.rs` | FAT32 boot partition filesystem driver and kernel file reader. |
| `bootloader/src/elf.rs` | ELF64 executable header parser and segment allocator/loader. |
| `bootloader/src/serial.rs` | 16550 UART COM1 serial logger for UEFI boot phase. |

---

## UEFI Boot Handoff Protocol

```
+-------------------+      SysV ABI: %rdi -> BootInfo
| UEFI Bootloader   |  ---------------------------------->  +------------------+
| (BOOTX64.EFI)     |      Win64 ABI: %rcx -> BootInfo      | Kernel Entry     |
+-------------------+  ---------------------------------->  | (entry.S / _start|
                                                            +------------------+
```

When `exit_boot_services()` succeeds, UEFI memory allocation and boot services are terminated. The bootloader transfers execution using raw inline assembly:

```rust
core::arch::asm!(
    "jmp {entry}",
    in("rdi") boot_info_ptr as u64,
    in("rcx") boot_info_ptr as u64,
    entry = in(reg) loaded_kernel.entry_point,
    options(noreturn)
);
```
