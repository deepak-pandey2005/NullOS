# NullOS

*A custom, bare-metal 64-bit operating system for the `x86_64` architecture, built using `C++20` , `Rust` and Assembly. initializing essential kernel subsystems from scratch.*

> 📖 **Architecture & Subsystems Documentation**: See the [**Docs Directory**](docs/README.md).
>
> 🎓 **Learning Operating Systems from Scratch?** Read the [**Absolute Beginner's Guide to Operating Systems & NullOS**](docs/BEGINNERS_GUIDE.md) — an in-depth, first-principles textbook explaining the GDT, TSS, ISRs (hardware vs. software interrupts), 4-level paging, and Ring 3 user mode!

# NullOS — Milestone Roadmap

## Phase 0 — Foundation

### M0 — Repository & Toolchain
- [x] Repository structure
- [x] Rust workspace
- [x] `x86_64-unknown-none` target
- [x] `x86_64-unknown-uefi` target
- [x] Cargo configuration
- [x] Linker scripts
- [x] QEMU
- [x] GDB
- [x] Formatting and linting
- [x] CI build
---

# Phase 1 — Boot

### M1 — UEFI Bootloader
- [x] UEFI entry point
- [x] UEFI console output
- [x] FAT32 filesystem access
- [x] ELF64 parser
- [x] Parse ELF program headers
- [x] Load `PT_LOAD` segments
- [x] Kernel loading
- [x] Kernel entry address
- [x] UEFI memory map
- [x] GOP framebuffer
- [x] ACPI RSDP discovery
- [x] Kernel command line
- [x] Shared `BootInfo` structure
- [x] `ExitBootServices()`
- [x] Transfer control to kernel

### M2 — Kernel Entry & Debugging
- [x] `_start` assembly entry
- [x] Kernel stack
- [x] `kernel_main`
- [x] COM1 serial driver
- [x] `print!` macro
- [x] `println!` macro
- [x] Panic handler
- [x] Kernel panic output
- [x] QEMU serial console
- [x] QEMU GDB server
- [x] GDB kernel debugging
- [x] Kernel symbols
- [x] Debug build
- [x] Release build

### M3 — Boot Contract
- [x] Finalize `BootInfo`
- [x] Memory map abstraction
- [x] Framebuffer abstraction
- [x] ACPI abstraction
- [x] Kernel/bootloader ABI
- [x] Validate all `BootInfo` fields
- [x] Kernel boots without UEFI services

---

# Phase 2 — CPU Architecture

### M4 — GDT & TSS
- [x] GDT
- [x] Kernel code segment
- [x] Kernel data segment
- [x] User code segment
- [x] User data segment
- [x] TSS
- [x] Kernel stack
- [x] IST stacks
- [x] Load GDT
- [x] Load TSS
- [x] Segment reload

### M5 — IDT & Exceptions
- [x] IDT
- [x] Interrupt descriptor structures
- [x] Assembly interrupt stubs
- [x] Common interrupt handler
- [x] Divide-by-zero handler
- [x] Debug handler
- [x] Breakpoint handler
- [x] Invalid opcode handler
- [x] General protection fault
- [x] Page fault handler
- [x] Double fault handler
- [x] Register dump
- [x] Exception diagnostics

### M6 — APIC & Timers
- [x] Disable legacy PIC
- [x] Local APIC
- [x] IOAPIC
- [x] APIC initialization
- [x] APIC timer
- [x] Timer calibration
- [x] Timer interrupt
- [x] Kernel tick counter
- [x] Sleep/delay primitive

---

# Phase 3 — Memory Management

### M7 — Physical Memory Manager
- [x] Parse UEFI memory map
- [x] Identify usable memory
- [x] Reserve kernel memory
- [x] Reserve bootloader memory
- [x] Reserve framebuffer
- [x] 4 KiB page abstraction
- [x] Bitmap allocator
- [x] `alloc_frame()`
- [x] `dealloc_frame()`
- [x] `alloc_frames(n)`
- [x] Free frame counter
- [x] Physical memory statistics
- [x] Memory allocation tests

### M8 — Virtual Memory
- [x] PML4
- [x] PDPT
- [x] Page directory
- [x] Page tables
- [x] Page mapping
- [x] Page unmapping
- [x] Virtual address abstraction
- [x] Physical address abstraction
- [x] Address translation
- [x] Kernel address space
- [x] Higher-half kernel
- [x] Guard pages
- [x] NX
- [x] Huge pages

### M9 — Kernel Heap
- [x] Initial bump allocator
- [x] Linked-list allocator
- [x] Kernel heap region
- [x] `GlobalAlloc`
- [x] `Box`
- [x] `Vec`
- [x] `String`
- [x] Allocation statistics
- [x] Slab allocator
- [x] Kernel object caches

---

# Phase 4 — Kernel Execution

### M10 — Kernel Threads
- [x] Thread structure
- [x] Kernel stack per thread
- [x] Register context
- [x] Context switch
- [x] `switch_to` assembly
- [x] Thread creation
- [x] Thread termination
- [x] Thread states
- [x] Kernel thread entry point

### M11 — Scheduler
- [x] Run queue
- [x] Round-robin scheduler
- [x] Timer-driven preemption
- [x] `yield()`
- [x] Sleep
- [x] Wake
- [x] Scheduler state
- [x] Thread priorities
- [x] Scheduler statistics
- [x] Context-switch benchmark

---

# Phase 5 — User Mode

### M12 — User Mode
- [x] Ring 3
- [x] User code segment
- [x] User data segment
- [x] User stack
- [x] User page tables
- [x] Kernel/user address separation
- [x] User ELF loader
- [x] Process address space
- [x] User program entry
- [x] User-mode crash handling

### M13 — Syscalls
- [x] `SYSCALL` instruction
- [x] `SYSRET`
- [x] `STAR` MSR
- [x] `LSTAR` MSR
- [x] `FMASK` MSR
- [x] Syscall entry assembly
- [x] Syscall dispatcher
- [x] Syscall ABI
- [x] `write`
- [x] `exit`
- [x] `yield`
- [x] `getpid`
- [x] `mmap`

### M14 — Process Management
- [x] PID allocator
- [x] Process structure
- [x] Process table
- [x] Process creation
- [x] Process termination
- [x] `exec`
- [x] `wait`
- [x] Parent/child relationships
- [x] Process states
- [x] First user process

---

# Phase 6 — Filesystems & Storage

### M15 — Initramfs
- [x] TAR (USTAR) archive parser (512-byte blocks, octal parsing, zero-copy slice extraction)
- [x] Embedded RamFS filesystem (`/dev`, `/etc`, `/tmp`, `/bin`, `/hello.txt`)
- [x] File abstraction (`RamFile` backed by thread-safe `Vec<u8>`)
- [x] Directory abstraction (`RamDirectory` with child node lookup and directory nesting)
- [x] Path resolution (absolute path canonicalization, prefix matching, relative traversal)
- [x] `open` syscall (`SYS_OPEN = 2` with `OpenFlags` bitflags parsing)
- [x] `read` syscall (`SYS_READ = 0` reading from file descriptors)
- [x] `write` syscall (`SYS_WRITE = 1` writing to file descriptors & `/dev/console` stdout/stderr)
- [x] `close` syscall (`SYS_CLOSE = 3` releasing file descriptors)
- [x] File descriptors (`FileDescriptorTable` with stdin/stdout/stderr pre-populated)

### M16 — VFS
- [x] VFS abstraction (global singleton `VFS` mount table and path resolver)
- [x] Inode (`Inode` trait with `stat`, `read_at`, `write_at`, `lookup`, `read_dir`, `create`)
- [x] Dentry (`DirEntry` structure with inode number and `FileType`)
- [x] File object (`FileDescriptor` with seek offset cursor, POSIX `lseek` whence support)
- [x] Filesystem trait (`FileSystem` trait with `name()` and `root()`)
- [x] Mount system (dynamic prefix-matching mount table with longest-prefix routing)
- [x] Mount points (`/` root mountpoint populated with RamFS)
- [x] Path traversal (`resolve_path` with directory validation and segment iteration)
- [x] File descriptor table (`FileDescriptorTable` integrated into `Process` PCB)
- [x] Multiple filesystem support (trait-based architecture ready for FAT32 / ext2)

### M17 — PCI
- [ ] PCI configuration space
- [ ] PCI bus enumeration
- [ ] PCI device discovery
- [ ] Vendor/device IDs
- [ ] BAR discovery
- [ ] MMIO
- [ ] I/O ports
- [ ] MSI
- [ ] MSI-X
- [ ] PCI driver abstraction

### M18 — VirtIO Block
- [ ] VirtIO transport
- [ ] VirtIO PCI
- [ ] Virtqueues
- [ ] Descriptor tables
- [ ] Available ring
- [ ] Used ring
- [ ] DMA buffers
- [ ] Block read
- [ ] Block write
- [ ] Block device abstraction
- [ ] Block cache

### M19 — Filesystems
- [ ] FAT32 read
- [ ] FAT32 write
- [ ] ext2 read-only
- [ ] Filesystem mounting
- [ ] VFS integration
- [ ] Filesystem tests

---

# Phase 7 — Userspace

### M20 — Init Process
- [ ] PID 1
- [ ] Userspace initialization
- [ ] Mount filesystems
- [ ] Start services
- [ ] Start shell
- [ ] Reap orphan processes

### M21 — Minimal libc
- [ ] `string.h`
- [ ] `stdlib.h`
- [ ] `stdio.h`
- [ ] `unistd.h`
- [ ] `stdint.h`
- [ ] Syscall wrappers
- [ ] Process wrappers
- [ ] File wrappers
- [ ] Memory wrappers
- [ ] Minimal C runtime

### M22 — Shell
- [ ] Command parser
- [ ] Environment variables
- [ ] Built-in commands
- [ ] Program execution
- [ ] Pipes
- [ ] Redirection
- [ ] Background processes
- [ ] Job control
- [ ] `ls`
- [ ] `cat`
- [ ] `echo`
- [ ] `ps`
- [ ] `kill`

---

# Phase 8 — Networking

### M23 — Network Driver
- [ ] VirtIO network
- [ ] Virtqueues
- [ ] RX queues
- [ ] TX queues
- [ ] Network buffers
- [ ] DMA
- [ ] MAC address
- [ ] Interrupt handling

### M24 — Network Stack
- [ ] Ethernet
- [ ] ARP
- [ ] IPv4
- [ ] ICMP
- [ ] `ping`
- [ ] UDP
- [ ] TCP
- [ ] Socket abstraction
- [ ] `socket`
- [ ] `bind`
- [ ] `listen`
- [ ] `accept`
- [ ] `connect`
- [ ] `send`
- [ ] `recv`

---

# Phase 9 — SMP

### M25 — SMP
- [ ] ACPI MADT parsing
- [ ] CPU discovery
- [ ] AP trampoline
- [ ] SIPI
- [ ] AP startup
- [ ] Per-CPU data
- [ ] Per-CPU stacks
- [ ] Per-CPU scheduler
- [ ] Spinlocks
- [ ] Atomics
- [ ] SMP scheduler
- [ ] Inter-processor interrupts

---

# Phase 10 — Security

### M26 — Kernel Hardening
- [ ] NX
- [ ] SMEP
- [ ] SMAP
- [ ] W^X
- [ ] Kernel/user isolation
- [ ] Guard pages
- [ ] Stack protection
- [ ] ASLR
- [ ] Secure syscall validation
- [ ] Memory permission enforcement
- [ ] Capability/permission model

### M27 — IPC
- [ ] Pipes
- [ ] Shared memory
- [ ] Message queues
- [ ] Signals
- [ ] Unix-style sockets
- [ ] Process synchronization
- [ ] IPC permissions

---

# Phase 11 — Threads & Runtime

### M28 — User Threads
- [ ] User thread creation
- [ ] User thread termination
- [ ] Thread scheduling
- [ ] Thread-local storage
- [ ] TLS ABI
- [ ] Mutex
- [ ] Semaphore
- [ ] Condition variable
- [ ] Futex-like primitive
- [ ] pthread-like API

### M29 — Async & Event Infrastructure
- [ ] Event queues
- [ ] Async I/O
- [ ] Completion queues
- [ ] Timers
- [ ] Event polling
- [ ] Non-blocking I/O
- [ ] Zero-copy buffers
- [ ] Scatter/gather I/O

---

# Phase 12 — Hardware

### M30 — Framebuffer & Graphics
- [ ] Framebuffer console
- [ ] Text renderer
- [ ] Font rendering
- [ ] Scrolling
- [ ] Double buffering
- [ ] Basic graphics API
- [ ] Input events
- [ ] Window abstraction
- [ ] Basic window manager

### M31 — USB
- [ ] xHCI
- [ ] USB controller
- [ ] USB device enumeration
- [ ] USB descriptors
- [ ] HID
- [ ] Keyboard
- [ ] Mouse
- [ ] Input subsystem

### M32 — Audio
- [ ] HDA controller
- [ ] Codec discovery
- [ ] PCM
- [ ] Audio buffers
- [ ] Basic playback
- [ ] Audio API

---

# Phase 13 — ACPI & Power

### M33 — ACPI
- [ ] ACPI table parser
- [ ] RSDP
- [ ] RSDT/XSDT
- [ ] FADT
- [ ] MADT
- [ ] HPET
- [ ] Power management
- [ ] Shutdown
- [ ] Reboot
- [ ] Sleep
- [ ] CPU power states

---

# Phase 14 — Dynamic Linking

### M34 — Dynamic Loader
- [ ] ELF shared objects
- [ ] ELF relocations
- [ ] Symbol resolution
- [ ] GOT
- [ ] PLT
- [ ] Shared libraries
- [ ] `ld.so`
- [ ] `dlopen`
- [ ] `dlsym`
- [ ] Dynamic libc

---

# Phase 15 — Advanced Storage

### M35 — Storage Subsystem
- [ ] Page cache
- [ ] Write-back cache
- [ ] Block cache
- [ ] I/O scheduler
- [ ] Asynchronous block I/O
- [ ] Journaling
- [ ] NVMe driver
- [ ] Storage benchmarking

---

# Phase 16 — Advanced Networking

### M36 — High-Performance Networking
- [ ] Multiqueue networking
- [ ] RSS
- [ ] Network polling
- [ ] Zero-copy networking
- [ ] Packet batching
- [ ] Network buffer pools
- [ ] TCP optimization
- [ ] Network performance benchmarks

---

# Phase 17 — Performance Engineering

### M37 — Kernel Profiling
- [ ] TSC support
- [ ] Cycle counters
- [ ] CPU performance counters
- [ ] Kernel tracing
- [ ] Tracepoints
- [ ] Scheduler latency measurement
- [ ] Syscall latency measurement
- [ ] Interrupt latency measurement
- [ ] Memory allocation benchmarks
- [ ] Context-switch benchmarks
- [ ] Lock contention measurement
- [ ] CPU utilization metrics

### M38 — Kernel Optimization
- [ ] Scheduler optimization
- [ ] Memory allocator optimization
- [ ] Page-table optimization
- [ ] Syscall optimization
- [ ] Lock optimization
- [ ] Cache-aware data structures
- [ ] NUMA awareness
- [ ] Benchmark suite
- [ ] Regression testing

---

# Phase 18 — Developer Experience

### M39 — Helios SDK
- [ ] Helios headers
- [ ] Helios libc
- [ ] Startup code
- [ ] Linker scripts
- [ ] Compiler configuration
- [ ] SDK build system
- [ ] Application templates
- [ ] Debugging tools
- [ ] Package format

### M40 — Debugging & Observability
- [ ] Kernel debugger
- [ ] Crash dumps
- [ ] Stack unwinding
- [ ] Symbolizer
- [ ] Kernel logs
- [ ] Structured logging
- [ ] Trace buffers
- [ ] Memory diagnostics
- [ ] Deadlock detection
- [ ] Debug shell

---

# Phase 19 — Release

### M41 — Bootable Helios Distribution
- [ ] x86_64 UEFI image
- [ ] AArch64 UEFI image
- [ ] FAT32 ESP
- [ ] Kernel image
- [ ] Initramfs
- [ ] Userspace
- [ ] Boot configuration
- [ ] QEMU testing
- [ ] Real hardware testing
- [ ] Release build
- [ ] Installation documentation

### M42 — Reliability
- [ ] Kernel unit tests
- [ ] Integration tests
- [ ] Filesystem tests
- [ ] Network tests
- [ ] SMP tests
- [ ] Stress tests
- [ ] Memory corruption tests
- [ ] Fuzzing
- [ ] Long-running stability tests
- [ ] CI regression tests

---

# Major Checkpoints

## Checkpoint 1 — Bootable Kernel
- [x] M0
- [x] M1
- [x] M2
- [x] M3

## Checkpoint 2 — Real Kernel
- [x] M4
- [x] M5
- [x] M6
- [x] M7
- [x] M8
- [x] M9

## Checkpoint 3 — Multitasking OS
- [x] M10
- [x] M11
- [x] M12
- [x] M13
- [x] M14

---

# Running on Real Hardware (Bare-Metal USB Boot)

NullOS is a standard 64-bit UEFI operating system that boots natively on real physical hardware (PCs and laptops).

### Creating a Bootable USB:
Plug in your USB flash drive and run:
```bash
sudo ./scripts/create_bootable_usb.sh
```
Or specify the target drive directly:
```bash
sudo ./scripts/create_bootable_usb.sh --device /dev/sdX
```

### Hardware Boot Requirements:
1. **Disable Secure Boot** in your motherboard/laptop BIOS (as NullOS is custom OS code and not signed by Microsoft's CA).
2. **UEFI Mode**: Ensure UEFI boot mode is enabled (do not use CSM / Legacy BIOS mode).
3. **Boot Menu**: Press your PC's boot menu hotkey (F12, F11, F9, or F8 depending on manufacturer) and select `UEFI: <USB Drive>`.
4. The OS will boot through `BOOTX64.EFI`, initialize the GOP framebuffer display, and run kernel diagnostics & Ring 3 user mode directly on the screen.

## Checkpoint 4 — Usable OS
- [ ] M15
- [ ] M16
- [ ] M17
- [ ] M18
- [ ] M19
- [ ] M20
- [ ] M21
- [ ] M22

## Checkpoint 5 — Networked OS
- [ ] M23
- [ ] M24

## Checkpoint 6 — SMP & Secure OS
- [ ] M25
- [ ] M26
- [ ] M27
- [ ] M28

## Checkpoint 7 — Advanced OS
- [ ] M29
- [ ] M30
- [ ] M31
- [ ] M32
- [ ] M33
- [ ] M34

## Checkpoint 8 — High-Performance OS
- [ ] M35
- [ ] M36
- [ ] M37
- [ ] M38

## Checkpoint 9 — Release
- [ ] M39
- [ ] M40
- [ ] M41
- [ ] M42
