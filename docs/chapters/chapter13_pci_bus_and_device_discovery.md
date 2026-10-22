# Chapter 13: The PCI Bus, Hardware Discovery & Base Address Registers (BARs)

> *"Before the operating system can drive a device, it must find it. The PCI bus is the OS's telephone directory for the motherboard."*

---

## 1. The Core Problem: How Does the OS Know What Hardware Exists?

When your computer boots, the CPU has no innate knowledge of what expansion cards or peripherals are plugged into the motherboard:
- Is there a GPU in the PCIe slot?
- Is there an NVMe SSD or SATA hard drive?
- Is there a Realtek or Intel Gigabit Ethernet card?
- Where in physical memory are their control registers located?

If an operating system had to hardcode memory addresses for every graphics card and network card ever manufactured, it would be impossible.

### The Solution: Plug and Play via the PCI Standard
The **Peripheral Component Interconnect (PCI)** standard solved this. Every PCI and PCI Express (PCIe) device on modern motherboards implements a standardized **Configuration Space**. The operating system can query this configuration space using standard CPU instructions to discover:
1. Who manufactured the device? (Vendor ID, e.g., `0x8086` for Intel, `0x10DE` for NVIDIA).
2. What model is it? (Device ID).
3. What does it do? (Class code: GPU, Sound Card, Network, Storage).
4. Where should the OS communicate with it? (**Base Address Registers / BARs**).

---

## 2. Accessing PCI Configuration Space: Ports `0xCF8` and `0xCFC`

On x86_64 architectures, accessing the PCI configuration space does not require memory mapping; it uses two standardized 32-bit I/O ports:

| Port | Name | Function |
|---|---|---|
| `0xCF8` | `CONFIG_ADDRESS` | Specifies the target Bus, Device, Function, and Register offset. |
| `0xCFC` | `CONFIG_DATA` | Reads or writes the 32-bit DWORD selected by `CONFIG_ADDRESS`. |

### The `CONFIG_ADDRESS` 32-Bit Bitfield Format

To tell the motherboard which chip and register we want to read, we assemble a 32-bit integer:

```
 31   30      24 23      16 15     11 10      8 7       2 1   0
+----+----------+----------+---------+---------+---------+-----+
| En | Reserved |  Bus     | Device  | Function| Register|  00 |
| (1)|  (7 bits)| (8 bits) | (5 bits)| (3 bits)| (6 bits)|(2b) |
+----+----------+----------+---------+---------+---------+-----+
```

- **Bit 31 (`Enable Bit`)**: Must always be `1`. If `0`, the bus ignores the request.
- **Bits `23:16` (`Bus Number`)**: `0` to `255` (Bus 0 is usually the root motherboard bus).
- **Bits `15:11` (`Device / Slot Number`)**: `0` to `31` (which physical slot on that bus).
- **Bits `10:8` (`Function Number`)**: `0` to `7` (multi-function devices, e.g., a GPU with an HDMI Audio function).
- **Bits `7:2` (`Register Offset`)**: `0` to `63` (which 32-bit register inside the device's 256-byte header).
- **Bits `1:0`**: Always `00` (register reads must be 4-byte aligned).

### The Rust Implementation in NullOS:
```rust
fn config_address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    0x8000_0000u32
        | ((bus as u32) << 16)
        | (((device & 0x1F) as u32) << 11)
        | (((function & 0x07) as u32) << 8)
        | ((offset & 0xFC) as u32)
}

pub fn pci_read_u32(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let address = config_address(bus, device, function, offset);
    unsafe {
        // 1. Tell motherboard where to look:
        core::arch::asm!("out dx, eax", in("dx") 0xCF8u16, in("eax") address);
        // 2. Read the 32-bit value:
        let value: u32;
        core::arch::asm!("in eax, dx", in("dx") 0xCFCu16, out("eax") value);
        value
    }
}
```

---

## 3. The PCI Configuration Header (Type 0x00)

Every single PCI function presents a standardized 64-byte header:

```
+-----------------------------------+-----------------------------------+
|       Device ID (16 bits)         |        Vendor ID (16 bits)        | 0x00
+-----------------------------------+-----------------------------------+
|         Status (16 bits)          |        Command (16 bits)          | 0x04
+-----------------+-----------------+-----------------+-----------------+
| Class (8 bits)  | Subclass (8 b)  |  Prog IF (8 b)  | Revision (8 b)  | 0x08
+-----------------+-----------------+-----------------+-----------------+
|      BIST       |   Header Type   |  Latency Timer  | Cache Line Size | 0x0C
+-----------------+-----------------+-----------------+-----------------+
|                     Base Address Register 0 (BAR0)                    | 0x10
+-----------------------------------------------------------------------+
|                     Base Address Register 1 (BAR1)                    | 0x14
+-----------------------------------------------------------------------+
|                     Base Address Register 2 (BAR2)                    | 0x18
+-----------------------------------------------------------------------+
|                     Base Address Register 3 (BAR3)                    | 0x1C
+-----------------------------------------------------------------------+
|                     Base Address Register 4 (BAR4)                    | 0x20
+-----------------------------------------------------------------------+
|                     Base Address Register 5 (BAR5)                    | 0x24
+-----------------------------------------------------------------------+
|                    CardBus CIS Pointer (32 bits)                      | 0x28
+-----------------------------------+-----------------------------------+
|       Subsystem ID (16 bits)      |    Subsystem Vendor ID (16 bits)  | 0x2C
+-----------------------------------+-----------------------------------+
|                   Expansion ROM Base Address (32 bits)                | 0x30
+-----------------------------------------------------+-----------------+
|                      Reserved                       | Capabilities Ptr| 0x34
+-----------------------------------------------------+-----------------+
|                      Reserved (32 bits)                               | 0x38
+-----------------+-----------------+-----------------+-----------------+
|    Max Latency  |    Min Grant    |  Interrupt PIN  | Interrupt LINE  | 0x3C
+-----------------+-----------------+-----------------+-----------------+
```

### The Magic "Empty Slot" Marker: `0xFFFF`
If an operating system reads offset `0x00` (Vendor ID) and receives `0xFFFF`, **no device exists** in that slot! The OS immediately skips to the next device without wasting CPU cycles.

---

## 4. Base Address Registers (BARs): How Hardware Gets Mapped

This is the most critical concept in hardware driver engineering. A device has internal RAM or control registers. How does the CPU read and write to them?

The PCI header provides **6 Base Address Registers (BAR0 through BAR5)**. A BAR can be either:
1. **Memory-Mapped I/O (MMIO)**: The device registers appear at physical RAM addresses. Reading and writing is done via standard memory pointers!
2. **Port I/O**: The device registers are accessed via x86 `in` and `out` assembly instructions.

### The BAR Bit Layout
To check what kind of BAR it is, look at **Bit 0**:
- If `Bit 0 == 0`: **Memory Space (MMIO)**
  - `Bits 2:1`: Type (`00` = 32-bit address, `10` = 64-bit address).
  - `Bit 3`: Prefetchable (safe to read ahead like RAM, vs control registers that trigger side effects).
  - `Bits 31:4` (or `63:4`): The physical memory address!
- If `Bit 0 == 1`: **I/O Port Space**
  - `Bits 31:2`: The x86 I/O port number (e.g., `0x6040`).

### The BAR Sizing Algorithm: How Big is the Device's Memory?
How does the OS know how much RAM a device needs (e.g. 16 MiB for a GPU vs 4 KiB for an Ethernet card)?

You cannot read the size directly! Instead, the PCI specification defines a clever hardware probing trick:
1. Save the original value of the BAR.
2. Write **all 1s (`0xFFFFFFFF`)** to the BAR register.
3. The hardware device internally masks off all bits it cannot use and keeps only the high bits!
4. Read back the value from the BAR.
5. Invert the unmasked bits and add 1: `size = !(read_back & mask) + 1`.
6. Restore the original value of the BAR.

---

## 5. Device Control: The Command Register

By default, firmware leaves many device features **disabled** for safety. Before a driver can communicate with a device, the kernel must configure the **Command Register (Offset 0x04)**:
- **Bit 0 (`I/O Space Enable`)**: Allows the device to respond to Port I/O accesses.
- **Bit 1 (`Memory Space Enable`)**: Allows the device to respond to MMIO accesses.
- **Bit 2 (`Bus Master Enable`)**: Crucial! Allows the device to initiate **DMA (Direct Memory Access)** transfers into system RAM on its own.

---

## 6. What NullOS Discovered

When NullOS initializes the PCI bus in `drivers/pci::init()`, it enumerates the hardware:

```text
[PCI] Discovered 5 PCI hardware devices:
  [00:00.0] 8086:29c0 | Host Bridge (Intel Corporation)
  [00:01.0] 1234:1111 | VGA Compatible Controller (QEMU Virtual Hardware)
    BAR0: MMIO32 0x80000000 (size: 16384 KiB, prefetch: true)
    BAR2: MMIO32 0x81011000 (size: 4 KiB, prefetch: false)
  [00:1f.0] 8086:2918 | ISA Bridge (Intel Corporation)
  [00:1f.2] 8086:2922 | SATA/AHCI Controller (Intel Corporation)
    BAR4: I/O Port 0x6040 (size: 32 bytes)
    BAR5: MMIO32 0x81010000 (size: 4 KiB, prefetch: false)
    Caps: MSI (Message Signaled Interrupts), SATA Data Index
  [00:1f.3] 8086:2930 | SMBus Controller (Intel Corporation)
    BAR4: I/O Port 0x6000 (size: 64 bytes)
```

And in the interactive shell:
```text
nullos> lspci
SLOT     ID         CLASS                            VENDOR
--------------------------------------------------------------------
00:00.0 8086:29c0  Host Bridge                      Intel Corporation
00:01.0 1234:1111  VGA Compatible Controller        QEMU Virtual Hardware
00:1f.0 8086:2918  ISA Bridge                       Intel Corporation
00:1f.2 8086:2922  SATA/AHCI Controller             Intel Corporation
00:1f.3 8086:2930  SMBus Controller                 Intel Corporation
```

With the PCI subsystem complete, NullOS is now ready to discover real storage controllers like **VirtIO Block Devices** and **AHCI SATA Drives** in Phase 6 / M18!
