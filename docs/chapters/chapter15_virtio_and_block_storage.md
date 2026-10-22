# Chapter 15: VirtIO & Paravirtualized Block Storage — High-Speed Cloud I/O

> *"Emulating legacy spinning metal hard drives with motors and magnetic platters inside virtual memory is wasteful. Paravirtualization abandons the physical illusion entirely, replacing hardware registers with zero-copy shared-memory ring buffers."*

---

## 1. Emulation vs. Paravirtualization: Why VirtIO Exists

When operating systems ran on physical hardware in the 1990s and 2000s, storage drives were connected over physical cables (IDE, PATA, SATA, AHCI). To read a 512-byte sector:
1. The CPU wrote sector numbers and cylinder coordinates into legacy motherboard I/O ports (`0x1F0`..`0x1F7`).
2. The hard drive controller spun a magnetic disk platter and moved a mechanical read head.
3. Once the magnetic flux was converted to voltage pulses, an interrupt fired, and the CPU copied the bytes into RAM.

When cloud computing and virtual machines (KVM, QEMU, AWS Nitro, Google Cloud Compute Engine) took over the world, early hypervisors attempted to **emulate** these legacy IDE and SATA controllers in software.

### The Full Emulation Tax
Every single register access triggered a high-overhead **VM-Exit** (where the virtual CPU pauses execution, transfers control back to the host Linux kernel/KVM hypervisor, simulates the spinning platter state machine, and resumes the guest VM). Doing hundreds of VM-exits just to read one file resulted in abysmal I/O throughput.

```
Full Hardware Emulation (Slow):
Guest OS -> OUT instruction -> [VM-EXIT to Host] -> Emulate IDE Chipset -> Emulate Platter -> [VM-ENTRY to Guest]

Paravirtualization (VirtIO) (Fast):
Guest OS writes request into Shared RAM Ring Buffer -> Single Kick -> Host DMA copies data -> Done!
```

### The Solution: VirtIO
In 2008, Rusty Russell designed **VirtIO** (Virtual I/O): an open standard for **paravirtualized device drivers**.

In a paravirtualized world, the operating system *knows* it is running inside a hypervisor. Instead of pretending there is a physical IDE cable:
1. The OS kernel allocates shared memory in RAM.
2. The OS writes read/write requests directly into a shared ring buffer called a **Virtqueue**.
3. The hypervisor reads the memory directly via ultra-fast host DMA.
4. Throughput jumps from thousands of IOPS to millions of IOPS!

---

## 2. Discovering VirtIO on the PCI Bus

VirtIO devices present themselves to the guest operating system as standard **PCI Hardware Devices**.

Using our PCI bus enumeration engine built in **Chapter 13**, we probe the PCI configuration space across all 256 buses:
- **Vendor ID**: Always `0x1AF4` (Red Hat / VirtIO standard).
- **Device ID**:
  - `0x1001`: Legacy / Transitional VirtIO Block Device.
  - `0x1042`: Modern VirtIO 1.0 Block Device.
  - `0x1000` / `0x1041`: VirtIO Network Adapter (Net).
  - `0x1050`: VirtIO GPU Display Adapter.

### BAR0: The Legacy Control Registers
On the transitional VirtIO block device, **BAR0** is an **I/O Port range** (typically `0x6000` or `0xC000`). By issuing 8-bit, 16-bit, and 32-bit `in` and `out` assembly instructions to offsets within BAR0, the kernel controls the device:

| Offset | Width | Name | Description |
| :--- | :--- | :--- | :--- |
| **`0x00`** | 32-bit | `DEVICE_FEATURES` | Read-only bitmask of features supported by the host. |
| **`0x04`** | 32-bit | `GUEST_FEATURES` | Write-only bitmask of features accepted by the guest OS. |
| **`0x08`** | 32-bit | `QUEUE_ADDRESS` | Physical Page Frame Number (`phys_addr / 4096`) of the Virtqueue. |
| **`0x0C`** | 16-bit | `QUEUE_SIZE` | Total number of descriptors in the queue (e.g. 128 or 256). |
| **`0x0E`** | 16-bit | `QUEUE_SELECT` | Queue index selector (Queue 0 for block requests). |
| **`0x10`** | 16-bit | `QUEUE_NOTIFY` | Doorbell register: writing queue index kicks the host to process requests. |
| **`0x12`** | 8-bit | `DEVICE_STATUS` | Driver lifecycle state machine (`ACKNOWLEDGE`, `DRIVER`, `DRIVER_OK`). |
| **`0x13`** | 8-bit | `ISR_STATUS` | Reading clears and acknowledges device interrupts. |
| **`0x14`** | 64-bit | `CAPACITY` | Total storage capacity in **512-byte sectors**! |

---

## 3. The Split Virtqueue Architecture

The beating heart of VirtIO is the **Virtqueue**. In the VirtIO 0.9.5 / 1.0 specification, a **Split Virtqueue** consists of three contiguous memory structures aligned to a 4096-byte boundary:

```
+-------------------------------------------------------------------------------+
|                       Split Virtqueue Memory Layout                           |
+-------------------------------------------------------------------------------+
| Page 0 (0x0000 - 0x0FFF):                                                     |
|   1. Descriptor Table (256 entries * 16 bytes = 4,096 bytes)                  |
|      [Desc 0: Header] -> [Desc 1: 512B Buffer] -> [Desc 2: Status Byte]       |
+-------------------------------------------------------------------------------+
| Page 1 (0x1000 - 0x1FFF):                                                     |
|   2. Available Ring (Flags, Index, Ring[256] array of descriptor indices)     |
|      (Guest driver writes head descriptor index here to queue requests)       |
+-------------------------------------------------------------------------------+
| Page 2 (0x2000 - 0x2FFF):                                                     |
|   3. Used Ring (Aligned to 4096 boundary per Legacy VirtIO specification)     |
|      (Hypervisor writes completed request status and advances used index)     |
+-------------------------------------------------------------------------------+
```

### 1. The Descriptor Table
The descriptor table is an array of 16-byte `VirtqDesc` structs describing scattered physical memory chunks:

```rust
#[repr(C, align(16))]
pub struct VirtqDesc {
    pub addr: u64,   // 64-bit physical memory address of buffer
    pub len: u32,    // Length of the buffer in bytes
    pub flags: u16,  // Flags (1 = NEXT, 2 = WRITE)
    pub next: u16,   // Index of the next chained descriptor if NEXT flag is set
}
```

- **`VIRTQ_DESC_F_NEXT` (`0x01`)**: Tells the host that this buffer is chained to another descriptor at index `next`.
- **`VIRTQ_DESC_F_WRITE` (`0x02`)**: Tells the host that this memory buffer is **device-writable** (the hypervisor writes data *into* guest RAM). If this bit is 0, the buffer is **device-readable** (the hypervisor reads data *from* guest RAM).

### 2. The Available Ring
The guest driver writes descriptor indices into this ring to submit work to the host:
```rust
#[repr(C)]
pub struct VirtqAvail {
    pub flags: u16,
    pub idx: u16,        // Free-running index incremented with every submission
    pub ring: [u16; 256],// Array of head descriptor indices
    pub used_event: u16,
}
```

### 3. The Used Ring
The hypervisor writes into this ring when a request finishes:
```rust
#[repr(C)]
pub struct VirtqUsed {
    pub flags: u16,
    pub idx: u16,        // Free-running index incremented by the host on completion
    pub ring: [VirtqUsedElem; 256],
    pub avail_event: u16,
}
```

---

## 4. The 3-Stage Descriptor Chain for Block I/O

Every read or write request submitted to a VirtIO block device is composed of a **3-part chained descriptor**:

```
[Descriptor 0: Request Header] 
    | addr: &VirtIoBlkReqHeader (16 bytes)
    | flags: VIRTQ_DESC_F_NEXT
    | next: 1
    v
[Descriptor 1: Data Payload]
    | addr: &buffer (512 bytes)
    | flags: VIRTQ_DESC_F_NEXT | (if read: VIRTQ_DESC_F_WRITE else: 0)
    | next: 2
    v
[Descriptor 2: Status Byte]
    | addr: &status_byte (1 byte)
    | flags: VIRTQ_DESC_F_WRITE
    | next: 0
```

1. **Descriptor 0 (Header)**: Contains the request type (`0 = READ`, `1 = WRITE`, `4 = FLUSH`) and the 64-bit **sector number** on disk. The device *reads* this header.
2. **Descriptor 1 (Data Buffer)**: Pointer to the 512-byte payload.
   - For a **Disk Read**: `flags = NEXT | WRITE` (the hypervisor copies data from host disk *into* this buffer).
   - For a **Disk Write**: `flags = NEXT` (the hypervisor reads data from this buffer and persists it to disk).
3. **Descriptor 2 (Status Byte)**: Pointer to a 1-byte status field. Marked with `flags = WRITE`. The hypervisor writes:
   - `0` (`VIRTIO_BLK_S_OK`): Success!
   - `1` (`VIRTIO_BLK_S_IOERR`): Hardware I/O error.
   - `2` (`VIRTIO_BLK_S_UNSUPP`): Unsupported command.

---

## 5. Implementation in NullOS

Let us study the complete driver in [kernel/src/drivers/virtio_blk.rs](file:///home/deepak/OPENSOURCE/NullOS/kernel/src/drivers/virtio_blk.rs).

### Probing and Initializing the VirtIO Device
```rust
pub fn probe(pci_dev: &PciDevice) -> Result<Self, BlockError> {
    let io_base = match pci_dev.bars.first() {
        Some(bar) => match bar.bar_type {
            BarType::Io { port, .. } => port,
            _ => return Err(BlockError::Unsupported),
        },
        _ => return Err(BlockError::Unsupported),
    };

    // 1. Enable Bus Mastering (DMA) and I/O Port decoding
    pci_dev.enable_bus_mastering();
    pci_dev.enable_io_space();

    unsafe {
        // 2. Reset device
        outb(io_base + VIRTIO_REG_DEVICE_STATUS, 0);

        // 3. Acknowledge and announce driver presence
        outb(io_base + VIRTIO_REG_DEVICE_STATUS, VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER);

        // 4. Select Block Request Queue (Queue 0)
        outw(io_base + VIRTIO_REG_QUEUE_SELECT, 0);
        let q_size = inw(io_base + VIRTIO_REG_QUEUE_SIZE);

        // 5. Allocate contiguous physical memory frames for the split virtqueue
        let desc_bytes = (q_size as usize) * 16;
        let avail_bytes = 4 + (q_size as usize) * 2;
        let used_offset = (desc_bytes + avail_bytes + 4095) & !4095;
        let used_bytes = 4 + (q_size as usize) * 8;
        let total_bytes = (used_offset + used_bytes + 4095) & !4095;
        let num_frames = total_bytes / PAGE_SIZE_4K as usize;

        let base_frame = memory::alloc_contiguous_frames(num_frames).ok_or(BlockError::DeviceBusy)?;
        let queue_ptr = base_frame.start_address().as_u64() as *mut u8;
        core::ptr::write_bytes(queue_ptr, 0, total_bytes);

        // 6. Inform device of Virtqueue physical address (Page Frame Number)
        let queue_pfn = (base_frame.start_address().as_u64() / 4096) as u32;
        outl(io_base + VIRTIO_REG_QUEUE_ADDRESS, queue_pfn);

        // 7. Transition device to DRIVER_OK state
        outb(io_base + VIRTIO_REG_DEVICE_STATUS, VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_DRIVER_OK);

        // 8. Read capacity (offset 0x14 in configuration space)
        let cap_low = inl(io_base + VIRTIO_REG_DEVICE_CONFIG);
        let cap_high = inl(io_base + VIRTIO_REG_DEVICE_CONFIG + 4);
        let capacity_sectors = ((cap_high as u64) << 32) | (cap_low as u64);

        Ok(Self { ... })
    }
}
```

### Performing 512-Byte Sector Transfers
```rust
fn submit_request(&self, req_type: u32, sector: u64, buffer_phys: u64, is_write: bool) -> Result<(), BlockError> {
    let mut last_used = self.last_used_idx.lock();
    let req_header = VirtIoBlkReqHeader { req_type, reserved: 0, sector };
    let status_byte: u8 = 0xFF;

    unsafe {
        // Setup 3 chained descriptors
        (*desc.add(0)) = VirtqDesc { addr: &req_header as *const _ as u64, len: 16, flags: VIRTQ_DESC_F_NEXT, next: 1 };
        (*desc.add(1)) = VirtqDesc { addr: buffer_phys, len: 512, flags: buf_flags, next: 2 };
        (*desc.add(2)) = VirtqDesc { addr: &status_byte as *const _ as u64, len: 1, flags: VIRTQ_DESC_F_WRITE, next: 0 };

        // Put Descriptor 0 index onto Available Ring
        let avail = &mut *self.avail_ring;
        let slot = (avail.idx % self.queue_size) as usize;
        avail.ring[slot] = 0;
        avail.idx = avail.idx.wrapping_add(1);

        // Doorbell kick to hypervisor
        outw(self.io_base + VIRTIO_REG_QUEUE_NOTIFY, 0);

        // Poll Used Ring for completion
        let used = &*self.used_ring;
        let expected = last_used.wrapping_add(1);
        while used.idx != expected {
            core::hint::spin_loop();
        }
        *last_used = expected;

        if status_byte == VIRTIO_BLK_S_OK { Ok(()) } else { Err(BlockError::IoError) }
    }
}
```

---

## 6. Verification & Automated Self-Tests

During kernel boot, the driver executes self-tests:
1. Detects the VirtIO block device at PCI slot `00:02.0`.
2. Reads the drive capacity: 32,768 sectors (16 MiB).
3. Writes a deterministic 512-byte mathematical pattern (`(i * 7 + 13) & 0xFF`) to sector 10.
4. Reads back sector 10 and performs a byte-by-byte assertion.

```text
----------------------------------------
 [VIRTIO-BLK] Probing VirtIO Storage    
----------------------------------------
[VIRTIO-BLK] Found VirtIO Block Device at PCI 00:02.0 (Vendor: 0x1af4, Device: 0x1001)
[VIRTIO-BLK] Initialized 'virtio0': 32768 sectors (16 MiB capacity)
[VIRTIO-BLK] Running VirtIO block verification self-tests...
[VIRTIO-BLK] Testing read/write sector on primary device 'virtio0' (32768 sectors)...
[VIRTIO-BLK] Successfully wrote 512 bytes to sector 10!
[VIRTIO-BLK] Successfully read 512 bytes from sector 10!
[VIRTIO-BLK] Sector data integrity verified: write pattern matched read pattern 100%!
[VIRTIO-BLK] All VirtIO Block self-tests passed successfully!
```

In the interactive shell:
```text
nullos> disks
NAME       SECTORS      SECTOR_SIZE  CAPACITY
------------------------------------------------------
virtio0    32768        512          16 MiB (16777216 bytes)
```

---

## 7. Summary & Next Steps

We now understand:
1. Why paravirtualization eliminates the high cost of VM-Exits during disk I/O.
2. How VirtIO devices are discovered via PCI vendor `0x1AF4`.
3. How Split Virtqueues use Descriptor Tables, Available Rings, and Used Rings to exchange requests.
4. How a 3-part descriptor chain packages header, payload, and status.
5. How the generic `BlockDevice` trait connects block drivers to the rest of the kernel.

Now that we have persistent 512-byte sector read/write access to real disk media, in **Chapter 16**, we will build on-disk **Filesystem Drivers (FAT32 & ext2)** to mount real partitions into the NullOS VFS!
