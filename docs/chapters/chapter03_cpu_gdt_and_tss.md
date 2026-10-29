# Chapter 3: CPU Segmentation, Protection & Privilege
### *Mastering the GDT, Privilege Rings, and the TSS*

When the CPU executes instructions, how does it know whether code is allowed to touch hardware? Why can't a user-space application disable interrupts or overwrite kernel memory?

In this chapter, we explore CPU privilege enforcement from the silicon up: the **Global Descriptor Table (GDT)**, segment selectors, the **Task State Segment (TSS)**, and the hardware mechanisms that guarantee stack safety during privilege transitions.

---

## 1. The Silicon Reality: CPU Privilege Rings

Inside an x86_64 CPU core, there is an internal 2-bit state register known as the **CPL (Current Privilege Level)**.
- `CPL = 00` $\rightarrow$ **Ring 0 (Supervisor / Kernel Mode)**
- `CPL = 01` $\rightarrow$ **Ring 1** (rarely used in modern OSes)
- `CPL = 10` $\rightarrow$ **Ring 2** (rarely used in modern OSes)
- `CPL = 11` $\rightarrow$ **Ring 3 (User Mode)**

```
+-------------------------------------------------------------+
|                     Ring 3: Applications                    |
|             (Web browsers, games, user utilities)           |
+------------------------------+------------------------------+
                               |
                   [Hardware Gate: SYSCALL / IDT]
                               |
                               v
+-------------------------------------------------------------+
|                     Ring 0: Operating System                |
|             (NullOS Kernel, Device Drivers, Sched)          |
+-------------------------------------------------------------+
```

### What Happens When Ring 3 Violates Security?
When the CPU decodes an instruction, its microcode checks the CPL:
- If `CPL == 3` and the instruction is a privileged instruction (like `cli`, `sti`, `lidt`, `lgdt`, `mov cr3, ...`, or `in`/`out`):
  **The CPU execution unit refuses to execute it!**
- Instead, the CPU instantly triggers a **General Protection Fault (`#GP`, Exception 13)**, transferring control to the kernel's fault handler.

---

## 2. Why Does 64-Bit Mode Still Need a GDT?

In legacy 16-bit and 32-bit x86 architectures, memory addressing used **Segmentation**:
$$\text{Linear Address} = (\text{Segment Selector} \times 16) + \text{Offset}$$
A segment defined a base address and a limit (e.g., Segment A starts at 1 MB and is 500 KB long).

In **64-bit Long Mode**, segmentation was largely abolished:
- Memory is **flat**: base address is fixed to `0` and the limit is effectively infinite.
- Address translation is handled entirely by **Paging** (Chapter 6).

### If Segmentation is Dead, Why Do We Need the GDT?
Because x86_64 CPUs **still require the GDT** to define:
1. **Operating Mode**: Whether code is running as 64-bit Long Mode code or 32-bit compatibility code.
2. **Privilege Level (DPL)**: Whether a code segment belongs to Ring 0 or Ring 3.
3. **The Task State Segment (TSS)**: Where the kernel stack pointer (`RSP0`) and emergency stacks (`IST`) live.

---

## 3. Anatomy of a GDT Segment Descriptor (8 Bytes)

A standard GDT entry is exactly **8 bytes (64 bits)**:

```
 63          56 55 52 51    48 47          40 39        32 31               16 15                0
+--------------+-----+--------+--------------+------------+-------------------+--------------------+
| Base [31:24] |Flags| Limit  | Access Byte  | Base[23:16]|   Base [15:0]     |   Limit [15:0]     |
|              |     | [19:16]|              |            |                   |                    |
+--------------+-----+--------+--------------+------------+-------------------+--------------------+
```

### The Access Byte (Bits 40..47)
This is where the magic happens:
- **Bit 47 (`Present`)**: Must be `1` for any valid segment.
- **Bits 45-46 (`DPL` - Descriptor Privilege Level)**:
  - `00` = Ring 0 (Kernel).
  - `11` = Ring 3 (User).
- **Bit 44 (`Descriptor Type`)**: `1` for Code/Data segments; `0` for System segments (like TSS).
- **Bit 43 (`Executable`)**: `1` for Code segments; `0` for Data/Stack segments.
- **Bit 41 (`Readable / Writable`)**: For code, readable; for data, writable.

### The Flags Nibble (Bits 52..55)
- **Bit 53 (`L` - Long Mode)**: When set to **`1`**, this declares that code running in this segment is **native 64-bit code**!
- **Bit 54 (`D/B` - Default Size)**: In 64-bit mode with `L=1`, this bit must be set to `0`.
- **Bit 55 (`G` - Granularity)**: When set, the segment limit is scaled by 4 KiB pages rather than single bytes.

---

## 4. The NullOS GDT Layout

In NullOS ([kernel/src/arch/x86_64/gdt.rs](../../kernel/src/arch/x86_64/gdt.rs)), our GDT is configured with 6 entries:

| Index | Offset / Selector | Name | DPL (Ring) | Type | Flags |
| :---: | :---: | :--- | :---: | :--- | :--- |
| **0** | `0x00` | **Null Descriptor** | - | - | All zeroes (mandatory by x86 architecture). |
| **1** | `0x08` | **Kernel Code** | **Ring 0** | Code (Execute/Read) | `L=1` (64-bit Long Mode) |
| **2** | `0x10` | **Kernel Data** | **Ring 0** | Data (Read/Write) | Standard data |
| **3** | `0x18` | **User Data** | **Ring 3** | Data (Read/Write) | Standard data, `DPL=3` |
| **4** | `0x20` | **User Code** | **Ring 3** | Code (Execute/Read) | `L=1` (64-bit Long Mode), `DPL=3` |
| **5** | `0x28` | **TSS Descriptor** | **Ring 0** | System (TSS Available) | **16 bytes wide!** |

---

## 5. Segment Selectors: What do `0x08`, `0x10`, `0x23` Mean?

When you tell the CPU to use a segment (by loading `%cs`, `%ds`, or `%ss`), you do not pass a pointer to the descriptor. You pass a **Segment Selector (16 bits)**:

```
15                                                    3   2   1   0
+-------------------------------------------------------+---+-------+
|                    GDT Table Index                    |TI |  RPL  |
+-------------------------------------------------------+---+-------+
```
- **Bits 3..15 (`Index`)**: Which row in the GDT table to use ($0, 1, 2, 3, \dots$).
- **Bit 2 (`TI` - Table Indicator)**: `0` for GDT, `1` for LDT (Local Descriptor Table).
- **Bits 0..1 (`RPL` - Requested Privilege Level)**: `00` for Ring 0; `11` for Ring 3.

### Breaking Down the Math:
- **Kernel Code Selector**:
  - Index = 1 $\rightarrow 1 \ll 3 = 8 = \mathbf{0x08}$.
  - RPL = 0.
  - Selector = $\mathbf{0x08}$.
- **Kernel Data Selector**:
  - Index = 2 $\rightarrow 2 \ll 3 = 16 = \mathbf{0x10}$.
  - RPL = 0.
  - Selector = $\mathbf{0x10}$.
- **User Data Selector**:
  - Index = 3 $\rightarrow 3 \ll 3 = 24 = 0x18$.
  - RPL = 3 (bits `11`).
  - Selector = $0x18 \mid 3 = \mathbf{0x1B}$!
- **User Code Selector**:
  - Index = 4 $\rightarrow 4 \ll 3 = 32 = 0x20$.
  - RPL = 3 (bits `11`).
  - Selector = $0x20 \mid 3 = \mathbf{0x23}$!

Whenever the CPU is in User Mode (Ring 3), its `%cs` register **must** contain `0x23`! The lowest two bits (`3`) tell the hardware that the current CPL is Ring 3!

---

## 6. How the CPU Loads the GDT: `lgdt`

The CPU has a register called **`GDTR`** (GDT Register):

```
15               0 63                                              0
+-----------------+-------------------------------------------------+
|  Limit (2 bytes)|          64-bit Linear Base Address             |
+-----------------+-------------------------------------------------+
```
To load it in Rust assembly:
```rust
#[repr(C, packed)]
struct GdtDescriptor {
    size: u16,   // Total size in bytes minus 1
    offset: u64, // Virtual address of the GDT array
}

unsafe {
    core::arch::asm!("lgdt [{}]", in(reg) &gdt_descriptor, options(readonly, nostack));
}
```

Once loaded, we immediately reload the segment registers:
```rust
core::arch::asm!(
    "mov ds, ax",
    "mov es, ax",
    "mov fs, ax",
    "mov gs, ax",
    "mov ss, ax",
    in("ax") 0x10u16 // Kernel Data Selector
);
```

---

## 7. The Task State Segment (TSS) & Stack Safety

The **TSS** is a 104-byte hardware structure defined by x86_64 architecture:

```
Offset   Field
0x00     Reserved (4 bytes)
0x04     RSP0 (64-bit Kernel Privilege Stack Pointer)  <-- VITAL FOR SECURITY!
0x0C     RSP1 (64-bit)
0x14     RSP2 (64-bit)
0x1C     Reserved (8 bytes)
0x24     IST1 (64-bit Interrupt Stack Table 1)         <-- VITAL FOR DOUBLE FAULT!
0x2C     IST2 (64-bit)
...
0x5C     IST7 (64-bit)
0x64     Reserved (10 bytes)
0x66     I/O Map Base Address (16 bits)
```

### Why is `RSP0` Critical for Operating System Security?
Imagine user mode application code is running. The user's stack pointer (`%rsp`) is pointing to address `0x7000_0FF0`.
Now, the timer interrupt fires!
The CPU must immediately transition to Ring 0 and push the return address (`RIP`, `CS`, `RFLAGS`, `RSP`, `SS`) onto the stack.

**What if the CPU pushed these onto the user's stack?**
1. The user application could have an invalid `%rsp` (e.g. `0x0`), causing an instant crash before the interrupt handler even runs.
2. The user application could inspect or modify the saved kernel state.

**The Hardware Solution:**
The CPU hardware checks if `Old_CPL == 3` and `New_CPL == 0`.
If switching privilege, **the CPU automatically ignores the user's `%rsp` and loads `%rsp = TSS.rsp0`**!
The return frame is pushed onto the kernel's dedicated stack, completely isolated from user space!

### Why is the TSS GDT Descriptor 16 Bytes?
A standard GDT entry only has 32 bits for base address. In 64-bit mode, memory addresses are 64 bits wide. To hold a full 64-bit base address for the TSS, AMD64 expanded the TSS descriptor to consume **two consecutive GDT slots (16 bytes)**!

We inform the CPU of the TSS using the `ltr` (Load Task Register) instruction:
```assembly
ltr ax   # ax = 0x28 (TSS GDT selector)
```

---

## 🧠 Self-Test & Review Questions

1. **Why does loading a code segment selector with value `0x23` place the CPU into Ring 3, while `0x20` would be rejected?**
   <details>
   <summary>Click to view answer</summary>
   A segment selector's lowest 2 bits represent the Requested Privilege Level (RPL). <code>0x23</code> in binary ends in <code>11</code> (RPL = 3), indicating User Mode. If you passed <code>0x20</code> (ending in <code>00</code>), the RPL would be 0, which conflicts with a descriptor configured with DPL = 3, causing a General Protection Fault.
   </details>

2. **What catastrophic failure would occur if an operating system did not configure `TSS.rsp0` before switching to User Mode?**
   <details>
   <summary>Click to view answer</summary>
   The moment any hardware interrupt (like the timer) or exception fired while in user mode, the CPU would attempt to load <code>TSS.rsp0</code> to switch stacks. Finding it uninitialized (zero or garbage), the CPU would fail to push the interrupt frame, causing a Double Fault (Exception 8), and then immediately a Triple Fault, resetting the physical computer.
   </details>

3. **Why does the TSS descriptor in the GDT occupy 16 bytes while normal Code/Data descriptors occupy only 8 bytes?**
   <details>
   <summary>Click to view answer</summary>
   In 64-bit Long Mode, base addresses for normal code/data segments are forced to 0 (flat model). However, system descriptors like the TSS must store a true 64-bit virtual memory base address, which cannot fit in a standard 8-byte descriptor. Thus, the architecture expands system descriptors to 16 bytes.
   </details>

---

*In [Chapter 4: Interrupts, Exceptions & Hardware Timers](chapter04_interrupts_exceptions_apic.md), we explore how the CPU handles hardware events, sets up the IDT, and calibrates the Local APIC timer.*
