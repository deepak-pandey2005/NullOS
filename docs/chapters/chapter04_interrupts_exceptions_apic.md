# Chapter 4: Interrupts, Exceptions & Hardware Timers
### *The Event-Driven Heart of the Kernel: IDT, ISRs, and the APIC*

Imagine if an operating system had to constantly ask every device: *"Did you press a key? Did you press a key? Did a network packet arrive? Did 1 millisecond pass?"*
The CPU would spend 99.9% of its power asking questions instead of doing real work.

Operating systems are **event-driven**. The CPU runs tasks uninterrupted until an event occurs. When an event happens, the CPU is paused, runs a dedicated handler function, and resumes execution.

In this chapter, we explore how interrupts and exceptions work down to the electrical signals and assembly stubs: the **IDT (Interrupt Descriptor Table)**, **ISRs (Interrupt Service Routines)**, and the **Local APIC (Advanced Programmable Interrupt Controller)**.

---

## 1. Hardware Interrupts vs. CPU Exceptions

Not all interrupts are the same. They fall into two distinct families:

```
                                    EVENTS
                                       |
          +----------------------------+----------------------------+
          |                                                         |
          v                                                         v
  HARDWARE INTERRUPTS                                       CPU EXCEPTIONS
  Source: External physical chips                           Source: CPU Execution Core
  Timing: Asynchronous (any microsecond)                    Timing: Synchronous (specific instruction)
  Examples:                                                 Examples:
    - APIC Timer tick (Vector 32)                             - Divide-by-Zero (#DE, Vector 0)
    - Keyboard keypress (Vector 33)                           - Invalid Opcode (#UD, Vector 6)
    - Network card packet arrival                             - Page Fault (#PF, Vector 14)
```

---

### CPU Exceptions: Faults, Traps, and Aborts
When the CPU core encounters an illegal state while executing an instruction, it generates an **Exception**:

1. **Fault**:
   - The instruction encountered a recoverable problem.
   - The saved `RIP` points to the **exact instruction that caused the fault**.
   - *Example*: **Page Fault (`#PF`)**. If a program tries to read virtual memory that is swapped to disk, the kernel loads the page into RAM and executes `iretq`. The CPU **re-executes the exact same instruction**, and this time it succeeds!
2. **Trap**:
   - The instruction executed successfully, but an intentional event occurred.
   - The saved `RIP` points to the **next instruction** to be executed.
   - *Example*: Breakpoint exception (`int 3`, Vector 3) used by debuggers (like GDB).
3. **Abort**:
   - A catastrophic, unrecoverable hardware or kernel error.
   - The saved `RIP` may be meaningless.
   - *Example*: **Double Fault (`#DF`, Vector 8)** or **Machine Check (`#MC`, Vector 18)**.

---

## 2. The IDT (Interrupt Descriptor Table)

The **IDT** is an array of 256 entries in memory telling the CPU where to jump when any vector ($0$ to $255$) triggers.

```
Vector Range   Usage
0 .. 31        Reserved for CPU Architectural Exceptions (#DE, #DF, #GP, #PF...)
32 .. 254      User-Defined Hardware Interrupts (APIC Timer = 32, Devices = 33..254)
255            Spurious Interrupt Vector (APIC Spurious Register)
```

### The 16-Byte Gate Descriptor Layout (64-Bit Mode)
In 64-bit Long Mode, each IDT gate descriptor is **16 bytes (128 bits)**:

```
127                                                             64
+---------------------------------------------------------------+
|                       Reserved (32 bits = 0)                  |
+---------------------------------------------------------------+
|                   Offset / ISR Pointer [63:32] (32 bits)      |
+---------------------------------------------------------------+
63                  48 47  46 45 44 43    40 39  36 35  32 31   0
+---------------------+---+-----+--+--------+------+------+-----+
| Offset [31:16]      | P | DPL |0 | Type=0xE|Res=0 | IST  | Sel |
+---------------------+---+-----+--+--------+------+------+-----+
|  Kernel Code Segment Selector (0x08)      | Offset [15:0]     |
+-------------------------------------------+-------------------+
```

### Key Bitfields:
- **Offset (Bits 0..15, 48..63, 64..95)**: The full 64-bit virtual memory address of the assembly ISR function.
- **Selector (Bits 16..31)**: The Code Segment selector (`0x08` for Kernel Code).
- **IST (Bits 32..34)**: Interrupt Stack Table index ($0 = \text{normal stack}$, $1 = \text{IST1 for Double Fault}$).
- **Type (Bits 40..43)**: `0xE` (64-bit Interrupt Gate). When the CPU enters an Interrupt Gate, it **automatically disables interrupts (`IF=0`)** to prevent nested race conditions!
- **DPL (Bits 45..46)**: Privilege level required to invoke via software (`int X`). Set to `0` for hardware/faults so user mode cannot spoof a page fault or timer!
- **Present (Bit 47)**: Must be `1` for the entry to be active.

We load the IDT using:
```assembly
lidt [pointer_to_idt_descriptor]
```

---

## 3. What the CPU Hardware Does During an Interrupt

The moment an interrupt vector triggers:
1. **Privilege & Stack Evaluation**:
   - If currently in Ring 3: The CPU reads `TSS.rsp0` and switches `%rsp` to the kernel stack.
   - If the IDT entry specifies `IST > 0`: The CPU switches `%rsp` to the corresponding `TSS.ist[N]` stack.
2. **Pushing the Hardware Frame**:
   The CPU automatically pushes 5 registers onto the stack:
   ```
   [rsp + 32] -> SS     (Old Stack Segment)
   [rsp + 24] -> RSP    (Old Stack Pointer)
   [rsp + 16] -> RFLAGS (Old Status Flags)
   [rsp + 8]  -> CS     (Old Code Segment)
   [rsp + 0]  -> RIP    (Old Instruction Pointer - Resume Address!)
   ```
3. **Error Code**:
   For certain exceptions (Vectors 8, 10, 11, 12, 13, 14, 17, 21), the CPU hardware pushes a 64-bit **Error Code** onto the stack immediately after `RIP`.
   *(For all other interrupts, no error code is pushed!)*

---

## 4. The Assembly ISR Stub (`interrupts.S`)

Because the CPU only pushes `RIP, CS, RFLAGS, RSP, SS`, all other 15 general-purpose registers (`RAX, RBX, RCX...`) still hold whatever values the interrupted program was working on!

If our Rust code immediately started running, the Rust compiler would overwrite `%rax`, `%rdi`, etc., corrupting the interrupted program!

Therefore, every interrupt **must** enter a carefully hand-crafted assembly stub ([kernel/src/arch/x86_64/interrupts.S](file:///home/deepak/OPENSOURCE/NullOS/kernel/src/arch/x86_64/interrupts.S)):

```assembly
# Macro for exceptions that DO NOT push an error code:
.macro ISR_NO_ERR vector
.global isr\vector
isr\vector:
    push $0          # Push dummy error code to align stack!
    push $\vector    # Push interrupt vector number
    jmp isr_common
.endm

# Macro for exceptions that DO push an error code:
.macro ISR_ERR vector
.global isr\vector
isr\vector:
    push $\vector    # Push interrupt vector number
    jmp isr_common
.endm
```

### The Common Assembly Wrapper:
```assembly
isr_common:
    # 1. Save all 15 general-purpose registers onto the stack
    push %r15
    push %r14
    push %r13
    push %r12
    push %r11
    push %r10
    push %r9
    push %r8
    push %rbp
    push %rdi
    push %rsi
    push %rdx
    push %rcx
    push %rbx
    push %rax

    # 2. Pass pointer to this complete saved frame as Arg 1 (%rdi) to Rust
    mov %rsp, %rdi
    call exception_handler

    # 3. Restore all 15 registers
    pop %rax
    pop %rbx
    pop %rcx
    pop %rdx
    pop %rsi
    pop %rdi
    pop %rbp
    pop %r8
    pop %r9
    pop %r10
    pop %r11
    pop %r12
    pop %r13
    pop %r14
    pop %r15

    # 4. Clean up vector number and error code
    add $16, %rsp

    # 5. Return to interrupted code!
    iretq
```

---

## 5. The Hardware Timer: APIC vs. Legacy 8259 PIC

How does the OS keep track of time and preempt threads?
In 1981 PCs, motherboards used the **Intel 8259 PIC** (Programmable Interrupt Controller). The 8259 was slow, supported only 15 interrupt lines, and could not handle multi-core systems.

### Disabling the Legacy PIC
Before using the modern APIC, we must permanently disable the legacy 8259 PIC so it doesn't fire spurious interrupts. We do this by writing `0xFF` to I/O ports `0x21` and `0xA1` ([apic.rs](file:///home/deepak/OPENSOURCE/NullOS/kernel/src/arch/x86_64/apic.rs)):
```rust
outb(0x21, 0xFF); // Mask all interrupts on Master PIC
outb(0xA1, 0xFF); // Mask all interrupts on Slave PIC
```

### The Modern Local APIC
Every CPU core has its own integrated **Local APIC**.
The Local APIC registers are accessed via **Memory-Mapped I/O (MMIO)** starting at physical address **`0xFEE0_0000`**:

```
Register Offset   Name                         Function
0x0020            Local APIC ID                Unique ID of this CPU core
0x00B0            EOI (End of Interrupt)       Write 0 here when ISR finishes!
0x00F0            Spurious Interrupt Register  Set bit 8 to enable APIC
0x0320            LVT Timer Register           Configures Timer (Vector 32, Periodic)
0x0380            Timer Initial Count          Starting counter value (e.g. 100,000)
0x0390            Timer Current Count          Counts down to zero on CPU bus clock
0x03E0            Timer Divide Config (DCR)    Divide timer clock by 2, 4, 8, 16...
```

### How the APIC Timer Drives NullOS:
1. We configure the APIC Timer in **Periodic Mode** on Vector 32 (`0x20`).
2. Every time the timer ticks, the hardware raises Interrupt 32.
3. The CPU pauses the current thread and enters `timer_handler` in [idt.rs](file:///home/deepak/OPENSOURCE/NullOS/kernel/src/arch/x86_64/idt.rs):
   ```rust
   pub extern "C" fn timer_handler() {
       super::apic::on_timer_tick();       // Increment tick counter
       super::apic::end_of_interrupt();     // Write 0 to APIC EOI register
       let ticks = super::apic::get_ticks();
       crate::task::timer_tick(ticks);      // Preempt thread if timeslice expired!
   }
   ```
4. **End of Interrupt (EOI)**: Hardware controllers require an explicit acknowledgment when an interrupt is processed. Writing `0` to `0xFEE0_00B0` signals the APIC that it is safe to send the next interrupt!

---

## 🧠 Self-Test & Review Questions

1. **Why must an assembly ISR stub save all general-purpose registers before calling a Rust exception handler?**
   <details>
   <summary>Click to view answer</summary>
   The CPU hardware only saves <code>RIP, CS, RFLAGS, RSP, SS</code>. The Rust compiler expects to freely use registers like <code>RAX, RDI, RSI, RDX</code> for arithmetic and local variables. If the assembly stub did not push them, the interrupted program's register variables would be overwritten and corrupted upon return.
   </details>

2. **What is the difference between a Fault and a Trap regarding the saved Instruction Pointer (`RIP`)?**
   <details>
   <summary>Click to view answer</summary>
   A Fault saves a <code>RIP</code> pointing to the <strong>exact instruction that caused the fault</strong>, so that the OS can fix the issue (e.g., paging in memory) and re-execute it. A Trap saves a <code>RIP</code> pointing to the <strong>next instruction</strong>, as the triggering instruction already completed (e.g., a debugger breakpoint).
   </details>

3. **What happens if an operating system forgets to write to the APIC End-of-Interrupt (EOI) register at the end of an interrupt handler?**
   <details>
   <summary>Click to view answer</summary>
   The Local APIC will believe that the CPU is still actively servicing the interrupt. It will block all future interrupts of equal or lower priority on that vector, causing the system timer to freeze permanently.
   </details>

---

*In [Chapter 5: Physical Memory Management (PMM & Frames)](chapter05_physical_memory.md), we explore how raw physical RAM chips are organized into 4 KiB frames and tracked using an ultra-compact Bitmap.*
