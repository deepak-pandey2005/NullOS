//! Virtual File System (VFS) Abstraction (Phase 6 - M16 VFS).
//!
//! # Architecture & Purpose
//!
//! The Virtual File System (VFS) provides an operating system-level unified abstraction
//! for disparate storage mechanisms, directory hierarchies, and file types. By defining
//! polymorphic traits ([`Inode`] and [`FileSystem`]), the VFS decouples higher-level
//! system calls (`open`, `read`, `write`, `stat`, `unlink`, `poll`) from concrete on-disk
//! layouts (such as FAT32, ext2, or in-memory RamFS).
//!
//! # Core Concepts
//! - **[`Inode`]**: Represents an individual file system object (file, directory, device).
//!   Provides fundamental operations like reading, writing, directory enumeration, and polling.
//! - **[`FileSystem`]**: Represents a mounted storage driver instance capable of supplying
//!   its root inode.
//! - **Mount Table**: Managed by the global [`Vfs`] singleton, mapping path prefixes
//!   (e.g., `"/"`, `"/disk"`, `"/mnt"`) to concrete [`FileSystem`] instances using
//!   longest-prefix matching.
//! - **Path Resolution**: Normalizes POSIX paths, strips redundant separators and `.`/`..`
//!   components, matches against the mount table, and walks directory inodes segment-by-segment.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::sync::Spinlock;

/// VFS error codes conforming to standard POSIX-like error conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum FsError {
    /// File or directory does not exist (`ENOENT`).
    NotFound,
    /// Caller lacks sufficient privileges for the requested operation (`EACCES`).
    PermissionDenied,
    /// Target file or directory already exists (`EEXIST`).
    AlreadyExists,
    /// Operation expected a directory but encountered a non-directory node (`ENOTDIR`).
    NotADirectory,
    /// Operation cannot be performed on a directory (`EISDIR`).
    IsADirectory,
    /// Invalid path or function argument supplied (`EINVAL`).
    InvalidInput,
    /// Low-level physical or virtual storage I/O communication failure (`EIO`).
    IoError,
    /// Storage medium or table capacity exceeded (`ENOSPC`).
    NoSpace,
    /// Requested operation is unsupported by the underlying filesystem driver (`ENOTSUP`).
    Unsupported,
    /// Invalid or unopened file descriptor (`EBADF`).
    BadFileDescriptor,
    /// Non-blocking operation would block awaiting resources (`EAGAIN` / `EWOULDBLOCK`).
    WouldBlock,
}

/// Standard poll event flags conforming to POSIX `poll(2)` and `epoll(7)`.
/// Indicates readable data is available on the file descriptor or device.
pub const POLLIN: u32 = 0x0001;
/// Exceptional condition or out-of-band/urgent data is available.
pub const POLLPRI: u32 = 0x0002;
/// Writing to the descriptor will not block.
pub const POLLOUT: u32 = 0x0004;
/// Asynchronous error condition detected on the descriptor.
pub const POLLERR: u32 = 0x0008;
/// Hang up or disconnect detected on the underlying stream or socket.
pub const POLLHUP: u32 = 0x0010;
/// Invalid request: file descriptor is not open.
pub const POLLNVAL: u32 = 0x0020;

/// The type of file node in the VFS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum FileType {
    /// Regular byte-stream file with readable and writable contents.
    File,
    /// Directory containing named directory entries pointing to child inodes.
    Directory,
    /// Character-oriented device node (e.g., serial console, zero, null).
    CharDevice,
    /// Block-oriented storage device node (e.g., raw disk partition).
    BlockDevice,
    /// Named pipe (FIFO) for inter-process byte stream communication.
    Fifo,
    /// Network or UNIX domain communication socket.
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

/// File metadata returned by `stat(2)` operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStat {
    /// Globally unique or filesystem-unique 64-bit inode identification number.
    pub inode_num: u64,
    /// Classification of the file object (file, directory, device, socket, etc.).
    pub file_type: FileType,
    /// Total logical size of the file contents in bytes.
    pub size: usize,
    /// POSIX file permission mode bits and flags (e.g., `0o755`, `0o644`).
    pub mode: u32,
}

/// Directory entry item returned when listing the contents of a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct DirEntry {
    /// Leaf name of the child file or directory entry.
    pub name: String,
    /// Inode number associated with the target child node.
    pub inode_num: u64,
    /// Node type of the child entry.
    pub file_type: FileType,
}

/// Flags specified when opening a file, controlling access mode and creation semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OpenFlags {
    /// Open the file for reading.
    pub read: bool,
    /// Open the file for writing.
    pub write: bool,
    /// Create the file if it does not already exist (`O_CREAT`).
    pub create: bool,
    /// Append all writes to the end of the file (`O_APPEND`).
    pub append: bool,
    /// Truncate file length to zero upon opening (`O_TRUNC`).
    pub truncate: bool,
    /// Enable non-blocking I/O operations (`O_NONBLOCK`).
    pub nonblock: bool,
}

impl OpenFlags {
    /// Returns an empty flag set with all access and creation options disabled.
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

    /// Read-only access flag preset (`O_RDONLY`).
    pub const READ_ONLY: Self = Self {
        read: true,
        write: false,
        create: false,
        append: false,
        truncate: false,
        nonblock: false,
    };

    /// Write-only access flag preset (`O_WRONLY`).
    pub const WRITE_ONLY: Self = Self {
        read: false,
        write: true,
        create: false,
        append: false,
        truncate: false,
        nonblock: false,
    };

    /// Read-write access flag preset (`O_RDWR`).
    pub const READ_WRITE: Self = Self {
        read: true,
        write: true,
        create: false,
        append: false,
        truncate: false,
        nonblock: false,
    };

    /// Converts raw bit flags (e.g., from POSIX `open(2)` system calls) into an `OpenFlags` struct.
    ///
    /// Decodes:
    /// - Bits 0..1: Access mode (0 = `O_RDONLY`, 1 = `O_WRONLY`, 2 = `O_RDWR`).
    /// - Bit 6 (`0o100`): `O_CREAT`.
    /// - Bit 9 (`0o1000`): `O_TRUNC`.
    /// - Bit 10 (`0o2000`): `O_APPEND`.
    /// - Bit 11 (`0o4000`): `O_NONBLOCK`.
    pub fn from_bits(bits: u64) -> Self {
        let access = bits & 0x3;
        let read = access == 0 || access == 2; // O_RDONLY (0) or O_RDWR (2)
        let write = access == 1 || access == 2; // O_WRONLY (1) or O_RDWR (2)
        let create = (bits & 0o100) != 0; // O_CREAT (0100)
        let truncate = (bits & 0o1000) != 0; // O_TRUNC (01000)
        let append = (bits & 0o2000) != 0; // O_APPEND (02000)
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

/// Core Inode interface implemented by all concrete filesystem drivers.
///
/// An `Inode` represents a single filesystem object. Methods provide reading,
/// writing, metadata inspection, directory traversal, child creation, and polling.
#[allow(dead_code)]
pub trait Inode: Send + Sync {
    /// Returns metadata (type, size, inode number, permissions) about this inode.
    fn stat(&self) -> FileStat;

    /// Reads up to `buf.len()` bytes starting at byte `offset`.
    ///
    /// Returns the exact number of bytes transferred into `buf`, or an [`FsError`].
    /// Returning `Ok(0)` when `buf.len() > 0` indicates End-Of-File (EOF).
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, FsError>;

    /// Writes `buf.len()` bytes starting at byte `offset`.
    ///
    /// Returns the number of bytes written, extending the underlying file if necessary.
    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, FsError>;

    /// Looks up a child entry by name within this directory inode.
    ///
    /// # Errors
    /// - [`FsError::NotADirectory`]: If called on a non-directory node.
    /// - [`FsError::NotFound`]: If no child with `name` exists.
    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>, FsError>;

    /// Enumerates all child directory entries if this inode represents a directory.
    fn read_dir(&self) -> Result<Vec<DirEntry>, FsError>;

    /// Creates a new child file or directory under this directory node.
    fn create(&self, name: &str, file_type: FileType) -> Result<Arc<dyn Inode>, FsError>;

    /// Truncates or extends the file length to `size` bytes.
    fn truncate(&self, _size: usize) -> Result<(), FsError> {
        Ok(())
    }

    /// Polls the file node for ready I/O conditions (`POLLIN`, `POLLOUT`, `POLLERR`, `POLLHUP`).
    fn poll(&self) -> u32 {
        POLLIN | POLLOUT
    }

    /// Removes a named child entry from this directory.
    fn unlink(&self, _name: &str) -> Result<(), FsError> {
        Err(FsError::Unsupported)
    }

    /// Provides dynamic type inspection and downcasting for specialized inode types.
    fn as_any(&self) -> Option<&dyn core::any::Any> {
        None
    }
}

/// Core FileSystem trait implemented by concrete filesystem drivers (RamFs, FAT32, ext2).
#[allow(dead_code)]
pub trait FileSystem: Send + Sync {
    /// Driver identifier name (e.g., `"ramfs"`, `"fat32"`, `"ext2"`).
    fn name(&self) -> &str;

    /// Returns the root inode of this mounted filesystem instance.
    fn root(&self) -> Arc<dyn Inode>;
}

/// Global mount table and hierarchical path resolution manager.
pub struct Vfs {
    /// Map of normalized mountpoint prefixes (e.g., `"/"`, `"/disk"`) to filesystem instances.
    mounts: Spinlock<BTreeMap<String, Arc<dyn FileSystem>>>,
}

impl Vfs {
    /// Creates an empty VFS mount table.
    pub const fn new() -> Self {
        Self {
            mounts: Spinlock::new(BTreeMap::new()),
        }
    }

    /// Mounts a filesystem driver instance at the specified mountpoint path.
    pub fn mount(&self, mount_point: &str, fs: Arc<dyn FileSystem>) -> Result<(), FsError> {
        let clean_path = clean_path(mount_point);
        let mut mounts = self.mounts.lock();
        mounts.insert(clean_path, fs);
        Ok(())
    }

    /// Resolves an absolute path string into the target [`Inode`].
    ///
    /// # Resolution Algorithm
    /// 1. Normalizes the input path via [`clean_path`].
    /// 2. Scans the mount table for the longest prefix match.
    /// 3. Computes the path relative to the matched mountpoint.
    /// 4. Iteratively resolves each directory path segment using [`Inode::lookup`].
    pub fn resolve_path(&self, path: &str) -> Result<Arc<dyn Inode>, FsError> {
        let clean = clean_path(path);
        let mounts = self.mounts.lock();

        // 1. Find the longest matching mount point prefix
        let mut matched_mount: Option<(String, Arc<dyn FileSystem>)> = None;
        for (mount_point, fs) in mounts.iter() {
            let is_match = if mount_point == "/" {
                clean.starts_with('/')
            } else {
                clean == *mount_point || clean.starts_with(&alloc::format!("{}/", mount_point))
            };

            if is_match {
                if matched_mount.as_ref().map_or(true, |(m, _)| mount_point.len() > m.len()) {
                    matched_mount = Some((mount_point.clone(), fs.clone()));
                }
            }
        }

        let (mount_point, fs) = matched_mount.ok_or(FsError::NotFound)?;
        drop(mounts);

        // 2. Compute relative path from mount point
        let rel_path = if clean == mount_point {
            ""
        } else if mount_point == "/" {
            clean.trim_start_matches('/')
        } else {
            clean.strip_prefix(&mount_point).unwrap_or("").trim_start_matches('/')
        };

        if rel_path.is_empty() {
            return Ok(fs.root());
        }

        // 3. Step through path segments
        let mut current_node = fs.root();
        for segment in rel_path.split('/') {
            if segment.is_empty() || segment == "." {
                continue;
            }
            if current_node.stat().file_type != FileType::Directory {
                return Err(FsError::NotADirectory);
            }
            current_node = current_node.lookup(segment)?;
        }

        Ok(current_node)
    }

    /// Opens an inode with the given flags, creating it if `flags.create` is specified and missing.
    pub fn open(&self, path: &str, flags: OpenFlags) -> Result<Arc<dyn Inode>, FsError> {
        let inode = match self.resolve_path(path) {
            Ok(inode) => {
                if flags.truncate {
                    inode.truncate(0)?;
                }
                inode
            }
            Err(FsError::NotFound) if flags.create => {
                // If create flag set, find parent directory and create file
                let clean = clean_path(path);
                let (parent_path, file_name) = split_path(&clean)?;
                let parent_inode = self.resolve_path(parent_path)?;
                parent_inode.create(file_name, FileType::File)?
            }
            Err(e) => return Err(e),
        };
        Ok(inode)
    }

    /// Unlinks (removes) a file or child node from its parent directory.
    pub fn unlink(&self, path: &str) -> Result<(), FsError> {
        let clean = clean_path(path);
        let (parent_path, leaf) = split_path(&clean)?;
        let parent_inode = self.resolve_path(parent_path)?;
        parent_inode.unlink(leaf)
    }
}

/// Global singleton VFS instance.
pub static VFS: Vfs = Vfs::new();

/// Normalizes a path string: collapses consecutive slashes, resolves `.` and `..`, and removes trailing slashes.
pub fn clean_path(path: &str) -> String {
    let mut parts = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        } else if part == ".." {
            parts.pop();
        } else {
            parts.push(part);
        }
    }

    if parts.is_empty() {
        "/".to_string()
    } else {
        let mut res = String::new();
        for part in parts {
            res.push('/');
            res.push_str(part);
        }
        res
    }
}

/// Splits an absolute path into parent directory and leaf filename components.
///
/// Returns `(parent_path, leaf_name)`.
pub fn split_path(path: &str) -> Result<(&str, &str), FsError> {
    let clean = path.trim_end_matches('/');
    if let Some(idx) = clean.rfind('/') {
        let parent = if idx == 0 { "/" } else { &clean[..idx] };
        let leaf = &clean[idx + 1..];
        if leaf.is_empty() {
            Err(FsError::InvalidInput)
        } else {
            Ok((parent, leaf))
        }
    } else {
        Ok(("/", clean))
    }
}
