# NullOS: The Complete Operating Systems Course
### *A 16-Chapter Masterclass in 64-Bit Operating System Engineering from Scratch*

Welcome to the comprehensive textbook documentation for **NullOS**. This curriculum teaches operating system engineering from absolute first principles: starting from raw motherboard electricity and transistors, all the way to multitasking, 4-level paging, and Ring 3 user mode with fast hardware system calls.

---

## 📖 Chapter Index

### [Chapter 1: The Silicon & Bare-Metal Reality](chapter01_silicon_and_hardware.md)
- What is an instruction? Transistors, voltage levels, and machine code.
- The CPU memory latency hierarchy (Registers $\rightarrow$ L1/L2/L3 Cache $\rightarrow$ RAM $\rightarrow$ Storage).
- x86_64 general-purpose and special registers (`%rip`, `%rflags`).
- How code communicates with physical hardware: Port-Mapped I/O (`in`/`out`) vs Memory-Mapped I/O (MMIO).
- CPU control registers (`CR0.WP`, `CR2`, `CR3`, `CR4.PGE`).
- System V AMD64 Calling Convention and callee-saved registers.

### [Chapter 2: Boot & Firmware Protocols](chapter02_boot_and_firmware.md)
- The motherboard electrical startup and CPU Reset Vector (`0xFFFFFFF0`).
- Legacy BIOS vs. Modern UEFI: Why modern computing abandoned the 16-bit 1980s BIOS.
- Partition tables & storage: GPT headers and the FAT32 EFI System Partition (ESP).
- GOP (Graphics Output Protocol): How writing hex numbers to a framebuffer illuminates pixels on a monitor.
- The ELF64 binary executable format: ELF Headers, Program Headers, `PT_LOAD`, and `.bss` zero-filling.
- `ExitBootServices()`: The irreversible point of no return where the firmware terminates.
- The `BootInfo` contract: Bridging bootloader and kernel.

### [Chapter 3: CPU Segmentation, Protection & Privilege](chapter03_cpu_gdt_and_tss.md)
- Silicon privilege enforcement: The 2-bit CPL register (Ring 0 vs Ring 3).
- Why 64-bit Long Mode strictly requires a GDT even with flat memory.
- Anatomy of an 8-byte GDT Descriptor (Base, Limit, Access byte, DPL, Long Mode bit).
- Segment Selectors: How the CPU uses `0x08`, `0x10`, and `0x23` with RPL bits.
- Loading the GDT via `GDTR` and the `lgdt` instruction.
- The Task State Segment (TSS): Why it requires 16 bytes in 64-bit mode.
- Stack security during privilege switches: How `TSS.rsp0` prevents stack hijacking.
- The Interrupt Stack Table (`IST`): Emergency stacks for Double Fault (`#DF`) recovery.

### [Chapter 4: Interrupts, Exceptions & Hardware Timers](chapter04_interrupts_exceptions_apic.md)
- The event-driven kernel: The bookmark analogy of interrupts.
- Hardware interrupts (asynchronous external pins) vs CPU exceptions (synchronous pipeline faults).
- Categorizing exceptions: Faults, Traps, and Aborts.
- The Interrupt Descriptor Table (IDT): 256 gates, 16-byte gate descriptor layout, and `lidt`.
- Hardware stack frames automatically pushed by the CPU (`SS`, `RSP`, `RFLAGS`, `CS`, `RIP`, Error Code).
- The assembly ISR stub in `interrupts.S`: Register preservation and `iretq`.
- The Local APIC: Disabling the legacy 8259 PIC, APIC MMIO registers (`0xFEE0_0000`), EOI signaling, and periodic timer ticks.

### [Chapter 5: Physical Memory Management](chapter05_physical_memory.md)
- Physical vs. Virtual addresses: Real silicon capacitor banks vs artificial coordinates.
- Why 4 KiB (4,096 bytes) is the universal atomic grain of memory.
- Discovering physical RAM: Parsing the UEFI Memory Map (`ConventionalMemory`).
- The Bitmap Allocator: Why a tiny 16 KiB bitmap manages 512 Megabytes of RAM.
- Reserving critical memory: Lower 1 MiB, Kernel ELF segments, Framebuffer, and PMM bitmap.
- Frame allocation algorithms: `alloc_frame()`, `dealloc_frame()`, and coalescing.

### [Chapter 6: Virtual Memory & 4-Level Paging](chapter06_virtual_memory_paging.md)
- Why Virtual Memory is an architectural miracle: Process isolation, permissions, and guard pages.
- 48-bit canonical virtual address decomposition (PML4, PDPT, PD, PT, Offset).
- Anatomy of a 64-bit Page Table Entry (PTE): `Present`, `Writable`, `User/Supervisor`, `NX`.
- The intermediate table flag inheritance rule (`USER_ACCESSIBLE` propagation).
- The `CR3` register: Switching memory universes in a single assembly instruction.
- The Translation Lookaside Buffer (TLB): Hardware caching and `invlpg` invalidation.
- Recursive Page Table Mapping: Pointing `PML4[511]` to itself as a kernel "cheat code".
- Page Faults (`#PF`, Vector 14): `CR2` faulting address and error code bits.

### [Chapter 7: Dynamic Kernel Heap Management](chapter07_heap_allocator.md)
- Stack memory vs. Heap memory.
- Implementing Rust's `GlobalAlloc` trait in `#![no_std]` to unlock `Box`, `Vec`, and `String`.
- Memory allocation strategies: Bump allocators, linked-list free lists, and slab caches.
- The NullOS Expandable Heap: Allocating initial pages at `0x4444_4444_0000` and expanding on demand via PMM.
- Understanding and combating internal and external memory fragmentation.

### [Chapter 8: Multitasking, Context Switching & Schedulers](chapter08_multitasking_and_scheduler.md)
- Time slicing: How a single CPU core runs multiple tasks concurrently.
- The Thread Control Block (TCB): Thread ID, lifecycle states, and dedicated 16 KiB stack.
- The Context Switch primitive in assembly (`switch.S`): Saving callee-preserved registers and swapping `%rsp`.
- The Thread Bootstrap Trampoline: Pre-populating artificial stack frames so new threads can start.
- Cooperative multitasking (`yield_now()`) vs. Preemptive multitasking (APIC timer preemption).
- Preventing deadlock: The `IrqSpinlock` with local `cli`/`sti` interrupt disabling.

### [Chapter 9: User Mode (Ring 3), System Calls & Crash Resilience](chapter09_usermode_and_syscalls.md)
- The user/kernel boundary: Why `jmp` cannot enter user mode.
- The `iretq` privilege drop: Crafting the fake interrupt stack frame to switch CPL to 3.
- Fast hardware system calls: Why `int 0x80` was replaced with `SYSCALL`/`SYSRET`.
- Hardware MSR configuration: `STAR`, `LSTAR`, `FMASK`, and `EFER.SCE`.
- The assembly `syscall_entry` stub: Dedicated kernel syscall stack and user context preservation.
- The NullOS System Call ABI (`SYS_WRITE`, `SYS_EXIT`, `SYS_YIELD`, `SYS_GETPID`, `SYS_MMAP`).
- Process Address Spaces (`AddressSpace`): Dedicated PML4 per process with shared kernel higher half.
- User-Mode Crash Handling: Intercepting Ring 3 exceptions in `idt.rs` and cleanly terminating rogue threads without crashing the kernel.

### [Chapter 10: Bare-Metal Hardware, UEFI Quirks & Real-World Deployment](chapter10_bare_metal_and_debugging.md)
- The reality gap: Why code boots in QEMU but can freeze on physical PCs.
- Navigating Secure Boot: Why custom OSes are blocked and how to disable it in BIOS.
- Flashing bare-metal bootable USB media using `scripts/create_bootable_usb.sh`.
- Motherboard boot menu keys (Dell, Lenovo, HP, ASUS, Acer, MSI).
- Debugging without a debugger: COM1 serial ports and GOP visual panic screens.
- Architectural comparison: NullOS vs. Linux vs. MIT xv6.

### [Chapter 11: Filesystems & Initramfs — From Raw Bytes to Files](chapter11_filesystems_and_initramfs.md)
- The core problem: Storage hardware sectors vs. human file and folder concepts.
- What is an Initramfs? Overcoming the chicken-and-egg storage driver paradox.
- The POSIX USTAR (TAR) archive format: 512-byte header blocks, octal ASCII fields, and padding.
- Zero-copy streaming archive parsing in `#![no_std]` Rust.
- The in-memory RamFS: Files, directories, and character device nodes (`/dev/console`, `/dev/null`, `/dev/zero`).

### [Chapter 12: The Virtual File System (VFS), Inodes & File Descriptors](chapter12_vfs_inodes_and_file_descriptors.md)
- Why an OS needs a VFS: Polymorphic file interfaces across multiple filesystem drivers.
- Inodes vs. Dentries: Decoupling file metadata and identity from path names.
- The `Inode` and `FileSystem` trait architecture in NullOS.
- Path traversal and mount point prefix routing.
- File Descriptors: Security isolation, independent read/write cursors, and standard streams (`stdin 0`, `stdout 1`, `stderr 2`).
- Standard POSIX file syscalls: `open`, `close`, `read`, `write`, `stat`, `lseek`.

### [Chapter 13: The PCI Bus, Hardware Discovery & Base Address Registers (BARs)](chapter13_pci_bus_and_device_discovery.md)
- How an OS discovers physical motherboard hardware without hardcoded addresses.
- Accessing PCI configuration space via I/O ports `0xCF8` (`CONFIG_ADDRESS`) and `0xCFC` (`CONFIG_DATA`).
- The 32-bit `CONFIG_ADDRESS` format (Bus, Device, Function, Register offset).
- The standardized 64-byte Type 0x00 PCI configuration header.
- Base Address Registers (BARs): Memory-Mapped I/O (MMIO 32/64-bit) vs. Port I/O.
- The BAR sizing algorithm: Probing address spaces by writing all 1s.
- The PCI Command Register: Enabling Bus Mastering (DMA), MMIO, and Port I/O.
- Capabilities linked lists: Discovering MSI, MSI-X, SATA, and PCIe extensions.

### [Chapter 14: Keyboard Drivers & Input Subsystems](chapter14_keyboard_drivers_and_input_subsystems.md)
- The mechanical keyboard illusion: Why keys send Make and Break codes instead of ASCII bytes.
- The Intel 8042 PS/2 controller architecture: Data port `0x60`, Status port `0x64`, and Command port `0x64`.
- IBM PC Scan Code Set 1: Bit 7 release encoding (`make_code | 0x80`) and extended `0xE0` prefixes.
- The modifier state machine: Tracking Shift, CapsLock, Ctrl, and Alt with Boolean XOR logic ($\text{Shift} \oplus \text{CapsLock}$).
- Non-blocking polling vs. Interrupts.
- Unifying physical keyboard input, QEMU graphical display, and COM1 serial lines in the interactive shell.

### [Chapter 15: VirtIO & Paravirtualized Block Storage](chapter15_virtio_and_block_storage.md)
- Full hardware emulation vs. Paravirtualization: Eliminating VM-Exit penalties.
- Discovering VirtIO on the PCI bus (Vendor `0x1AF4`, Device `0x1001` / `0x1042`).
- Legacy PCI I/O control registers (BAR0 port layout, device status, queue notify).
- The Split Virtqueue architecture: Descriptor Tables, Available Rings, and Used Rings in shared RAM.
- The 3-part chained descriptor protocol for 512-byte block read and write DMA transfers.
- The generic `BlockDevice` trait and interactive shell `disks` inspection command.

### [Chapter 16: Persistent Filesystems — FAT32 & ext2](chapter16_persistent_filesystems_fat32_ext2.md)
- Bridging flat 512-byte sectors to hierarchical directory trees.
- FAT32 BIOS Parameter Block (BPB) parsing and geometry discovery.
- File Allocation Table (FAT) cluster chains, allocation, and End-of-Chain markers.
- 32-byte directory entries: 8.3 format, attributes, cluster pointers, and case-insensitive matching.
- Linux ext2 architecture: Superblock at byte 1024 (`0xEF53`), block groups, and direct/indirect block pointers.
- Mounting block devices to VFS mount points (`/disk`, `/mnt`) for transparent file access.



