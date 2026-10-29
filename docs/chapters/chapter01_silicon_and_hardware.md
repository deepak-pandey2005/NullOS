# Chapter 1: The Silicon & Bare-Metal Reality
### *How Code Interacts with Raw Physical Hardware*

Welcome to the very beginning. Before we write an operating system, we must demystify what a computer actually is.

Most software engineers spend their careers working inside layers of insulation: inside Python, JavaScript, Java, or standard C++ running on Linux or Windows. In that world, when you write `print("Hello")` or `malloc(100)`, thousands of unseen mechanisms protect you:
- Memory is infinite and always starts at clean addresses.
- If your code has an error, your process exits with an error code; the computer does not restart.
- You can read files by typing a file path.

**In bare-metal OS development, none of that exists.** You are writing code that talks directly to electronic circuits. If you write the wrong value to a control register, the CPU physically freezes or restarts the computer in less than a microsecond.

In this chapter, we explore how software and hardware meet at the silicon level.

---

## 1. The Core Question: What is an Instruction?

At the physical level, a computer chip is billions of microscopic **transistors** etched onto silicon wafers. A transistor is simply an electrically controlled switch:
- High voltage (e.g. $+1.2\text{V}$) represents a binary **`1`**.
- Low voltage (e.g. $0\text{V}$) represents a binary **`0`**.

### Machine Code vs. Assembly
The CPU understands only binary numbers. For example, on an x86_64 CPU:
```text
48 c7 c0 01 00 00 00
```
This sequence of 7 bytes means: *"Move the 64-bit number 1 into the CPU register RAX"*.

Because humans cannot read raw hexadecimal sequences without going mad, we use **Assembly Language**, which provides human-readable mnemonics for these exact bytes:
```assembly
mov $1, %rax
```
An **assembler** (like `as` or the assembler inside Rust's `rustc`) translates that mnemonic text directly into the machine bytes.

---

## 2. The CPU Memory Hierarchy: From Picoseconds to Milliseconds

Why doesn't the CPU just run code directly from your hard drive or SSD?
**Speed.**

Electricity takes time to travel through copper wires. Light and electrical signals travel roughly 20 centimeters in one nanosecond. If your CPU core runs at $4.0\text{ GHz}$, one CPU cycle takes **0.25 nanoseconds (250 picoseconds)**.

```
+-------------------------------------------------------------+
| Registers (RAX, RBX, RSP...)  | ~0.25 - 0.5 ns (1 cycle)    |
+-------------------------------------------------------------+
| L1 CPU Cache (~64 KiB)        | ~1.0 ns (4 cycles)          |
+-------------------------------------------------------------+
| L2 CPU Cache (~512 KiB)       | ~3 - 4 ns (12 cycles)       |
+-------------------------------------------------------------+
| L3 Shared Cache (~16 - 64 MiB)| ~10 - 15 ns (40 cycles)     |
+-------------------------------------------------------------+
| Physical RAM (DDR4 / DDR5)    | ~50 - 100 ns (200+ cycles)  |
+-------------------------------------------------------------+
| NVMe SSD / Storage Disk       | ~10,000 - 100,000 ns        |
+-------------------------------------------------------------+
```

### The Human Analogy of Latency
If 1 CPU cycle (0.25 ns) was equal to **1 second**:
- Accessing a **CPU Register** takes **1 second** (reaching for a pencil on your desk).
- Accessing **L1 Cache** takes **4 seconds** (taking a book from the shelf next to you).
- Accessing **RAM** takes **4 minutes** (walking down the street to the library).
- Reading from an **SSD** takes **4 days** (taking a train to another city to pick up a package).
- Reading from a **Spinning Hard Disk** takes **several months**!

Because RAM is so far away from the arithmetic execution units inside the core, the CPU maintains general-purpose **Registers** right inside the silicon core.

---

## 3. The x86_64 Registers: The CPU's Scratchpad

In 64-bit x86_64 mode, the CPU provides sixteen 64-bit general-purpose registers:

```
63                                            31              15      7      0
+-----------------------------------------------------------------------------+
|                                    RAX                                      |
+---------------------------------------------+-------------------------------+
                                              |              EAX              |
                                              +---------------+---------------+
                                                              |      AX       |
                                                              +-------+-------+
                                                              |  AH   |  AL   |
                                                              +-------+-------+
```

Each register can be accessed as a whole 64-bit value, or its lower 32 bits, 16 bits, or 8 bits:
- **`%rax`**: Primary accumulator (used for function return values and syscall numbers).
- **`%rbx`**: Base register (callee-saved general register).
- **`%rcx`**: Counter register (used by loops, string operations, and stores user RIP during `syscall`).
- **`%rdx`**: Data register (used for I/O operations and multiplication/division).
- **`%rsi`**: Source index pointer (used for string/memory copies and SysV argument 2).
- **`%rdi`**: Destination index pointer (used for memory copies and SysV argument 1).
- **`%rbp`**: Base pointer (traditionally marks the start of a function stack frame).
- **`%rsp`**: **Stack Pointer** (points to the current top of the active memory stack; crucial for execution!).
- **`%r8` through `%r15`**: Extended general-purpose registers introduced in 64-bit mode.

### Special Registers (The Puppet Masters)
- **`%rip` (Instruction Pointer)**: Stores the memory address of the **next instruction** the CPU will fetch and execute. You cannot write to `%rip` directly with a `mov`; you change it using jumps (`jmp`), calls (`call`), returns (`ret`), or interrupts (`iretq`).
- **`%rflags`**: 64-bit status register containing condition flags:
  - `CF` (Carry), `ZF` (Zero Flag: set when an arithmetic result is 0), `SF` (Sign Flag).
  - **`IF` (Interrupt Enable Flag, bit 9)**: When `1`, hardware interrupts can interrupt the CPU. When `0` (cleared via `cli`), the CPU ignores hardware interrupts!

---

## 4. How Code Talks to Physical Hardware

How does software turn on an LED, read a keyboard key, or send bytes out of a serial cable?
Computers use two distinct hardware communication mechanisms:

```
                    +--------------------------------+
                    |        CPU Execution Core      |
                    +---------------+----------------+
                                    |
          +-------------------------+-------------------------+
          |                                                   |
          v                                                   v
   [PORT-MAPPED I/O]                                  [MEMORY-MAPPED I/O]
   Special Bus & Opcodes                              Standard Physical RAM Bus
   Instructions: in / out                             Instructions: mov, read, write
   Example: COM1 Serial (0x3F8)                       Example: GOP Screen (0x80000000)
            Legacy Keyboard (0x60)                             Local APIC (0xFEE00000)
```

---

### A. Port-Mapped I/O (PMIO)
The x86 architecture has a separate 16-bit address space called the **I/O Port Space** (ports `0x0000` to `0xFFFF`).
These are not RAM addresses. They are electrical wires hooked up to legacy peripheral controller chips.

To talk to an I/O port, you must use special CPU instructions:
- **`in %dx, %al`**: Reads an 8-bit byte from hardware port address in `%dx` into register `%al`.
- **`out %al, %dx`**: Writes the byte in `%al` out to hardware port address in `%dx`.

In NullOS, we use Port I/O to initialize the **COM1 Serial Port** at port `0x3F8` ([kernel/src/drivers/serial.rs](../../kernel/src/drivers/serial.rs)):
```rust
pub unsafe fn outb(port: u16, value: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") value,
        options(nomem, nostack, preserves_flags)
    );
}

pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") port,
        out("al") value,
        options(nomem, nostack, preserves_flags)
    );
    value
}
```

---

### B. Memory-Mapped I/O (MMIO)
Modern high-speed devices (GPUs, NVMe SSDs, network cards, and modern interrupt controllers like the Local APIC) cannot be bottlenecked by slow 8-bit I/O ports.

Instead, the motherboard routes certain ranges of physical memory addresses directly to the device's internal registers!
- For example, physical address `0xFEE0_0000` is **NOT RAM**. It is connected directly to the **Local APIC chip**.
- When the CPU executes a normal memory write:
  ```rust
  let apic_eoi = 0xFEE0_00B0 as *mut u32;
  unsafe { *apic_eoi = 0; }
  ```
  The memory controller detects that this address belongs to the APIC, and the electrical data is sent directly into the APIC's End-Of-Interrupt register!
- Similarly, physical address `0x8000_0000` is the **GOP Framebuffer**. Writing 32-bit values there writes directly to GPU video RAM, immediately illuminating pixels on your monitor!

---

## 5. Control Registers: The CPU's Master Switches

The x86_64 architecture provides several **Control Registers** (`CR0`, `CR2`, `CR3`, `CR4`) and **MSRs** (Model-Specific Registers) that dictate how the silicon behaves.

### CR0: System Control Flags
```
31                                                         16 15                    0
+------------------------------------------------------------+-----------------------+
|PG|                                                       |WP|                   |PE|
+------------------------------------------------------------+-----------------------+
```
- **Bit 0 (`PE` - Protection Enable)**: Enables Protected Mode (32-bit/64-bit protection).
- **Bit 16 (`WP` - Write Protect)**: When set to `1`, the CPU forbids even Ring 0 kernel code from writing to pages marked read-only! (Critical security feature against kernel memory corruption bugs).
- **Bit 31 (`PG` - Paging)**: Enables hardware memory translation (Virtual Memory).

### CR2: Page Fault Linear Address
When a **Page Fault (`#PF`)** exception occurs, the hardware automatically saves the exact memory address that caused the fault into **`CR2`**! The kernel reads `CR2` to know where the program crashed.

### CR3: Page Directory Base Register
Holds the physical memory address of the active **PML4 Page Table**. Writing to `CR3` switches the active memory map.

### CR4: Architectural Extensions
- **Bit 7 (`PGE` - Page Global Enable)**: Allows frequently used pages (like the kernel) to stay cached in the TLB across task switches.
- **Bit 9 (`OSFXSR`)**: Enables fast SSE / AVX floating-point instructions.

---

## 6. The Calling Convention (System V AMD64 ABI)

When you call a function in C or Rust:
```rust
fn add(a: u64, b: u64, c: u64) -> u64
```
How does the CPU know where `a`, `b`, and `c` are?

On x86_64 Linux, BSD, and NullOS, we adhere to the **System V AMD64 ABI** specification:
1. **Integer/Pointer Arguments**: Passed in registers in this exact order:
   - Arg 1: **`%rdi`**
   - Arg 2: **`%rsi`**
   - Arg 3: **`%rdx`**
   - Arg 4: **`%rcx`** (or `%r10` during system calls)
   - Arg 5: **`%r8`**
   - Arg 6: **`%r9`**
   - Any additional arguments are pushed onto the stack.
2. **Return Value**: Placed into **`%rax`**.
3. **Preserved Registers (Callee-Saved)**:
   A called function is allowed to modify `%rax, %rcx, %rdx, %rsi, %rdi, %r8, %r9, %r10, %r11`.
   **BUT** it must preserve: **`%rbx, %rsp, %rbp, %r12, %r13, %r14, %r15`**! If a function uses these registers, it must push them to the stack and restore them before returning!
   *(This rule is the secret to how context switching works in Chapter 8!)*

---

## 🧠 Self-Test & Review Questions

Test your understanding of Chapter 1 before moving to Chapter 2:

1. **Why does the CPU use general-purpose registers instead of running all computations directly in RAM?**
   <details>
   <summary>Click to view answer</summary>
   Accessing a register takes ~0.25 nanoseconds (1 cycle), whereas accessing RAM takes ~50-100 nanoseconds (200+ cycles). Registers sit directly inside the core silicon next to the arithmetic logic unit (ALU), avoiding the massive electrical transit latency over the memory bus.
   </details>

2. **What is the difference between Port-Mapped I/O (PMIO) and Memory-Mapped I/O (MMIO)?**
   <details>
   <summary>Click to view answer</summary>
   PMIO uses a separate 16-bit address space accessed exclusively via special CPU instructions (`in` and `out`). MMIO assigns device controller registers to normal physical memory addresses, allowing software to communicate with hardware using standard memory read and write instructions (`mov`).
   </details>

3. **If a user program tries to access an address that is not mapped in page tables, what register automatically captures the faulting address?**
   <details>
   <summary>Click to view answer</summary>
   Control Register 2 (<code>CR2</code>). The CPU hardware automatically stores the linear faulting address in <code>CR2</code> before executing the Page Fault exception handler (Vector 14).
   </details>

4. **In the System V AMD64 calling convention, where does a function look for its first two arguments, and where does it place its return value?**
   <details>
   <summary>Click to view answer</summary>
   Argument 1 is in <code>%rdi</code>, Argument 2 is in <code>%rsi</code>, and the return value is returned in <code>%rax</code>.
   </details>

---

*In [Chapter 2: Boot & Firmware Protocols](chapter02_boot_and_firmware.md), we explore how the motherboard turns on, how modern UEFI locates our bootloader on disk, and how the ELF kernel is parsed and launched.*
