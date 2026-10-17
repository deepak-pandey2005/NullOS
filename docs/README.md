# NullOS Documentation

Welcome to the internal engineering documentation for **NullOS**, a bare-metal 64-bit operating system for `x86_64` written in Rust and Assembly.

This documentation serves as an architectural blueprint, implementation guide, and progress log to explain how every component works, how control flows through the system, and how future milestones will build on top of existing foundations.

---

## Table of Contents

### 1. Architecture & Execution Flow
- [**Boot and Execution Flow**](architecture/boot_flow.md) — Step-by-step lifecycle from UEFI firmware initialization to kernel idle loop.
- [**System Memory Layout**](architecture/memory_layout.md) — Address spaces, physical memory partitioning, and framebuffer mapping.

### 2. Subsystems Documentation
- [**UEFI Bootloader (`bootloader/`)**](subsystems/bootloader.md) — GOP setup, ACPI discovery, ELF loading, memory map acquisition, and boot handoff.
- [**Shared Boot Info (`bootinfo/`)**](subsystems/bootinfo.md) — The zero-dependency ABI contract bridging bootloader and kernel.
- [**CPU Architecture (`kernel/src/arch/x86_64/`)**](subsystems/cpu_arch.md) — GDT, TSS, IDT, Exception Dispatcher, and Local APIC.
- [**Kernel Drivers (`kernel/src/drivers/`)**](subsystems/drivers.md) — COM1 Serial port and GOP Framebuffer graphical text console.

### 3. Research & Specifications
- [**References & Research Guide**](references.md) — Official Intel/AMD/UEFI manuals, OSDev articles, books, CPU registers, and hardware cheat sheets.

### 4. Phases & Milestones
- [**Phase 0: Foundation (M0)**](phases/phase0_foundation.md) — Toolchain, Cargo configuration, Linker scripts, QEMU runner.
- [**Phase 1: Boot (M1 - M3)**](phases/phase1_boot.md) — Bootloader, ELF loader, GOP, kernel handoff.
- [**Phase 2: CPU Architecture (M4 - M6)**](phases/phase2_cpu.md) — GDT, TSS, IDT, exceptions, APIC & timer.
- [**Phase 3: Memory Management (M7 - M10)** *(Upcoming)*](phases/phase3_memory.md) — Physical frame allocator, paging, heap.

### 5. Development & Scripts
- `./scripts/build.sh`: Compiles bootloader & kernel and builds the GPT disk image with ESP partition.
- `./scripts/run.sh`: Launches QEMU with OVMF firmware, `-serial stdio`, and graphical GOP window. (`--test` for automated headless check).
- `./scripts/clean.sh`: Removes all build artifacts, temporary mount points, and backup files. (`--all` to also clean Cargo target caches).

---

## How to Maintain and Extend These Docs

When implementing new phases or milestones:
1. **Document the design before/during implementation**: Add a new document in `docs/phases/phaseX_...md` or update the relevant `docs/subsystems/` guide.
2. **Include hardware register & structure tables**: Document bitfields, CPU privilege levels, and interrupt vectors.
3. **Keep the Execution Flow updated**: Reflect how new initializers hook into `kernel_main`.
