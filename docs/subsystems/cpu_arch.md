# Subsystem: CPU Architecture (`kernel/src/arch/x86_64/`)

This directory contains low-level hardware structures, assembly stubs, and CPU control logic for the `x86_64` architecture.

---

## 1. GDT & TSS (`gdt.rs`)

### Purpose
In 64-bit long mode, segmentation is mostly inactive (base is fixed to 0), but the **Global Descriptor Table (GDT)** is still strictly required for:
1. Setting the processor privilege level (Ring 0 for Kernel, Ring 3 for User mode).
2. Enabling 64-bit Long Mode code execution via the `L` bit.
3. Loading the **Task State Segment (TSS)** for privilege stack switches and Interrupt Stack Tables (IST).

### Layout of the GDT
| Index | Selector | Description | Privilege | Flags |
|---|---|---|---|---|
| 0 | `0x00` | Null Descriptor | - | - |
| 1 | `0x08` | Kernel Code (64-bit) | Ring 0 | Present, Executable, Readable, Long Mode |
| 2 | `0x10` | Kernel Data (64-bit) | Ring 0 | Present, Writable |
| 3 | `0x18` | User Data (64-bit) | Ring 3 | Present, Writable, DPL=3 |
| 4 | `0x20` | User Code (64-bit) | Ring 3 | Present, Executable, Readable, Long Mode, DPL=3 |
| 5-6 | `0x28` | Task State Segment (TSS) | Ring 0 | 16-byte System Segment, Type 0x9 (Available 64-bit TSS) |

### Task State Segment (TSS) & Interrupt Stack Table (IST)
The TSS holds:
- **`rsp0`**: Kernel stack pointer loaded automatically when transitioning from Ring 3 to Ring 0.
- **`ist[0]` (IST1)**: A dedicated 16 KiB emergency stack for handling **Double Faults (`#DF`)**. If the regular kernel stack overflows or corrupts, handling the resulting exception on the same stack triggers an immediate, unrecoverable Triple Fault (instant reboot). IST1 ensures Double Fault has a clean stack.

---

## 2. IDT & Exception Handlers (`idt.rs`, `interrupts.S`)

### Purpose
The **Interrupt Descriptor Table (IDT)** tells the CPU which code address to jump to when a hardware interrupt or CPU exception occurs.

### Structure of an IDT Gate (16 bytes in x86_64)
- **Offset `15..0`**, **`31..16`**, **`63..32`**: 64-bit entry address of the handler stub.
- **Selector (16 bits)**: Code segment selector (`0x08` - Kernel Code).
- **IST (3 bits)**: Specifies which IST stack from the TSS to use (0 = current stack, 1 = IST1 for Double Fault).
- **Type/Attributes (8 bits)**: `0x8E` = Present, DPL 0, 64-bit Interrupt Gate (clears `IF` on entry).

### Exception Dispatch Flow
```
[CPU Exception / Fault]
          │
          ▼
   [Assembly Stub: interrupts.S]
   - Pushes dummy error code if CPU didn't push one
   - Pushes vector number
   - Pushes all 15 General Purpose Registers (RAX..R15)
   - Loads %rdi with %rsp (pointer to ExceptionContext)
          │
          ▼
   [Rust Dispatcher: idt::exception_handler]
   - Decodes vector (e.g. #DE, #GP, #PF, #DF)
   - Reads %cr2 if vector == 14 (#PF)
   - Prints full register dump to Serial and Screen Console
   - Halts CPU
```

---

## 3. Local APIC & Timers (`apic.rs`)

### Purpose
Provides modern, multiprocessor-ready hardware interrupt control, superseding the legacy 8259 PIC.

### Key Components
1. **8259 PIC Disabling**:
   - The legacy 8259 PIC vectors overlap with x86_64 CPU exceptions (0..31).
   - `disable_pic()` remaps PIC to vectors 32 and 40, then writes `0xFF` to `0x21` and `0xA1` to mask all legacy interrupts.
2. **Local APIC Discovery & Enable**:
   - Reads `IA32_APIC_BASE_MSR` (`0x1B`) to find the MMIO base address (typically `0xFEE00000`).
   - Sets bit 11 in MSR `0x1B` to enable the Local APIC globally.
   - Writes `0x1FF` to Spurious Interrupt Vector register (`0x0F0`) to enable software APIC operation with vector 255.
3. **Periodic APIC Timer**:
   - Configures divide register (`0x3E0`) to divide by 16.
   - Sets LVT Timer register (`0x320`) to periodic mode (`0x20000`) on vector `0x20` (Vector 32).
   - Writes initial counter value to start periodic countdown.
   - On each tick, CPU calls `timer_handler()` in `idt.rs`, which increments `TICKS` and writes `0` to APIC EOI (`0x0B0`).
