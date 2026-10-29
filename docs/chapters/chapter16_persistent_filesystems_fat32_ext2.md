# Chapter 16: Persistent Filesystems — FAT32 & ext2 On-Disk Architecture

> *"A storage drive is nothing more than a giant linear array of numbered 512-byte sectors. A filesystem is the mathematical blueprint that transforms that flat desert of bytes into a living tree of files, directories, and data."*

---

## 1. The On-Disk Reality: From Sectors to Files

In **Chapter 15**, we built the VirtIO block driver and gained the ability to read and write numbered 512-byte blocks (`read_sector` and `write_sector`).

However, human software cannot reasonably operate on sector numbers. When an application requests `/disk/notes.txt`, it has no idea:
- Which sector contains the file's bytes?
- How many sectors long is the file?
- What if the file is fragmented across non-consecutive sectors?
- Where is the filename `"notes.txt"` stored on disk?

An **On-Disk Filesystem** solves this problem by defining standardized data structures stored directly on the media. In this chapter, we explore the two most influential filesystems in computer history:
1. **FAT32 (File Allocation Table 32)**: The universal standard for USB drives, SD cards, and EFI System Partitions.
2. **ext2 (Second Extended Filesystem)**: The classic Linux on-disk filesystem foundation.

We will build full `#![no_std]` Rust drivers for both, mount them to the NullOS Virtual File System at `/disk` and `/mnt`, and achieve true persistent read/write storage!

---

## 2. FAT32 Architecture & Anatomy

FAT32 was created by Microsoft in 1996 for Windows 95 OSR2. Because of its simplicity, efficiency, and lack of licensing restrictions, it is supported by every operating system on Earth.

A FAT32 storage volume is divided into three distinct physical regions:

```
+-------------------------------------------------------------------------------+
|                             FAT32 Disk Layout                                 |
+-------------------------------------------------------------------------------+
| Sector 0: Boot Sector / BPB (BIOS Parameter Block)                            |
| Sector 1..31: Reserved Sectors (FSInfo, Backup Boot Sector)                   |
+-------------------------------------------------------------------------------+
| Sector 32..N: File Allocation Table 1 (FAT 1)                                 |
| Sector N+1..M: File Allocation Table 2 (FAT 2 - Backup Copy)                  |
+-------------------------------------------------------------------------------+
| Sector M+1..End: Data Region (Clusters 2, 3, 4, 5, ... up to Total Clusters)  |
|   - Holds File Contents and Directory Clusters                                |
+-------------------------------------------------------------------------------+
```

### 1. The BIOS Parameter Block (BPB)
The very first sector (LBA 0) contains the **BPB**—a 512-byte header describing the geometry of the volume:

| Byte Offset | Field Name | Size | Purpose in NullOS |
| :--- | :--- | :--- | :--- |
| `0x00` | `jmp_boot` | 3 bytes | x86 jump instruction to bootstrap code. |
| `0x0B` | `bytes_per_sector` | 2 bytes | Sector size (always `512` in standard media). |
| `0x0D` | `sectors_per_cluster` | 1 byte | Number of sectors in a cluster (e.g. 1, 2, 4, 8). |
| `0x0E` | `reserved_sector_count` | 2 bytes | Number of reserved sectors before FAT 1 (typically `32`). |
| `0x10` | `num_fats` | 1 byte | Number of FAT copies (always `2`). |
| `0x20` | `total_sectors_32` | 4 bytes | Total 32-bit count of sectors on the volume. |
| `0x24` | `sectors_per_fat_32` | 4 bytes | Size of each File Allocation Table in sectors. |
| `0x2C` | `root_cluster` | 4 bytes | Cluster number of the root directory (always $\ge 2$, typically `2`). |
| `0x1FE` | `boot_signature` | 2 bytes | Valid boot signature bytes: `0x55`, `0xAA`. |

### 2. Clusters: The Atomic Allocation Unit
While the disk controller reads and writes **512-byte sectors**, FAT32 allocates space in larger chunks called **Clusters** (typically 1 to 8 sectors, or 512 to 4,096 bytes).

Notice that cluster numbers start at **`2`** (Clusters 0 and 1 do not exist). To convert any cluster number $C$ to its physical starting disk sector:

$$\text{Data Start Sector} = \text{reserved\_sectors} + (\text{num\_fats} \times \text{sectors\_per\_fat})$$

$$\text{Sector}(C) = \text{Data Start Sector} + (C - 2) \times \text{sectors\_per\_cluster}$$

### 3. The File Allocation Table (FAT)
How does FAT32 know which clusters belong to a file?

The **File Allocation Table (FAT)** is a giant array of 32-bit integers. There is exactly one entry in the table for every cluster on the disk:

$$\text{FAT Offset for Cluster } C = C \times 4 \text{ bytes}$$
$$\text{FAT Sector} = \text{reserved\_sectors} + \frac{C \times 4}{512}$$
$$\text{Offset within Sector} = (C \times 4) \pmod{512}$$

Each 32-bit FAT entry contains the cluster number of the **next** cluster in the file, forming a linked list:
- `0x00000000`: Cluster is **FREE** (unallocated).
- `0x00000002`..`0x0FFFFFEF`: Number of the **next cluster** in the file's chain.
- `0x0FFFFFF8`..`0x0FFFFFFF`: **End of Cluster Chain (EOC)** (this is the final cluster of the file).
- `0x0FFFFFF7`: Bad cluster (damaged sector on disk).

```
File "NOTES.TXT" starts at Cluster 5:

  FAT Table:
  Index:   [ 2 ]   [ 3 ]   [ 4 ]   [ 5 ]   [ 6 ]   [ 7 ]   [ 8 ]
  Value:     0       0       0       6       8       0     EOC

  Follow the chain: 5 -> 6 -> 8 -> EOC (File occupies clusters 5, 6, and 8).
```

### 4. Directory Entries (8.3 Standard)
In FAT32, directories are just normal files whose data consists of a table of **32-byte records**:

```rust
#[repr(C, packed)]
pub struct RawDirEntry {
    pub name: [u8; 8],         // Filename (padded with spaces)
    pub ext: [u8; 3],          // Extension (padded with spaces)
    pub attr: u8,              // File attributes (0x10 = Subdirectory, 0x20 = Archive)
    pub reserved: u8,
    pub ctime_tenth: u8,
    pub ctime: u16,            // Creation time
    pub cdate: u16,            // Creation date
    pub adate: u16,            // Last access date
    pub cluster_high: u16,     // Upper 16 bits of starting cluster
    pub mtime: u16,            // Modification time
    pub mdate: u16,            // Modification date
    pub cluster_low: u16,      // Lower 16 bits of starting cluster
    pub file_size: u32,        // Size of file in bytes
}
```

The starting cluster is reconstructed by combining `cluster_high` and `cluster_low`:
$$\text{First Cluster} = (\text{cluster\_high} \ll 16) \mid \text{cluster\_low}$$

Special markers in the first byte of `name`:
- `0x00`: Free entry; **all subsequent entries in this directory are also free** (stop searching).
- `0xE5`: Deleted entry (skip and continue).

---

## 3. The Linux ext2 Filesystem Architecture

The **Second Extended Filesystem (ext2)** was designed by Rémy Card for Linux in 1993. Unlike FAT32, which uses linked cluster chains, ext2 uses **Inodes** and **Direct/Indirect Block Pointers**.

### The ext2 Superblock (Byte 1024)
Ext2 stores its superblock at byte offset `1024` from the start of the partition:
```rust
#[repr(C, packed)]
pub struct Ext2Superblock {
    pub s_inodes_count: u32,
    pub s_blocks_count: u32,
    pub s_r_blocks_count: u32,
    pub s_free_blocks_count: u32,
    pub s_free_inodes_count: u32,
    pub s_first_data_block: u32,
    pub s_log_block_size: u32,      // Block size = 1024 << s_log_block_size
    pub s_blocks_per_group: u32,
    pub s_inodes_per_group: u32,
    pub s_magic: u16,               // Must equal 0xEF53
    // ...
}
```

### Inodes & Direct Block Pointers
In ext2, Inode number `2` is always the root directory `/`. Each 128-byte inode contains `i_block[15]`:
- `i_block[0..11]`: **12 Direct Block Pointers** (pointing directly to data blocks).
- `i_block[12]`: **Singly Indirect Pointer** (points to a block containing more block pointers).
- `i_block[13]`: **Doubly Indirect Pointer**.
- `i_block[14]`: **Triply Indirect Pointer**.

---

## 4. Integration with the NullOS VFS

In [kernel/src/fs/mod.rs](../../kernel/src/fs/mod.rs), `mount_block_devices()` automatically connects any detected block device to our Virtual File System:

```rust
pub fn mount_block_devices() {
    if let Some(block_dev) = crate::drivers::block::primary_device() {
        // 1. Try mounting as FAT32
        if let Ok(fat32_fs) = fat32::Fat32FileSystem::new(block_dev.clone()) {
            VFS.mount("/disk", fat32_fs.clone()).unwrap();
            VFS.mount("/mnt", fat32_fs.clone()).unwrap();
            verify_fat32_operations();
            return;
        }

        // 2. Try mounting as ext2
        if let Ok(ext2_fs) = ext2::Ext2FileSystem::new(block_dev.clone()) {
            VFS.mount("/disk", ext2_fs.clone()).unwrap();
            return;
        }
    }
}
```

Because the VFS uses prefix-matching path resolution:
- Navigating to `/` accesses the root in-memory RamFS.
- Navigating to `/disk` seamlessly routes all file and directory calls to the FAT32 driver on the VirtIO block device!

---

## 5. Verification & Live Shell Walkthrough

During boot, the kernel probes the VirtIO disk, validates the FAT32 BPB, mounts `/disk`, and runs automated read/write tests:

```text
----------------------------------------
 [FS] Mounting Storage Block Devices    
----------------------------------------
[FS] Probing block device 'virtio0' for filesystems...
[FS] Detected FAT32 filesystem (36 MiB, root cluster: 2).
[FS] Successfully mounted FAT32 filesystem at /disk.
[FS] Successfully mounted FAT32 filesystem at /mnt.
[FS] Verifying FAT32 persistent read/write operations...
[FS] Read 44 bytes from /disk/README.TXT: "Welcome to NullOS Persistent FAT32 Storage!"
[FS] Read 75 bytes from /disk/NOTES.TXT: "This file was read directly from the VirtIO block device via FAT32 driver."
[FS] Wrote 49 bytes to /disk/TEST_WR.TXT on persistent storage.
[FS] FAT32 verification complete.
```

In the interactive shell:
```text
nullos> ls /disk
Listing directory: /disk
  <FILE>   Inode #3    README.TXT
  <FILE>   Inode #4    NOTES.TXT
  <FILE>   Inode #5    NVVARS
  <FILE>   Inode #9    TEST_WR.TXT

nullos> cat /disk/README.TXT
Welcome to NullOS Persistent FAT32 Storage!

nullos> write /disk/USER.TXT Hello from interactive user terminal!
Wrote 43 bytes to '/disk/USER.TXT'

nullos> cat /disk/USER.TXT
Hello from interactive user terminal!
```

---

## 6. Summary & Roadmap

We now possess:
1. Complete mathematical parsing of the **FAT32 Boot Sector / BPB**.
2. **Cluster-to-LBA** sector conversion algorithms.
3. Dynamic **FAT table cluster chain traversal and allocation**.
4. 32-byte directory entry parsing and case-insensitive filename lookups.
5. Persistent **file creation, reading, and writing** across power cycles.
6. An **ext2 read-only** driver capable of inspecting Linux partitions.

With persistent storage complete, in **Phase 7 (Userspace & Coreutils)**, we will build a minimal C runtime (`libc`) and execute standalone user programs directly from disk!
