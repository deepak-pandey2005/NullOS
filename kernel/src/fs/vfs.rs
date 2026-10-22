//! Virtual File System (VFS) Abstraction (Phase 6 - M16 VFS).
//!
//! Provides an OS-level unified abstraction for filesystems, inodes, directory traversal,
//! mount points, and file access.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::sync::Spinlock;

/// VFS error codes conforming to standard POSIX-like error conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum FsError {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    NotADirectory,
    IsADirectory,
    InvalidInput,
    IoError,
    NoSpace,
    Unsupported,
    BadFileDescriptor,
}

/// The type of file node in the VFS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum FileType {
    File,
    Directory,
    CharDevice,
    BlockDevice,
    Fifo,
    Socket,
}

/// File metadata returned by stat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStat {
    pub inode_num: u64,
    pub file_type: FileType,
    pub size: usize,
    pub mode: u32,
}

/// Directory entry item returned when listing a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct DirEntry {
    pub name: String,
    pub inode_num: u64,
    pub file_type: FileType,
}

/// Flags specified when opening a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenFlags {
    pub read: bool,
    pub write: bool,
    pub create: bool,
    pub append: bool,
    pub truncate: bool,
}

impl OpenFlags {
    pub const READ_ONLY: Self = Self {
        read: true,
        write: false,
        create: false,
        append: false,
        truncate: false,
    };

    pub const WRITE_ONLY: Self = Self {
        read: false,
        write: true,
        create: false,
        append: false,
        truncate: false,
    };

    pub const READ_WRITE: Self = Self {
        read: true,
        write: true,
        create: false,
        append: false,
        truncate: false,
    };

    /// Converts raw integer flags (e.g. from POSIX syscalls) into `OpenFlags`.
    pub fn from_bits(bits: u64) -> Self {
        let access = bits & 0x3;
        let read = access == 0 || access == 2; // O_RDONLY (0) or O_RDWR (2)
        let write = access == 1 || access == 2; // O_WRONLY (1) or O_RDWR (2)
        let create = (bits & 0o100) != 0; // O_CREAT (0100)
        let truncate = (bits & 0o1000) != 0; // O_TRUNC (01000)
        let append = (bits & 0o2000) != 0; // O_APPEND (02000)

        Self {
            read,
            write,
            create,
            append,
            truncate,
        }
    }
}

/// Core Inode interface that all filesystem drivers implement.
#[allow(dead_code)]
pub trait Inode: Send + Sync {
    /// Returns metadata about this inode.
    fn stat(&self) -> FileStat;

    /// Reads up to `buf.len()` bytes starting at `offset`. Returns the number of bytes read.
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, FsError>;

    /// Writes `buf.len()` bytes starting at `offset`. Returns the number of bytes written.
    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, FsError>;

    /// Looks up a child entry by name within this directory inode.
    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>, FsError>;

    /// Lists directory entries if this is a directory.
    fn read_dir(&self) -> Result<Vec<DirEntry>, FsError>;

    /// Creates a new child file or directory under this directory.
    fn create(&self, name: &str, file_type: FileType) -> Result<Arc<dyn Inode>, FsError>;

    /// Truncates file length to `size` bytes.
    fn truncate(&self, _size: usize) -> Result<(), FsError> {
        Ok(())
    }

    /// Removes a child entry by name from this directory.
    fn unlink(&self, _name: &str) -> Result<(), FsError> {
        Err(FsError::Unsupported)
    }
}

/// Core FileSystem trait implemented by file system drivers (Initramfs, FAT32, ext2).
#[allow(dead_code)]
pub trait FileSystem: Send + Sync {
    /// Name identifier for the filesystem driver (e.g. "ramfs", "fat32").
    fn name(&self) -> &str;

    /// Returns the root inode of this mounted filesystem instance.
    fn root(&self) -> Arc<dyn Inode>;
}

/// Global mount table and path resolution manager.
pub struct Vfs {
    mounts: Spinlock<BTreeMap<String, Arc<dyn FileSystem>>>,
}

impl Vfs {
    pub const fn new() -> Self {
        Self {
            mounts: Spinlock::new(BTreeMap::new()),
        }
    }

    /// Mounts a filesystem driver at a specified path mountpoint (e.g. "/" or "/mnt").
    pub fn mount(&self, mount_point: &str, fs: Arc<dyn FileSystem>) -> Result<(), FsError> {
        let clean_path = clean_path(mount_point);
        let mut mounts = self.mounts.lock();
        mounts.insert(clean_path, fs);
        Ok(())
    }

    /// Resolves an absolute path string into the corresponding `Inode`.
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

    /// Opens an inode with the given flags.
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

/// Normalizes a path string, removing duplicate slashes and trailing slashes.
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

/// Splits a path into parent directory and leaf filename.
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
