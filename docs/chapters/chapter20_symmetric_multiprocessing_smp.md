# Chapter 20: Symmetric Multiprocessing (SMP) & Multi-Core Architecture

### *From Single-Core Execution to Parallel Silicon: ACPI MADT Parsing, the 16-to-64 Bit AP Trampoline, INIT-SIPI-SIPI Bootstrapping, and Per-Core Isolation*

---

## 1. The Silicon Reality of Multi-Core Computing

For the first thirty years of personal computing, operating systems lived in a comfortable uniprocessor reality. The central processing unit executed instructions in a single, strictly sequential pipeline. Multitasking was an elegant illusion created by rapid hardware timer interrupts preempting the CPU dozens of times per second (as explored in Chapters 4 and 10). 

By the mid-2000s, semiconductor physics collided with the **Power Wall**: increasing clock frequency ($P \propto C \cdot V^2 \cdot f$) produced unsustainable thermal dissipation and quantum tunneling leakage currents. Chip architects pivoted from building faster single cores to stamping multiple identical processing cores onto the same silicon die: **Symmetric Multiprocessing (SMP)**.

```
       +-------------------------------------------------------------+
       |                     DRAM Physical Memory                    |
       +-------------------------------------------------------------+
                                      |
       +-------------------------------------------------------------+
       |                  Shared System Bus / Ring / Mesh             |
       +-------------------------------------------------------------+
               |                       |                       |
       +---------------+       +---------------+       +---------------+
       |  Core 0 (BSP) |       |  Core 1 (AP)  |       |  Core 2 (AP)  |
       | +-----------+ |       | +-----------+ |       | +-----------+ |
       | | Registers | |       | | Registers | |       | | Registers | |
       | +-----------+ |       | +-----------+ |       | +-----------+ |
       | | L1/L2 Cache| |       | | L1/L2 Cache| |       | | L1/L2 Cache| |
       | +-----------+ |       | +-----------+ |       | +-----------+ |
       | | Local APIC| |       | | Local APIC| |       | | Local APIC| |
       | +-----------+ |       | +-----------+ |       | +-----------+ |
       +---------------+       +---------------+       +---------------+
```

In a modern x86_64 SMP machine:
1. **Symmetry**: All CPU cores share identical access to system DRAM, PCI devices, and system timers via a coherent memory bus.
2. **Asymmetry at Boot**: When power is applied to the motherboard, hardware arbitration elects exactly one core to execute firmware instructions: the **Bootstrap Processor (BSP)**. All other cores—termed **Application Processors (APs)**—are placed into an uninitialized, halted architectural sleep state (Wait-For-SIPI state) waiting for the operating system to awaken them.
3. **Local APIC per Core**: Every individual core possesses its own dedicated **Local APIC (Advanced Programmable Interrupt Controller)** mapped to physical MMIO address `0xFEE0_0000`. Through its Local APIC, a core can issue **Inter-Processor Interrupts (IPIs)** to coordinate with, awaken, or interrupt other cores.

---

## 2. Hardware Topology Discovery via ACPI

Before the kernel can awaken secondary cores, it must answer fundamental questions: *How many CPU cores exist on this motherboard? What are their hardware APIC IDs? Where are the interrupt routing controllers located?*

In modern systems, this topology is discovered through **ACPI (Advanced Configuration and Power Interface)** tables provided by the UEFI firmware.

```
[ UEFI Firmware ] 
       │
       ▼
 [ RSDP Pointer ] (0x1fe53014 - "RSD PTR ")
       │
       ▼  (64-bit physical address)
 [ XSDT Table ]  (Extended System Description Table)
       │
       ├───► FACP Table (ACPI Hardware & Power Control)
       ├───► HPET Table (High Precision Event Timer)
       ├───► APIC Table (Multiple APIC Description Table - MADT)
       │
       ▼
 [ MADT ("APIC") ]
       ├── Local APIC Address: 0xFEE00000
       ├── Flags: PC-AT Compatible Dual-8259 Legacy PIC present
       │
       ├── Entry 0 (Type 0): Processor Local APIC #0 (BSP, Enabled)
       ├── Entry 1 (Type 0): Processor Local APIC #1 (AP,  Enabled)
       ├── Entry 2 (Type 0): Processor Local APIC #2 (AP,  Enabled)
       ├── Entry 3 (Type 0): Processor Local APIC #3 (AP,  Enabled)
       └── Entry 4 (Type 1): I/O APIC #0 (MMIO: 0xFEC00000, GSI: 0)
```

### 2.1 The Root System Description Pointer (RSDP)
The bootloader (or kernel) locates the **RSDP** structure, identified by the 8-byte ASCII signature `"RSD PTR "` and validated by an 8-bit checksum:
```rust
#[repr(C, packed)]
pub struct RsdpDescriptor20 {
    pub signature: [u8; 8],     // "RSD PTR "
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub revision: u8,           // 0 = ACPI 1.0 (RSDT), 2 = ACPI 2.0+ (XSDT)
    pub rsdt_address: u32,      // 32-bit physical address of RSDT
    pub length: u32,            // Total table length
    pub xsdt_address: u64,      // 64-bit physical address of XSDT
    pub ext_checksum: u8,
    pub reserved: [u8; 3],
}
```

### 2.2 Traversing the XSDT to Find the MADT
The **XSDT** contains an array of 64-bit physical pointers to other ACPI tables. We iterate through these pointers inspecting each table's 4-byte signature looking for `"APIC"`:
```rust
let entries_count = (xsdt.header.length as usize - size_of::<AcpiTableHeader>()) / 8;
for i in 0..entries_count {
    let table_phys = xsdt.table_ptrs[i];
    let header = unsafe { &*(table_phys as *const AcpiTableHeader) };
    if &header.signature == b"APIC" {
        // Found the MADT!
        parse_madt(table_phys);
    }
}
```

### 2.3 The Multiple APIC Description Table (MADT)
The **MADT** (signature `"APIC"`) contains variable-length records describing the multiprocessor topology:
- **Type 0 (Processor Local APIC)**: Corresponds to an execution core.
  - `acpi_processor_id`: OS-visible CPU ID.
  - `apic_id`: Hardware Local APIC ID (used for routing IPIs).
  - `flags`: Bit 0 indicates if the processor is enabled and ready to boot.
- **Type 1 (I/O APIC)**: Hardware chip managing external device IRQs (e.g. PCI, keyboards, storage) and routing them to Local APICs. Contains the base MMIO address (`0xFEC0_0000`) and the Global System Interrupt (GSI) base offset.

NullOS parses all MADT records into a structured `MadtInfo` structure containing verified CPU cores and I/O APICs:
```
[ACPI] Discovered 4 CPU cores and 1 I/O APIC(s):
  - CPU #0 (APIC ID: 0, Processor UID: 0, Status: Enabled [BSP])
  - CPU #1 (APIC ID: 1, Processor UID: 1, Status: Enabled [AP])
  - CPU #2 (APIC ID: 2, Processor UID: 2, Status: Enabled [AP])
  - CPU #3 (APIC ID: 3, Processor UID: 3, Status: Enabled [AP])
  - I/O APIC #0 (MMIO: 0xfec00000, GSI Base: 0)
```

---

## 3. The Multi-Core Boot Challenge: The 16-Bit Reset Vector

When an Application Processor (AP) receives an electrical reset or wakeup command, silicon backward compatibility forces it to wake up in **16-bit 8086 Real Mode**!

Even if the BSP is running in modern 64-bit Long Mode with 4-level paging, Write-Protect, and SSE enabled, an awakened AP starts execution with:
- `CS:IP` set to real-mode physical segment and offset.
- 16-bit operand and address sizes.
- Virtual memory paging disabled (`CR0.PG = 0`).
- Protection disabled (`CR0.PE = 0`).
- Control registers and MSRs cleared to zero.

To bring an AP from this archaic 1978 16-bit state into 2026 64-bit Long Mode running pure Rust, the kernel must provide a **bootstrap trampoline** in low memory (below 1 Megabyte, accessible in real mode).

---

## 4. The 16-to-64 Bit Bootstrap Trampoline

NullOS positions its AP bootstrap trampoline at physical address **`0x8000`** (which corresponds to SIPI vector `0x08`, calculated as `0x8000 >> 12`).

```
Physical Address 0x8000:
+--------------------------------------------------------------+
| 0x8000: Short Jump (0xEB) to start16                          |
| 0x8008: Mailbox Area (CR3, Stack, Entry, CPU_ID, Status)     |
| 0x8030: Trampoline GDT (Null, 32-bit Code, Data, 64-bit Code)|
| 0x8050: Trampoline GDTR Descriptor                          |
+--------------------------------------------------------------+
| start16 (16-Bit Real Mode):                                  |
|   cli; cld; zero segment registers (DS, ES, SS)              |
|   lgdt %cs:gdt32_desc                                        |
|   mov %cr0, %eax; or $1, %al; mov %eax, %cr0 (Protected Mode)|
|   ljmp $0x08, $0x8000+start32                                |
+--------------------------------------------------------------+
| start32 (32-Bit Protected Mode):                             |
|   mov $0x10, %ax; mov %ax, %ds, %es, %ss                     |
|   Enable PAE, PGE, OSFXSR, OSXMMEXCPT in CR4                 |
|   Load PML4 table address from Mailbox into CR3              |
|   Enable LME (bit 8) and NXE (bit 11) in EFER MSR            |
|   Enable Paging (PG, bit 31) and WP (bit 16) in CR0          |
|   ljmp $0x18, $0x8000+start64 (Jump to 64-bit Long Mode)     |
+--------------------------------------------------------------+
| start64 (64-Bit Long Mode):                                  |
|   Reload data selectors to 0x10                              |
|   Load per-CPU kernel stack pointer from Mailbox into RSP    |
|   Load CPU ID from Mailbox into RDI (System V ABI Arg 0)     |
|   Write 1 to mailbox_status (Signal online handshake to BSP) |
|   Load Rust entry point from Mailbox into RAX                |
|   jmp *%rax (Transfer execution to Rust ap_kernel_entry)     |
+--------------------------------------------------------------+
```

### 4.1 The Mailbox Synchronization Protocol
The BSP and AP communicate through a designated **Mailbox Area** embedded at the head of the trampoline (`0x8008`..`0x802F`):
```assembly
ap_mailbox_cr3:     .quad 0    /* PML4 physical address to load into CR3 */
ap_mailbox_stack:   .quad 0    /* 16 KiB top-of-stack for this core */
ap_mailbox_entry:   .quad 0    /* 64-bit pointer to ap_kernel_entry() */
ap_mailbox_cpu_id:  .quad 0    /* Unique core ID (0..N) */
ap_mailbox_status:  .long 0    /* Handshake flag: 0=Waiting, 1=Online */
```

### 4.2 Critical Trap: The EFER.NXE Silicon Dependency
During the development of NullOS Phase 9, an elusive triple fault was diagnosed using QEMU register tracing:
```
check_exception old: 0xffffffff new 0xe
v=0e e=000a IP=0018:00000000001a3914 RSP=0000444444446058 CR2=0000444444446068
Triple fault
```
The page fault occurred on the AP's first stack write with error code `0x000A` (`0b1010`):
- **Bit 1 = 1**: Write access violation.
- **Bit 3 = 1**: **Reserved Bit Violation** in page table entry!

**The Cause**: NullOS uses the modern `NO_EXECUTE` bit (bit 63) across its kernel heap and data pages. When the trampoline transitioned from 32-bit to 64-bit mode, it enabled Long Mode in the EFER MSR (`bts $8, %eax`), but forgot to enable No-Execute (`EFER.NXE`, bit 11). Without `EFER.NXE = 1`, the MMU treats bit 63 of all page table entries as an illegal reserved bit! The instant the AP pushed to its stack, the CPU faulted into a triple fault reset.

**The Fix**: In `trampoline.S`, both `LME` and `NXE` must be asserted in `EFER` (MSR `0xC0000080`):
```assembly
mov $0xC0000080, %ecx
rdmsr
bts $8, %eax        /* Bit 8:  Long Mode Enable (LME) */
bts $11, %eax       /* Bit 11: No-Execute Enable (NXE) */
wrmsr
```

---

## 5. The INIT-SIPI-SIPI Inter-Processor Interrupt Protocol

The standard protocol defined by Intel and AMD to awaken secondary cores uses the **Interrupt Command Register (ICR)** of the BSP's Local APIC.

The ICR is an 8-byte register split across two 32-bit MMIO offsets:
- `LAPIC_ICR_HIGH` (`0xFEE0_0310`): Bits 24..31 hold the Destination APIC ID.
- `LAPIC_ICR_LOW` (`0xFEE0_0300`): Delivery Mode, Vector, Level, and Trigger flags.

```
       BSP (Core 0)                                      AP (Core 1)
+------------------------+                        +------------------------+
| 1. Write Mailbox       |                        | Halted in              |
|    (CR3, RSP, Entry)   |                        | Wait-For-SIPI state    |
+------------------------+                        +------------------------+
            |
            ▼
| 2. Send INIT IPI       | ═══════ IPI ═════════► | Core resets internal   |
|    Delivery mode: 0b101|                        | registers, enters INIT |
+------------------------+                        +------------------------+
            |
            ▼
| 3. Wait 10 ms          |
+------------------------+
            |
            ▼
| 4. Send Startup IPI    | ═══════ IPI ═════════► | Core wakes up at       |
|    (SIPI, Vector 0x08) |                        | CS:IP = 0x0800:0x0000  |
+------------------------+                        | (Physical 0x8000)      |
            |                                     +------------------------+
            ▼                                                 │
| 5. Poll Mailbox Status |                                    ▼
|    for status == 1     |                        | Trampoline execution:  |
|                        |                        | Real -> Prot -> Long   |
|                        |                        | Sets status = 1        |
|                        | ◄════ Mailbox ════════ | Calls ap_kernel_entry  |
| 6. Handshake Complete! |                        +------------------------+
+------------------------+
```

### 5.1 Step-by-Step IPI Dispatching
1. **INIT IPI**: The BSP clears interrupts, sets the target APIC ID in ICR High, and writes `0x0000_0500` (Delivery Mode 5 = INIT, Edge-triggered, Assert) to ICR Low.
2. **Stabilization Delay**: The BSP waits 10 milliseconds for the AP hardware state to stabilize.
3. **Startup IPI (SIPI)**: The BSP writes `0x0000_0600 | 0x08` (Delivery Mode 6 = Startup, Vector `0x08`) to ICR Low. Vector `0x08` instructs the AP to begin execution at physical memory page `0x08 * 4096 = 0x8000`.
4. **Retry Loop**: If the AP does not respond within 200 microseconds, a second SIPI is dispatched per the Intel MP specification.

---

## 6. Per-CPU Kernel Stacks and State Tables

Each active CPU core must execute on its own private stack to avoid corrupting call frames and register states. NullOS maintains a global, thread-safe `CPU_TABLE`:

```rust
pub struct CpuData {
    pub cpu_id: usize,
    pub apic_id: u8,
    pub is_bsp: bool,
    pub stack_top: u64,
    pub online: AtomicBool,
}

pub static CPU_TABLE: Spinlock<Vec<CpuData>> = Spinlock::new(Vec::new());
```

During SMP initialization:
1. For each discovered core in the MADT, a **16 KiB kernel stack** is allocated.
2. The BSP records `stack_top` and initial offline status in `CPU_TABLE`.
3. When dispatching SIPI, the BSP writes that specific core's `stack_top` and `cpu_id` into the trampoline mailbox.
4. When the AP arrives in 64-bit mode, it atomically transitions `online` to `true`.

---

## 7. The Rust AP Kernel Entry Point

When an AP completes the trampoline jump, execution transfers to `ap_kernel_entry(cpu_id)` in pure `#![no_std]` Rust:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn ap_kernel_entry(cpu_id: usize) -> ! {
    // 1. Initialize CPU control registers & SSE on this core
    super::cpu::init();

    // 2. Load IDT on this core
    super::idt::init();

    // 3. Initialize Local APIC on this core (mask timer)
    super::apic::init_ap();

    // 4. Initialize Fast SYSCALL / SYSRET ABI
    super::syscall::init();

    // 5. Mark CPU as online in the global table
    {
        let table = CPU_TABLE.lock();
        if let Some(data) = table.iter().find(|d| d.cpu_id == cpu_id) {
            data.online.store(true, Ordering::SeqCst);
        }
    }

    // 6. Park AP in idle loop
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}
```

---

## 8. Multi-Core Shell Diagnostics and Automated Verification

NullOS integrates symmetric multiprocessing diagnostics directly into its interactive Unix shell `nsh`.

Users can inspect active cores using `cpus` or `smp`:
```
nullos> cpus
CPU_ID APIC_ID    ROLE       STATUS      
----------------------------------------------
0      0          BSP        Online      
1      1          AP         Online      
2      2          AP         Online      
3      3          AP         Online      
```

### Automated Self-Tests in QEMU
NullOS runs automated continuous verification in headless QEMU with `-smp 4`:
```bash
./scripts/run.sh --headless
```
The test verifies:
1. ACPI RSDP, XSDT, and MADT discovery.
2. 4 CPU cores enumerated with correct APIC IDs (`0`, `1`, `2`, `3`).
3. Trampoline installation at `0x8000`.
4. INIT-SIPI-SIPI dispatch and 64-bit handshake.
5. All 4 cores online in `CPU_TABLE`.
6. Shell command `cpus` execution and formatted tabular reporting.

---

## 9. Summary & Architectural Lessons

Symmetric Multiprocessing represents the pinnacle of bare-metal x86_64 systems programming:
1. **Hardware Transparency**: ACPI provides the authoritative contract between firmware and operating system to discover physical cores.
2. **The Trampoline Bridge**: Low-memory real-mode assembly is unavoidable when bootstrapping modern multi-core x86 silicon.
3. **Control Register Hygiene**: 64-bit execution requires careful synchronization of `CR0`, `CR3`, `CR4`, and `EFER` flags (especially `EFER.NXE`) before high-level Rust code can execute.
4. **Per-Core Isolation**: Independent stacks and atomic synchronization structures are mandatory to prevent race conditions across parallel cores.

With Phase 9 M25 complete, NullOS has evolved from a single-core kernel into a multi-core symmetric multiprocessing operating system.
