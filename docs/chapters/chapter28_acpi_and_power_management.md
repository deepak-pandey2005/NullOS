# Chapter 28: ACPI Table Architecture & System Power Management
### *ACPI 2.0+ Specification, RSDP, XSDT/RSDT Table Hierarchies, FADT Register Blocks, DSDT AML Parsing, S5 Sleep States, and Hardware Reset Primitives*

---

## 1. Evolution of System Configuration: From PnP BIOS & APM to ACPI

In the early personal computer era, operating systems had no unified mechanism to discover system topology or control hardware power consumption. Configuration relied on fragmented legacy interfaces:

1. **PC/AT Fixed Ports & BIOS Data Area (BDA)**: Fixed I/O addresses and legacy BIOS interrupts (`INT 15h`, `INT 10h`) hardcoded motherboard layout.
2. **Advanced Power Management (APM)**: Introduced in 1992, APM delegated power control to the motherboard BIOS through Real Mode or 16-bit Protected Mode calls. When the OS was idle, it called the BIOS, which decided whether to throttle the CPU or enter standby. The OS had zero visibility into why or how power transitions occurred, leading to frequent device driver deadlocks.
3. **Plug and Play (PnP) BIOS**: An early attempt at dynamic resource allocation that suffered from race conditions between BIOS device assignment and OS peripheral initialization.

To solve these architectural deficiencies, Intel, Microsoft, and Toshiba published the **Advanced Configuration and Power Interface (ACPI)** specification in 1996. ACPI fundamentally shifted control of hardware configuration and power management from proprietary firmware to the operating system (**OS-Directed Power Management - OSPM**).

```
┌────────────────────────────────────────────────────────────────────────┐
│                   NullOS ACPI Subsystem Architecture                   │
├────────────────────────────────────────────────────────────────────────┤
│                                                                        │
│   ┌────────────────────────────────────────────────────────────────┐   │
│   │       Power Management API & Shell Commands (poweroff, reboot) │   │
│   └───────────────────────────────▲────────────────────────────────┘   │
│                                   │                                    │
│   ┌───────────────────────────────┴────────────────────────────────┐   │
│   │    Power Primitives: ACPI S5 Sleep State, 8042 Pulse, PCI CF9  │   │
│   └───────────────────────────────▲────────────────────────────────┘   │
│                                   │                                    │
│   ┌───────────────────────────────┴────────────────────────────────┐   │
│   │    DSDT & AML Parser: \_S5 Package Resolution (SLP_TYPa/b)     │   │
│   └───────────────────────────────▲────────────────────────────────┘   │
│                                   │                                    │
│   ┌───────────────────────────────┴────────────────────────────────┐   │
│   │    FADT ("FACP"): PM1a/b Blocks, SMI_CMD, ACPI_ENABLE, RESET   │   │
│   └───────────────────────────────▲────────────────────────────────┘   │
│                                   │                                    │
│   ┌───────────────────────────────┴────────────────────────────────┐   │
│   │    Root Pointer & Descriptor Tables: RSDP -> XSDT (64-bit)     │   │
│   │            (MADT, FADT, HPET, MCFG, BGRT Catalog)              │   │
│   └────────────────────────────────────────────────────────────────┘   │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 2. ACPI Table Hierarchy: From RSDP to System Description Tables

ACPI organizes hardware descriptions as an interconnected tree of data and bytecode tables residing in physical RAM.

### 2.1 The Root System Description Pointer (RSDP)

The **RSDP** is the entry point to the entire ACPI hierarchy. On modern UEFI systems, the 64-bit physical address of the RSDP is discovered directly through the UEFI System Table (`EFI_CONFIGURATION_TABLE`) and passed to the kernel within the `BootInfo` contract.

```
ACPI 1.0 RSDP Descriptor (20 Bytes):
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                     Signature ("RSD PTR ")                    |
|                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|    Checksum   |               OEM ID (6 Bytes)                |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|    OEM ID     |    Revision   |         RSDT Address          |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|        RSDT Address (cont.)   |

ACPI 2.0+ Extended RSDP Descriptor (36 Bytes):
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                         Length (36)                           |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                     64-bit XSDT Physical Address              |
|                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
| Ext. Checksum |                  Reserved                     |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

### 2.2 Standard ACPI Table Header

Every System Description Table begins with a standard 36-byte header:

```rust
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct AcpiTableHeader {
    pub signature: [u8; 4],       // E.g., "XSDT", "FACP", "APIC", "HPET"
    pub length: u32,              // Total length of table including header
    pub revision: u8,             // Specification revision
    pub checksum: u8,             // Entire table byte-sum modulo 256 must equal 0
    pub oem_id: [u8; 6],          // Manufacturer ID (e.g., "BOCHS ")
    pub oem_table_id: [u8; 8],    // Model/Board ID
    pub oem_revision: u32,
    pub creator_id: [u8; 4],      // Compiler / ASL generator ID
    pub creator_revision: u32,
}
```

### 2.3 RSDT vs. XSDT

- **RSDT (Root System Description Table)**: ACPI 1.0 standard containing an array of 32-bit physical pointers to other description tables.
- **XSDT (Extended System Description Table)**: ACPI 2.0+ standard replacing the 32-bit pointers with 64-bit physical addresses, allowing ACPI tables to reside above the 4 GiB physical address boundary.

---

## 3. Fixed ACPI Description Table (FADT / "FACP")

The **FADT** defines fixed hardware register blocks for power management, sleep state transitions, and processor idle states:

```
┌──────────────────────────────────────────────────────────────┐
│             FADT ("FACP") Register Geometry                  │
├──────────────────────────────────────────────────────────────┤
│ Offset 0x24: 32-bit DSDT Physical Address                    │
│ Offset 0x30: 32-bit SMI_CMD I/O Port                         │
│ Offset 0x34: 8-bit  ACPI_ENABLE Command Value                │
│ Offset 0x35: 8-bit  ACPI_DISABLE Command Value               │
│ Offset 0x40: 32-bit PM1a_CNT_BLK I/O Base Port               │
│ Offset 0x44: 32-bit PM1b_CNT_BLK I/O Base Port               │
│ Offset 0x58: 8-bit  PM1_CNT_LEN (typically 2 bytes)          │
│ Offset 0x70: 32-bit FLAGS (Bit 10 = RESET_REG_SUP)           │
│ Offset 0x74: 12-byte RESET_REG (Generic Address Structure)   │
│ Offset 0x80: 8-bit  RESET_VALUE                              │
│ Offset 0x8C: 64-bit X_DSDT (Extended 64-bit physical address)│
│ Offset 0x98: 12-byte X_PM1a_CNT_BLK Generic Address          │
│ Offset 0xA4: 12-byte X_PM1b_CNT_BLK Generic Address          │
└──────────────────────────────────────────────────────────────┘
```

### 3.1 Enabling ACPI Hardware Control

On systems where firmware boots in legacy mode, the operating system must assert control over the power management registers:
1. Inspect bit 0 (`SCI_EN`) of the `PM1a_CNT_BLK` register.
2. If `SCI_EN == 0`, write `fadt.acpi_enable` to the `fadt.smi_cmd` port.
3. Spin-wait until `(inw(pm1a_cnt_blk) & 1) == 1`, acknowledging that the System Management Interrupt (SMI) handler has handed power control to the OS.

---

## 4. Differentiated System Description Table (DSDT) & AML Bytecode

Unlike tabular records (such as MADT or HPET), the **DSDT** contains byte-compiled **ACPI Machine Language (AML)**. AML is an interpreted, platform-independent bytecode evaluated by an in-kernel virtual machine.

### 4.1 The `\_S5` Object (Soft Off)

The sleep state 5 (`S5`) represents full software-controlled shutdown:
- In AML source code:
```asl
Name (\_S5, Package () {
    0x00, // SLP_TYPa for PM1a
    0x00, // SLP_TYPb for PM1b
    0x00, // Reserved
    0x00  // Reserved
})
```
- In compiled AML bytecode:
```
[0x08] ("NameOp")
  [0x5C] [0x5F] [0x53] [0x35] [0x5F] ("\_S5_")
    [0x12] ("PackageOp")
      [PkgLength]
      [NumElements = 0x02 or 0x04]
        [BytePrefix 0x0A] [SLP_TYPa Value]
        [BytePrefix 0x0A] [SLP_TYPb Value]
```

By scanning the DSDT for the `_S5_` token and parsing the subsequent `PackageOp`, the kernel extracts the precise bit values for `SLP_TYPa` and `SLP_TYPb`.

---

## 5. Hardware Power Primitives: Shutdown & Reboot

### 5.1 System Shutdown (`poweroff`)

Once `SLP_TYPa` is determined:
1. Construct the 16-bit PM1 Control word:
   $$\text{PM1\_CNT} = (\text{SLP\_TYPa} \ll 10) \mid (1 \ll 13)$$
   where bit 13 is `SLP_EN` (Sleep Enable).
2. Write the word to `PM1a_CNT_BLK` (and `PM1b_CNT_BLK` if non-zero).
3. If hardware does not turn off immediately (e.g. legacy QEMU or Bochs emulation), execute emulator fallback I/O writes:
   - Port `0x604`: `outw(0x604, 0x2000)` (QEMU default poweroff)
   - Port `0xB004`: `outw(0xB004, 0x2000)` (Bochs poweroff)
4. Execute final `cli; hlt` loop.

### 5.2 System Reboot (`reboot`)

To guarantee a clean hardware restart across diverse motherboards, the kernel employs a cascading 4-stage reboot strategy:

```
┌────────────────────────────────────────────────────────┐
│               Cascading Reboot Sequence                │
├────────────────────────────────────────────────────────┤
│ Stage 1: ACPI 2.0+ FADT RESET_REG (if supported)       │
│          outb(reset_reg.address, fadt.reset_value)     │
│                       │ (failed / unsupported)         │
│                       ▼                                │
│ Stage 2: 8042 Keyboard Controller Pulse Line           │
│          outb(0x64, 0xFE)                              │
│                       │ (failed)                       │
│                       ▼                                │
│ Stage 3: PCI Chipset Reset Port 0xCF9                  │
│          outb(0xCF9, 0x06) / outb(0xCF9, 0x0E)         │
│                       │ (failed)                       │
│                       ▼                                │
│ Stage 4: CPU Hardware Triple Fault                     │
│          lidt [0; 0]; int3 (Forces silicon reset)      │
└────────────────────────────────────────────────────────┘
```

---

## 6. Implementation Summary in NullOS

The ACPI subsystem is located in `kernel/src/arch/x86_64/acpi/`:
- `tables.rs`: Generic ACPI table discovery, checksum validation, and RSDP/XSDT parser.
- `fadt.rs`: Fixed ACPI Description Table parser, PM1a/b port extraction, and ACPI mode activation.
- `dsdt.rs`: DSDT mapping and AML bytecode `\_S5` scanner.
- `madt.rs`: Multiple APIC Description Table parser (CPUs and I/O APICs).
- `power.rs`: System shutdown, reboot, and emulator power controls.
- `mod.rs`: Centralized ACPI API, system diagnostics, and automated Test Phase 25.

