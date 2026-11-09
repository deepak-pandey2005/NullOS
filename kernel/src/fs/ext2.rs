//! ext2 Read-Only Filesystem Driver (Phase 6 - M19 Filesystems).
//!
//! # Architecture Overview
//!
//! Implements the Linux Second Extended Filesystem (ext2) standard (Rev 0 and Rev 1)
//! for mounting persistent Linux partitions in NullOS.
//!
//! ## ext2 On-Disk Geometry:
//! 1. **Boot Record (Bytes 0..1024)**: Unused by ext2, reserved for x86 bootloaders.
//! 2. **Superblock (Byte 1024..2048)**:
//!    - Contains volume metadata: total blocks, total inodes, block size (`1024 << s_log_block_size`),
//!      blocks per block group, inodes per block group, and magic signature (`0xEF53`).
//! 3. **Block Group Descriptor Table (BGDT)**:
//!    - Starts immediately after the superblock (block 2 if 1024-byte blocks, or block 1 if >1024).
//!    - Each 32-byte descriptor points to block bitmap, inode bitmap, and inode table for that group.
//! 4. **Inode Table**:
//!    - Array of 128-byte (or `s_inode_size`) inode structures.
//!    - Inode 2 is traditionally reserved as the root directory (`/`).
//! 5. **Block Pointer Resolution**:
//!    - Inodes contain 15 block pointers:
//!      - `0..11`: Direct block pointers.
//!      - `12`: Singly indirect block pointer.
//!      - `13`: Doubly indirect block pointer.
//!      - `14`: Triply indirect block pointer.
//!
//! Current driver provides read-only VFS integration.
#![allow(dead_code)]

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::drivers::block::{BlockDevice, SECTOR_SIZE};
use crate::fs::vfs::{DirEntry, FileStat, FileType, FileSystem, FsError, Inode};

/// Standard ext2 magic signature (`0xEF53`) located at offset 56 of the Superblock.
pub const EXT2_MAGIC: u16 = 0xEF53;

/// Inode mode bit indicating directory file type (`S_IFDIR`).
const EXT2_S_IFDIR: u16 = 0x4000;
/// Inode mode bit indicating regular file type (`S_IFREG`).
const EXT2_S_IFREG: u16 = 0x8000;

/// Ext2 On-Disk Superblock (located at byte offset 1024).
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct Ext2Superblock {
    /// Total number of inodes across all block groups.
    pub s_inodes_count: u32,
    /// Total number of blocks in the filesystem.
    pub s_blocks_count: u32,
    /// Number of blocks reserved for the superuser.
    pub s_r_blocks_count: u32,
    /// Total number of unallocated blocks.
    pub s_free_blocks_count: u32,
    /// Total number of unallocated inodes.
    pub s_free_inodes_count: u32,
    /// First data block number (0 for >1KB blocks, 1 for 1KB blocks).
    pub s_first_data_block: u32,
    /// Block size indicator: `block_size = 1024 << s_log_block_size`.
    pub s_log_block_size: u32,
    /// Fragment size indicator: `fragment_size = 1024 << s_log_frag_size`.
    pub s_log_frag_size: u32,
    /// Number of blocks grouped per block group.
    pub s_blocks_per_group: u32,
    /// Number of fragments grouped per block group.
    pub s_frags_per_group: u32,
    /// Number of inodes allocated per block group.
    pub s_inodes_per_group: u32,
    /// POSIX timestamp of last mount.
    pub s_mtime: u32,
    /// POSIX timestamp of last write.
    pub s_wtime: u32,
    /// Number of mounts since last consistency check.
    pub s_mnt_count: u16,
    /// Maximum allowed mounts before forced consistency check.
    pub s_max_mnt_count: u16,
    /// Magic signature (`EXT2_MAGIC = 0xEF53`).
    pub s_magic: u16,
    /// Filesystem state flag (1 = cleanly unmounted, 2 = errors detected).
    pub s_state: u16,
    /// Error handling behavior when errors detected.
    pub s_errors: u16,
    /// Minor revision level.
    pub s_minor_rev_level: u16,
    /// Timestamp of last check.
    pub s_lastcheck: u32,
    /// Interval between automatic checks.
    pub s_checkinterval: u32,
    /// Operating system that created the filesystem.
    pub s_creator_os: u32,
    /// Revision level (0 = original, 1 = dynamic/extended superblock).
    pub s_rev_level: u32,
}

/// Ext2 Block Group Descriptor (32 bytes).
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct Ext2GroupDesc {
    /// Logical block number of the group's block allocation bitmap.
    pub bg_block_bitmap: u32,
    /// Logical block number of the group's inode allocation bitmap.
    pub bg_inode_bitmap: u32,
    /// Logical block number of the group's starting inode table block.
    pub bg_inode_table: u32,
    /// Number of free blocks in this group.
    pub bg_free_blocks_count: u16,
    /// Number of free inodes in this group.
    pub bg_free_inodes_count: u16,
    /// Number of directories allocated in this group.
    pub bg_used_dirs_count: u16,
    /// Alignment padding.
    pub bg_pad: u16,
    /// Reserved space.
    pub bg_reserved: [u32; 3],
}

/// Ext2 Inode structure (128 bytes in Rev 0).
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct Ext2Inode {
    /// File type and POSIX access mode bits.
    pub i_mode: u16,
    /// Owner user ID (low 16 bits).
    pub i_uid: u16,
    /// File size in bytes (lower 32 bits).
    pub i_size: u32,
    /// Last access timestamp.
    pub i_atime: u32,
    /// Creation timestamp.
    pub i_ctime: u32,
    /// Last modification timestamp.
    pub i_mtime: u32,
    /// Deletion timestamp.
    pub i_dtime: u32,
    /// Owner group ID (low 16 bits).
    pub i_gid: u16,
    /// Hard link counter.
    pub i_links_count: u16,
    /// 512-byte sector count reserved for this file.
    pub i_blocks: u32,
    /// Inode behavior flags.
    pub i_flags: u32,
    /// OS-dependent value 1.
    pub i_osd1: u32,
    /// Direct and indirect block pointers (0..11 direct, 12 singly, 13 doubly, 14 triply).
    pub i_block: [u32; 15],
    /// File version / NFS generation counter.
    pub i_generation: u32,
    /// File Access Control List (ACL) block.
    pub i_file_acl: u32,
    /// Directory ACL block.
    pub i_dir_acl: u32,
    /// Fragment block address.
    pub i_faddr: u32,
    /// OS-dependent value 2.
    pub i_osd2: [u32; 3],
}

/// A mounted Ext2 Filesystem instance.
pub struct Ext2FileSystem {
    /// Underlying storage block device.
    pub block_dev: Arc<dyn BlockDevice>,
    /// Logical filesystem block size in bytes (e.g., 1024, 2048, 4096).
    pub block_size: usize,
    /// Number of inodes allocated per block group.
    pub inodes_per_group: usize,
    /// Number of blocks per block group.
    pub blocks_per_group: usize,
    /// Size of each on-disk inode structure in bytes.
    pub inode_size: usize,
    /// Starting block number of the Block Group Descriptor Table.
    pub group_desc_block: u32,
}

impl Ext2FileSystem {
    /// Attempts to probe and mount an ext2 filesystem from a block device.
    ///
    /// Reads the superblock at byte 1024, validates the magic signature, and parses geometry.
    pub fn new(block_dev: Arc<dyn BlockDevice>) -> Result<Arc<Self>, FsError> {
        // Read Superblock from byte 1024 (sectors 2 and 3)
        let mut sec2 = [0u8; SECTOR_SIZE];
        let mut sec3 = [0u8; SECTOR_SIZE];
        block_dev.read_sector(2, &mut sec2).map_err(|_| FsError::IoError)?;
        block_dev.read_sector(3, &mut sec3).map_err(|_| FsError::IoError)?;

        let mut sb_bytes = [0u8; 1024];
        sb_bytes[..512].copy_from_slice(&sec2);
        sb_bytes[512..].copy_from_slice(&sec3);

        let sb: Ext2Superblock = unsafe { core::ptr::read_unaligned(sb_bytes.as_ptr() as *const Ext2Superblock) };

        if sb.s_magic != EXT2_MAGIC {
            return Err(FsError::InvalidInput);
        }

        let block_size = 1024usize << sb.s_log_block_size;
        let group_desc_block = if block_size == 1024 { 2 } else { 1 };

        let inode_size = if sb.s_rev_level >= 1 {
            // Read inode size at offset 88 in extended superblock
            u16::from_le_bytes([sb_bytes[88], sb_bytes[89]]) as usize
        } else {
            128
        };

        Ok(Arc::new(Self {
            block_dev,
            block_size,
            inodes_per_group: sb.s_inodes_per_group as usize,
            blocks_per_group: sb.s_blocks_per_group as usize,
            inode_size,
            group_desc_block,
        }))
    }

    /// Reads a logical ext2 filesystem block from the underlying block device.
    pub fn read_block(&self, block_num: u32, buf: &mut [u8]) -> Result<(), FsError> {
        if buf.len() < self.block_size {
            return Err(FsError::InvalidInput);
        }

        let sectors_per_block = self.block_size / SECTOR_SIZE;
        let start_sector = block_num as u64 * sectors_per_block as u64;

        for s in 0..sectors_per_block {
            let offset = s * SECTOR_SIZE;
            self.block_dev.read_sector(start_sector + s as u64, &mut buf[offset..offset + SECTOR_SIZE])
                .map_err(|_| FsError::IoError)?;
        }

        Ok(())
    }

    /// Reads an inode by 1-based inode index (e.g. Inode 2 for Root).
    pub fn read_inode(&self, inode_num: u32) -> Result<Ext2Inode, FsError> {
        if inode_num == 0 {
            return Err(FsError::NotFound);
        }

        let group = (inode_num - 1) as usize / self.inodes_per_group;
        let index_in_group = (inode_num - 1) as usize % self.inodes_per_group;

        // Read Block Group Descriptor Table
        let mut bgd_buf = alloc::vec![0u8; self.block_size];
        self.read_block(self.group_desc_block, &mut bgd_buf)?;

        let bgd_offset = group * core::mem::size_of::<Ext2GroupDesc>();
        if bgd_offset + core::mem::size_of::<Ext2GroupDesc>() > bgd_buf.len() {
            return Err(FsError::IoError);
        }

        let bgd: Ext2GroupDesc = unsafe {
            core::ptr::read_unaligned(bgd_buf.as_ptr().add(bgd_offset) as *const Ext2GroupDesc)
        };

        // Locate inode table block
        let inode_offset = index_in_group * self.inode_size;
        let table_block = bgd.bg_inode_table + (inode_offset / self.block_size) as u32;
        let offset_in_block = inode_offset % self.block_size;

        let mut block_buf = alloc::vec![0u8; self.block_size];
        self.read_block(table_block, &mut block_buf)?;

        let inode: Ext2Inode = unsafe {
            core::ptr::read_unaligned(block_buf.as_ptr().add(offset_in_block) as *const Ext2Inode)
        };

        Ok(inode)
    }

    /// Reads all data content of an inode into memory using its direct block pointers.
    pub fn read_inode_data(&self, inode: &Ext2Inode) -> Result<Vec<u8>, FsError> {
        let size = inode.i_size as usize;
        let mut data = Vec::with_capacity(size);
        let blocks_needed = (size + self.block_size - 1) / self.block_size;
        let mut block_buf = alloc::vec![0u8; self.block_size];

        for i in 0..blocks_needed.min(12) {
            let block = inode.i_block[i];
            if block != 0 {
                self.read_block(block, &mut block_buf)?;
                let remaining = size - data.len();
                let to_copy = remaining.min(self.block_size);
                data.extend_from_slice(&block_buf[..to_copy]);
            } else {
                data.resize(data.len() + self.block_size.min(size - data.len()), 0);
            }
        }

        Ok(data)
    }
}

impl FileSystem for Ext2FileSystem {
    fn name(&self) -> &str {
        "ext2"
    }

    fn root(&self) -> Arc<dyn Inode> {
        let root_inode = self.read_inode(2).unwrap_or(Ext2Inode {
            i_mode: EXT2_S_IFDIR | 0o755,
            i_uid: 0,
            i_size: 1024,
            i_atime: 0,
            i_ctime: 0,
            i_mtime: 0,
            i_dtime: 0,
            i_gid: 0,
            i_links_count: 2,
            i_blocks: 2,
            i_flags: 0,
            i_osd1: 0,
            i_block: [0; 15],
            i_generation: 0,
            i_file_acl: 0,
            i_dir_acl: 0,
            i_faddr: 0,
            i_osd2: [0; 3],
        });

        Arc::new(Ext2InodeWrapper {
            fs: Arc::new(Self {
                block_dev: self.block_dev.clone(),
                block_size: self.block_size,
                inodes_per_group: self.inodes_per_group,
                blocks_per_group: self.blocks_per_group,
                inode_size: self.inode_size,
                group_desc_block: self.group_desc_block,
            }),
            inode_num: 2,
            inode: root_inode,
        })
    }
}

/// Wrapper around an `Ext2Inode` implementing the VFS `Inode` trait.
pub struct Ext2InodeWrapper {
    fs: Arc<Ext2FileSystem>,
    inode_num: u64,
    inode: Ext2Inode,
}

impl Inode for Ext2InodeWrapper {
    fn stat(&self) -> FileStat {
        let is_dir = (self.inode.i_mode & EXT2_S_IFDIR) != 0;
        FileStat {
            inode_num: self.inode_num,
            file_type: if is_dir { FileType::Directory } else { FileType::File },
            size: self.inode.i_size as usize,
            mode: (self.inode.i_mode & 0o777) as u32,
        }
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        let size = self.inode.i_size as usize;
        if offset >= size {
            return Ok(0);
        }

        let data = self.fs.read_inode_data(&self.inode)?;
        let to_copy = (size - offset).min(buf.len());
        if offset + to_copy <= data.len() {
            buf[..to_copy].copy_from_slice(&data[offset..offset + to_copy]);
            Ok(to_copy)
        } else {
            Ok(0)
        }
    }

    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize, FsError> {
        // Read-only filesystem implementation
        Err(FsError::PermissionDenied)
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>, FsError> {
        let is_dir = (self.inode.i_mode & EXT2_S_IFDIR) != 0;
        if !is_dir {
            return Err(FsError::NotADirectory);
        }

        let data = self.fs.read_inode_data(&self.inode)?;
        let mut offset = 0;

        while offset + 8 <= data.len() {
            let inode_id = u32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]);
            let rec_len = u16::from_le_bytes([data[offset + 4], data[offset + 5]]) as usize;
            let name_len = data[offset + 6] as usize;

            if rec_len == 0 {
                break;
            }

            if inode_id != 0 && offset + 8 + name_len <= data.len() {
                if let Ok(entry_name) = core::str::from_utf8(&data[offset + 8..offset + 8 + name_len]) {
                    if entry_name.eq_ignore_ascii_case(name) {
                        let child_inode = self.fs.read_inode(inode_id)?;
                        return Ok(Arc::new(Ext2InodeWrapper {
                            fs: self.fs.clone(),
                            inode_num: inode_id as u64,
                            inode: child_inode,
                        }));
                    }
                }
            }

            offset += rec_len;
        }

        Err(FsError::NotFound)
    }

    fn read_dir(&self) -> Result<Vec<DirEntry>, FsError> {
        let is_dir = (self.inode.i_mode & EXT2_S_IFDIR) != 0;
        if !is_dir {
            return Err(FsError::NotADirectory);
        }

        let data = self.fs.read_inode_data(&self.inode)?;
        let mut offset = 0;
        let mut entries = Vec::new();

        while offset + 8 <= data.len() {
            let inode_id = u32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]);
            let rec_len = u16::from_le_bytes([data[offset + 4], data[offset + 5]]) as usize;
            let name_len = data[offset + 6] as usize;
            let file_type_byte = data[offset + 7];

            if rec_len == 0 {
                break;
            }

            if inode_id != 0 && offset + 8 + name_len <= data.len() {
                if let Ok(name_str) = core::str::from_utf8(&data[offset + 8..offset + 8 + name_len]) {
                    let file_type = match file_type_byte {
                        2 => FileType::Directory,
                        1 => FileType::File,
                        _ => FileType::File,
                    };

                    entries.push(DirEntry {
                        name: String::from(name_str),
                        inode_num: inode_id as u64,
                        file_type,
                    });
                }
            }

            offset += rec_len;
        }

        Ok(entries)
    }

    // TODO(ext2-write): Currently, ext2 is mounted strictly read-only. Implement ext2 write support:
    // 1. Block bitmap and inode bitmap allocation routines.
    // 2. Inode table updates and directory entry insertion.
    // 3. Superblock and Block Group Descriptor free block/inode counter synchronization.
    fn create(&self, _name: &str, _file_type: FileType) -> Result<Arc<dyn Inode>, FsError> {
        Err(FsError::PermissionDenied)
    }
}
