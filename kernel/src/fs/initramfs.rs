//! In-Memory RamFS & Initramfs Filesystem Driver (Phase 6 - M15 Initramfs).
//!
//! Provides a fully functional in-memory filesystem that supports directories, files,
//! dynamic reading, writing, and loading from TAR archives.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use super::tar::{TarArchive, TarType};
use super::vfs::{DirEntry, FileStat, FileType, FileSystem, FsError, Inode};
use crate::drivers::serial;
use crate::sync::Spinlock;

static NEXT_INODE: AtomicU64 = AtomicU64::new(1);

fn alloc_inode_num() -> u64 {
    NEXT_INODE.fetch_add(1, Ordering::SeqCst)
}

/// An in-memory regular file node.
pub struct RamFile {
    inode_num: u64,
    data: Spinlock<Vec<u8>>,
    mode: u32,
}

impl RamFile {
    pub fn new(initial_data: &[u8], mode: u32) -> Self {
        Self {
            inode_num: alloc_inode_num(),
            data: Spinlock::new(initial_data.to_vec()),
            mode,
        }
    }
}

impl Inode for RamFile {
    fn stat(&self) -> FileStat {
        let data = self.data.lock();
        FileStat {
            inode_num: self.inode_num,
            file_type: FileType::File,
            size: data.len(),
            mode: self.mode,
        }
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        let data = self.data.lock();
        if offset >= data.len() {
            return Ok(0); // EOF
        }
        let available = data.len() - offset;
        let to_copy = core::cmp::min(available, buf.len());
        buf[..to_copy].copy_from_slice(&data[offset..offset + to_copy]);
        Ok(to_copy)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, FsError> {
        let mut data = self.data.lock();
        let end = offset + buf.len();
        if end > data.len() {
            data.resize(end, 0);
        }
        data[offset..end].copy_from_slice(buf);
        Ok(buf.len())
    }

    fn lookup(&self, _name: &str) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn read_dir(&self) -> Result<Vec<DirEntry>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn create(&self, _name: &str, _file_type: FileType) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::NotADirectory)
    }
}

/// An in-memory directory node.
pub struct RamDirectory {
    inode_num: u64,
    children: Spinlock<BTreeMap<String, Arc<dyn Inode>>>,
    dirs: Spinlock<BTreeMap<String, Arc<RamDirectory>>>,
    mode: u32,
}

impl RamDirectory {
    pub fn new(mode: u32) -> Self {
        Self {
            inode_num: alloc_inode_num(),
            children: Spinlock::new(BTreeMap::new()),
            dirs: Spinlock::new(BTreeMap::new()),
            mode,
        }
    }

    pub fn insert_child(&self, name: &str, node: Arc<dyn Inode>) {
        let mut children = self.children.lock();
        children.insert(name.to_string(), node);
    }

    pub fn insert_dir(&self, name: &str, dir: Arc<RamDirectory>) {
        self.children.lock().insert(name.to_string(), dir.clone());
        self.dirs.lock().insert(name.to_string(), dir);
    }

    #[allow(dead_code)]
    pub fn get_dir(&self, name: &str) -> Option<Arc<RamDirectory>> {
        self.dirs.lock().get(name).cloned()
    }
}

impl Inode for RamDirectory {
    fn stat(&self) -> FileStat {
        let children = self.children.lock();
        FileStat {
            inode_num: self.inode_num,
            file_type: FileType::Directory,
            size: children.len(),
            mode: self.mode,
        }
    }

    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize, FsError> {
        Err(FsError::IsADirectory)
    }

    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize, FsError> {
        Err(FsError::IsADirectory)
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>, FsError> {
        let children = self.children.lock();
        children.get(name).cloned().ok_or(FsError::NotFound)
    }

    fn read_dir(&self) -> Result<Vec<DirEntry>, FsError> {
        let children = self.children.lock();
        let mut entries = Vec::new();
        for (name, node) in children.iter() {
            let stat = node.stat();
            entries.push(DirEntry {
                name: name.clone(),
                inode_num: stat.inode_num,
                file_type: stat.file_type,
            });
        }
        Ok(entries)
    }

    fn create(&self, name: &str, file_type: FileType) -> Result<Arc<dyn Inode>, FsError> {
        let mut children = self.children.lock();
        if children.contains_key(name) {
            return Err(FsError::AlreadyExists);
        }

        let new_node: Arc<dyn Inode> = match file_type {
            FileType::File => Arc::new(RamFile::new(&[], 0o644)),
            FileType::Directory => Arc::new(RamDirectory::new(0o755)),
            _ => return Err(FsError::Unsupported),
        };

        children.insert(name.to_string(), new_node.clone());
        Ok(new_node)
    }
}

/// Character device node representing `/dev/console` and standard output.
pub struct ConsoleDevice {
    inode_num: u64,
}

impl ConsoleDevice {
    pub fn new() -> Self {
        Self {
            inode_num: alloc_inode_num(),
        }
    }
}

impl Inode for ConsoleDevice {
    fn stat(&self) -> FileStat {
        FileStat {
            inode_num: self.inode_num,
            file_type: FileType::CharDevice,
            size: 0,
            mode: 0o666,
        }
    }

    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize, FsError> {
        Ok(0) // Non-blocking EOF for basic console read
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize, FsError> {
        for &byte in buf {
            unsafe {
                serial::write_byte(byte);
            }
        }
        Ok(buf.len())
    }

    fn lookup(&self, _name: &str) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn read_dir(&self) -> Result<Vec<DirEntry>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn create(&self, _name: &str, _file_type: FileType) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::NotADirectory)
    }
}

/// Character device node representing `/dev/zero`.
pub struct ZeroDevice {
    inode_num: u64,
}

impl ZeroDevice {
    pub fn new() -> Self {
        Self {
            inode_num: alloc_inode_num(),
        }
    }
}

impl Inode for ZeroDevice {
    fn stat(&self) -> FileStat {
        FileStat {
            inode_num: self.inode_num,
            file_type: FileType::CharDevice,
            size: 0,
            mode: 0o666,
        }
    }

    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        buf.fill(0);
        Ok(buf.len())
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize, FsError> {
        Ok(buf.len())
    }

    fn lookup(&self, _name: &str) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn read_dir(&self) -> Result<Vec<DirEntry>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn create(&self, _name: &str, _file_type: FileType) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::NotADirectory)
    }
}

/// Character device node representing `/dev/null`.
pub struct NullDevice {
    inode_num: u64,
}

impl NullDevice {
    pub fn new() -> Self {
        Self {
            inode_num: alloc_inode_num(),
        }
    }
}

impl Inode for NullDevice {
    fn stat(&self) -> FileStat {
        FileStat {
            inode_num: self.inode_num,
            file_type: FileType::CharDevice,
            size: 0,
            mode: 0o666,
        }
    }

    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize, FsError> {
        Ok(0)
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize, FsError> {
        Ok(buf.len())
    }

    fn lookup(&self, _name: &str) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn read_dir(&self) -> Result<Vec<DirEntry>, FsError> {
        Err(FsError::NotADirectory)
    }

    fn create(&self, _name: &str, _file_type: FileType) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::NotADirectory)
    }
}

/// In-memory RamFS filesystem driver instance.
pub struct RamFs {
    root: Arc<RamDirectory>,
}

impl RamFs {
    pub fn new() -> Self {
        let root = Arc::new(RamDirectory::new(0o755));
        Self { root }
    }

    /// Populates standard root directories and device nodes (`/dev`, `/bin`, `/etc`, `/tmp`).
    pub fn populate_defaults(&self) {
        // Create /dev directory and devices
        let dev = Arc::new(RamDirectory::new(0o755));
        dev.insert_child("console", Arc::new(ConsoleDevice::new()));
        dev.insert_child("zero", Arc::new(ZeroDevice::new()));
        dev.insert_child("null", Arc::new(NullDevice::new()));
        self.root.insert_dir("dev", dev);

        // Create /etc directory and files
        let etc = Arc::new(RamDirectory::new(0o755));
        etc.insert_child(
            "os-release",
            Arc::new(RamFile::new(b"NAME=\"NullOS\"\nVERSION=\"0.1.0\"\nID=nullos\n", 0o644)),
        );
        etc.insert_child(
            "hostname",
            Arc::new(RamFile::new(b"nullos-box\n", 0o644)),
        );
        self.root.insert_dir("etc", etc);

        // Create /tmp directory
        let tmp = Arc::new(RamDirectory::new(0o777));
        self.root.insert_dir("tmp", tmp);

        // Create /bin directory with test executable placeholder
        let bin = Arc::new(RamDirectory::new(0o755));
        self.root.insert_dir("bin", bin);

        // Create root files
        self.root.insert_child(
            "hello.txt",
            Arc::new(RamFile::new(b"Hello from NullOS Initramfs VFS!\n", 0o644)),
        );
    }

    /// Recursively populates directories and files from a parsed TAR archive.
    #[allow(dead_code)]
    pub fn populate_from_tar(&self, archive: &TarArchive) {
        for entry in TarArchive::new(archive.data) {
            let path = entry.name.trim_start_matches('/');
            let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
            if segments.is_empty() {
                continue;
            }

            let mut current_dir = self.root.clone();

            // Navigate / create intermediate directories
            for &seg in &segments[..segments.len() - 1] {
                let next_dir = match current_dir.get_dir(seg) {
                    Some(dir) => dir,
                    None => {
                        let new_dir = Arc::new(RamDirectory::new(0o755));
                        current_dir.insert_dir(seg, new_dir.clone());
                        new_dir
                    }
                };
                current_dir = next_dir;
            }

            let leaf_name = segments.last().unwrap();
            match entry.entry_type {
                TarType::NormalFile => {
                    let file = Arc::new(RamFile::new(entry.data, entry.mode));
                    current_dir.insert_child(leaf_name, file);
                }
                TarType::Directory => {
                    if current_dir.get_dir(leaf_name).is_none() {
                        let new_dir = Arc::new(RamDirectory::new(entry.mode));
                        current_dir.insert_dir(leaf_name, new_dir);
                    }
                }
                _ => {}
            }
        }
    }
}

impl FileSystem for RamFs {
    fn name(&self) -> &str {
        "ramfs"
    }

    fn root(&self) -> Arc<dyn Inode> {
        self.root.clone()
    }
}
