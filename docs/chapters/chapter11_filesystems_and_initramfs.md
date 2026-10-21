# Chapter 11: Filesystems & Initramfs — From Raw Bytes to Files

> *"Computers deal with numbers; humans deal with files and folders. The operating system's job is to make raw storage look like human concepts."*

---

## 1. The Core Problem: Storage Hardware vs. Human Needs

At the hardware level, storage devices (hard drives, SSDs, USB drives, or RAM) have **no idea what a "file" or "folder" is**. 

To a storage device, storage is simply a colossal, flat array of fixed-size blocks (typically 512 bytes or 4,096 bytes per sector):

```
Sector 0       Sector 1       Sector 2       Sector 3             Sector N
[ 512 bytes ]  [ 512 bytes ]  [ 512 bytes ]  [ 512 bytes ]  ...   [ 512 bytes ]
```

If you ask the hard drive: *"Give me `hello.txt`"*, the hard drive will look at you blankly. It only understands:
- `READ sector #48291`
- `WRITE sector #10294`

A **Filesystem** is an operating system data structure and software layer that translates between:
1. **Human concepts**: File names (`hello.txt`), folders (`/etc/config`), hierarchies, and file sizes.
2. **Hardware realities**: Sectors, byte offsets, and raw blocks on physical media.

---

## 2. What is an Initramfs? Why Can't We Just Read the Hard Drive at Boot?

When your computer first powers on, your operating system faces the classic **Chicken-and-Egg Problem**:
- To read files (like the kernel, drivers, or shell), the kernel needs a **hard disk driver** (SATA, NVMe, or USB).
- But the disk driver is stored as a file on that very same hard drive!
- Furthermore, modern hard drives are partitioned with complex filesystems (like ext4, Btrfs, or NTFS) that require complicated driver code to parse.

### The Solution: Initramfs (Initial RAM File System)

The bootloader (like UEFI or GRUB) already has basic firmware-level disk access. Before jumping to the kernel, the bootloader loads a small package containing essential files and drivers into **RAM** as a memory buffer. 

When the kernel starts:
1. It mounts this in-memory buffer as an **Initramfs** (Initial RAM Filesystem).
2. It can instantly open configuration files, start `/bin/init`, and load drivers—with zero hardware disk access!
3. Once the real NVMe or SATA drivers initialize, the kernel mounts the real hard drive and switches over.

---

## 3. The TAR (USTAR) Archive Format Explained from Scratch

One of the oldest, simplest, and most elegant archive formats in computing history is **TAR** (*Tape Archive*, standardized as **POSIX USTAR**).

Unlike `.zip` or `.gz`, an uncompressed TAR archive does **not** compress data. It simply glues files together sequentially, each preceded by a **512-byte Header Block**.

```
+-------------------+-------------------+-------------------+-------------------+
| 512-byte Header   | File Data Content | 512-byte Header   | File Data Content |
| for "hello.txt"   | (padded to 512B)  | for "config.ini"  | (padded to 512B)  |
+-------------------+-------------------+-------------------+-------------------+
```

### The 512-byte USTAR Header Layout

Every single file entry in a TAR archive starts with this exact 512-byte structure:

| Byte Offset | Field Name | Size (Bytes) | Description |
|---|---|---|---|
| `0..100` | `name` | 100 | File or directory path (null-terminated string) |
| `100..108` | `mode` | 8 | File permissions (octal ASCII string, e.g., `"0000644\0"`) |
| `108..116` | `uid` | 8 | User ID (octal ASCII) |
| `116..124` | `gid` | 8 | Group ID (octal ASCII) |
| `124..136` | `size` | 12 | File size in bytes (octal ASCII string!) |
| `136..148` | `mtime` | 12 | Last modification timestamp in seconds |
| `148..156` | `chksum` | 8 | Checksum of header bytes |
| `156` | `typeflag` | 1 | `'0'` for normal file, `'5'` for directory |
| `257..263` | `magic` | 6 | `"ustar\0"` POSIX magic identifier |

### Why Octal ASCII?
Early Unix systems used octal (base 8) for permissions (0755, 0644). In TAR headers, even numbers like the file size are written as human-readable ASCII text characters in base 8! For example, a 40-byte file has its size stored as the ASCII string `"00000000050 "`.

---

## 4. How NullOS Parses TAR Archives: Zero-Copy Streaming

In `kernel/src/fs/tar.rs`, NullOS implements a zero-copy TAR parser. "Zero-copy" means we do not allocate memory to copy file bytes around; we point Rust slices (`&'a [u8]`) directly into the archive memory!

```rust
pub struct TarEntry<'a> {
    pub name: &'a str,
    pub size: usize,
    pub mode: u32,
    pub entry_type: TarType,
    pub data: &'a [u8], // Direct reference into the TAR buffer!
}
```

### Parsing Loop
When stepping through the archive:
1. Read the 512-byte header at current `offset`.
2. If all 512 bytes are zeroes, the archive has ended.
3. Parse `name` from bytes `0..100`.
4. Parse `size` from octal ASCII at bytes `124..136`.
5. The file's data begins at `offset + 512` and ends at `offset + 512 + size`.
6. Advance `offset` past the data, rounding up to the next 512-byte boundary:
   ```rust
   let padding = (512 - (size % 512)) % 512;
   self.offset = offset + 512 + size + padding;
   ```

---

## 5. The In-Memory Filesystem: RamFS

Once the archive is parsed (or default nodes are initialized), NullOS stores files in memory using `RamFs` (`kernel/src/fs/initramfs.rs`):

- **`RamFile`**: Holds file contents in a thread-safe `Spinlock<Vec<u8>>`. Reading copies bytes from the vector; writing appends or modifies the vector dynamically.
- **`RamDirectory`**: Holds a map of child nodes (`Spinlock<BTreeMap<String, Arc<dyn Inode>>>`).
- **Character Device Nodes**:
  - `/dev/console`: Writes directly to the hardware serial port / VGA screen.
  - `/dev/zero`: Returns infinite zero bytes when read.
  - `/dev/null`: Silently discards anything written to it.

```
                  Root Directory (/)
                  /        |       \
               /bin      /dev      /etc
              /            |          \
          init         console     os-release
                       zero
                       null
```

---

## 6. Verification and Testing

When NullOS boots, it verifies its filesystem capabilities with automated self-tests:
1. Synthesizes a valid POSIX USTAR archive block in memory.
2. Parses the archive using `TarArchive` and asserts that the extracted file name, size, mode, and payload match with byte-for-byte fidelity.
3. Populates `/` with `RamFs` and confirms path resolution across `/dev/console`, `/etc/os-release`, and `/hello.txt`.
