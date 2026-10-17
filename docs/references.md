# NullOS References & Research Guide

This document aggregates authoritative technical specifications, manuals, OSDev guides, and deep-dive reading material to help you understand, research, and expand every component of **NullOS**.

---

## 1. Primary Hardware Specifications (The Ground Truth)

When in doubt about processor behavior, register formats, or hardware quirks, these official architecture manuals are the primary source of truth:

- **[Intel® 64 and IA-32 Architectures Software Developer's Manuals (SDM)](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html)**
  - **Volume 3A / 3B: System Programming Guide**:
    - *Chapter 3*: Protected-Mode Memory Management (GDT, Segment Selectors, Descriptors).
    - *Chapter 6*: Interrupt and Exception Handling (IDT format, error codes, IST, stack layout).
    - *Chapter 7*: Task Management (TSS layout in 64-bit mode, privilege transitions).
    - *Chapter 10*: Advanced Programmable Interrupt Controller (APIC, LAPIC registers, LVT Timer, Spurious Vector).
  - **Volume 2: Instruction Set Reference**: Full opcode semantics (`lgdt`, `lidt`, `ltr`, `lretq`, `rdmsr`, `wrmsr`, `cli`, `sti`, `hlt`).
- **[AMD64 Architecture Programmer's Manual](https://www.amd.com/en/support/tech-docs/amd64-architecture-programmers-manual-volumes-1-5)**
  - *Volume 2: System Programming*: Excellent, concise explanation of 64-bit Long Mode differences compared to legacy 32-bit protected mode.
- **[UEFI Specification (v2.10+)](https://uefi.org/specifications)**
  - *Chapter 7*: Boot Services (`GetMemoryMap`, `AllocatePool`, `ExitBootServices`).
  - *Chapter 12*: Console Support (`GraphicsOutputProtocol`, Pixel Formats, Blt operations).
  - *Chapter 13*: Protocols — Media Access (`SimpleFileSystemProtocol`, file reading).
- **[ACPI (Advanced Configuration and Power Interface) Specification](https://uefi.org/htmlspecs/ACPI_Spec_6_4_html/)**
  - Finding RSDP (Root System Description Pointer) via UEFI Configuration Tables.
  - RSDT vs XSDT table pointers.

---

## 2. OSDev Wiki Curated Articles

The [OSDev Wiki](https://wiki.osdev.org/) is the community gold standard for practical OS implementation tricks and hardware edge cases:

### Boot & UEFI
- **[UEFI Overview](https://wiki.osdev.org/UEFI)** — How UEFI organizes boot services and runtime handoff.
- **[GOP (Graphics Output Protocol)](https://wiki.osdev.org/GOP)** — Framebuffer base addresses, pitches/strides, and pixel formats.
- **[Loading ELF in a Bootloader](https://wiki.osdev.org/ELF_Tutorial)** — Parsing 64-bit ELF headers and loading `PT_LOAD` segments.

### CPU Architecture (Phase 2)
- **[Global Descriptor Table (x86_64)](https://wiki.osdev.org/Global_Descriptor_Table)** — Long mode segment flags, conforming code, DPL rings.
- **[Task State Segment (x86_64)](https://wiki.osdev.org/Task_State_Segment)** — Privilege level stack tables (`rsp0`) and Interrupt Stack Table (`ist1..7`).
- **[Interrupt Descriptor Table](https://wiki.osdev.org/Interrupt_Descriptor_Table)** — 64-bit gate descriptors and attributes.
- **[Exceptions](https://wiki.osdev.org/Exceptions)** — Comprehensive table of CPU exceptions (0 to 31), which push an error code, and faulting behavior.
- **[Double Fault](https://wiki.osdev.org/Double_Fault)** — Why IST stacks are mandatory to prevent Triple Fault crash reboots.
- **[8259 PIC (Programmable Interrupt Controller)](https://wiki.osdev.org/8259_PIC)** — Remapping and masking the legacy chips.
- **[APIC](https://wiki.osdev.org/APIC)** & **[APIC Timer](https://wiki.osdev.org/APIC_timer)** — Setting up the Local APIC, MMIO registers, and timer calibration.

### Upcoming Phase 3: Memory Management
- **[Page Frame Allocation](https://wiki.osdev.org/Page_Frame_Allocation)** — Physical bitmap allocators vs free-list buddies.
- **[Paging (x86_64)](https://wiki.osdev.org/Paging)** — 4-level paging: PML4 -> PDPT -> PD -> PT (4 KiB, 2 MiB, 1 GiB pages).
- **[Higher Half x86_64 Bare Bones](https://wiki.osdev.org/Higher_Half_x86_64_Bare_Bones)** — Mapping kernel space to virtual `0xFFFF800000000000+`.

---

## 3. Recommended Books & Reference Textbooks

1. **Operating Systems: Three Easy Pieces (OSTEP)** — Remzi & Andrea Arpaci-Dusseau
   - *Free online at [ostep.org](https://pages.cs.wisc.edu/~remzi/OSTEP/)*
   - Focus areas: Virtualization (CPU virtualization, Address translation, Paging, TLBs) and Concurrency (Semaphores, Locks, Interrupts).
2. **Writing an OS in Rust** — Philipp Oppermann
   - *Free online at [os.phil-opp.com](https://os.phil-opp.com/)*
   - Excellent reference for modern Rust idioms in bare-metal systems, `no_std` environments, and x86_64 abstractions.
3. **The Design of the UNIX Operating System** — Maurice J. Bach
   - Foundational book for understanding VFS, inode structures, buffer caches, and process lifecycle.

---

## 4. Hardware Reference Cheat Sheets

### x86_64 CPU Exception Vectors Reference
| Vector | Mnemonic | Description | Type | Error Code? |
|---|---|---|---|---|
| `0` | `#DE` | Divide-by-Zero Error | Fault | No |
| `1` | `#DB` | Debug Exception | Fault/Trap | No |
| `2` | `NMI` | Non-Maskable Interrupt | Interrupt | No |
| `3` | `#BP` | Breakpoint (`int3`) | Trap | No |
| `4` | `#OF` | Overflow (`into`) | Trap | No |
| `5` | `#BR` | Bound Range Exceeded (`bound`) | Fault | No |
| `6` | `#UD` | Invalid Opcode (Undefined Opcode) | Fault | No |
| `7` | `#NM` | Device Not Available (No Math Coprocessor) | Fault | No |
| `8` | `#DF` | **Double Fault** *(Handled on IST1)* | Abort | Yes (0) |
| `10` | `#TS` | Invalid TSS | Fault | Yes |
| `11` | `#NP` | Segment Not Present | Fault | Yes |
| `12` | `#SS` | Stack-Segment Fault | Fault | Yes |
| `13` | `#GP` | **General Protection Fault** | Fault | Yes |
| `14` | `#PF` | **Page Fault** *(Faulting addr in `%cr2`)* | Fault | Yes |
| `16` | `#MF` | x87 FPU Floating-Point Error | Fault | No |
| `17` | `#AC` | Alignment Check | Fault | Yes (0) |
| `18` | `#MC` | Machine Check | Abort | No |
| `19` | `#XM` | SIMD Floating-Point Exception | Fault | No |
| `20` | `#VE` | Virtualization Exception | Fault | No |
| `21` | `#CP` | Control Protection Exception | Fault | Yes |

### Local APIC MMIO Register Offsets (Base `0xFEE00000`)
| Offset | Name | Description |
|---|---|---|
| `0x020` | LAPIC ID | Local APIC ID |
| `0x030` | LAPIC Version | Version and Max LVT entries |
| `0x080` | Task Priority (TPR) | Priority filter for incoming interrupts |
| `0x0B0` | EOI Register | Write `0` to signal End-of-Interrupt |
| `0x0F0` | Spurious Interrupt Vector | Bit 8 enables APIC; bits 7:0 set spurious vector (`0xFF`) |
| `0x300` | ICR (Low) | Interrupt Command Register (send IPIs) |
| `0x320` | LVT Timer | Timer configuration (Periodic bit 17, Vector bits 7:0) |
| `0x380` | Timer Initial Count | Starting counter for countdown timer |
| `0x390` | Timer Current Count | Current value of ticking countdown timer |
| `0x3E0` | Timer Divide Config | Clock divider prescaler (Divide by 2, 4, 8, 16...) |

---

## 5. Helpful Debugging Tips

### Running with GDB
In one terminal:
```bash
# Launch QEMU paused waiting for GDB on :1234
qemu-system-x86_64 -machine q35 -cpu qemu64 -m 512M -net none -bios /usr/share/edk2/x64/OVMF.4m.fd -drive format=raw,file=build/disk.img -serial stdio -s -S
```
In another terminal:
```bash
gdb target/x86_64-unknown-none/debug/kernel
(gdb) target remote :1234
(gdb) break kernel_main
(gdb) c
```

### Inspecting CPU Tables in QEMU Console
Inside the QEMU graphical window, press **`Ctrl + Alt + 2`** to open the QEMU monitor:
- `info registers`: Dumps all GPRs (`RAX`..`R15`), control registers (`CR0`, `CR2`, `CR3`, `CR4`), segments (`CS`, `DS`, `SS`), and descriptor table registers (`GDT`, `IDT`, `TR`).
- `info pic`: Inspects legacy PIC interrupt request (IRR) and in-service (ISR) registers.
- `info tlb`: Displays active virtual-to-physical memory translations cached in the TLB.
- Press **`Ctrl + Alt + 1`** to switch back to the graphical display.
