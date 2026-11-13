//! In-Memory RamFS & Initramfs Filesystem Driver (Phase 6 - M15 Initramfs).
//!
//! # Architecture Overview
//!
//! `RamFs` is a volatile in-memory filesystem that implements the VFS [`FileSystem`]
//! and [`Inode`] traits. It serves two essential purposes in NullOS:
//! 1. **Early Boot Initramfs**: Acts as the initial root filesystem (`"/"`) before
//!    persistent storage drivers (SATA, NVMe, VirtIO) and partitions are probed.
//! 2. **Standard Device Node Provider**: Hosts volatile virtual character devices
//!    (`/dev/console`, `/dev/null`, `/dev/zero`) and temporary directory spaces (`/tmp`).
//!
//! # Node Types
//! - [`RamFile`]: In-memory regular file containing a dynamically resizable byte buffer (`Vec<u8>`).
//! - [`RamDirectory`]: In-memory directory maintaining an ordered map of child names to inodes.
//! - [`ConsoleDevice`]: Special device routing write operations to the serial UART output.
//! - [`ZeroDevice`]: Reads yield continuous streams of zero bytes.
//! - [`NullDevice`]: Discards all written data; reads yield immediate EOF.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use super::tar::{TarArchive, TarType};
use super::vfs::{DirEntry, FileStat, FileType, FileSystem, FsError, Inode};
use crate::drivers::serial;
use crate::println;
use crate::sync::Spinlock;

/// Monotonically increasing atomic counter allocating unique 64-bit inode numbers.
static NEXT_INODE: AtomicU64 = AtomicU64::new(1);

/// Allocates a new unique 64-bit inode identification number.
fn alloc_inode_num() -> u64 {
    NEXT_INODE.fetch_add(1, Ordering::SeqCst)
}

/// An in-memory regular file node storing byte data in a heap buffer.
pub struct RamFile {
    inode_num: u64,
    data: Spinlock<Vec<u8>>,
    mode: u32,
}

impl RamFile {
    /// Creates a new in-memory file pre-initialized with the specified byte slice and mode.
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

    fn truncate(&self, size: usize) -> Result<(), FsError> {
        let mut data = self.data.lock();
        data.truncate(size);
        Ok(())
    }
}

/// An in-memory directory node storing child inodes in an associative map.
pub struct RamDirectory {
    inode_num: u64,
    children: Spinlock<BTreeMap<String, Arc<dyn Inode>>>,
    dirs: Spinlock<BTreeMap<String, Arc<RamDirectory>>>,
    mode: u32,
}

impl RamDirectory {
    /// Creates a new empty directory node with the specified mode bits.
    pub fn new(mode: u32) -> Self {
        Self {
            inode_num: alloc_inode_num(),
            children: Spinlock::new(BTreeMap::new()),
            dirs: Spinlock::new(BTreeMap::new()),
            mode,
        }
    }

    /// Inserts a child inode under the directory entry `name`.
    pub fn insert_child(&self, name: &str, node: Arc<dyn Inode>) {
        let mut children = self.children.lock();
        children.insert(name.to_string(), node);
    }

    /// Inserts a typed directory child, tracking it in both `children` and `dirs`.
    pub fn insert_dir(&self, name: &str, dir: Arc<RamDirectory>) {
        self.children.lock().insert(name.to_string(), dir.clone());
        self.dirs.lock().insert(name.to_string(), dir);
    }

    /// Retrieves a reference to a child directory if present.
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

    fn unlink(&self, name: &str) -> Result<(), FsError> {
        let mut children = self.children.lock();
        if children.remove(name).is_some() {
            self.dirs.lock().remove(name);
            Ok(())
        } else {
            Err(FsError::NotFound)
        }
    }

    fn as_any(&self) -> Option<&dyn core::any::Any> {
        Some(self)
    }
}

/// Character device node representing `/dev/console` and standard output.
pub struct ConsoleDevice {
    inode_num: u64,
}

impl ConsoleDevice {
    /// Creates a new console device node.
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

    fn poll(&self) -> u32 {
        let mut revents = crate::fs::vfs::POLLOUT;
        if crate::drivers::keyboard::has_data() || crate::drivers::serial::has_byte() {
            revents |= crate::fs::vfs::POLLIN;
        }
        revents
    }
}

/// Character device node representing `/dev/zero`.
pub struct ZeroDevice {
    inode_num: u64,
}

impl ZeroDevice {
    /// Creates a new zero device node.
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

    fn poll(&self) -> u32 {
        crate::fs::vfs::POLLIN | crate::fs::vfs::POLLOUT
    }
}

/// Character device node representing `/dev/null`.
pub struct NullDevice {
    inode_num: u64,
}

impl NullDevice {
    /// Creates a new null sink device node.
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

    fn poll(&self) -> u32 {
        crate::fs::vfs::POLLIN | crate::fs::vfs::POLLOUT
    }
}

/// Block device node representing a raw storage device (e.g. `/dev/virtio0`, `/dev/ahci0`).
pub struct BlockDeviceNode {
    inode_num: u64,
    device: Arc<dyn crate::drivers::block::BlockDevice>,
}

impl BlockDeviceNode {
    /// Creates a new block device node wrapping `device`.
    pub fn new(device: Arc<dyn crate::drivers::block::BlockDevice>) -> Self {
        Self {
            inode_num: alloc_inode_num(),
            device,
        }
    }
}

impl Inode for BlockDeviceNode {
    fn stat(&self) -> FileStat {
        FileStat {
            inode_num: self.inode_num,
            file_type: FileType::BlockDevice,
            size: self.device.capacity_bytes() as usize,
            mode: 0o660,
        }
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        let sector_size = self.device.sector_size();
        if sector_size == 0 || buf.is_empty() {
            return Ok(0);
        }
        let total_capacity = self.device.capacity_bytes() as usize;
        if offset >= total_capacity {
            return Ok(0); // EOF
        }

        let to_read = core::cmp::min(buf.len(), total_capacity - offset);
        let start_sector = (offset / sector_size) as u64;
        let sector_offset = offset % sector_size;
        let end_sector = ((offset + to_read + sector_size - 1) / sector_size) as u64;

        let mut read_bytes = 0usize;
        let mut temp_sec = vec![0u8; sector_size];

        for sec in start_sector..end_sector {
            self.device
                .read_sector(sec, &mut temp_sec)
                .map_err(|_| FsError::IoError)?;

            let src_start = if sec == start_sector { sector_offset } else { 0 };
            let src_end = if sec == end_sector - 1 {
                let tail = (offset + to_read) % sector_size;
                if tail == 0 { sector_size } else { tail }
            } else {
                sector_size
            };

            let chunk_len = src_end.saturating_sub(src_start);
            let dst_end = read_bytes + chunk_len;
            if dst_end > to_read {
                break;
            }
            buf[read_bytes..dst_end].copy_from_slice(&temp_sec[src_start..src_end]);
            read_bytes += chunk_len;
        }

        Ok(read_bytes)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, FsError> {
        let sector_size = self.device.sector_size();
        if sector_size == 0 || buf.is_empty() {
            return Ok(0);
        }
        let total_capacity = self.device.capacity_bytes() as usize;
        if offset >= total_capacity {
            return Err(FsError::NoSpace);
        }

        let to_write = core::cmp::min(buf.len(), total_capacity - offset);
        let start_sector = (offset / sector_size) as u64;
        let sector_offset = offset % sector_size;
        let end_sector = ((offset + to_write + sector_size - 1) / sector_size) as u64;

        let mut written_bytes = 0usize;
        let mut temp_sec = vec![0u8; sector_size];

        for sec in start_sector..end_sector {
            let is_partial = (sec == start_sector && sector_offset != 0)
                || (sec == end_sector - 1 && ((offset + to_write) % sector_size) != 0);

            if is_partial {
                self.device
                    .read_sector(sec, &mut temp_sec)
                    .map_err(|_| FsError::IoError)?;
            }

            let dst_start = if sec == start_sector { sector_offset } else { 0 };
            let dst_end = if sec == end_sector - 1 {
                let tail = (offset + to_write) % sector_size;
                if tail == 0 { sector_size } else { tail }
            } else {
                sector_size
            };

            let chunk_len = dst_end.saturating_sub(dst_start);
            let src_end = written_bytes + chunk_len;
            if src_end > to_write {
                break;
            }
            temp_sec[dst_start..dst_end].copy_from_slice(&buf[written_bytes..src_end]);

            self.device
                .write_sector(sec, &temp_sec)
                .map_err(|_| FsError::IoError)?;
            written_bytes += chunk_len;
        }

        Ok(written_bytes)
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

    fn poll(&self) -> u32 {
        crate::fs::vfs::POLLIN | crate::fs::vfs::POLLOUT
    }

    fn as_any(&self) -> Option<&dyn core::any::Any> {
        Some(self)
    }
}

/// Global packet reception queue for userspace TAP/TUN daemons reading from `/dev/net/tun0`.
static TUN_RX_QUEUE: Spinlock<VecDeque<Vec<u8>>> = Spinlock::new(VecDeque::new());

/// Enqueues an incoming or loopback raw Ethernet frame for userspace network daemons.
pub fn tun_enqueue_rx(packet: &[u8]) {
    let mut q = TUN_RX_QUEUE.lock();
    if q.len() >= 64 {
        q.pop_front();
    }
    let mut v = Vec::with_capacity(packet.len());
    v.extend_from_slice(packet);
    q.push_back(v);
}

/// Character device node representing a raw network TAP/TUN device (`/dev/net/tun0` and `/dev/tun0`).
///
/// Implements user-mode network stack packet transport (Issue #43):
/// - `write_at`: Transmits raw Ethernet frame to the active network driver (VirtIO, e1000, r8169).
/// - `read_at`: Reads next raw Ethernet frame from the userspace packet RX queue (or polls hardware NIC).
/// - `poll`: Reports POLLOUT (always write-ready) and POLLIN (when packets are queued).
pub struct TunDeviceNode {
    #[allow(dead_code)]
    name: String,
    inode_num: u64,
}

impl TunDeviceNode {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            inode_num: alloc_inode_num(),
        }
    }

    #[allow(dead_code)]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Inode for TunDeviceNode {
    fn stat(&self) -> FileStat {
        FileStat {
            inode_num: self.inode_num,
            file_type: FileType::CharDevice,
            size: 0,
            mode: 0o666,
        }
    }

    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        // First, check if a packet is queued in TUN_RX_QUEUE
        if let Some(pkt) = TUN_RX_QUEUE.lock().pop_front() {
            let to_copy = core::cmp::min(buf.len(), pkt.len());
            buf[..to_copy].copy_from_slice(&pkt[..to_copy]);
            return Ok(to_copy);
        }

        // Second, try polling the active NIC driver directly
        let mut temp_rx = [0u8; 1536];
        if let Ok(Some(len)) = crate::drivers::virtio_net::receive(&mut temp_rx) {
            let to_copy = core::cmp::min(buf.len(), len);
            buf[..to_copy].copy_from_slice(&temp_rx[..to_copy]);
            return Ok(to_copy);
        }

        Ok(0) // Non-blocking: no packet currently available
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize, FsError> {
        if buf.is_empty() {
            return Ok(0);
        }

        // Transmit frame via active hardware/virtual network interface
        let _ = crate::drivers::virtio_net::send(buf);

        // Echo back packet to RX queue for userspace loopback & demuxing
        tun_enqueue_rx(buf);

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

    fn poll(&self) -> u32 {
        let mut revents = crate::fs::vfs::POLLOUT;
        if !TUN_RX_QUEUE.lock().is_empty() {
            revents |= crate::fs::vfs::POLLIN;
        }
        revents
    }

    fn as_any(&self) -> Option<&dyn core::any::Any> {
        Some(self)
    }
}

/// Character device node representing `/dev/audio` and `/dev/snd/pcm0`.
///
/// Implements Ring 3 userspace audio streaming (Issue #44):
/// - `write_at`: Streams raw 16-bit interleaved PCM audio frames directly into the hardware DMA audio engine.
/// - `poll`: Reports POLLOUT (ready for audio sample writes).
pub struct AudioDeviceNode {
    #[allow(dead_code)]
    name: String,
    inode_num: u64,
}

impl AudioDeviceNode {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            inode_num: alloc_inode_num(),
        }
    }

    #[allow(dead_code)]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Inode for AudioDeviceNode {
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
        if buf.is_empty() {
            return Ok(0);
        }
        let _ = crate::drivers::audio::play_pcm_bytes(buf);
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

    fn poll(&self) -> u32 {
        crate::fs::vfs::POLLOUT
    }

    fn as_any(&self) -> Option<&dyn core::any::Any> {
        Some(self)
    }
}

/// Character device node representing `/dev/power` and `/dev/acpi`.
///
/// Implements Ring 3 userspace ACPI power management (Issue #45):
/// - `read_at`: Reads raw ACPI DSDT table and AML bytecode bytes for Ring 3 AML interpretation.
/// - `write_at`: Receives validated sleep states (SLP_TYPa, SLP_TYPb) and power directives from /bin/powerd.elf.
/// - `poll`: Reports POLLIN | POLLOUT.
pub struct PowerDeviceNode {
    #[allow(dead_code)]
    name: String,
    inode_num: u64,
}

impl PowerDeviceNode {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            inode_num: alloc_inode_num(),
        }
    }

    #[allow(dead_code)]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Inode for PowerDeviceNode {
    fn stat(&self) -> FileStat {
        let size = crate::arch::x86_64::acpi::ACPI_INFO.lock()
            .as_ref()
            .and_then(|info| info.dsdt)
            .map(|d| d.length)
            .unwrap_or(256);

        FileStat {
            inode_num: self.inode_num,
            file_type: FileType::CharDevice,
            size,
            mode: 0o666,
        }
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        let guard = crate::arch::x86_64::acpi::ACPI_INFO.lock();
        if let Some(ref acpi) = *guard {
            if let Some(dsdt) = acpi.dsdt {
                if dsdt.address != 0 && dsdt.length > 0 {
                    if offset >= dsdt.length {
                        return Ok(0); // EOF
                    }
                    let avail = dsdt.length - offset;
                    let to_read = core::cmp::min(buf.len(), avail);
                    unsafe {
                        let src = (dsdt.address as *const u8).add(offset);
                        core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), to_read);
                    }
                    return Ok(to_read);
                }
            }
        }

        // Fallback minimal DSDT block with _S5 package if ACPI DSDT is not yet mapped
        static FALLBACK_DSDT: &[u8] = &[
            b'D', b'S', b'D', b'T', // Signature
            0x28, 0x00, 0x00, 0x00, // Length (40 bytes)
            0x02,                   // Revision
            0x00,                   // Checksum
            b'N', b'U', b'L', b'L', b'O', b'S', // OEM ID
            b'D', b'S', b'D', b'T', 0x20, 0x20, 0x20, 0x20, // OEM Table ID
            0x01, 0x00, 0x00, 0x00, // OEM Revision
            b'I', b'N', b'T', b'L', // Creator ID
            0x01, 0x00, 0x00, 0x00, // Creator Revision
            // AML Payload: Name(_S5, Package(4) { 0x00, 0x00, 0x00, 0x00 })
            0x08, b'_', b'S', b'5', b'_', 0x12, 0x06, 0x04, 0x00, 0x00, 0x00, 0x00,
        ];

        if offset >= FALLBACK_DSDT.len() {
            return Ok(0);
        }
        let avail = FALLBACK_DSDT.len() - offset;
        let to_read = core::cmp::min(buf.len(), avail);
        buf[..to_read].copy_from_slice(&FALLBACK_DSDT[offset..offset + to_read]);
        Ok(to_read)
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize, FsError> {
        if buf.len() >= 6 {
            // Power management command packet: [cmd: u32, slp_typa: u8, slp_typb: u8]
            let cmd = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
            let slp_typa = buf[4];
            let slp_typb = buf[5];

            let mut guard = crate::arch::x86_64::acpi::ACPI_INFO.lock();
            if let Some(ref mut acpi) = *guard {
                if let Some(ref mut dsdt) = acpi.dsdt {
                    dsdt.slp_typa = slp_typa;
                    dsdt.slp_typb = slp_typb;
                    dsdt.s5_supported = true;
                }
            }

            println!(
                "[POWER] Userspace powerd configured ACPI S5 parameters: SLP_TYPa=0x{:02x}, SLP_TYPb=0x{:02x} (cmd={})",
                slp_typa, slp_typb, cmd
            );
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

    fn poll(&self) -> u32 {
        crate::fs::vfs::POLLIN | crate::fs::vfs::POLLOUT
    }

    fn as_any(&self) -> Option<&dyn core::any::Any> {
        Some(self)
    }
}

/// In-memory RamFS filesystem driver instance.
pub struct RamFs {
    root: Arc<RamDirectory>,
}

impl RamFs {
    /// Creates a new RamFS filesystem instance with an initial root directory.
    pub fn new() -> Self {
        let root = Arc::new(RamDirectory::new(0o755));
        Self { root }
    }

    /// Populates standard root directories and device nodes (`/dev`, `/bin`, `/lib`, `/etc`, `/tmp`, `/disk`, `/mnt`).
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

        // Create /bin directory with test executable
        let bin = Arc::new(RamDirectory::new(0o755));
        bin.insert_child(
            "hello.elf",
            Arc::new(RamFile::new(include_bytes!("../../../user/build/bin/hello.elf"), 0o755)),
        );
        self.root.insert_dir("bin", bin);

        // Create /lib directory with libc.so shared library (M34)
        let lib = Arc::new(RamDirectory::new(0o755));
        lib.insert_child(
            "libc.so",
            Arc::new(RamFile::new(include_bytes!("../../../user/libc/build/libc.so"), 0o755)),
        );
        self.root.insert_dir("lib", lib);

        // Create /disk and /mnt mountpoint directories for persistent storage
        let disk = Arc::new(RamDirectory::new(0o755));
        self.root.insert_dir("disk", disk);
        let mnt = Arc::new(RamDirectory::new(0o755));
        self.root.insert_dir("mnt", mnt);

        // Create root files
        self.root.insert_child(
            "hello.txt",
            Arc::new(RamFile::new(b"Hello from NullOS Initramfs VFS!\n", 0o644)),
        );
    }

    /// Recursively populates directories and files from a parsed TAR archive.
    ///
    /// Normalizes leading slashes and `.` path components so POSIX USTAR archives
    /// packed relative to a staging directory (`./bin/...` or `bin/...`) map directly
    /// under `/bin/...`, `/lib/...`, `/etc/...`, and `/pkg/...`.
    pub fn populate_from_tar(&self, archive: &TarArchive) -> usize {
        let mut unpacked_entries = 0usize;
        for entry in TarArchive::new(archive.data) {
            let path = entry.name.trim_start_matches('/');
            let segments: Vec<&str> = path
                .split('/')
                .filter(|s| !s.is_empty() && *s != ".")
                .collect();
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
                    unpacked_entries += 1;
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
        unpacked_entries
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
