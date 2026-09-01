# NullOS

*A custom, bare-metal 64-bit operating system for the `x86_64` architecture, built using `C++20` , `Rust` and Assembly. initializing essential kernel subsystems from scratch.*

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
- [ ] UEFI entry point
- [ ] UEFI console output
- [ ] FAT32 filesystem access
- [ ] ELF64 parser
- [ ] Parse ELF program headers
- [ ] Load `PT_LOAD` segments
- [ ] Kernel loading
- [ ] Kernel entry address
- [ ] UEFI memory map
- [ ] GOP framebuffer
- [ ] ACPI RSDP discovery
- [ ] Kernel command line
- [ ] Shared `BootInfo` structure
- [ ] `ExitBootServices()`
- [ ] Transfer control to kernel

### M2 — Kernel Entry & Debugging
- [ ] `_start` assembly entry
- [ ] Kernel stack
- [ ] `kernel_main`
- [ ] COM1 serial driver
- [ ] `print!` macro
- [ ] `println!` macro
- [ ] Panic handler
- [ ] Kernel panic output
- [ ] QEMU serial console
- [ ] QEMU GDB server
- [ ] GDB kernel debugging
- [ ] Kernel symbols
- [ ] Debug build
- [ ] Release build

### M3 — Boot Contract
- [ ] Finalize `BootInfo`
- [ ] Memory map abstraction
- [ ] Framebuffer abstraction
- [ ] ACPI abstraction
- [ ] Kernel/bootloader ABI
- [ ] Validate all `BootInfo` fields
- [ ] Kernel boots without UEFI services

---

# Phase 2 — CPU Architecture

### M4 — GDT & TSS
- [ ] GDT
- [ ] Kernel code segment
- [ ] Kernel data segment
- [ ] User code segment
- [ ] User data segment
- [ ] TSS
- [ ] Kernel stack
- [ ] IST stacks
- [ ] Load GDT
- [ ] Load TSS
- [ ] Segment reload

### M5 — IDT & Exceptions
- [ ] IDT
- [ ] Interrupt descriptor structures
- [ ] Assembly interrupt stubs
- [ ] Common interrupt handler
- [ ] Divide-by-zero handler
- [ ] Debug handler
- [ ] Breakpoint handler
- [ ] Invalid opcode handler
- [ ] General protection fault
- [ ] Page fault handler
- [ ] Double fault handler
- [ ] Register dump
- [ ] Exception diagnostics

### M6 — APIC & Timers
- [ ] Disable legacy PIC
- [ ] Local APIC
- [ ] IOAPIC
- [ ] APIC initialization
- [ ] APIC timer
- [ ] Timer calibration
- [ ] Timer interrupt
- [ ] Kernel tick counter
- [ ] Sleep/delay primitive

---

# Phase 3 — Memory Management

### M7 — Physical Memory Manager
- [ ] Parse UEFI memory map
- [ ] Identify usable memory
- [ ] Reserve kernel memory
- [ ] Reserve bootloader memory
- [ ] Reserve framebuffer
- [ ] 4 KiB page abstraction
- [ ] Bitmap allocator
- [ ] `alloc_frame()`
- [ ] `dealloc_frame()`
- [ ] `alloc_frames(n)`
- [ ] Free frame counter
- [ ] Physical memory statistics
- [ ] Memory allocation tests

### M8 — Virtual Memory
- [ ] PML4
- [ ] PDPT
- [ ] Page directory
- [ ] Page tables
- [ ] Page mapping
- [ ] Page unmapping
- [ ] Virtual address abstraction
- [ ] Physical address abstraction
- [ ] Address translation
- [ ] Kernel address space
- [ ] Higher-half kernel
- [ ] Guard pages
- [ ] NX
- [ ] Huge pages

### M9 — Kernel Heap
- [ ] Initial bump allocator
- [ ] Linked-list allocator
- [ ] Kernel heap region
- [ ] `GlobalAlloc`
- [ ] `Box`
- [ ] `Vec`
- [ ] `String`
- [ ] Allocation statistics
- [ ] Slab allocator
- [ ] Kernel object caches

---

# Phase 4 — Kernel Execution

### M10 — Kernel Threads
- [ ] Thread structure
- [ ] Kernel stack per thread
- [ ] Register context
- [ ] Context switch
- [ ] `switch_to` assembly
- [ ] Thread creation
- [ ] Thread termination
- [ ] Thread states
- [ ] Kernel thread entry point

### M11 — Scheduler
- [ ] Run queue
- [ ] Round-robin scheduler
- [ ] Timer-driven preemption
- [ ] `yield()`
- [ ] Sleep
- [ ] Wake
- [ ] Scheduler state
- [ ] Thread priorities
- [ ] Scheduler statistics
- [ ] Context-switch benchmark

---

# Phase 5 — User Mode

### M12 — User Mode
- [ ] Ring 3
- [ ] User code segment
- [ ] User data segment
- [ ] User stack
- [ ] User page tables
- [ ] Kernel/user address separation
- [ ] User ELF loader
- [ ] Process address space
- [ ] User program entry
- [ ] User-mode crash handling

### M13 — Syscalls
- [ ] `SYSCALL` instruction
- [ ] `SYSRET`
- [ ] `STAR` MSR
- [ ] `LSTAR` MSR
- [ ] `FMASK` MSR
- [ ] Syscall entry assembly
- [ ] Syscall dispatcher
- [ ] Syscall ABI
- [ ] `write`
- [ ] `exit`
- [ ] `yield`
- [ ] `getpid`
- [ ] `mmap`

### M14 — Process Management
- [ ] PID allocator
- [ ] Process structure
- [ ] Process table
- [ ] Process creation
- [ ] Process termination
- [ ] `exec`
- [ ] `wait`
- [ ] Parent/child relationships
- [ ] Process states
- [ ] First user process

---

# Phase 6 — Filesystems & Storage

### M15 — Initramfs
- [ ] CPIO parser
- [ ] TAR parser
- [ ] Embedded filesystem
- [ ] File abstraction
- [ ] Directory abstraction
- [ ] Path resolution
- [ ] `open`
- [ ] `read`
- [ ] `write`
- [ ] `close`
- [ ] File descriptors

### M16 — VFS
- [ ] VFS abstraction
- [ ] Inode
- [ ] Dentry
- [ ] File object
- [ ] Filesystem trait
- [ ] Mount system
- [ ] Mount points
- [ ] Path traversal
- [ ] File descriptor table
- [ ] Multiple filesystem support

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
- [ ] M0
- [ ] M1
- [ ] M2
- [ ] M3

## Checkpoint 2 — Real Kernel
- [ ] M4
- [ ] M5
- [ ] M6
- [ ] M7
- [ ] M8
- [ ] M9

## Checkpoint 3 — Multitasking OS
- [ ] M10
- [ ] M11
- [ ] M12
- [ ] M13
- [ ] M14

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
