# NullOS

*A custom, bare-metal 64-bit operating system for `x86_64` written from scratch in Rust, Assembly, and C — from UEFI firmware handoff to Ring 3 userspace, SMP, networking, dynamic linking, and automated reliability testing.*

> - 📖 **Documentation Index**: [docs/README.md](docs/README.md)
> - 🗺️ **Completed Phases & Milestones (Phase 0 – Phase 19, M0 – M42)**: [docs/phases/ROADMAP.md](docs/phases/ROADMAP.md)
> - 📦 **Installation & Release Guide**: [docs/INSTALLATION.md](docs/INSTALLATION.md)
> - 🎓 **Learning OS Development from Scratch**: [Absolute Beginner's Guide](docs/BEGINNERS_GUIDE.md) · [OS Course Chapters](docs/chapters/README.md)

---

## Status

All **20 development phases (Phase 0 – Phase 19)**, **43 milestones (`M0` – `M42`)**, and **9 major checkpoints** are complete. The detailed per-milestone checklist lives in [docs/phases/ROADMAP.md](docs/phases/ROADMAP.md).

| Checkpoint | Phases | Milestones | Focus |
| :--- | :--- | :--- | :--- |
| **1 — Bootable Kernel** | Phase 0 – 1 | M0 – M3 | Toolchain, UEFI bootloader (`BOOTX64.EFI`/`BOOTAA64.EFI`), ELF64 loader, `BootInfo` ABI, COM1/Panic |
| **2 — Real Kernel** | Phase 2 – 3 | M4 – M9 | GDT/TSS, IDT/exceptions, APIC & timer, bitmap PMM, 4-level paging, slab & heap allocators |
| **3 — Multitasking OS** | Phase 4 – 5 | M10 – M14 | Kernel threads, preemptive scheduler, Ring 3 isolation, `SYSCALL`/`SYSRET`, processes & `waitpid` |
| **4 — Usable OS** | Phase 6 – 7 | M15 – M22 | USTAR initramfs, VFS, PCI, PS/2 input, VirtIO-blk, FAT32/ext2, `init`, `libc`, `nsh` shell |
| **5 — Networked OS** | Phase 8 | M23 – M24 | VirtIO-net, Ethernet, ARP, IPv4, ICMP (`ping`), UDP, TCP FSM, Berkeley sockets |
| **6 — SMP & Secure OS** | Phase 9 – 11 | M25 – M28 | ACPI MADT SMP bring-up, NX/SMEP/SMAP/W^X/canaries, pipes/shm/signals/Unix sockets, pthreads & futex |
| **7 — Advanced OS** | Phase 11 – 14 | M29 – M34 | `epoll`/`poll`/`timerfd`/`eventfd`, framebuffer WM, USB xHCI/HID, Intel HDA audio, ACPI power, `ld.so`/`dlopen`/`libc.so` |
| **8 — High-Performance OS** | Phase 15 – 17 | M35 – M38 | Page/write-back cache, I/O scheduler, NVMe, multiqueue/RSS net, TSC profiling & benchmarks |
| **9 — Release** | Phase 18 – 19 | M39 – M42 | NullOS SDK & `.npkg`, kernel debugger/lockdep/klog, release image tooling & `reliability` regression suite |

---

## Implemented Architecture & Subsystems

- **Boot & Firmware (`bootloader/`, `bootinfo/`)**: UEFI bootloader parsing `nullos.cfg`, loading `kernel.elf` & `KERNEL.SYM`, acquiring the memory map, GOP framebuffer, and ACPI RSDP before `ExitBootServices()`.
- **CPU, Memory & SMP (`kernel/src/arch/`, `memory/`, `sync/`)**: GDT/TSS with IST stacks, IDT exception handlers, Local APIC + IOAPIC timers, ACPI MADT AP startup via INIT-SIPI-SIPI trampoline, 4 KiB bitmap PMM, 4-level paging with higher-half mapping & guard pages, linked-list + slab heap allocators, and interrupt-safe spinlocks.
- **Execution, Userspace & Runtime (`kernel/src/task/`, `user/`, `sdk/`)**: Preemptive priority/round-robin scheduler, Ring 3 process isolation (`ProcessTable`, `exec`, `waitpid`), fast `SYSCALL`/`SYSRET`, dynamic ELF linking (`ld.so`, `dlopen`/`dlsym`, `libc.so`), POSIX threads (`futex`, mutex, semaphore, condvar, TLS), async/event I/O (`poll`, `epoll`, `timerfd`, `eventfd`, `readv`/`writev`), minimal C library/runtime, NullOS SDK (`sdk/`), and interactive `nsh` shell with pipelines, redirection, environment variables, and job control.
- **Storage & Filesystems (`kernel/src/fs/`, `storage/`, `drivers/`)**: USTAR initramfs, VFS mount table (`/`, `/disk`, `/mnt`), RamFS, read/write FAT32, read-only ext2, VirtIO-blk with DMA bounce buffering, NVMe, page/block write-back caches, journaling, and I/O scheduler.
- **Networking & IPC (`kernel/src/net/`, `ipc/`)**: VirtIO-net, Ethernet II, ARP, IPv4 (with loopback fast-path), ICMP (`ping`), UDP, TCP, socket API, pipes, shared memory, message queues, signals, and `AF_UNIX` sockets.
- **Hardware, Graphics & Power (`kernel/src/drivers/`, `graphics/`)**: PCI bus enumeration, PS/2 keyboard + COM1 dual input, GOP double-buffered framebuffer console & basic window manager, USB xHCI + HID mouse/keyboard, Intel HDA audio playback, and ACPI table/power management (shutdown/reboot/sleep).
- **Security, Observability & Reliability (`kernel/src/security/`, `debug/`, `perf/`, `reliability/`)**: NX, SMEP, SMAP, W^X, stack canaries, validated `copy_from_user`/`copy_to_user`, kernel debugger & symbolizer, structured klog, trace buffers, lockdep deadlock detection, TSC latency profiling, and the M42 unit/FS/net/SMP/stress/fuzz/soak test harness (`reliability` shell command).

---

## Quick Start: Building, Running & Testing

See [docs/INSTALLATION.md](docs/INSTALLATION.md) for host package prerequisites (`parted`, `dosfstools`, `mtools`, QEMU/OVMF) and Rust target setup (`x86_64-unknown-none`, `x86_64-unknown-uefi`, and optional `aarch64-unknown-uefi`).

### Linux / macOS

```bash
./scripts/build.sh             # Build bootloader, userspace/SDK, kernel, initramfs & disk images
./scripts/run.sh               # Launch graphical QEMU session
./scripts/run.sh --headless    # Boot headless QEMU and run automated kernel & shell tests (Tests 1–34)
./scripts/release.sh           # Build optimized release distribution & SHA256SUMS in build/release/
./scripts/release.sh --test    # Build release distribution and boot-test x86_64 & AArch64 UEFI in QEMU
./scripts/ci_regression.sh     # Run full CI regression suite (checks, Tests 1–34, release & USB verification)
./scripts/clean.sh             # Clean build artifacts (--all to also clean Cargo target/)
```

### Windows (PowerShell or Command Prompt)
*Prerequisites: Rust Nightly (`rustup default nightly`), QEMU (`winget install SoftwareFreedomConservancy.QEMU`).*

**PowerShell:**
```powershell
.\scripts\windows\run.ps1             # Automatically builds and starts QEMU
.\scripts\windows\run.ps1 -Headless   # Run automated headless test suite
```

**Command Prompt:**
```cmd
scripts\windows\run.bat               # Automatically builds and starts QEMU
scripts\windows\run.bat --headless    # Run automated headless test suite
```
*(OVMF UEFI firmware is automatically detected in standard QEMU install directories or downloaded automatically on first run.)*

---

## Running on Real Hardware (Bare-Metal USB Boot)

NullOS is a 64-bit UEFI operating system that boots on real physical `x86_64` hardware via a GPT + FAT32 EFI System Partition.

### Creating & Verifying a Bootable USB

Verify release image checksums and GPT/FAT32 layout without modifying any device:
```bash
./scripts/create_bootable_usb.sh --verify --release
```

Flash to a USB drive:
```bash
sudo ./scripts/create_bootable_usb.sh
# Or specify the target device explicitly:
sudo ./scripts/create_bootable_usb.sh --device /dev/sdX
```

### Hardware Boot Requirements
1. **Disable Secure Boot** in BIOS/UEFI settings (custom unsigned OS binary).
2. **Enable UEFI Mode** (do not use Legacy BIOS / CSM).
3. **Select Boot Device**: Open your firmware boot menu (commonly `F12`, `F11`, `F9`, or `F8`) and choose `UEFI: <USB Drive>`.
4. The firmware loads `BOOTX64.EFI`, parses `nullos.cfg`, initializes the GOP display, and boots `kernel.elf` into the Ring 3 shell environment.

---

## Repository Structure

```text
NullOS/
├── bootinfo/       # Shared no_std kernel/bootloader BootInfo ABI crate
├── bootloader/     # UEFI bootloader (x86_64 & AArch64 targets, FAT32/ELF/GOP/ACPI)
├── kernel/         # NullOS kernel (arch, memory, task, fs, storage, net, drivers, graphics, ipc, security, perf, debug, reliability)
├── user/           # Userspace minimal C runtime/libc & ELF binaries (hello, cat, threads, async_test, dyn_test)
├── sdk/            # NullOS user SDK, headers, linker scripts, templates & .npkg demo
├── linker/         # Kernel linker scripts
├── scripts/        # Build, QEMU runner, release, bare-metal USB flasher & CI regression scripts
└── docs/           # Architecture docs, subsystem guides, masterclass chapters, installation & ROADMAP.md
```

---

## License

Licensed under the [MIT License](LICENSE).
