# The Absolute Beginner's Guide to Operating Systems & NullOS
### *From Raw Silicon to User Mode: A Complete First-Principles Course*

Welcome! If you have ever wondered how an operating system actually works under the hood—not just theoretically, but down to the electrical registers, assembly instructions, and byte layouts—this guide is written specifically for you.

We will explain every concept starting from zero assumptions: what the hardware is doing, why a data structure exists, how big it is down to the exact byte, and how NullOS implements it.

---

## Table of Contents
1. [The Big Picture: What is an Operating System?](#1-the-big-picture-what-is-an-operating-system)
2. [The Boot Journey: Power Button to Kernel](#2-the-boot-journey-power-button-to-kernel)
3. [CPU Privilege & Protection: GDT and TSS](#3-cpu-privilege--protection-gdt-and-tss)
4. [Interrupts, Exceptions, and ISRs: Hardware vs. Software](#4-interrupts-exceptions-and-isrs-hardware-vs-software)
5. [Memory Management: Physical Frames, Paging, and Heap](#5-memory-management-physical-frames-paging-and-heap)
6. [Multitasking & Preemptive Scheduling: How Threads Work](#6-multitasking--preemptive-scheduling-how-threads-work)
7. [User Mode (Ring 3) & Fast System Calls (SYSCALL / SYSRET)](#7-user-mode-ring-3--fast-system-calls-syscall--sysret)
8. [Summary & Architecture Map](#8-summary--architecture-map)

---

## 1. The Big Picture: What is an Operating System?

When a computer turns on, there are no files, no folders, no windows, no `printf`, and no memory protection. The CPU is simply an electronic processor cycling billions of times per second. It fetches numbers from memory, decodes them as machine instructions (like `add`, `mov`, `jmp`), and executes them.

An **Operating System (OS)** is the very first program that takes control of the bare computer. Its job is to:
1. **Manage Hardware**: Communicate with the screen, keyboard, disks, timers, and memory chips.
2. **Provide Isolation & Security**: Ensure that Program A cannot snoop on, corrupt, or crash Program B or the kernel.
3. **Provide Abstractions**: Give applications friendly tools—such as "write bytes to a console" or "allocate 1 MB of memory"—instead of forcing them to send electrical commands to raw hardware.

NullOS is built from scratch in **Rust** and **x86_64 Assembly** to demonstrate exactly how every single layer of this foundation works.

---

## 2. The Boot Journey: Power Button to Kernel

### The Problem: How Does Code Get Loaded?
When you press the power button, the computer's RAM is completely blank (all transistors are uncharged). The CPU cannot run code from an empty RAM.

### Step 1: Motherboard Firmware (UEFI)
The motherboard contains a small non-volatile flash ROM chip. As soon as power stabilizes, the CPU starts executing code burned into this chip. On modern computers, this firmware is called **UEFI** (Unified Extensible Firmware Interface).
- UEFI initializes the motherboard, detects RAM, tests CPU cores, and scans storage drives (NVMe, SSDs, USB drives).
- It looks for a drive formatted with a **GPT** (GUID Partition Table) containing an **EFI System Partition (ESP)** formatted as FAT32.
- Inside that partition, it searches for a specific file: `/EFI/BOOT/BOOTX64.EFI`.

### Step 2: The NullOS Bootloader (`bootloader/`)
In NullOS, we wrote our own UEFI bootloader (`bootloader/src/main.rs`). When UEFI finds `/EFI/BOOT/BOOTX64.EFI`, it loads it into memory and runs it. Our bootloader performs crucial preparations:
1. **Screen Graphics (GOP)**: Asks UEFI for the Graphics Output Protocol framebuffer. This gives us the screen's width (e.g. 1280), height (800), stride, and the physical memory address of the screen pixels.
2. **ACPI Discovery**: Locates the `RSDP` (Root System Description Pointer) table so the OS knows what CPU cores and power controllers exist.
3. **Loading the Kernel (`kernel.elf`)**: Reads `kernel.elf` from the FAT32 partition. 
   - *What is an ELF?* ELF (Executable and Linkable Format) is the standard binary format on Linux and Unix. It contains **headers** that tell the loader: *"Take 200 KB from file offset 0x1000 and copy it to memory address 0x100000; then start running code at address 0x110000."*
4. **Acquiring the Physical Memory Map**: Asks UEFI for the memory map—a list of every chunk of physical RAM indicating whether it is usable RAM, reserved for hardware, or used by firmware.
5. **ExitBootServices (The Point of No Return)**: Calls `ExitBootServices()`. This permanently shuts down UEFI firmware services. From this exact microsecond forward, the firmware is dead. Our kernel owns the computer.
6. **Jump to Kernel**: The bootloader passes a pointer to a struct called `BootInfo` in register `%rdi` and executes a `jmp` to `0x110000` (the kernel entry point).

---

## 3. CPU Privilege & Protection: GDT and TSS

### The Problem: Why Can't Any Program Access Everything?
If any program could execute any CPU instruction, a web browser or game could overwrite the operating system's memory, read passwords directly from RAM, or shut off the CPU. 

To prevent this, the x86_64 CPU hardware enforces **Privilege Rings**:
- **Ring 0 (Supervisor / Kernel Mode)**: Maximum privilege. Can execute any instruction (like `cli` to stop interrupts, `mov cr3` to switch memory maps, or `in`/`out` for hardware ports).
- **Ring 3 (User Mode)**: Restricted privilege. Cannot touch hardware directly, cannot modify page tables, and cannot change CPU control registers.

```
+---------------------------------------------------+
|               Ring 3: User Space                  |
|          (Applications, Games, Browsers)          |
+-------------------------+-------------------------+
                          |
              [SYSCALL / Fault Interrupt]
                          |
                          v
+---------------------------------------------------+
|               Ring 0: Kernel Space                |
|       (NullOS Kernel, Device Drivers, Sched)      |
+---------------------------------------------------+
```

---

### What is the GDT (Global Descriptor Table)?
The **GDT** is a hardware table in memory that tells the CPU what memory segments exist and what privilege level is required to run in them.

#### Why does it exist if 64-bit mode has "flat memory"?
In older 16-bit and 32-bit CPUs, memory was divided into chunks called "segments". In 64-bit mode (Long Mode), segmentation is disabled for address translation—all memory is flat from address `0x0` to `0xFFFF_FFFF_FFFF_FFFF`. 

**HOWEVER**, x86_64 CPUs **still require the GDT** for three critical reasons:
1. To declare whether code is running in 64-bit mode or 32-bit compatibility mode.
2. To assign the CPU's privilege level (**Ring 0** vs **Ring 3**).
3. To tell the CPU where the **TSS** (Task State Segment) is.

#### What does a GDT Entry look like?
A standard GDT entry (a Segment Descriptor) is exactly **8 bytes (64 bits)** wide:
- **Base Address** (32 bits, ignored in 64-bit mode).
- **Segment Limit** (20 bits, ignored in 64-bit mode).
- **Access Byte**: Contains flags like:
  - `Present`: Must be 1 if the segment is active.
  - `DPL` (Descriptor Privilege Level): `00` for Kernel (Ring 0), `11` for User (Ring 3).
  - `Executable`: `1` for Code Segments, `0` for Data/Stack Segments.
- **Flags**: Contains the `L` bit (Long Mode flag). When `1`, tells the CPU this is native 64-bit code.

In NullOS ([kernel/src/arch/x86_64/gdt.rs](../kernel/src/arch/x86_64/gdt.rs)), our GDT has 6 entries:
1. `Index 0 (Offset 0x00)`: **Null Descriptor** (8 bytes of zeroes, mandatory by x86 architecture).
2. `Index 1 (Offset 0x08)`: **Kernel Code Segment (64-bit, Ring 0)**.
3. `Index 2 (Offset 0x10)`: **Kernel Data Segment (Ring 0)**.
4. `Index 3 (Offset 0x18)`: **User Data Segment (Ring 3)**.
5. `Index 4 (Offset 0x20)`: **User Code Segment (64-bit, Ring 3)**.
6. `Index 5 (Offset 0x28)`: **TSS Descriptor** (In 64-bit mode, the TSS descriptor is special: it takes **16 bytes / two GDT slots** because it must hold a full 64-bit base address).

#### How does the CPU load the GDT?
The CPU has a dedicated internal register called **GDTR** (GDT Register). We load it using the assembly instruction:
```assembly
lgdt [pointer_to_gdt_descriptor]
```
The descriptor contains the total size of the GDT minus 1 (`limit: u16`) and the 64-bit memory address of the table (`base: u64`).

---

### What is the TSS (Task State Segment)?
In 32-bit x86, the TSS was used for hardware multitasking. In 64-bit mode, that feature is obsolete, but the TSS still has **two vital hardware responsibilities**:

1. **`RSP0` (Kernel Privilege Stack Pointer)**:
   Suppose an application is running in User Mode (Ring 3) and an interrupt (like a timer or page fault) occurs. The CPU must switch to Ring 0 to handle it. 
   *Can the CPU push register data onto the user's stack?* **NO!** The user's stack pointer (`%rsp`) might be pointing to invalid memory, or a hacker could craft an exploit. 
   Therefore, the CPU hardware automatically looks at `TSS.rsp0`, loads that address into `%rsp`, and safely pushes the interrupt frame onto the kernel's private stack!

2. **`IST` (Interrupt Stack Table)**:
   The TSS provides 7 special stack pointers called `IST1` through `IST7`. If an extreme disaster happens—like a **Double Fault (`#DF`)** caused by a corrupted kernel stack—the CPU switches to `IST1` so the exception handler has a guaranteed clean, working stack.

---

## 4. Interrupts, Exceptions, and ISRs: Hardware vs. Software

Imagine you are reading a book in your living room. Suddenly, your phone rings. 
- You place a bookmark on the line you were reading.
- You answer the phone and talk for two minutes.
- You hang up, open the book back to your bookmark, and resume reading as if nothing happened.

This is exactly how an **Interrupt** works in a computer.

```
       Normal Program Execution: Instruction A -> Instruction B -> Instruction C
                                                                         |
                                                            [INTERRUPT FIRES!]
                                                                         |
                                                                         v
                                                       CPU pauses program & saves RIP/RSP
                                                                         |
                                                                         v
                                                           Execute Handler (ISR)
                                                                         |
                                                                         v
                                                       iretq: Resume Instruction D!
```

---

### Hardware vs. Software: What is the Difference?

| Type | What Causes It? | Is it Planned? | Example |
| :--- | :--- | :--- | :--- |
| **Hardware Interrupt** | An electronic signal sent by an external physical chip via a wire/bus to the CPU's interrupt controller (APIC). | Asynchronous (can happen between ANY two CPU instructions). | The APIC Timer ticks every millisecond; a keyboard key was pressed; an Ethernet packet arrived. |
| **CPU Exception** | The CPU execution unit itself detects an error while attempting to decode or execute an instruction. | Synchronous (happens precisely on the offending instruction). | Dividing by zero (`#DE`, vector 0); accessing unmapped memory (Page Fault `#PF`, vector 14); invalid instruction (`#UD`, vector 6). |
| **Software Interrupt / Syscall** | Program explicitly runs an instruction requesting the kernel to do something. | Intentional and planned by software. | User app calls `syscall` to print text to the screen. |

---

### What is an ISR (Interrupt Service Routine)?
An **ISR** is simply a function in the kernel dedicated to handling a specific interrupt.
- For vector 0: `divide_by_zero_handler`
- For vector 14: `page_fault_handler`
- For vector 32: `timer_handler`

### The IDT (Interrupt Descriptor Table)
How does the CPU know which function to call when interrupt 14 occurs?
The **IDT**! The IDT is an array of 256 entries in memory (each entry is **16 bytes** in 64-bit mode).
Each entry (Gate Descriptor) specifies:
- The 64-bit memory address of the assembly ISR function.
- The Code Segment selector (`0x08` for Kernel Code).
- The IST stack index (`0` for normal stack, `1` for Double Fault).
- Gate Type (`0xE` for 64-bit Interrupt Gate).
- DPL (Privilege Level: `0` for kernel-only, `3` if user mode is allowed to trigger it via `int`).

We load the IDT using the assembly instruction:
```assembly
lidt [pointer_to_idt_descriptor]
```

### What Happens Step-by-Step When an Interrupt Occurs:
1. **Hardware Action**: The CPU pauses the currently running code.
2. **Stack Switch**: If switching from Ring 3 to Ring 0, the CPU loads `%rsp` from `TSS.rsp0`.
3. **Hardware Frame Push**: The CPU automatically pushes 5 values onto the stack:
   - `SS` (Stack Segment)
   - `RSP` (Stack Pointer before interrupt)
   - `RFLAGS` (CPU status flags)
   - `CS` (Code Segment)
   - `RIP` (Instruction Pointer: where to resume!)
   *(For exceptions with error codes, like `#PF` or `#GP`, the CPU also pushes an `Error Code`).*
4. **Assembly Stub (`interrupts.S`)**: The CPU jumps to our assembly stub. Our assembly code saves all general-purpose registers (`rax`, `rbx`, `rcx`, `rdx`, `rsi`, `rdi`, `rbp`, `r8`..`r15`) so the interrupted program's register contents are not destroyed.
5. **Rust Dispatcher (`idt.rs`)**: Calls our safe Rust `exception_handler` or `timer_handler`.
6. **Return (`iretq`)**: When finished, our assembly pops all saved registers and executes `iretq` (Interrupt Return). The CPU pops `RIP`, `CS`, `RFLAGS`, `RSP`, `SS`, restores the privilege level, and resumes the original program seamlessly!

---

## 5. Memory Management: Physical Frames, Paging, and Heap

A computer's memory subsystem is split into three layers:

```
+-------------------------------------------------------------------+
| 3. Kernel Heap (Box, Vec, String - arbitrary byte-size allocations)|
+-------------------------------------------------------------------+
                                  |
                                  v
+-------------------------------------------------------------------+
| 2. Virtual Memory / 4-Level Paging (PML4 -> PDPT -> PD -> PT)     |
|    Translates virtual addresses to physical; enforces read/write/NX|
+-------------------------------------------------------------------+
                                  |
                                  v
+-------------------------------------------------------------------+
| 1. Physical Memory Manager (PMM)                                  |
|    Allocates raw 4 KiB hardware page frames via Bitmap            |
+-------------------------------------------------------------------+
```

---

### Layer 1: Physical Memory Manager (PMM)
Physical memory is the actual RAM sticks plugged into the motherboard slots.
- The CPU and memory controller do not manage RAM byte-by-byte; they manage it in **4 KiB chunks (4,096 bytes)** called **Page Frames**.
- Address `0x0` is Frame 0; address `0x1000` (4096) is Frame 1; address `0x2000` (8192) is Frame 2, and so on.

#### How does NullOS track which frames are free?
We use a **Bitmap Allocator** ([kernel/src/memory/frame_allocator.rs](../kernel/src/memory/frame_allocator.rs)).
- Every single bit represents one 4 KiB frame.
- Bit = `0` -> Frame is FREE!
- Bit = `1` -> Frame is USED.
- **Why is a Bitmap so powerful?** A single byte has 8 bits, tracking $8 \times 4\text{ KiB} = 32\text{ KiB}$ of RAM. A tiny **16 KiB bitmap** can manage **512 Megabytes** of physical RAM!

---

### Layer 2: Virtual Memory & 4-Level Paging
Without virtual memory, programs would access physical addresses directly:
- If Program A wrote to address `0x10000`, it could overwrite Program B's data.
- If physical memory became fragmented, you couldn't allocate large contiguous buffers.

With **Virtual Memory**, every address an instruction uses is a "fake" (virtual) address. The CPU's hardware **MMU (Memory Management Unit)** automatically translates every virtual address into a physical address on the fly.

#### The 4-Level Page Table Translation
In x86_64, translation uses a 4-level tree:
```
Virtual Address (48 bits used):
 [ PML4 Index: 9 bits ] [ PDPT Index: 9 bits ] [ PD Index: 9 bits ] [ PT Index: 9 bits ] [ Offset: 12 bits ]
           |                      |                     |                    |                   |
           v                      v                     v                    v                   v
      PML4 Table             PDPT Table              PD Table             PT Table         Physical Frame
      (512 entries)          (512 entries)         (512 entries)        (512 entries)       (4096 bytes)
```
- Every page table at every level is exactly **4,096 bytes (4 KiB)** and contains **512 entries** (each entry is 8 bytes / 64 bits).
- $9 + 9 + 9 + 9 = 36$ bits for table indexing, plus 12 bits for offset inside the 4 KiB page = **48-bit canonical virtual address space (256 Terabytes!)**.

#### Page Table Entry Flags (The 64 bits):
- **Bit 0 (`Present`)**: Is this page mapped in memory? If 0, accessing it causes a **Page Fault (`#PF`)**.
- **Bit 1 (`Writable`)**: If 1, code can write to it. If 0, writing triggers `#PF` (read-only protection).
- **Bit 2 (`User/Supervisor`)**: If 1, User Mode (Ring 3) can access it. If 0, only Ring 0 can access it!
- **Bit 63 (`No-Execute / NX`)**: If 1, attempting to execute code from this page triggers a Page Fault! (Prevents buffer-overflow exploits from running shellcode in data/stack pages).

#### The CR3 Register:
Which PML4 table does the CPU use?
The CPU has a register called **`CR3`**. It stores the physical frame address of the active PML4 root table. When switching between processes, the OS simply writes a new address to `CR3` (`mov cr3, rax`), and the entire memory universe instantaneously switches!

---

### Layer 3: The Kernel Heap
The PMM only allocates 4 KiB chunks. But in Rust or C++, what if your code wants a `String` with 15 bytes, or a `Vec` with 8 numbers?
That is what the **Heap Allocator** ([kernel/src/memory/heap.rs](../kernel/src/memory/heap.rs)) does:
- Maps virtual memory pages starting at `0x4444_4444_0000`.
- Manages sub-page memory allocations (implementing Rust's `GlobalAlloc`).
- Allows standard collections: `Box<T>`, `Vec<T>`, `String`, `format!()`.
- **Dynamic Expansion**: If the heap runs out of memory, our allocator automatically calls the PMM, allocates new physical frames, maps new virtual pages, and expands the heap limit on the fly!

---

## 6. Multitasking & Preemptive Scheduling: How Threads Work

How can a single-core CPU run a web browser, music player, and terminal at the same time?
**Time Slicing!** The CPU executes Thread 1 for 10 milliseconds, pauses it, executes Thread 2 for 10 milliseconds, pauses it, and cycles through them so fast that humans perceive them as running simultaneously.

### What is a Thread?
A thread is an execution context ([kernel/src/task/thread.rs](../kernel/src/task/thread.rs)):
1. **Thread ID & Name**: Unique identifier.
2. **State**: `Ready` (waiting for CPU), `Running` (currently on CPU), `Blocked` (sleeping/waiting), `Terminated` (finished).
3. **Dedicated Stack**: A 16 KiB region in memory where this thread's local variables, function calls, and return addresses live.
4. **Saved Stack Pointer (`%rsp`)**: When the thread is paused, this stores where its stack pointer was left.

---

### The Context Switch (`kernel/src/arch/x86_64/switch.S`)
When switching from Thread A to Thread B:
```assembly
switch_threads(prev_rsp: *mut u64, next_rsp: u64):
    # 1. Save Thread A's callee-preserved registers onto Thread A's stack
    pushfq
    push %rbp
    push %rbx
    push %r12
    push %r13
    push %r14
    push %r15

    # 2. Save Thread A's %rsp into its Thread struct
    mov %rsp, (%rdi)

    # 3. SWITCH THE CPU'S STACK POINTER TO THREAD B!
    mov %rsi, %rsp

    # 4. Pop Thread B's saved registers from Thread B's stack
    pop %r15
    pop %r14
    pop %r13
    pop %r12
    pop %rbx
    pop %rbp
    popfq

    # 5. Return into Thread B's code!
    ret
```
*Notice how simple and brilliant this is!* By swapping the `%rsp` register, the CPU is suddenly running on Thread B's stack. When `ret` executes, it pops Thread B's return address and resumes Thread B exactly where it was previously suspended!

---

### Cooperative vs. Preemptive Scheduling
1. **Cooperative Multitasking**: A thread voluntarily gives up the CPU by calling `yield_now()`. (If a thread enters an infinite `loop {}`, the entire system freezes).
2. **Preemptive Multitasking**: The **APIC Hardware Timer** fires an interrupt (Vector 32) every few milliseconds. The timer interrupt handler calls `scheduler::timer_tick()`. If the current thread has used up its time slice, the kernel pauses it and switches to the next thread **against its will**!

---

## 7. User Mode (Ring 3) & Fast System Calls (SYSCALL / SYSRET)

Now we arrive at the crown jewel of operating system architecture: **User Mode Isolation** and **System Calls**.

### 1. Entering User Mode (`iretq`)
Can a kernel just execute `jmp user_code` to enter user mode?
**NO!** A normal `jmp` does not change the CPU's privilege level (CPL). The CPU would remain in Ring 0.

To transition from Ring 0 down to Ring 3, we use the `iretq` instruction.
The kernel manually pushes a fake interrupt stack frame:
```
Top of Stack -> [ RIP    ]  (Address of user entry point)
                [ CS     ]  (User Code Selector: 0x20 | 3 = 0x23)
                [ RFLAGS ]  (0x202: Interrupts Enabled)
                [ RSP    ]  (Address of user stack)
                [ SS     ]  (User Data Selector: 0x18 | 3 = 0x1B)
```
Then the kernel executes `iretq`. The CPU believes it is returning from an interrupt, pops all five values, changes its internal CPL to **Ring 3**, and begins executing user code!

---

### 2. Fast System Calls (`SYSCALL` / `SYSRET`)
Once an application is in Ring 3, it cannot access hardware or kernel memory. When it wants to write a message or allocate memory, it issues a **System Call**.

Historically, CPUs used software interrupts (`int 0x80`). But software interrupts are slow—they flush CPU pipeline stages and look up descriptors in the IDT, taking hundreds of cycles.

Modern x86_64 CPUs introduce two dedicated hardware instructions: **`syscall`** and **`sysret`**.
They work via Model Specific Registers (MSRs) configured at boot ([kernel/src/arch/x86_64/syscall.rs](../kernel/src/arch/x86_64/syscall.rs)):
- **`LSTAR` MSR**: Holds the kernel address of `syscall_entry`. When user code calls `syscall`, the hardware jumps here in **1 instruction**!
- **`STAR` MSR**: Configures the segment selectors for kernel and user mode.
- **`FMASK` MSR**: Automatically clears the interrupt flag (`IF`) on entry for atomic entry.

#### The Syscall ABI:
- `%rax`: Syscall Number (`1 = SYS_WRITE`, `2 = SYS_EXIT`, `3 = SYS_YIELD`, `4 = SYS_GETPID`, `9 = SYS_MMAP`).
- Arguments passed in `%rdi`, `%rsi`, `%rdx`, `%r10`, `%r8`, `%r9`.
- Result returned in `%rax`.
- Executing `sysretq` returns immediately to Ring 3!

---

### 3. Process Address Space Isolation (`AddressSpace`)
Every user process gets its own dedicated PML4 page table:
- **Lower Half (`0x0000_0000_0000_0000` to `0x0000_7FFF_FFFF_FFFF`)**: Private to that process. Process A cannot see or access Process B's memory!
- **Higher Half (`0xFFFF_8000_0000_0000` to `0xFFFF_FFFF_FFFF_FFFF`)**: Shared kernel space. When an interrupt or syscall occurs, the kernel's code, stack, and drivers are immediately mapped and available.

---

### 4. User-Mode Crash Handling (Resilience)
What happens if user code has a bug (e.g. dereferences a null pointer `*ptr = 42` or executes an illegal instruction)?
1. The CPU generates an exception (like Page Fault `#PF`, Vector 14).
2. The CPU switches to the kernel stack and enters our `exception_handler` in `idt.rs`.
3. Our handler checks `(ctx.cs & 3) == 3`:
   - If `CS` indicates **Ring 3**: The kernel does NOT panic! It logs:
     ```
     [USER CRASH] User mode process crashed with Page Fault (#PF) at RIP: 0x50000a!
     [USER CRASH] Terminating faulting user thread cleanly and continuing OS execution...
     ```
   - The kernel calls `task::exit()`, reclaims the thread's resources, and schedules the next thread.
   - **The operating system keeps running smoothly without crashing!**

---

## 8. Summary & Architecture Map

Here is how the entire system connects together:

```
+=============================================================================+
|                                REAL HARDWARE                                |
|  - Motherboard UEFI Firmware: Loads BOOTX64.EFI from FAT32 ESP partition   |
|  - CPU Registers: GDTR, IDTR, CR0, CR3, CR4, MSRs (STAR, LSTAR, FMASK)      |
|  - Physical RAM: Managed in 4 KiB frames via PMM Bitmap                     |
+=============================================================================+
                                      |
                                      v
+=============================================================================+
|                             NULLOS KERNEL (Ring 0)                          |
|                                                                             |
|  [Architecture]                                                             |
|   - GDT & TSS: Defines segments, DPL 0/3, and TSS.rsp0 kernel stack         |
|   - IDT & ISRs: 256 gates for Exceptions (#DE, #PF, #DF) & APIC Timer (32)  |
|                                                                             |
|  [Memory Management]                                                        |
|   - PMM Bitmap: Allocates 4 KiB physical frames                             |
|   - 4-Level Paging: PML4 -> PDPT -> PD -> PT (CR3 translation, NX protect)  |
|   - Kernel Heap: GlobalAlloc supporting Box, Vec, and String                |
|   - AddressSpace: Isolated PML4 per process with shared kernel higher-half  |
|                                                                             |
|  [Task Execution & Scheduling]                                              |
|   - Thread: 16 KiB stack, saved RSP, state management                       |
|   - Preemptive Scheduler: Round-robin time slicing driven by APIC timer     |
|   - Context Switch: switch_threads in switch.S                              |
|                                                                             |
|  [Binary Loading & Syscalls]                                                |
|   - ELF64 Loader: Parses headers, maps PT_LOAD segments, fills BSS          |
|   - Fast Syscall Dispatcher: Handles SYS_WRITE, SYS_MMAP, SYS_EXIT, etc.    |
|   - Crash Protection: Catches Ring 3 faults and kills process gracefully    |
+=============================================================================+
                                      |
                 [iretq: Ring 0 -> Ring 3 Transition]
                 [syscall / sysretq: Fast Hardware ABI]
                                      v
+=============================================================================+
|                             USER SPACE (Ring 3)                             |
|                                                                             |
|  - Sandboxed execution (cannot touch hardware or kernel memory)             |
|  - User code at 0x0040_0000, User stack at 0x7000_0000                      |
|  - Dynamic memory mapping via SYS_MMAP (0x6000_0000)                        |
|  - Fast I/O via SYS_WRITE and clean termination via SYS_EXIT                |
+=============================================================================+
```

You now understand the complete engineering flow of modern operating systems—from the moment the power switch is flipped to the execution of isolated user applications!
