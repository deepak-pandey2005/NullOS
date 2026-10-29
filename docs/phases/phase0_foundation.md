# Phase 0 — Foundation (Milestone M0)

This phase establishes the foundational build systems, toolchains, linker scripts, and virtual machine test runners for NullOS.

---

## Completed Milestones

### M0 — Toolchain & Infrastructure
- [x] **Rust Nightly Configuration**: Pinned toolchain with `#![no_std]` and `#![no_main]` support for freestanding OS development.
- [x] **Target Specifications**: Configured `x86_64-unknown-none` (Kernel) and `x86_64-unknown-uefi` (Bootloader).
- [x] **Linker Scripts**: Designed `kernel/linker.ld` ensuring high/low memory placement (`0x100000` physical entry) and page alignment (`4 KiB`).
- [x] **GPT & FAT32 Disk Pipeline**: Engineered `scripts/build.sh` using `mtools`, `parted`, and `dosfstools` to assemble bootable GPT images with EFI System Partitions (ESP).
- [x] **QEMU Virtual Runner**: Developed `scripts/run.sh` supporting OVMF UEFI firmware, graphical GOP display, and COM1 headless automated test verification (`--headless`).

