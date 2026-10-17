# Phase 2 — CPU Architecture (Milestones M4 - M6)

This phase establishes full 64-bit CPU architectural control, isolation, and exception handling for NullOS.

---

## Completed Milestones

### M4 — GDT & TSS
- [x] **Global Descriptor Table (GDT)**: Created 64-bit flat descriptors (Null, Kernel Code `0x08`, Kernel Data `0x10`, User Data `0x18`, User Code `0x20`).
- [x] **Task State Segment (TSS)**: Configured 104-byte TSS structure loaded into GDT entry `0x28`.
- [x] **Interrupt Stack Table (IST)**: Allocated dedicated 16 KiB `DOUBLE_FAULT_STACK` in `.bss` and assigned to `ist[0]` (IST1).
- [x] **Segment Reload**: Used 64-bit far return (`lretq`) to reload `%cs`, reloaded `%ds`, `%es`, `%ss`, and cleared `%fs`/`%gs`.
- [x] **TSS Activation**: Executed `ltr 0x28` to bind hardware task register.

### M5 — IDT & Exceptions
- [x] **Interrupt Descriptor Table (IDT)**: 256-entry table of 16-byte 64-bit Interrupt Gates loaded with `lidt`.
- [x] **Assembly Interrupt Stubs**: `isr0` to `isr21` defined in `interrupts.S` handling both CPU error-code and non-error-code exceptions.
- [x] **Double Fault Protection**: Bound `#DF` (vector 8) to `IST1`, protecting against kernel stack corruption.
- [x] **Exception Diagnostics**: Full register dump (`RIP`, `RSP`, `RFLAGS`, general purpose registers, `%cr2` on Page Fault).

### M6 — APIC & Timers
- [x] **Legacy PIC Disabling**: Remapped 8259 PIC to IRQ offsets 32 and 40, masked all lines with `0xFF`.
- [x] **Local APIC Discovery**: Located MMIO base via MSR `0x1B` (`IA32_APIC_BASE_MSR`), enabled APIC bit 11, and enabled software operation via Spurious Vector `0xFF`.
- [x] **Periodic APIC Timer**: Configured periodic timer on interrupt vector `0x20` (Vector 32) with a divide-by-16 prescaler.
- [x] **Interrupt Handler**: Connected `isr32` to Rust `timer_handler()` calling `apic::end_of_interrupt()`.
- [x] **Kernel Tick Counter & Sleep**: Implemented thread-safe `AtomicU64` tick counter and `apic::sleep()` delay primitive.

---

## How to Verify Phase 2

1. **Automated Headless Test**:
   ```bash
   ./scripts/run.sh --test
   ```
   Verifies:
   - Kernel GDT & TSS initialization without triple-faulting.
   - IDT loading.
   - APIC Timer tick accumulation (> 10 ticks).

2. **Interactive Graphical Run**:
   ```bash
   ./scripts/run.sh
   ```
   Both the terminal and the QEMU graphical display window should display:
   ```
   [KERNEL] GDT & TSS initialized (Kernel CS: 0x8, TSS: 0x28)
   [KERNEL] IDT initialized with 20 CPU exception handlers.
   [KERNEL] APIC initialized, 8259 PIC disabled, periodic timer active.
   ...
   [KERNEL] APIC Timer verified: 15 ticks recorded.
   [KERNEL] Kernel initialization complete. CPU idling with interrupts enabled.
   ```
