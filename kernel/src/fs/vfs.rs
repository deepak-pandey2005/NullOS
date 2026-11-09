//! # Virtual File System (VFS) Abstraction (Phase 6 - M16 VFS)
//!
//! This module defines the **kernel's unified filesystem interface**. It sits
//! between the system-call layer and concrete filesystem drivers (RamFS, FAT32,
//! ext2) and provides a single coherent API for every filesystem operation.
//!
//! ## Design: Trait-based Polymorphism
//!
//! NullOS uses Rust **trait objects** (`Arc<dyn Inode>`, `Arc<dyn FileSystem>`)
//! to achieve the same kind of polymorphism that Linux achieves with vtable
//! pointers embedded in C `inode` and `super_block` structs.  At runtime the
//! compiler generates a vtable for each concrete type (`RamFile`, `RamDirectory`,
//! `Fat32Inode`, `Ext2Inode`, …) and stores a fat pointer (`(data_ptr, vtable_ptr)`)
//! inside the `Arc<dyn Inode>`.  Calling any method on the trait object dispatches
//! through that vtable – exactly one pointer indirection, comparable to a C
//! virtual function call.
//!
//! ## Inode Abstraction
//!
//! An **inode** (index node) is the fundamental unit of VFS metadata. Every file,
//! directory, device node, pipe, or socket is represented as an inode.  Inodes
//! carry:
//! * A unique integer identifier (`inode_num`) within their filesystem.
//! * Type information (`FileType`).
//! * Size and permission mode (`FileStat`).
//! * Methods to read/write data, enumerate children (for directories), create
//!   children, and poll for readiness.
//!
//! The [`Inode`] trait is `Send + Sync`, which means `Arc<dyn Inode>` is safe to
//! share between threads (or in NullOS's case, between interrupt handlers and the
//! main kernel loop).  Implementations must protect all mutable state with
//! `Spinlock` or similar.
//!
//! ## Mount Table
//!
//! The [`Vfs`] struct holds a `BTreeMap<String, Arc<dyn FileSystem>>` (the
//! **mount table**), where each key is a canonical absolute path (the mount point)
//! and the value is a reference-counted filesystem instance.  The map is ordered
//! alphabetically, which makes it easy to implement **longest-prefix matching**:
//! when resolving `/disk/README.TXT`, the VFS iterates all entries, finds both
//! `"/"` and `"/disk"` as candidates, and picks `"/disk"` because it is longer.
//!
//! ## Path Resolution (3-step algorithm)
//!
//! [`Vfs::resolve_path`] implements a simplified version of Linux's `namei`:
//!
//! 1. **Mount lookup** – scan the mount table for the longest prefix of the
//!    cleaned absolute path.  Return `NotFound` if nothing matches.
//! 2. **Relative path** – strip the mount-point prefix from the full path to get
//!    the path relative to the filesystem's root inode.  If the result is empty,
//!    return the root inode directly.
//! 3. **Segment walk** – split the relative path on `'/'` and call
//!    `current_node.lookup(segment)` for each component, threading the resulting
//!    inode forward.  If any intermediate node is not a directory, return
//!    `NotADirectory`.
//!
//! ## Poll / Event Loop Integration
//!
//! The [`POLLIN`] / [`POLLOUT`] / … constants mirror the Linux `poll(2)` event
//! flags.  [`Inode::poll`] returns a bitmask of currently asserted events.
//! [`FileDescriptor::read`](super::fd::FileDescriptor::read) checks `POLLIN`
//! before blocking in non-blocking mode; similarly for writes and `POLLOUT`.
//!
//! Provides an OS-level unified abstraction for filesystems, inodes, directory traversal,
//! mount points, and file access.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::sync::Spinlock;

/// VFS error codes conforming to standard POSIX-like error conditions.
///
/// Each variant maps to a standard POSIX `errno` value.  The mapping is:
///
/// | Variant | POSIX errno | Meaning |
/// |---------|-------------|---------|
/// | `NotFound`         | `ENOENT` (2)   | No such file or directory |
/// | `PermissionDenied` | `EACCES` (13)  | Permission check failed |
/// | `AlreadyExists`    | `EEXIST` (17)  | File/dir already exists (O_EXCL or mkdir) |
/// | `NotADirectory`    | `ENOTDIR` (20) | A path component is a file, not a directory |
/// | `IsADirectory`     | `EISDIR` (21)  | Tried to read/write a directory as a file |
/// | `InvalidInput`     | `EINVAL` (22)  | Bad argument (negative seek, bad whence, …) |
/// | `IoError`          | `EIO` (5)      | Hardware I/O error (sector read/write failure) |
/// | `NoSpace`          | `ENOSPC` (28)  | No space left on device or FD table full |
/// | `Unsupported`      | `ENOSYS` (38)  | Operation not implemented for this fs driver |
/// | `BadFileDescriptor`| `EBADF` (9)    | Invalid or closed file descriptor |
/// | `WouldBlock`       | `EAGAIN` (11)  | O_NONBLOCK set and operation would block |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum FsError {
    /// `ENOENT` – The named file or directory does not exist.
    NotFound,
    /// `EACCES` – The caller does not have the required permission.
    PermissionDenied,
    /// `EEXIST` – A file or directory with this name already exists.
    AlreadyExists,
    /// `ENOTDIR` – A non-directory component was used where a directory is required.
    NotADirectory,
    /// `EISDIR` – An attempt was made to read/write a directory as if it were a file.
    IsADirectory,
    /// `EINVAL` – An invalid argument was supplied (e.g. bad seek whence, negative offset).
    InvalidInput,
    /// `EIO` – A low-level hardware I/O error occurred (sector read/write failed).
    IoError,
    /// `ENOSPC` – No space left: file system full, or FD table exhausted.
    NoSpace,
    /// `ENOSYS` – The requested operation is not supported by this filesystem driver.
    Unsupported,
    /// `EBADF` – The file descriptor is out of range or points to a closed slot.
    BadFileDescriptor,
    /// `EAGAIN` – Non-blocking I/O was requested but the resource is not ready.
    WouldBlock,
}

/// **`POLLIN`** – There is data available to read without blocking.
///
/// Corresponds to `POLLIN` from `<poll.h>`.  When this bit is set in the mask
/// returned by [`Inode::poll`], the file descriptor is **readable**: a subsequent
/// `read()` will return at least 1 byte (or EOF) without blocking.
///
/// For `/dev/console` this flag is set when the keyboard or serial port has a
/// buffered keystroke.  For regular files it is always set.
///
/// Value: `0x0001` (bit 0).
pub const POLLIN: u32 = 0x0001;     // There is data to read

/// **`POLLPRI`** – There is urgent/exceptional data to read (out-of-band).
///
/// Corresponds to `POLLPRI` from `<poll.h>`.  On network sockets this signals
/// out-of-band data (TCP urgent pointer).  On serial devices it may signal a
/// break condition.  NullOS does not currently generate this event but the
/// constant is defined for completeness so user-space poll masks are preserved.
///
/// Value: `0x0002` (bit 1).
pub const POLLPRI: u32 = 0x0002;    // Exceptional condition / urgent data

/// **`POLLOUT`** – Writing will not block; the file descriptor is writable.
///
/// Corresponds to `POLLOUT` from `<poll.h>`.  When this bit is set, a subsequent
/// `write()` will succeed without blocking.  For the console device (UART) this
/// is always true because the UART TX FIFO is treated as always ready.  For block
/// devices it reflects whether the write queue has room.
///
/// Value: `0x0004` (bit 2).
pub const POLLOUT: u32 = 0x0004;    // Writing will not block

/// **`POLLERR`** – An error condition exists on the file descriptor.
///
/// Corresponds to `POLLERR` from `<poll.h>`.  This flag is set when the
/// underlying device or filesystem is in an error state.  It is always returned
/// (even if not in the event mask) if an error is pending, so callers must check
/// for it even when only monitoring `POLLIN` or `POLLOUT`.
///
/// Value: `0x0008` (bit 3).
pub const POLLERR: u32 = 0x0008;    // Error condition

/// **`POLLHUP`** – The peer has closed its end of a stream (hang up).
///
/// Corresponds to `POLLHUP` from `<poll.h>`.  Returned when the remote end of a
/// pipe, FIFO, or pseudo-terminal is closed.  After `POLLHUP`, reads return EOF
/// and writes return `EPIPE`.  Like `POLLERR`, this is always returned if
/// applicable regardless of the requested mask.
///
/// Value: `0x0010` (bit 4).
pub const POLLHUP: u32 = 0x0010;    // Stream disconnected / hung up

/// **`POLLNVAL`** – Invalid request: file descriptor is not open.
///
/// Corresponds to `POLLNVAL` from `<poll.h>`.  Returned (never requested) when
/// the file descriptor passed to `poll(2)` is not open.  In NullOS this would
/// correspond to passing a closed or out-of-range fd to a poll-based system call.
///
/// Value: `0x0020` (bit 5).
pub const POLLNVAL: u32 = 0x0020;   // Invalid request: fd not open

/// The type of file node in the VFS.
///
/// Every inode has exactly one type.  The type determines which methods are
/// meaningful:
///
/// | Variant | POSIX type | Meaningful methods |
/// |---------|------------|--------------------|
/// | `File`        | Regular file (`S_IFREG` = `0o100000`) | `read_at`, `write_at`, `truncate` |
/// | `Directory`   | Directory (`S_IFDIR` = `0o040000`)    | `lookup`, `read_dir`, `create`, `unlink` |
/// | `CharDevice`  | Character device (`S_IFCHR` = `0o020000`) | `read_at`, `write_at`, `poll` |
/// | `BlockDevice` | Block device (`S_IFBLK` = `0o060000`) | `read_at`, `write_at` |
/// | `Fifo`        | Named pipe (`S_IFIFO` = `0o010000`)   | `read_at`, `write_at`, `poll` |
/// | `Socket`      | Unix domain socket (`S_IFSOCK` = `0o140000`) | out-of-scope for NullOS M16 |
///
/// # Examples in NullOS
///
/// * `/dev/console` → `CharDevice` (backed by [`ConsoleDevice`](super::initramfs::ConsoleDevice))
/// * `/dev/null`    → `CharDevice` (backed by [`NullDevice`](super::initramfs::NullDevice))
/// * `/dev/zero`    → `CharDevice` (backed by [`ZeroDevice`](super::initramfs::ZeroDevice))
/// * `/bin/hello.elf` → `File` (backed by [`RamFile`](super::initramfs::RamFile))
/// * `/etc`         → `Directory` (backed by [`RamDirectory`](super::initramfs::RamDirectory))
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum FileType {
    /// A regular data file.  `read_at`/`write_at` transfer bytes; `truncate` adjusts size.
    File,
    /// A directory.  `lookup` finds children; `read_dir` lists them; `create` adds new ones.
    Directory,
    /// A character device (unbuffered I/O, e.g. serial port, keyboard, `/dev/null`).
    CharDevice,
    /// A block device (sector-based I/O, e.g. IDE disk, virtio-blk).
    BlockDevice,
    /// A named pipe (FIFO).  Data flows in FIFO order between writer and reader.
    Fifo,
    /// A Unix domain socket.  Not yet implemented in NullOS M16.
    Socket,
    // TODO(vfs): Add `Symlink` variant to support POSIX symbolic links.
    // Traversal must implement `follow_symlinks` with a recursion limit (e.g. `MAX_SYMLINK_DEPTH = 32`)
    // to prevent infinite loops from circular links (`ELOOP`).
}

// TODO(vfs): Implement POSIX Discretionary Access Control (DAC):
// - Extend `FileStat` and Inode abstractions with `uid: u32` and `gid: u32`.
// - Verify file permissions (`mode & 0777`) against caller's effective credentials `(euid, egid)`:
//   - Root (`euid == 0`) bypasses read/write permissions.
//   - Match owner if `euid == inode.uid`, check user bits (`rwx`).
//   - Match group if `egid == inode.gid`, check group bits (`rwx`).
//   - Fallback to other bits (`rwx`).
// - Return `FsError::PermissionDenied` (`EACCES`) upon unauthorized access.

/// File metadata returned by `stat`-family operations.
///
/// This is NullOS's equivalent of POSIX `struct stat`.  It is returned by
/// [`Inode::stat`] and carried inside [`DirEntry`] records.
///
/// # Fields
///
/// * `inode_num` – A **unique, monotonically increasing** 64-bit integer assigned
///   by [`alloc_inode_num`](super::initramfs::alloc_inode_num) using an atomic
///   counter.  Equivalent to `st_ino` in POSIX `struct stat`.  Uniqueness is
///   per-filesystem; two files in different filesystems may share the same number.
///
/// * `file_type` – The [`FileType`] of this inode (file, directory, char device, …).
///   Maps to the `S_IFMT` bits of POSIX `st_mode`.
///
/// * `size` – The number of **data bytes** in the file, or the **number of
///   children** for a directory.  Maps to `st_size`.  For character devices this
///   is always 0 (they produce/consume data on demand, not from a buffer).
///
/// * `mode` – POSIX **permission bits** in the lower 12 bits:
///   ```text
///   bits 11-9: setuid/setgid/sticky  (not enforced in M16)
///   bits  8-6: owner  rwx
///   bits  5-3: group  rwx
///   bits  2-0: others rwx
///   ```
///   Common values: `0o644` (rw-r--r-- regular file), `0o755` (rwxr-xr-x dir),
///   `0o777` (rwxrwxrwx scratch dir like `/tmp`), `0o666` (rw-rw-rw- device).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStat {
    /// Unique inode number within this filesystem.  Equivalent to POSIX `st_ino`.
    pub inode_num: u64,
    /// The kind of filesystem node (file, directory, device, …).
    pub file_type: FileType,
    /// Size in bytes (files) or number of direct children (directories).  Equivalent to `st_size`.
    pub size: usize,
    /// POSIX permission bits (lower 12 bits of `st_mode`).
    pub mode: u32,
}

/// Directory entry item returned when listing a directory.
///
/// A `DirEntry` is returned by [`Inode::read_dir`] for each child of a directory.
/// It carries just enough information to present a directory listing without
/// requiring a full `stat` call on each child.  Analogous to `struct dirent` in
/// POSIX (from `<dirent.h>`).
///
/// # Fields
///
/// * `name`      – The filename component (not a full path).  For example, inside
///   `/etc` the entry for `/etc/hostname` would have `name = "hostname"`.
///
/// * `inode_num` – The inode number of the child.  Can be used to detect hard
///   links (two entries sharing the same `inode_num`) or for caching.
///
/// * `file_type` – The [`FileType`] of the child.  Returned here as an
///   optimisation to avoid an extra `stat` call in common cases (e.g. `ls -l`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct DirEntry {
    /// The filename component (not a full path).
    pub name: String,
    /// The inode number of this directory entry's target inode.
    pub inode_num: u64,
    /// The type of the inode (file, directory, device, …).
    pub file_type: FileType,
}

/// Flags specified when opening a file, mirroring POSIX `open(2)` flags.
///
/// These flags are set once when the file is opened and stored inside a
/// [`FileDescriptor`](super::fd::FileDescriptor).  They govern all subsequent
/// operations on that descriptor.
///
/// # Fields
///
/// * `read`     – The descriptor may be used for reading.  Corresponds to
///   `O_RDONLY` (access mode 0) or `O_RDWR` (access mode 2).
///
/// * `write`    – The descriptor may be used for writing.  Corresponds to
///   `O_WRONLY` (access mode 1) or `O_RDWR` (access mode 2).
///
/// * `create`   – If the file does not exist, create it.  Corresponds to
///   `O_CREAT` (octal `0100`, decimal 64, hex `0x40`).  Without this flag,
///   opening a non-existent file returns `FsError::NotFound`.
///
/// * `append`   – Every write goes to the **end of the file**, regardless of
///   the current offset.  The offset is atomically updated after the write.
///   Corresponds to `O_APPEND` (octal `02000`, decimal 1024, hex `0x400`).
///
/// * `truncate` – On open, the file is truncated to 0 bytes.  Corresponds to
///   `O_TRUNC` (octal `01000`, decimal 512, hex `0x200`).
///
/// * `nonblock` – The descriptor operates in non-blocking mode.  `read` returns
///   `WouldBlock` instead of sleeping if no data is ready; `write` returns
///   `WouldBlock` if the buffer is full.  Corresponds to `O_NONBLOCK`
///   (octal `04000`, decimal 2048, hex `0x800`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OpenFlags {
    /// Allow reading from the file descriptor.
    pub read: bool,
    /// Allow writing to the file descriptor.
    pub write: bool,
    /// Create the file if it doesn't exist (`O_CREAT`).
    pub create: bool,
    /// All writes go to the end of the file (`O_APPEND`).
    pub append: bool,
    /// Truncate the file to 0 bytes on open (`O_TRUNC`).
    pub truncate: bool,
    /// Non-blocking I/O mode: return `WouldBlock` instead of waiting (`O_NONBLOCK`).
    pub nonblock: bool,
}

impl OpenFlags {
    /// Returns a fully-zeroed (no permissions, no flags) `OpenFlags`.
    ///
    /// Useful as a base when constructing custom flag combinations with struct
    /// update syntax.  Note that `OpenFlags::default()` does the same thing
    /// (since all fields are `bool` defaulting to `false`), but `empty()` is
    /// more intention-revealing.
    #[allow(dead_code)]
    pub const fn empty() -> Self {
        Self {
            read: false,
            write: false,
            create: false,
            append: false,
            truncate: false,
            nonblock: false,
        }
    }

    /// Pre-constructed read-only flag set.
    ///
    /// Equivalent to passing `O_RDONLY` (value `0`) to POSIX `open(2)`.
    /// Sets `read = true` and all other flags to `false`.
    pub const READ_ONLY: Self = Self {
        read: true,
        write: false,
        create: false,
        append: false,
        truncate: false,
        nonblock: false,
    };

    /// Pre-constructed write-only flag set.
    ///
    /// Equivalent to passing `O_WRONLY` (value `1`) to POSIX `open(2)`.
    /// Sets `write = true` and all other flags to `false`.
    pub const WRITE_ONLY: Self = Self {
        read: false,
        write: true,
        create: false,
        append: false,
        truncate: false,
        nonblock: false,
    };

    /// Pre-constructed read-write flag set.
    ///
    /// Equivalent to passing `O_RDWR` (value `2`) to POSIX `open(2)`.
    /// Sets `read = true`, `write = true` and all other flags to `false`.
    pub const READ_WRITE: Self = Self {
        read: true,
        write: true,
        create: false,
        append: false,
        truncate: false,
        nonblock: false,
    };

    /// Converts raw integer flags (as passed in a `sys_open` syscall) into an `OpenFlags`.
    ///
    /// Linux/POSIX encodes the **access mode** in the two lowest bits of the flags
    /// integer, with the remaining flags OR-ed into higher bits:
    ///
    /// ```text
    /// bits 1–0  (access mode):
    ///   0 = O_RDONLY  (read-only)
    ///   1 = O_WRONLY  (write-only)
    ///   2 = O_RDWR    (read-write)
    ///   3 = invalid / O_RDWR on some platforms
    ///
    /// bit  6 (octal 0100):  O_CREAT    – create file if not existing
    /// bit  9 (octal 01000): O_TRUNC    – truncate existing file to 0 bytes
    /// bit 10 (octal 02000): O_APPEND   – writes always go to end-of-file
    /// bit 11 (octal 04000): O_NONBLOCK – non-blocking I/O
    /// ```
    ///
    /// # Arguments
    ///
    /// * `bits` – The raw 64-bit flags integer, as received in a register from
    ///   user space (e.g. `%rsi` in the x86-64 System V syscall ABI).
    ///
    /// # Returns
    ///
    /// A fully populated `OpenFlags` struct.
    ///
    /// # Note on O_RDONLY
    ///
    /// Because `O_RDONLY` has the numeric value `0`, it cannot be tested with a
    /// bitwise AND.  Instead, the access mode field is extracted with `bits & 0x3`
    /// and compared: `0` → read, `1` → write, `2` → read+write.
    pub fn from_bits(bits: u64) -> Self {
        // The lowest two bits encode the access mode.
        let access = bits & 0x3;
        // O_RDONLY (0) or O_RDWR (2) both grant read permission.
        let read = access == 0 || access == 2; // O_RDONLY (0) or O_RDWR (2)
        // O_WRONLY (1) or O_RDWR (2) both grant write permission.
        let write = access == 1 || access == 2; // O_WRONLY (1) or O_RDWR (2)
        // O_CREAT is bit 6 → octal 0100 → decimal 64.
        let create = (bits & 0o100) != 0; // O_CREAT (0100)
        // O_TRUNC is bit 9 → octal 01000 → decimal 512.
        let truncate = (bits & 0o1000) != 0; // O_TRUNC (01000)
        // O_APPEND is bit 10 → octal 02000 → decimal 1024.
        let append = (bits & 0o2000) != 0; // O_APPEND (02000)
        // O_NONBLOCK is bit 11 → octal 04000 → decimal 2048 → hex 0x800.
        let nonblock = (bits & 0o4000) != 0; // O_NONBLOCK (04000 = 0x800)

        Self {
            read,
            write,
            create,
            append,
            truncate,
            nonblock,
        }
    }
}

/// Core Inode interface that **all filesystem drivers must implement**.
///
/// An `Inode` represents a single file-system node – be it a regular file,
/// directory, character device, block device, FIFO, or socket.  Every piece of
/// data in the VFS is ultimately accessed through an `Arc<dyn Inode>`.
///
/// # `Send + Sync` requirement
///
/// The `Send + Sync` bounds allow `Arc<dyn Inode>` to be stored in static
/// variables and shared across CPU cores without `unsafe`.  Implementors **must**
/// protect all interior mutable state with `Spinlock` or `AtomicXxx`.
///
/// # Method overview
///
/// | Method | Mandatory | Equivalent POSIX call |
/// |--------|-----------|----------------------|
/// | `stat`     | ✓ | `fstat(2)` |
/// | `read_at`  | ✓ | `pread(2)` |
/// | `write_at` | ✓ | `pwrite(2)` |
/// | `lookup`   | ✓ | `namei` (internal) |
/// | `read_dir` | ✓ | `getdents(2)` |
/// | `create`   | ✓ | `creat(2)` / `mkdir(2)` |
/// | `truncate` | ✗ (default Ok(())) | `truncate(2)` |
/// | `poll`     | ✗ (default POLLIN\|POLLOUT) | `poll(2)` |
/// | `unlink`   | ✗ (default Err(Unsupported)) | `unlink(2)` / `rmdir(2)` |
/// | `as_any`   | ✗ (default None) | (Rust-specific downcast) |
#[allow(dead_code)]
pub trait Inode: Send + Sync {
    /// Returns metadata about this inode as a [`FileStat`] snapshot.
    ///
    /// The returned value is a **copy** (not a reference), so the caller does not
    /// need to hold any lock after the call returns.  Equivalent to `fstat(2)`.
    ///
    /// # Implementation note
    ///
    /// For [`RamFile`](super::initramfs::RamFile), this locks the data spinlock
    /// momentarily to read the current size.  For directories it locks the
    /// children map to count entries.
    fn stat(&self) -> FileStat;

    /// Reads up to `buf.len()` bytes starting at byte position `offset`.
    ///
    /// This is a **positional** read (`pread(2)` semantics) – it does **not**
    /// modify any internal cursor.  The caller (the [`FileDescriptor`] layer)
    /// is responsible for maintaining and advancing the offset.
    ///
    /// # Arguments
    ///
    /// * `offset` – Zero-based byte offset into the file at which to start reading.
    /// * `buf`    – Mutable slice to fill with data.  At most `buf.len()` bytes
    ///   are written.
    ///
    /// # Returns
    ///
    /// * `Ok(n)` – `n` bytes were copied into `buf[..n]`.  `n == 0` signals
    ///   end-of-file (EOF) when `offset >= file_size`.
    /// * `Err(FsError::IsADirectory)` – Returned by directory nodes.
    /// * `Err(FsError::IoError)` – Hardware read failure (block devices).
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, FsError>;

    /// Writes `buf.len()` bytes at byte position `offset`.
    ///
    /// This is a **positional** write (`pwrite(2)` semantics) – it does **not**
    /// modify any internal cursor.  The [`FileDescriptor`] layer updates the
    /// offset after the call.
    ///
    /// For [`RamFile`](super::initramfs::RamFile), if `offset + buf.len()` exceeds
    /// the current file size, the backing `Vec<u8>` is **extended** with zero bytes
    /// to cover the gap, then the data is overwritten.
    ///
    /// # Arguments
    ///
    /// * `offset` – Zero-based byte offset at which to start writing.
    /// * `buf`    – The bytes to write.
    ///
    /// # Returns
    ///
    /// * `Ok(n)` – `n` bytes were written (always equals `buf.len()` for RamFS).
    /// * `Err(FsError::IsADirectory)` – Returned by directory nodes.
    /// * `Err(FsError::NoSpace)` – Device or filesystem full.
    /// * `Err(FsError::IoError)` – Hardware write failure.
    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, FsError>;

    /// Looks up a child inode by name within this directory inode.
    ///
    /// This is the building block of path traversal.  Given a single filename
    /// component (e.g. `"etc"` or `"os-release"`), returns an `Arc<dyn Inode>`
    /// for the child.
    ///
    /// # Arguments
    ///
    /// * `name` – A single filename component (no slashes).
    ///
    /// # Returns
    ///
    /// * `Ok(inode)` – The child inode.
    /// * `Err(FsError::NotFound)` – No child with this name exists.
    /// * `Err(FsError::NotADirectory)` – This inode is not a directory.
    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>, FsError>;

    /// Lists all children of this directory, returning a vector of [`DirEntry`] records.
    ///
    /// Analogous to `getdents64(2)`.  The order of returned entries is
    /// implementation-defined.  For [`RamDirectory`](super::initramfs::RamDirectory),
    /// entries are returned in `BTreeMap` iteration order (alphabetical by name).
    ///
    /// # Returns
    ///
    /// * `Ok(entries)` – A vector of all direct children (not recursive).
    /// * `Err(FsError::NotADirectory)` – This inode is a file, not a directory.
    fn read_dir(&self) -> Result<Vec<DirEntry>, FsError>;

    /// Creates a new child inode under this directory with the given name and type.
    ///
    /// For type `FileType::File`, this allocates a new empty
    /// [`RamFile`](super::initramfs::RamFile) with mode `0o644` and inserts it
    /// into the directory's children map.
    ///
    /// For type `FileType::Directory`, this allocates a new
    /// [`RamDirectory`](super::initramfs::RamDirectory) with mode `0o755`.
    ///
    /// Other types return `Err(FsError::Unsupported)` in the base RamFS
    /// implementation.
    ///
    /// # Arguments
    ///
    /// * `name`      – The filename for the new entry (must not already exist).
    /// * `file_type` – Whether to create a file or directory.
    ///
    /// # Returns
    ///
    /// * `Ok(inode)` – The newly created inode.
    /// * `Err(FsError::AlreadyExists)` – A child with this name already exists.
    /// * `Err(FsError::NotADirectory)` – This inode is not a directory.
    fn create(&self, name: &str, file_type: FileType) -> Result<Arc<dyn Inode>, FsError>;

    /// Truncates the file to exactly `size` bytes.
    ///
    /// If `size` is smaller than the current file length, data beyond `size` is
    /// discarded.  If `size` is larger, the behaviour is implementation-defined
    /// (RamFS zero-extends; block device drivers may leave a "hole").
    ///
    /// The default implementation is a no-op returning `Ok(())`, which is
    /// appropriate for device nodes that do not have a persistent data buffer.
    ///
    /// # Arguments
    ///
    /// * `_size` – The new file length in bytes.
    ///
    /// # Returns
    ///
    /// * `Ok(())` on success.
    /// * `Err(FsError::Unsupported)` if the filesystem driver does not support
    ///   truncation.
    fn truncate(&self, _size: usize) -> Result<(), FsError> {
        Ok(())
    }

    /// Polls this inode for ready I/O events, returning a bitmask of asserted flags.
    ///
    /// The returned bitmask uses the [`POLLIN`], [`POLLOUT`], [`POLLERR`],
    /// [`POLLHUP`], and [`POLLNVAL`] constants.  This method is used by the
    /// [`FileDescriptor`](super::fd::FileDescriptor) layer in non-blocking mode
    /// to decide whether a `read` or `write` can proceed without blocking.
    ///
    /// The default implementation returns `POLLIN | POLLOUT`, which is correct for
    /// regular files (always readable and writable).
    ///
    /// # Returns
    ///
    /// A bitmask of currently asserted poll events.
    fn poll(&self) -> u32 {
        POLLIN | POLLOUT
    }

    /// Removes (unlinks) a named child from this directory.
    ///
    /// For files this removes the directory entry.  If this was the last hard link
    /// to the inode, the inode's data is freed when the last `Arc<dyn Inode>` is
    /// dropped.  For directories, most implementations require the directory to be
    /// empty first (like POSIX `rmdir(2)`).
    ///
    /// The default implementation returns `Err(FsError::Unsupported)`.
    ///
    /// # Arguments
    ///
    /// * `_name` – The filename component to remove (must exist in this directory).
    ///
    /// # Returns
    ///
    /// * `Ok(())` on success.
    /// * `Err(FsError::NotFound)` – No child with this name.
    /// * `Err(FsError::Unsupported)` – This filesystem driver does not support unlink.
    fn unlink(&self, _name: &str) -> Result<(), FsError> {
        Err(FsError::Unsupported)
    }

    /// Provides dynamic type inspection and downcasting for specialized inode types.
    ///
    /// Returns a `&dyn Any` reference to `self` if the concrete implementor
    /// opts in; otherwise returns `None`.  This is a Rust-specific escape hatch
    /// that allows code with `Arc<dyn Inode>` to downcast to, e.g.,
    /// `Arc<ConsoleDevice>` using `Arc::downcast` after checking `as_any`.
    ///
    /// The default implementation returns `None`, meaning most inode types do not
    /// support downcasting.
    ///
    /// # Returns
    ///
    /// `Some(&dyn Any)` if the concrete type supports downcasting, `None` otherwise.
    fn as_any(&self) -> Option<&dyn core::any::Any> {
        None
    }
}

/// Core FileSystem trait implemented by all filesystem drivers (RamFS, FAT32, ext2).
///
/// A `FileSystem` is a **factory** that produces inodes.  The only mandatory
/// operations are identifying the driver by name and returning the root inode.
/// All further navigation happens through the inode's own methods (`lookup`,
/// `read_dir`, `create`, …).
///
/// A filesystem is mounted by inserting an `Arc<dyn FileSystem>` into the VFS
/// mount table under a mount point path.  The same `Arc` can be inserted at
/// multiple mount points (e.g. FAT32 is mounted at both `/disk` and `/mnt`).
#[allow(dead_code)]
pub trait FileSystem: Send + Sync {
    /// Returns a human-readable name for this filesystem driver.
    ///
    /// Used in log messages and diagnostics.  Examples: `"ramfs"`, `"fat32"`,
    /// `"ext2"`.  The string does **not** include the mount point.
    fn name(&self) -> &str;

    /// Returns the **root inode** of this mounted filesystem instance.
    ///
    /// For RamFS this is the root [`RamDirectory`](super::initramfs::RamDirectory).
    /// For FAT32 it is the root directory cluster.  For ext2 it is the inode with
    /// number 2 (always the ext2 root directory by convention).
    ///
    /// All path resolution starts from this inode and walks downward.
    fn root(&self) -> Arc<dyn Inode>;
}

/// Global mount table and path resolution manager for the NullOS kernel.
///
/// # `mounts` field
///
/// `mounts: Spinlock<BTreeMap<String, Arc<dyn FileSystem>>>`
///
/// The mount table maps **canonical absolute paths** (cleaned by [`clean_path`])
/// to filesystem instances.  Using `BTreeMap` (a sorted tree) rather than
/// `HashMap` is intentional: when searching for the longest-prefix match we
/// iterate in lexicographic order, and the last matching prefix is always the
/// longest one.
///
/// The `Spinlock` ensures that mounting and unmounting (future feature) are
/// atomic relative to path resolution even in an SMP kernel where multiple CPUs
/// might call `resolve_path` concurrently.
///
/// # Singleton
///
/// A single `static VFS: Vfs` is defined below.  This mirrors Linux's global
/// `struct vfsmount` root and `namespace` structures, simplified for a
/// single-namespace OS.
pub struct Vfs {
    mounts: Spinlock<BTreeMap<String, Arc<dyn FileSystem>>>,
}

impl Vfs {
    /// Creates an empty VFS with no mounts.
    ///
    /// This is `const` so the global `VFS` static can be initialized without
    /// running code at boot (before the heap is available).  The underlying
    /// `Spinlock::new(BTreeMap::new())` is also `const`.
    pub const fn new() -> Self {
        Self {
            mounts: Spinlock::new(BTreeMap::new()),
        }
    }

    /// Mounts a filesystem driver at the specified **absolute** mount point path.
    ///
    /// The path is normalized via [`clean_path`] before insertion.  Mounting the
    /// same path twice replaces the previous entry (last-write-wins), which is
    /// consistent with Linux's behaviour when re-mounting.
    ///
    /// # Longest-prefix matching
    ///
    /// All paths in the mount table are canonical (no trailing slash, no `..`).
    /// Resolution uses longest-prefix matching, so mounting `/disk` **shadows**
    /// the RamFS `/disk` stub directory: any path that starts with `/disk/` will
    /// now be handled by the FAT32 or ext2 driver, not by RamFS.
    ///
    /// # Arguments
    ///
    /// * `mount_point` – Absolute path where the filesystem should be accessible.
    ///   Examples: `"/"`, `"/disk"`, `"/mnt"`.
    /// * `fs`          – The filesystem driver to mount.
    ///
    /// # Returns
    ///
    /// `Ok(())` always (the current implementation cannot fail because there is
    /// no limit on the number of mounts and the spinlock cannot deadlock in a
    /// single-threaded boot sequence).
    pub fn mount(&self, mount_point: &str, fs: Arc<dyn FileSystem>) -> Result<(), FsError> {
        // Normalize: remove double slashes, resolve ".", "..", strip trailing slash.
        let clean_path = clean_path(mount_point);
        let mut mounts = self.mounts.lock();
        mounts.insert(clean_path, fs);
        Ok(())
    }

    /// Resolves an absolute path string into the corresponding `Inode`.
    ///
    /// This is the **central** VFS operation.  All file access goes through this
    /// method (directly or via [`open`](Self::open)).
    ///
    /// # Algorithm (3 steps)
    ///
    /// **Step 1 – Find the longest matching mount point**
    ///
    /// Iterate over all entries in the mount table.  For each entry, check whether
    /// the cleaned path:
    /// * Exactly equals the mount point, **or**
    /// * Starts with `"{mount_point}/"` (the target is inside this filesystem).
    ///
    /// Special case: the root mount `"/"` matches any absolute path.
    ///
    /// Among all matches, keep the one with the **longest** mount point string.
    /// This implements the standard POSIX mount shadowing rule.
    ///
    /// **Step 2 – Compute the relative path**
    ///
    /// Strip the mount point prefix from the cleaned path:
    /// * If the path equals the mount point → relative path is empty → return root inode.
    /// * If mount point is `"/"` → strip the leading `'/'` → remainder is the relative path.
    /// * Otherwise → strip the mount point prefix and any leading `'/'`.
    ///
    /// **Step 3 – Walk path segments**
    ///
    /// Split the relative path on `'/'` and iterate.  For each non-empty,
    /// non-`"."` segment:
    /// 1. Verify that the current node is a directory (else return `NotADirectory`).
    /// 2. Call `current_node.lookup(segment)` to descend into the child.
    ///
    /// Return the final node.
    ///
    /// # Arguments
    ///
    /// * `path` – An absolute path string (may contain `..`, double slashes, etc.;
    ///   it is cleaned internally).
    ///
    /// # Returns
    ///
    /// * `Ok(inode)` – The inode at the specified path.
    /// * `Err(FsError::NotFound)` – The path does not exist (or no mount covers it).
    /// * `Err(FsError::NotADirectory)` – A non-terminal path component is a file.
    pub fn resolve_path(&self, path: &str) -> Result<Arc<dyn Inode>, FsError> {
        let clean = clean_path(path);
        let mounts = self.mounts.lock();

        // 1. Find the longest matching mount point prefix
        let mut matched_mount: Option<(String, Arc<dyn FileSystem>)> = None;
        for (mount_point, fs) in mounts.iter() {
            let is_match = if mount_point == "/" {
                // The root mount matches every absolute path.
                clean.starts_with('/')
            } else {
                // A non-root mount matches if the path is exactly that mount point
                // or if the path is inside it (starts with "mount_point/").
                clean == *mount_point || clean.starts_with(&alloc::format!("{}/", mount_point))
            };

            if is_match {
                // Keep whichever match has the longer mount point string
                // (longer string = more specific = wins).
                if matched_mount.as_ref().map_or(true, |(m, _)| mount_point.len() > m.len()) {
                    matched_mount = Some((mount_point.clone(), fs.clone()));
                }
            }
        }

        let (mount_point, fs) = matched_mount.ok_or(FsError::NotFound)?;
        // Release the spinlock before the recursive inode walk to avoid holding
        // it across potentially long directory lookups.
        drop(mounts);

        // 2. Compute relative path from mount point
        let rel_path = if clean == mount_point {
            // The path is exactly the mount point → return the root inode directly.
            ""
        } else if mount_point == "/" {
            // Strip the leading "/" from the absolute path.
            clean.trim_start_matches('/')
        } else {
            // Strip the mount point prefix (e.g. "/disk") and any following slash.
            clean.strip_prefix(&mount_point).unwrap_or("").trim_start_matches('/')
        };

        if rel_path.is_empty() {
            // No further segments to walk – return the filesystem root directly.
            return Ok(fs.root());
        }

        // 3. Step through path segments, calling lookup() at each level.
        let mut current_node = fs.root();
        for segment in rel_path.split('/') {
            if segment.is_empty() || segment == "." {
                // Ignore empty segments (double slashes) and "." (current dir).
                continue;
            }
            // "..": would need to track the parent inode.  For now, clean_path
            // already resolves ".." before we reach here.
            if current_node.stat().file_type != FileType::Directory {
                // Cannot descend into a non-directory.  POSIX returns ENOTDIR.
                return Err(FsError::NotADirectory);
            }
            current_node = current_node.lookup(segment)?;
        }

        Ok(current_node)
    }

    /// Opens an inode at `path` with the given access `flags`.
    ///
    /// This is the VFS-level equivalent of the `open(2)` system call.  It
    /// combines path resolution with optional file creation and truncation.
    ///
    /// # Create-on-not-found logic
    ///
    /// If `resolve_path` returns `Err(FsError::NotFound)` **and** `flags.create`
    /// is set, `open` performs the following steps:
    ///
    /// 1. Split the path into parent (`/tmp`) and leaf (`test.txt`) via [`split_path`].
    /// 2. Resolve the **parent** directory inode.
    /// 3. Call `parent.create(leaf, FileType::File)` to allocate the new inode.
    ///
    /// This mirrors Linux's `do_last()` path in `fs/namei.c`.
    ///
    /// # Truncate logic
    ///
    /// If the file already exists **and** `flags.truncate` is set, `truncate(0)`
    /// is called on the inode before it is returned.  This resets the file content
    /// to 0 bytes, as required by `O_TRUNC`.
    ///
    /// # Arguments
    ///
    /// * `path`  – Absolute path to the file to open.
    /// * `flags` – Access mode and behavioral flags.
    ///
    /// # Returns
    ///
    /// * `Ok(inode)` – The inode is ready for I/O.
    /// * `Err(FsError::NotFound)` – File not found and `flags.create` is not set.
    /// * `Err(FsError::NotADirectory)` – A parent path component is not a directory.
    pub fn open(&self, path: &str, flags: OpenFlags) -> Result<Arc<dyn Inode>, FsError> {
        let inode = match self.resolve_path(path) {
            Ok(inode) => {
                if flags.truncate {
                    // O_TRUNC: reset file to 0 bytes before returning the inode.
                    inode.truncate(0)?;
                }
                inode
            }
            Err(FsError::NotFound) if flags.create => {
                // If create flag set, find parent directory and create file
                let clean = clean_path(path);
                let (parent_path, file_name) = split_path(&clean)?;
                // Resolve the parent directory (e.g. "/tmp" for "/tmp/test.txt").
                let parent_inode = self.resolve_path(parent_path)?;
                // Create the new file inside the parent directory.
                parent_inode.create(file_name, FileType::File)?
            }
            Err(e) => return Err(e),
        };
        Ok(inode)
    }

    /// Unlinks (removes) a file or child node from its parent directory.
    ///
    /// This is the VFS-level equivalent of `unlink(2)` (for files) or `rmdir(2)`
    /// (for directories, though the current RamFS implementation does not enforce
    /// the "must be empty" rule).
    ///
    /// # Parent lookup
    ///
    /// The path is split into parent and leaf via [`split_path`].  The parent
    /// directory inode is resolved, and then `parent.unlink(leaf)` is called.
    /// This mirrors Linux's `vfs_unlink()` which always operates on the parent.
    ///
    /// # Arguments
    ///
    /// * `path` – Absolute path of the entry to remove.
    ///
    /// # Returns
    ///
    /// * `Ok(())` on success.
    /// * `Err(FsError::NotFound)` – The file or its parent does not exist.
    /// * `Err(FsError::Unsupported)` – The filesystem driver does not support unlink.
    pub fn unlink(&self, path: &str) -> Result<(), FsError> {
        let clean = clean_path(path);
        // Split "/etc/hostname" → parent="/etc", leaf="hostname".
        let (parent_path, leaf) = split_path(&clean)?;
        let parent_inode = self.resolve_path(parent_path)?;
        parent_inode.unlink(leaf)
    }
}

/// Global singleton VFS instance used by all kernel subsystems.
///
/// This is the **one and only** VFS object in the kernel.  All filesystem
/// operations go through `VFS.resolve_path(...)`, `VFS.open(...)`, etc.
///
/// It is `const`-initialized (before `main`) so it is available as soon as
/// the `.bss` and `.data` sections are set up – no init function is needed.
/// The actual **content** (mounted filesystems) is populated by calling
/// [`init`](super::init) during the kernel boot sequence.
pub static VFS: Vfs = Vfs::new();

/// Normalizes a path string by removing redundant components.
///
/// This is NullOS's equivalent of `realpath(3)` / `path_clean` in Go / `canonicalize`.
/// It performs the following transformations:
///
/// 1. Split the path on `'/'`.
/// 2. Skip empty components (produced by double slashes like `//`).
/// 3. Skip `"."` (current directory – no-op).
/// 4. Pop the last component on `".."` (go up one level).
/// 5. Re-join the surviving components with a leading `'/'`.
/// 6. If no components survive (root or `/../`-style paths), return `"/"`.
///
/// # Examples
///
/// ```text
/// clean_path("/")          → "/"
/// clean_path("//etc//")   → "/etc"
/// clean_path("/etc/./hostname") → "/etc/hostname"
/// clean_path("/etc/../tmp")     → "/tmp"
/// clean_path("/../../..")       → "/"   (can't go above root)
/// ```
///
/// # Arguments
///
/// * `path` – Any path string (absolute or relative; leading `'/'` is consumed
///   during splitting, so the output is always "rooted" at `/`).
///
/// # Returns
///
/// A newly allocated `String` containing the canonical form.
pub fn clean_path(path: &str) -> String {
    let mut parts = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            // Skip empty segments (double slashes) and "." (current dir).
            continue;
        } else if part == ".." {
            // Go up one level by removing the last accumulated component.
            // If already at root (parts is empty), pop() is a no-op.
            parts.pop();
        } else {
            // A normal path component: accumulate it.
            parts.push(part);
        }
    }

    if parts.is_empty() {
        // All components cancelled out → the path resolves to root.
        "/".to_string()
    } else {
        // Re-join components with leading slashes.
        let mut res = String::new();
        for part in parts {
            res.push('/');
            res.push_str(part);
        }
        res
    }
}

/// Splits a **canonical absolute path** into its parent directory and leaf filename.
///
/// This is used by [`Vfs::open`] and [`Vfs::unlink`] to identify the containing
/// directory when creating or removing a file.
///
/// # Algorithm
///
/// 1. Strip any trailing `'/'` (guard against paths like `"/tmp/"`).
/// 2. Find the last `'/'` in the remaining string.
/// 3. Everything before that slash is the parent; everything after is the leaf.
/// 4. Special case: if the last `'/'` is at index 0, the parent is `"/"` (not
///    an empty string), to correctly represent the filesystem root.
/// 5. If there is no `'/'` at all, the whole path is the leaf and the parent
///    is implicitly `"/"`.
///
/// # Examples
///
/// ```text
/// split_path("/tmp/test.txt")     → Ok(("/tmp",  "test.txt"))
/// split_path("/etc/os-release")   → Ok(("/etc",  "os-release"))
/// split_path("/hello.txt")        → Ok(("/",     "hello.txt"))
/// split_path("/")                 → Err(FsError::InvalidInput)  (no leaf)
/// ```
///
/// # Arguments
///
/// * `path` – A canonical (clean) absolute path.
///
/// # Returns
///
/// * `Ok((parent, leaf))` – Borrowed slices into `path`.
/// * `Err(FsError::InvalidInput)` – The leaf component is empty (path was `/` or ended with `/`).
pub fn split_path(path: &str) -> Result<(&str, &str), FsError> {
    // Strip trailing slashes so "/tmp/" becomes "/tmp".
    let clean = path.trim_end_matches('/');
    if let Some(idx) = clean.rfind('/') {
        // Slice everything before the last '/' as the parent.
        // If idx == 0 that means the path is "/something" and the parent is root.
        let parent = if idx == 0 { "/" } else { &clean[..idx] };
        // The leaf is everything after the last '/'.
        let leaf = &clean[idx + 1..];
        if leaf.is_empty() {
            // The path was just "/" (after trimming) → no leaf.
            Err(FsError::InvalidInput)
        } else {
            Ok((parent, leaf))
        }
    } else {
        // No '/' found at all (e.g. a bare filename with no directory prefix).
        // Treat root as the parent.
        Ok(("/", clean))
    }
}
