# Chapter 12: The Virtual File System (VFS), Inodes & File Descriptors

> *"In Unix, everything is a file. If it's not a file, it's a process."*

---

## 1. What is a Virtual File System (VFS)?

Imagine an operating system that needs to support:
- An in-memory RAM disk (`ramfs` / `initramfs`)
- A FAT32 USB flash drive
- An ext4 Linux partition on an NVMe SSD
- A network file share over NFS
- Hardware devices like `/dev/console` and `/dev/random`

Without an abstraction layer, a user program would need different code for every disk type:
```c
// Horrible design without VFS:
if (is_fat32) {
    fat32_read(file, buf);
} else if (is_ext4) {
    ext4_read(file, buf);
} else if (is_ramfs) {
    ramfs_read(file, buf);
}
```

The **Virtual File System (VFS)** solves this by introducing a common, polymorphic interface. User programs and system calls interact **only** with the VFS. The VFS then delegates operations to the underlying filesystem driver via Rust traits:

```
+-------------------------------------------------------------+
|                     User Application                        |
|        read(fd, buf)     write(fd, buf)     open(path)      |
+-------------------------------------------------------------+
                              |
                              v
+-------------------------------------------------------------+
|                 Virtual File System (VFS)                   |
|        Path Resolution, Mount Table, Inode Trait            |
+-------------------------------------------------------------+
         |                     |                     |
         v                     v                     v
+-----------------+   +-----------------+   +-----------------+
|     RamFS       |   |     FAT32       |   |     ext2/4      |
|  (In-Memory)    |   |  (USB Drives)   |   |  (Hard Disks)   |
+-----------------+   +-----------------+   +-----------------+
```

---

## 2. Inodes and Dentries: The Two Halves of Every File

In operating systems terminology, files are separated into two distinct components:

### 1. Inode (Index Node)
An **Inode** represents the file's **identity, contents, and metadata**, completely divorced from its human-readable name:
- Inode number (unique ID)
- File type (Regular file, Directory, Character Device, Block Device)
- File size (in bytes)
- Permissions (e.g., `0o755` read/write/execute)
- Pointers to data blocks or memory buffers

Notice what is **missing** from an inode: **its name!** An inode does not know its own file name or which directory it lives in.

### 2. Dentry (Directory Entry)
A **Dentry** is simply a name tag attached to an inode. It lives inside a directory and pairs a human-readable string with an inode number:
```
Directory "/" entries:
  "hello.txt"   --> Inode #4
  "etc"         --> Inode #5
  "dev"         --> Inode #6
```

Because file names are decoupled from inodes, Unix allows **Hard Links**: multiple names in different directories pointing to the exact same underlying Inode!

---

## 3. The `Inode` Trait in NullOS

In `kernel/src/fs/vfs.rs`, NullOS defines the core `Inode` trait that every filesystem driver implements:

```rust
pub trait Inode: Send + Sync {
    /// Returns file metadata (size, permissions, file type).
    fn stat(&self) -> FileStat;

    /// Reads up to buf.len() bytes starting at offset.
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, FsError>;

    /// Writes buf.len() bytes starting at offset.
    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, FsError>;

    /// Looks up a child entry by name within this directory inode.
    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>, FsError>;

    /// Lists directory entries if this is a directory.
    fn read_dir(&self) -> Result<Vec<DirEntry>, FsError>;

    /// Creates a new child file or directory under this directory.
    fn create(&self, name: &str, file_type: FileType) -> Result<Arc<dyn Inode>, FsError>;
}
```

---

## 4. Path Traversal & Mount Points

How does the kernel resolve a path string like `"/etc/os-release"`?

1. **Mount Point Matching**: 
   The VFS inspects its mount table (`VFS.mounts`). It finds the longest matching prefix. In our case, `"/"` is mounted to `RamFs`.
2. **Path Canonicalization**:
   The path is cleaned: duplicate slashes (`//`), current directory segments (`.`), and parent directory segments (`..`) are normalized.
3. **Step-by-Step Traversal**:
   - Start at the filesystem's `root()` inode (`/`).
   - Call `root.lookup("etc")`. The root directory checks its children and returns the `Arc<dyn Inode>` for `etc`.
   - Ensure the returned node is actually a directory (`FileType::Directory`).
   - Call `etc.lookup("os-release")`. The `etc` directory returns the file inode.
4. Return the resolved `Inode`!

---

## 5. File Descriptors (FDs): Why Small Integers?

When a user program calls `open("/hello.txt", O_RDONLY)`, why doesn't the kernel return a raw memory pointer to the inode?

### 1. Security & Protection
In user mode (Ring 3), user code **cannot be trusted** with kernel memory pointers. If the kernel handed out pointers, a buggy or malicious program could alter kernel memory or bypass permissions.

### 2. Isolation & Offset State
Two different programs (or two threads) reading the same file simultaneously need **independent read cursors**. If Process A reads 10 bytes, it should be at byte offset 10; Process B should still be at byte offset 0.

### The File Descriptor Solution

A **File Descriptor (FD)** is simply a 0-indexed integer referencing an entry in the process's private **File Descriptor Table**:

```
Process File Descriptor Table
+----+---------------------------------------------------+
| FD | Open File Handle                                  |
+----+---------------------------------------------------+
| 0  | Stdin  --> ConsoleDevice (Read cursor: 0)         |
| 1  | Stdout --> ConsoleDevice (Serial / Screen)        |
| 2  | Stderr --> ConsoleDevice (Serial / Screen)        |
| 3  | File   --> Inode #4 ("hello.txt", Offset: 16)     |
+----+---------------------------------------------------+
```

### The Standard Streams: 0, 1, 2
Every Unix process inherits three default file descriptors when spawned:
- **`0` (Standard Input / `stdin`)**: Where user keystrokes arrive.
- **`1` (Standard Output / `stdout`)**: Where regular program text is printed.
- **`2` (Standard Error / `stderr`)**: Where error messages and diagnostics are printed.

---

## 6. The Standard File System Calls in NullOS

NullOS implements the standard POSIX-compatible system calls for file manipulation:

| Syscall | Number | Arguments | Description |
|---|---|---|---|
| `SYS_READ` | `0` | `fd`, `buf_ptr`, `count` | Reads up to `count` bytes from open file `fd` at its current offset. |
| `SYS_WRITE` | `1` | `fd`, `buf_ptr`, `count` | Writes `count` bytes to `fd` (fd 1 & 2 route to console/serial). |
| `SYS_OPEN` | `2` | `path_ptr`, `flags`, `mode` | Resolves path in VFS, creates file handle, and returns a new FD. |
| `SYS_CLOSE` | `3` | `fd` | Closes `fd` and frees the descriptor slot. |
| `SYS_STAT` | `4` | `path_ptr`, `stat_buf_ptr` | Retrieves metadata (size, type, mode) without opening the file. |
| `SYS_LSEEK` | `8` | `fd`, `offset`, `whence` | Adjusts the read/write cursor (`SEEK_SET`, `SEEK_CUR`, `SEEK_END`). |

### End-to-End Walkthrough of `sys_read`
```rust
// In kernel/src/task/syscall.rs:
let proc_table = PROCESS_TABLE.lock();
if let Some(proc) = proc_table.get(current_pid) {
    if let Ok(handle) = proc.fd_table.get(fd) {
        let slice = unsafe { core::slice::from_raw_parts_mut(buf_ptr, count) };
        handle.read(slice) // Inode::read_at() + cursor offset increment
    }
}
```

Now, whether a user process is reading from an in-memory RamFS file, reading from a future NVMe ext4 partition, or reading from a serial device, it uses the exact same `syscall` interface!
