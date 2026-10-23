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
- [x] PCI configuration space (I/O ports `0xCF8` `CONFIG_ADDRESS` and `0xCFC` `CONFIG_DATA`)
- [x] PCI bus enumeration (256 buses, 32 devices, 8 functions scanning)
- [x] PCI device discovery (detection of Host Bridge, VGA, ISA, SATA/AHCI, SMBus)
- [x] Vendor/device IDs (decoding Intel `0x8086`, QEMU `0x1234`, Red Hat VirtIO `0x1AF4`)
- [x] BAR discovery (32-bit MMIO, 64-bit MMIO, and Port I/O hardware size probing)
- [x] MMIO (address extraction, prefetchable flag detection)
- [x] I/O ports (base port address and size detection)
- [x] MSI (Message Signaled Interrupts capability linked-list discovery)
- [x] MSI-X (MSI-X capability structure decoding)
- [x] PCI driver abstraction (`PciDevice` with `enable_bus_mastering`, `enable_memory_space`, `enable_io_space`, `find_device`, `find_by_class`, and `lspci` shell command)

### PS/2 Keyboard Driver & Unified Input
- [x] Intel 8042 PS/2 controller driver (Data port `0x60`, Status/Command port `0x64`)
- [x] Scan Code Set 1 translation table (Make codes and Break codes `0x80` release decoding)
- [x] Modifier state machine (Left/Right Shift, CapsLock XOR logic, Ctrl, Alt, Extended `0xE0` keys)
- [x] Non-blocking input polling (`read_char()` with input/output buffer synchronization)
- [x] Dual-input interactive shell integration (unified QEMU graphical display + physical keyboard + COM1 serial console)

### M18 — VirtIO Block
- [x] VirtIO transport (Legacy PCI I/O port registers: device status, features, queue notify)
- [x] VirtIO PCI (PCI bus device discovery: Vendor `0x1AF4`, Device `0x1001` / `0x1042`, BAR0 extraction)
- [x] Virtqueues (Split virtqueue memory allocation aligned per specification)
- [x] Descriptor tables (16-byte chained descriptors with `NEXT` and `WRITE` flags)
- [x] Available ring (driver submission ring buffer with queue notify kicking)
- [x] Used ring (hypervisor completion ring buffer polling and ISR status clearing)
- [x] DMA buffers (direct zero-copy physical buffer memory mapping)
- [x] Block read (`read_sector` reading 512-byte blocks via VirtIO DMA)
- [x] Block write (`write_sector` writing 512-byte blocks via VirtIO DMA)
- [x] Block device abstraction (`BlockDevice` trait, `BlockError`, and global `BLOCK_DEVICES` registry)
- [x] Block cache & shell integration (`disks` CLI command and automated read/write self-tests)

### M19 — Filesystems
- [x] FAT32 read (BPB parsing, FAT cluster chain following, 8.3 directory reading)
- [x] FAT32 write (free cluster allocation, FAT table linking, sector flushing, file size updating)
- [x] ext2 read-only (Superblock parsing `0xEF53`, block group descriptors, inode reading, direct block resolution)
- [x] Filesystem mounting (dynamic block device probing and mounting to `/disk` and `/mnt`)
- [x] VFS integration (`FileSystem` and `Inode` trait implementations with transparent path dispatch)
- [x] Filesystem tests (boot-time FAT32 file reading/writing and automated shell verification)

---

# Phase 7 — Userspace

### M20 — Init Process
- [x] PID 1 (`spawn_init_process` loading ELF executable)
- [x] Userspace initialization (dedicated PML4 address space, user stack)
- [x] Mount filesystems (mount RamFS at `/`)
- [x] Start services (start console and interactive shell)
- [x] Start shell (spawn `nsh` interactive REPL thread)
- [x] Reap orphan processes (`waitpid` zombie reaping)

### M21 — Minimal libc
- [x] `string.h` (`strlen`, `strcpy`, `strncpy`, `strcat`, `strcmp`, `strncmp`, `strchr`, `strstr`, `strdup`, `memset`, `memcpy`, `memmove`, `memcmp`, `memchr`, `strerror`)
- [x] `stdlib.h` (`malloc`, `free`, `calloc`, `realloc`, `exit`, `abort`, `abs`, `atoi`, `atol`, `strtol`, `itoa`)
- [x] `stdio.h` (`printf`, `sprintf`, `snprintf`, `vprintf`, `puts`, `putchar`, `getchar`, `fopen`, `fclose`, `fread`, `fwrite`, `stdin`, `stdout`, `stderr`)
- [x] `unistd.h` (`read`, `write`, `open`, `close`, `lseek`, `exit`, `getpid`, `yield`, `waitpid`, `sleep`, `usleep`, `isatty`)
- [x] `stdint.h` & `stddef.h` (Standard integer types, fixed-width limits, `size_t`, `ssize_t`, `ptrdiff_t`, `NULL`, `offsetof`)
- [x] Syscall wrappers (Fast x86_64 inline assembly stubs `syscall0` through `syscall6` marshaling arguments to hardware registers)
- [x] Process wrappers (`getpid`, `exit`, `waitpid`, `yield`, `sleep`, `usleep`)
- [x] File wrappers (`open`, `close`, `read`, `write`, `lseek`, `fopen`, `fclose`, `fread`, `fwrite`, `fseek`, `ftell`, `fputs`, `fputc`)
- [x] Memory wrappers (Boundary-tag dynamic memory allocator with free-list coalescing powered by `SYS_MMAP`)
- [x] Minimal C runtime (`crt0.s` entry point `_start`, 16-byte stack alignment per System V AMD64 ABI, invoking `main(argc, argv)`, and calling `exit()`)

### M22 — Shell
- [x] Command parser (tokenization, quotes `'...'` vs `"..."`, and argument splitting)
- [x] Environment variables (global `ENV_VARS`, `export`, `env`, `unset`, `$VAR`, `${VAR}`, `$?`, `$$`)
- [x] Built-in commands (`help`, `clear`, `echo`, `pwd`, `cd`, `ls`, `cat`, `touch`, `write`, `rm`, `grep`, `wc`, `head`, `tail`, `env`, `export`, `unset`, `ps`, `jobs`, `kill`, `lspci`, `disks`, `mem`, `uptime`, `reboot`, `exec`)
- [x] Program execution (direct execution of Ring 3 ELF binaries from RamFS `/bin/` or persistent FAT32 `/disk/` with dedicated PML4 address space)
- [x] Pipes (`cmd1 | cmd2 | cmd3` multi-stage synchronous pipelines with stream passing)
- [x] Redirection (standard input `< file`, truncate output `> file`, append output `>> file`)
- [x] Background processes (asynchronous background execution `cmd &` via dedicated worker threads)
- [x] Job control (zero-heap fixed-buffer `Job` table and `jobs` status monitor)
- [x] `ls` (directory listing via VFS Inode traversal)
- [x] `cat` (read file contents via VFS or stream input)
- [x] `echo` (print arguments to console with variable expansion)
- [x] `ps` (display active processes, PIDs, and states from `ProcessTable`)
- [x] `kill` (signal delivery and process termination handling)


---

# Phase 8 — Networking

### M23 — Network Driver
- [x] VirtIO network (PCI discovery Vendor `0x1AF4`, Device `0x1000` / `0x1041`)
- [x] Virtqueues (Split virtqueues: Queue 0 RX and Queue 1 TX)
- [x] RX queues (Pre-populated receive descriptor ring with 32 buffers)
- [x] TX queues (Synchronous packet transmission ring with header chaining)
- [x] Network buffers (Ethernet MTU 1500 + 10-byte `VirtIoNetHdr` DMA buffers)
- [x] DMA (Direct physical frame mapping and memory fences)
- [x] MAC address (Hardware MAC discovery from PCI config space `0x14..0x19`)
- [x] Interrupt handling & status polling (ISR status acknowledgement and queue notify kicks)


### M24 — Network Stack
- [x] Ethernet (Ethernet II 14-byte frame parsing, serialization, padding to 60 bytes, EtherType dispatching)
- [x] ARP (Address Resolution Protocol packet encoding, dynamic BTreeMap ARP table cache, query broadcast, reply handling)
- [x] IPv4 (IPv4 20-byte header, RFC 1071 one's complement Internet checksumming, subnet mask evaluation, default gateway routing)
- [x] ICMP (Internet Control Message Protocol 8-byte header, automated Echo Reply responder, echo serialization)
- [x] `ping` (ICMP ping diagnostic routine measuring round-trip latency via APIC timer ticks)
- [x] UDP (User Datagram Protocol 8-byte datagram framing, IPv4 pseudo-header checksum, port demultiplexing)
- [x] TCP (Transmission Control Protocol 20-byte segment framing, 11-state TCP finite state machine, 3-way handshake)
- [x] Socket abstraction (Berkeley Sockets API `Socket` abstraction, global `SOCKET_TABLE`, POSIX error reporting)
- [x] `socket` (create AF_INET stream or datagram socket descriptor)
- [x] `bind` (bind socket to local port)
- [x] `listen` (transition TCP stream socket to LISTEN state)
- [x] `accept` (accept incoming TCP stream connection)
- [x] `connect` (active TCP 3-way handshake initiation)
- [x] `send` (transmit datagram or stream data through network stack)
- [x] `recv` (receive buffered packet or stream data from socket queue)

---

# Phase 9 — SMP

### M25 — SMP
- [x] ACPI MADT parsing (RSDP -> XSDT -> MADT table traversal, Local APIC MMIO and I/O APIC discovery)
- [x] CPU discovery (enumerating all logical CPU cores, APIC IDs, enabled flags, and BSP identification)
- [x] AP trampoline (16-bit Real Mode -> 32-bit Protected Mode -> 64-bit Long Mode bootstrap trampoline at 0x8000)
- [x] SIPI (INIT-SIPI-SIPI bootstrap protocol dispatched via BSP Local APIC ICR with vector 0x08)
- [x] AP startup (Application Processors awakened in 64-bit Long Mode, CR0/CR4/EFER.NXE configured, Local APIC enabled)
- [x] Per-CPU data (global `CPU_TABLE` tracking core IDs, APIC IDs, roles, and status)
- [x] Per-CPU stacks (dedicated 16 KiB isolated kernel stacks per core)
- [x] Spinlocks (ticket spinlocks and interrupt-safe `Spinlock` synchronization)
- [x] Atomics (atomic status flags, online core counters, and sequence synchronization)
- [x] Inter-processor interrupts (INIT and Startup IPI delivery with ICR idle status polling)
- [x] Multi-core shell diagnostics (`cpus` and `smp` commands reporting online cores in `nsh`)

---

# Phase 10 — Security

### M26 — Kernel Hardening
- [x] NX (EFER.NXE and PTE bit 63 No-Execute enforcement across stack, heap, and MMIO)
- [x] SMEP (Supervisor Mode Execution Prevention via CR4 bit 20 preventing user-space execution in Ring 0)
- [x] SMAP (Supervisor Mode Access Prevention via CR4 bit 21 with `stac`/`clac` guard wrappers)
- [x] W^X (Write XOR Execute memory permission policy separating executable code from writable data)
- [x] Kernel/user isolation (strict canonical address boundary isolation at `0x0000_8000_0000_0000`)
- [x] Guard pages (unmapped virtual boundary pages detecting stack exhaustion via #PF)
- [x] Stack protection (64-bit stack canaries and `__stack_chk_fail` tamper abort handler)
- [x] Secure syscall validation (user pointer boundary verification, integer overflow rejection, safe `copy_from_user`/`copy_to_user`)
- [x] Memory permission enforcement (Ring 0 write-protection `CR0.WP` and page-level permission checks)
- [x] Shell security diagnostics (`sec` / `hardening` command reporting real-time hardware mitigations)

### M27 — IPC
- [x] Pipes (anonymous unidirectional ring-buffered byte streams, `SYS_PIPE` [22], EOF & `EPIPE` signaling)
- [x] Shared memory (zero-copy physical memory frame sharing, `shmget`/`shmat`, process page table mapping)
- [x] Message queues (fixed-size message buffering and asynchronous message passing)
- [x] Signals (POSIX standard signals: SIGHUP, SIGINT, SIGKILL, SIGSEGV, SIGTERM, SIGCHLD, `SYS_KILL` [62])
- [x] Unix-style sockets (bidirectional stream IPC via `AF_UNIX` / `socketpair()` cross-connected ring buffers)
- [x] Process synchronization (atomic wait queues and cross-thread/process cooperative wakeups)
- [x] IPC permissions (POSIX owner/group read/write access control bitmasks `0o660`)
- [x] Shell IPC diagnostics (`ipc` command displaying real-time IPC stats, pipes, and shared memory segments)

---

# Phase 11 — Threads & Runtime

### M28 — User Threads
- [x] User thread creation (`SYS_CLONE` / `SYS_THREAD_CREATE` [56] with dedicated stack & entry point)
- [x] User thread termination (`SYS_THREAD_EXIT` [57], scheduler cleanup, thread joining)
- [x] Thread scheduling (preemptive round-robin time slicing, APIC timer interrupts, sleep/wake queues)
- [x] Thread-local storage (`fs_base` register backing thread-local storage)
- [x] TLS ABI (`SYS_ARCH_PRCTL` [158] `ARCH_SET_FS` / `ARCH_GET_FS` with `IA32_FS_BASE` MSR `0xC000_0100`)
- [x] Mutex (fast userspace mutex with atomic CAS and slow-path futex sleep, kernel `Mutex<T>`)
- [x] Semaphore (counting semaphore `sem_t` with wait queues, kernel `Semaphore`)
- [x] Condition variable (`pthread_cond_t` sequence counter & atomic lock release, kernel `Condvar`)
- [x] Futex-like primitive (`SYS_FUTEX` [202] supporting `FUTEX_WAIT`, `FUTEX_WAKE`, `FUTEX_REQUEUE`)
- [x] pthread-like API (`pthread_create`, `pthread_join`, `pthread_exit`, `pthread_self`, `sem_init`, `sem_wait`, `sem_post`)

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
- [x] M15
- [x] M16
- [x] M17
- [x] M18
- [x] M19
- [x] M20
- [x] M21
- [x] M22

## Checkpoint 5 — Networked OS
- [x] M23
- [x] M24

## Checkpoint 6 — SMP & Secure OS
- [x] M25
- [x] M26
- [x] M27
- [x] M28

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
