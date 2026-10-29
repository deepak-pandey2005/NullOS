//! FAT32 Persistent Filesystem Driver (Phase 6 - M19 Filesystems).
//!
//! Implements the Microsoft FAT32 specification over any `BlockDevice`:
//! - BIOS Parameter Block (BPB) and Extended BPB parsing.
//! - File Allocation Table (FAT) cluster chain traversal, allocation, and linking.
//! - Directory traversal (8.3 filenames with case-insensitive matching).
//! - File and Directory `Inode` implementation (`read_at`, `write_at`, `lookup`, `read_dir`, `create`).
//! - Full integration with the VFS mount table.
//!
//! // TODO(fat32-lfn): Implement Microsoft VFAT Long File Name (LFN) entries (ATTR_LFN = 0x0F).
//! // Currently, filenames are strictly constrained to 8.3 short format (e.g. `README.TXT`).
#![allow(dead_code)]

use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use crate::sync::Spinlock;
use crate::drivers::block::{BlockDevice, SECTOR_SIZE};
use crate::fs::vfs::{DirEntry, FileStat, FileType, FileSystem, FsError, Inode};

/// FAT32 Cluster Constants
const FAT32_EOC_MIN: u32 = 0x0FFF_FFF8;
const FAT32_EOC: u32     = 0x0FFF_FFFF;
const FAT32_MASK: u32    = 0x0FFF_FFFF;

/// Attributes in standard 32-byte FAT directory entries
const ATTR_READ_ONLY: u8 = 0x01;
const ATTR_HIDDEN: u8    = 0x02;
const ATTR_SYSTEM: u8    = 0x04;
const ATTR_VOLUME_ID: u8 = 0x08;
const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_ARCHIVE: u8   = 0x20;
const ATTR_LFN: u8       = 0x0F;

/// Raw 32-byte directory entry layout on disk.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct RawDirEntry {
    pub name: [u8; 8],
    pub ext: [u8; 3],
    pub attr: u8,
    pub reserved: u8,
    pub ctime_tenth: u8,
    pub ctime: u16,
    pub cdate: u16,
    pub adate: u16,
    pub cluster_high: u16,
    pub mtime: u16,
    pub mdate: u16,
    pub cluster_low: u16,
    pub file_size: u32,
}

/// Parsed Boot Sector / BPB configuration.
#[derive(Debug, Clone)]
pub struct Fat32Bpb {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sector_count: u16,
    pub num_fats: u8,
    pub total_sectors: u32,
    pub sectors_per_fat: u32,
    pub root_cluster: u32,
    pub data_start_sector: u64,
}

impl Fat32Bpb {
    /// Parses and validates the FAT32 Boot Sector from the first 512 bytes of a disk.
    pub fn parse(sector0: &[u8]) -> Result<Self, FsError> {
        if sector0.len() < 512 {
            return Err(FsError::InvalidInput);
        }

        // Check boot signature (0x55, 0xAA) at offset 510
        if sector0[510] != 0x55 || sector0[511] != 0xAA {
            return Err(FsError::InvalidInput);
        }

        let bytes_per_sector = u16::from_le_bytes([sector0[11], sector0[12]]);
        let sectors_per_cluster = sector0[13];
        let reserved_sector_count = u16::from_le_bytes([sector0[14], sector0[15]]);
        let num_fats = sector0[16];

        if bytes_per_sector != 512 || sectors_per_cluster == 0 || num_fats == 0 {
            return Err(FsError::InvalidInput);
        }

        let total_sectors_16 = u16::from_le_bytes([sector0[19], sector0[20]]);
        let total_sectors_32 = u32::from_le_bytes([sector0[32], sector0[33], sector0[34], sector0[35]]);
        let total_sectors = if total_sectors_16 != 0 {
            total_sectors_16 as u32
        } else {
            total_sectors_32
        };

        let sectors_per_fat = u32::from_le_bytes([sector0[36], sector0[37], sector0[38], sector0[39]]);
        let root_cluster = u32::from_le_bytes([sector0[44], sector0[45], sector0[46], sector0[47]]);

        if sectors_per_fat == 0 || root_cluster < 2 {
            return Err(FsError::InvalidInput);
        }

        let data_start_sector = reserved_sector_count as u64 + (num_fats as u64 * sectors_per_fat as u64);

        Ok(Self {
            bytes_per_sector,
            sectors_per_cluster,
            reserved_sector_count,
            num_fats,
            total_sectors,
            sectors_per_fat,
            root_cluster,
            data_start_sector,
        })
    }
}

/// A mounted FAT32 Filesystem instance.
pub struct Fat32FileSystem {
    pub block_dev: Arc<dyn BlockDevice>,
    pub bpb: Fat32Bpb,
    inode_counter: AtomicUsize,
    lock: Spinlock<()>,
}

impl Fat32FileSystem {
    /// Mounts a FAT32 filesystem from a block device.
    pub fn new(block_dev: Arc<dyn BlockDevice>) -> Result<Arc<Self>, FsError> {
        let mut sector0 = [0u8; SECTOR_SIZE];
        block_dev.read_sector(0, &mut sector0).map_err(|_| FsError::IoError)?;

        let bpb = Fat32Bpb::parse(&sector0)?;

        let fs = Arc::new(Self {
            block_dev,
            bpb,
            inode_counter: AtomicUsize::new(2),
            lock: Spinlock::new(()),
        });

        Ok(fs)
    }

    /// Converts a cluster number (>= 2) into its corresponding starting LBA sector.
    pub fn cluster_to_sector(&self, cluster: u32) -> u64 {
        if cluster < 2 {
            return self.bpb.data_start_sector;
        }
        self.bpb.data_start_sector + (cluster as u64 - 2) * (self.bpb.sectors_per_cluster as u64)
    }

    /// Reads the next cluster in the chain from the File Allocation Table (FAT).
    pub fn get_next_cluster(&self, cluster: u32) -> Result<Option<u32>, FsError> {
        let fat_offset = cluster as u64 * 4;
        let fat_sector = self.bpb.reserved_sector_count as u64 + (fat_offset / 512);
        let byte_offset = (fat_offset % 512) as usize;

        let mut sector_buf = [0u8; SECTOR_SIZE];
        self.block_dev.read_sector(fat_sector, &mut sector_buf).map_err(|_| FsError::IoError)?;

        let entry = u32::from_le_bytes([
            sector_buf[byte_offset],
            sector_buf[byte_offset + 1],
            sector_buf[byte_offset + 2],
            sector_buf[byte_offset + 3],
        ]) & FAT32_MASK;

        if entry >= FAT32_EOC_MIN {
            Ok(None)
        } else if entry < 2 {
            Ok(None)
        } else {
            Ok(Some(entry))
        }
    }

    /// Writes an entry into the File Allocation Table (FAT) across all FAT copies.
    pub fn set_fat_entry(&self, cluster: u32, next_cluster: u32) -> Result<(), FsError> {
        let fat_offset = cluster as u64 * 4;
        let byte_offset = (fat_offset % 512) as usize;

        let val_bytes = (next_cluster & FAT32_MASK).to_le_bytes();

        for fat_idx in 0..self.bpb.num_fats {
            let fat_start = self.bpb.reserved_sector_count as u64 + (fat_idx as u64 * self.bpb.sectors_per_fat as u64);
            let fat_sector = fat_start + (fat_offset / 512);

            let mut sector_buf = [0u8; SECTOR_SIZE];
            self.block_dev.read_sector(fat_sector, &mut sector_buf).map_err(|_| FsError::IoError)?;

            // Preserve top 4 bits of the existing entry per FAT32 specification
            sector_buf[byte_offset] = val_bytes[0];
            sector_buf[byte_offset + 1] = val_bytes[1];
            sector_buf[byte_offset + 2] = val_bytes[2];
            sector_buf[byte_offset + 3] = (sector_buf[byte_offset + 3] & 0xF0) | (val_bytes[3] & 0x0F);

            self.block_dev.write_sector(fat_sector, &sector_buf).map_err(|_| FsError::IoError)?;
        }

        Ok(())
    }

    /// Allocates a free cluster from the FAT, marks it as EOC, zeroes its contents, and returns its index.
    pub fn allocate_cluster(&self) -> Result<u32, FsError> {
        let total_clusters = (self.bpb.total_sectors - self.bpb.data_start_sector as u32) / (self.bpb.sectors_per_cluster as u32);

        // Scan FAT starting from cluster 2
        for cluster in 2..total_clusters {
            let fat_offset = cluster as u64 * 4;
            let fat_sector = self.bpb.reserved_sector_count as u64 + (fat_offset / 512);
            let byte_offset = (fat_offset % 512) as usize;

            let mut sector_buf = [0u8; SECTOR_SIZE];
            self.block_dev.read_sector(fat_sector, &mut sector_buf).map_err(|_| FsError::IoError)?;

            let entry = u32::from_le_bytes([
                sector_buf[byte_offset],
                sector_buf[byte_offset + 1],
                sector_buf[byte_offset + 2],
                sector_buf[byte_offset + 3],
            ]) & FAT32_MASK;

            if entry == 0 {
                // Found free cluster! Mark it as End of Chain (EOC)
                self.set_fat_entry(cluster, FAT32_EOC)?;

                // Zero out the newly allocated data sectors
                let start_sec = self.cluster_to_sector(cluster);
                let zero_buf = [0u8; SECTOR_SIZE];
                for s in 0..self.bpb.sectors_per_cluster {
                    let _ = self.block_dev.write_sector(start_sec + s as u64, &zero_buf);
                }

                return Ok(cluster);
            }
        }

        Err(FsError::NoSpace)
    }

    /// Reads all clusters in a chain into a single vector of bytes.
    pub fn read_cluster_chain(&self, start_cluster: u32) -> Result<Vec<u8>, FsError> {
        let mut data = Vec::new();
        let mut current_cluster = Some(start_cluster);
        let mut sector_buf = [0u8; SECTOR_SIZE];

        while let Some(cluster) = current_cluster {
            let start_sector = self.cluster_to_sector(cluster);
            for s in 0..self.bpb.sectors_per_cluster {
                self.block_dev.read_sector(start_sector + s as u64, &mut sector_buf).map_err(|_| FsError::IoError)?;
                data.extend_from_slice(&sector_buf);
            }

            current_cluster = self.get_next_cluster(cluster)?;
        }

        Ok(data)
    }
}

impl FileSystem for Fat32FileSystem {
    fn name(&self) -> &str {
        "fat32"
    }

    fn root(&self) -> Arc<dyn Inode> {
        Arc::new(Fat32Inode {
            fs: Arc::new(Self {
                block_dev: self.block_dev.clone(),
                bpb: self.bpb.clone(),
                inode_counter: AtomicUsize::new(self.inode_counter.load(Ordering::SeqCst)),
                lock: Spinlock::new(()),
            }),
            first_cluster: self.bpb.root_cluster,
            file_size: Spinlock::new(0),
            is_dir: true,
            inode_num: 2,
            dir_entry_location: None,
        })
    }
}

/// Location of a directory entry on disk so it can be updated when file size changes.
#[derive(Debug, Clone, Copy)]
pub struct DirEntryLocation {
    pub dir_cluster: u32,
    pub entry_offset: usize,
}

/// An Inode representing a FAT32 file or directory.
pub struct Fat32Inode {
    fs: Arc<Fat32FileSystem>,
    first_cluster: u32,
    file_size: Spinlock<usize>,
    is_dir: bool,
    inode_num: u64,
    dir_entry_location: Option<DirEntryLocation>,
}

impl Inode for Fat32Inode {
    fn stat(&self) -> FileStat {
        FileStat {
            inode_num: self.inode_num,
            file_type: if self.is_dir { FileType::Directory } else { FileType::File },
            size: *self.file_size.lock(),
            mode: if self.is_dir { 0o755 } else { 0o644 },
        }
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        if self.is_dir {
            return Err(FsError::IsADirectory);
        }

        let file_size = *self.file_size.lock();
        if offset >= file_size {
            return Ok(0);
        }

        let to_read = (file_size - offset).min(buf.len());
        let all_data = self.fs.read_cluster_chain(self.first_cluster)?;

        if offset < all_data.len() {
            let available = (all_data.len() - offset).min(to_read);
            buf[..available].copy_from_slice(&all_data[offset..offset + available]);
            Ok(available)
        } else {
            Ok(0)
        }
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize, FsError> {
        if self.is_dir {
            return Err(FsError::IsADirectory);
        }

        let _guard = self.fs.lock.lock();
        let bytes_per_cluster = (self.fs.bpb.sectors_per_cluster as usize) * 512;
        let needed_size = offset + buf.len();

        // 1. Gather or extend cluster chain
        let mut clusters = Vec::new();
        let mut curr = Some(self.first_cluster);
        while let Some(c) = curr {
            clusters.push(c);
            curr = self.fs.get_next_cluster(c)?;
        }

        let needed_clusters = (needed_size + bytes_per_cluster - 1) / bytes_per_cluster;
        while clusters.len() < needed_clusters {
            let new_c = self.fs.allocate_cluster()?;
            if let Some(&last_c) = clusters.last() {
                self.fs.set_fat_entry(last_c, new_c)?;
            }
            clusters.push(new_c);
        }

        // 2. Read full existing data or allocate empty buffer
        let mut full_data = self.fs.read_cluster_chain(self.first_cluster)?;
        if full_data.len() < needed_size {
            full_data.resize(needed_size, 0);
        }

        // 3. Overwrite buffer at offset
        full_data[offset..offset + buf.len()].copy_from_slice(buf);

        // 4. Write clusters back to disk
        for (i, &cluster) in clusters.iter().enumerate() {
            let chunk_start = i * bytes_per_cluster;
            if chunk_start >= full_data.len() {
                break;
            }
            let chunk_end = (chunk_start + bytes_per_cluster).min(full_data.len());
            let sector_start = self.fs.cluster_to_sector(cluster);

            let mut cluster_buf = vec![0u8; bytes_per_cluster];
            cluster_buf[..chunk_end - chunk_start].copy_from_slice(&full_data[chunk_start..chunk_end]);

            for s in 0..self.fs.bpb.sectors_per_cluster {
                let sec_idx = s as usize;
                let sec_slice = &cluster_buf[sec_idx * 512..(sec_idx + 1) * 512];
                self.fs.block_dev.write_sector(sector_start + s as u64, sec_slice).map_err(|_| FsError::IoError)?;
            }
        }

        // 5. Update in-memory and on-disk file size
        let mut size_lock = self.file_size.lock();
        if needed_size > *size_lock {
            *size_lock = needed_size;

            // Update directory entry on disk if location is known
            if let Some(loc) = self.dir_entry_location {
                let dir_data = self.fs.read_cluster_chain(loc.dir_cluster)?;
                if loc.entry_offset + 32 <= dir_data.len() {
                    let mut dir_entry_bytes = [0u8; 32];
                    dir_entry_bytes.copy_from_slice(&dir_data[loc.entry_offset..loc.entry_offset + 32]);

                    // Update size field at bytes 28..32
                    let size_bytes = (needed_size as u32).to_le_bytes();
                    dir_entry_bytes[28..32].copy_from_slice(&size_bytes);

                    // Write updated directory entry back to disk
                    let cluster_idx = loc.entry_offset / bytes_per_cluster;
                    let offset_in_cluster = loc.entry_offset % bytes_per_cluster;
                    let sector_in_cluster = (offset_in_cluster / 512) as u64;
                    let offset_in_sec = offset_in_cluster % 512;

                    let mut curr_dir = Some(loc.dir_cluster);
                    let mut target_cluster = loc.dir_cluster;
                    for _ in 0..cluster_idx {
                        if let Some(c) = curr_dir {
                            curr_dir = self.fs.get_next_cluster(c)?;
                            if let Some(next) = curr_dir { target_cluster = next; }
                        }
                    }

                    let target_sector = self.fs.cluster_to_sector(target_cluster) + sector_in_cluster;
                    let mut sec_buf = [0u8; SECTOR_SIZE];
                    let _ = self.fs.block_dev.read_sector(target_sector, &mut sec_buf);
                    sec_buf[offset_in_sec..offset_in_sec + 32].copy_from_slice(&dir_entry_bytes);
                    let _ = self.fs.block_dev.write_sector(target_sector, &sec_buf);
                }
            }
        }

        Ok(buf.len())
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>, FsError> {
        if !self.is_dir {
            return Err(FsError::NotADirectory);
        }

        let dir_data = self.fs.read_cluster_chain(self.first_cluster)?;
        let entries = parse_dir_entries(&dir_data);

        for (offset, raw) in entries {
            let entry_name = parse_83_name(&raw.name, &raw.ext);
            if entry_name.eq_ignore_ascii_case(name) {
                let cluster = ((raw.cluster_high as u32) << 16) | (raw.cluster_low as u32);
                let is_directory = (raw.attr & ATTR_DIRECTORY) != 0;

                return Ok(Arc::new(Fat32Inode {
                    fs: self.fs.clone(),
                    first_cluster: cluster,
                    file_size: Spinlock::new(raw.file_size as usize),
                    is_dir: is_directory,
                    inode_num: self.fs.inode_counter.fetch_add(1, Ordering::SeqCst) as u64,
                    dir_entry_location: Some(DirEntryLocation {
                        dir_cluster: self.first_cluster,
                        entry_offset: offset,
                    }),
                }));
            }
        }

        Err(FsError::NotFound)
    }

    fn read_dir(&self) -> Result<Vec<DirEntry>, FsError> {
        if !self.is_dir {
            return Err(FsError::NotADirectory);
        }

        let dir_data = self.fs.read_cluster_chain(self.first_cluster)?;
        let entries = parse_dir_entries(&dir_data);
        let mut result = Vec::new();

        for (_offset, raw) in entries {
            let entry_name = parse_83_name(&raw.name, &raw.ext);
            let is_directory = (raw.attr & ATTR_DIRECTORY) != 0;
            let cluster = ((raw.cluster_high as u32) << 16) | (raw.cluster_low as u32);

            result.push(DirEntry {
                name: entry_name,
                inode_num: cluster as u64,
                file_type: if is_directory { FileType::Directory } else { FileType::File },
            });
        }

        Ok(result)
    }

    fn create(&self, name: &str, file_type: FileType) -> Result<Arc<dyn Inode>, FsError> {
        if !self.is_dir {
            return Err(FsError::NotADirectory);
        }

        // Allocate a new cluster for the file
        let new_cluster = self.fs.allocate_cluster()?;
        let is_dir = file_type == FileType::Directory;

        // Create 8.3 formatted name
        let (name_part, ext_part) = match name.split_once('.') {
            Some((n, e)) => (n, e),
            None => (name, ""),
        };

        let mut raw_name = [b' '; 8];
        let mut raw_ext = [b' '; 3];

        for (i, b) in name_part.bytes().take(8).enumerate() {
            raw_name[i] = b.to_ascii_uppercase();
        }
        for (i, b) in ext_part.bytes().take(3).enumerate() {
            raw_ext[i] = b.to_ascii_uppercase();
        }

        let raw_entry = RawDirEntry {
            name: raw_name,
            ext: raw_ext,
            attr: if is_dir { ATTR_DIRECTORY } else { ATTR_ARCHIVE },
            reserved: 0,
            ctime_tenth: 0,
            ctime: 0,
            cdate: 0,
            adate: 0,
            cluster_high: ((new_cluster >> 16) & 0xFFFF) as u16,
            mtime: 0,
            mdate: 0,
            cluster_low: (new_cluster & 0xFFFF) as u16,
            file_size: 0,
        };

        let mut entry_bytes = [0u8; 32];
        unsafe {
            core::ptr::copy_nonoverlapping(
                &raw_entry as *const _ as *const u8,
                entry_bytes.as_mut_ptr(),
                32,
            );
        }

        // Find an empty entry slot in the directory
        let dir_data = self.fs.read_cluster_chain(self.first_cluster)?;
        let mut target_offset: Option<usize> = None;

        for (i, chunk) in dir_data.chunks_exact(32).enumerate() {
            if chunk[0] == 0x00 || chunk[0] == 0xE5 {
                target_offset = Some(i * 32);
                break;
            }
        }

        let bytes_per_cluster = (self.fs.bpb.sectors_per_cluster as usize) * 512;
        let entry_offset = match target_offset {
            Some(off) => off,
            None => dir_data.len(),
        };

        // Write the entry into the directory cluster
        let cluster_idx = entry_offset / bytes_per_cluster;
        let offset_in_cluster = entry_offset % bytes_per_cluster;
        let sector_in_cluster = (offset_in_cluster / 512) as u64;
        let offset_in_sec = offset_in_cluster % 512;

        let mut curr_dir = Some(self.first_cluster);
        let mut target_cluster = self.first_cluster;
        for _ in 0..cluster_idx {
            if let Some(c) = curr_dir {
                curr_dir = self.fs.get_next_cluster(c)?;
                if let Some(next) = curr_dir { target_cluster = next; }
            }
        }

        let target_sector = self.fs.cluster_to_sector(target_cluster) + sector_in_cluster;
        let mut sec_buf = [0u8; SECTOR_SIZE];
        let _ = self.fs.block_dev.read_sector(target_sector, &mut sec_buf);
        sec_buf[offset_in_sec..offset_in_sec + 32].copy_from_slice(&entry_bytes);
        let _ = self.fs.block_dev.write_sector(target_sector, &sec_buf);

        Ok(Arc::new(Fat32Inode {
            fs: self.fs.clone(),
            first_cluster: new_cluster,
            file_size: Spinlock::new(0),
            is_dir,
            inode_num: self.fs.inode_counter.fetch_add(1, Ordering::SeqCst) as u64,
            dir_entry_location: Some(DirEntryLocation {
                dir_cluster: self.first_cluster,
                entry_offset,
            }),
        }))
    }

    // TODO(fat32-unlink): Implement `unlink` to support persistent file deletion (`rm`) on FAT32:
    // 1. Locate the 32-byte directory entry matching `name`.
    // 2. Mark the entry as deleted by writing byte 0xE5 to `raw_entry.name[0]`.
    // 3. Traverse the file's cluster chain starting at `first_cluster` and write 0x00000000 to the FAT
    //    for each cluster to free the sectors back to the filesystem.
    // 4. Update the FAT32 FSInfo free cluster count.
    fn unlink(&self, _name: &str) -> Result<(), FsError> {
        Err(FsError::Unsupported)
    }
}

/// Helper function to parse 8.3 name into standard readable filename string.
fn parse_83_name(name_bytes: &[u8; 8], ext_bytes: &[u8; 3]) -> String {
    let name_str = core::str::from_utf8(name_bytes).unwrap_or("").trim();
    let ext_str = core::str::from_utf8(ext_bytes).unwrap_or("").trim();

    if ext_str.is_empty() {
        name_str.to_string()
    } else {
        format!("{}.{}", name_str, ext_str)
    }
}

/// Helper function to parse valid 32-byte directory entries from raw cluster byte buffer.
fn parse_dir_entries(dir_data: &[u8]) -> Vec<(usize, RawDirEntry)> {
    let mut entries = Vec::new();

    for (i, chunk) in dir_data.chunks_exact(32).enumerate() {
        let first_byte = chunk[0];
        // 0x00 indicates end of directory
        if first_byte == 0x00 {
            break;
        }
        // 0xE5 indicates deleted entry
        if first_byte == 0xE5 {
            continue;
        }

        let attr = chunk[11];
        // Skip Long File Name (LFN) or Volume ID entries
        if attr == ATTR_LFN || (attr & ATTR_VOLUME_ID) != 0 {
            continue;
        }

        let raw: RawDirEntry = unsafe { core::ptr::read_unaligned(chunk.as_ptr() as *const RawDirEntry) };
        entries.push((i * 32, raw));
    }

    entries
}
