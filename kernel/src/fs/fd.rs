//! File Descriptor Table & File Handles (Phase 6 - M15 & M16).
//!
//! Provides per-process file descriptor management (fd 0: stdin, 1: stdout, 2: stderr),
//! seek offsets, and atomic descriptor allocation.

use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::fs::initramfs::ConsoleDevice;
use crate::fs::vfs::{FsError, Inode, OpenFlags};
use crate::sync::Spinlock;

/// Seek origins conforming to POSIX `lseek(2)`.
pub const SEEK_SET: i32 = 0;
pub const SEEK_CUR: i32 = 1;
pub const SEEK_END: i32 = 2;

/// Maximum number of simultaneously open file descriptors per process.
pub const MAX_FDS: usize = 64;

/// An open file description holding an Inode, access flags, and current read/write cursor.
pub struct FileDescriptor {
    pub inode: Arc<dyn Inode>,
    pub flags: OpenFlags,
    pub offset: Spinlock<usize>,
}

impl FileDescriptor {
    pub fn new(inode: Arc<dyn Inode>, flags: OpenFlags) -> Self {
        Self {
            inode,
            flags,
            offset: Spinlock::new(0),
        }
    }

    /// Reads up to `buf.len()` bytes at the current offset and advances the offset.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, FsError> {
        if !self.flags.read {
            return Err(FsError::PermissionDenied);
        }
        let mut offset = self.offset.lock();
        let bytes_read = self.inode.read_at(*offset, buf)?;
        *offset += bytes_read;
        Ok(bytes_read)
    }

    /// Writes `buf.len()` bytes at the current offset and advances the offset.
    pub fn write(&self, buf: &[u8]) -> Result<usize, FsError> {
        if !self.flags.write {
            return Err(FsError::PermissionDenied);
        }
        let mut offset = self.offset.lock();
        let write_pos = if self.flags.append {
            self.inode.stat().size
        } else {
            *offset
        };
        let bytes_written = self.inode.write_at(write_pos, buf)?;
        *offset = write_pos + bytes_written;
        Ok(bytes_written)
    }

    /// Modifies the current offset based on POSIX `whence` (`SEEK_SET`, `SEEK_CUR`, `SEEK_END`).
    pub fn seek(&self, offset: i64, whence: i32) -> Result<usize, FsError> {
        let mut current_offset = self.offset.lock();
        let new_offset: i64 = match whence {
            SEEK_SET => offset,
            SEEK_CUR => (*current_offset as i64).checked_add(offset).ok_or(FsError::InvalidInput)?,
            SEEK_END => {
                let size = self.inode.stat().size as i64;
                size.checked_add(offset).ok_or(FsError::InvalidInput)?
            }
            _ => return Err(FsError::InvalidInput),
        };

        if new_offset < 0 {
            return Err(FsError::InvalidInput);
        }

        *current_offset = new_offset as usize;
        Ok(*current_offset)
    }
}

/// A per-process table of file descriptors.
pub struct FileDescriptorTable {
    fds: Spinlock<Vec<Option<Arc<FileDescriptor>>>>,
}

impl FileDescriptorTable {
    /// Creates an empty table pre-populated with standard I/O (stdin, stdout, stderr).
    pub fn new() -> Self {
        let mut initial = Vec::with_capacity(MAX_FDS);
        for _ in 0..MAX_FDS {
            initial.push(None);
        }

        let console_node: Arc<dyn Inode> = Arc::new(ConsoleDevice::new());

        // fd 0: stdin (read-only console)
        initial[0] = Some(Arc::new(FileDescriptor::new(
            console_node.clone(),
            OpenFlags::READ_ONLY,
        )));

        // fd 1: stdout (write-only console)
        initial[1] = Some(Arc::new(FileDescriptor::new(
            console_node.clone(),
            OpenFlags::WRITE_ONLY,
        )));

        // fd 2: stderr (write-only console)
        initial[2] = Some(Arc::new(FileDescriptor::new(
            console_node,
            OpenFlags::WRITE_ONLY,
        )));

        Self {
            fds: Spinlock::new(initial),
        }
    }

    /// Allocates the lowest available file descriptor index and associates it with `desc`.
    pub fn alloc_fd(&self, desc: Arc<FileDescriptor>) -> Result<i32, FsError> {
        let mut fds = self.fds.lock();
        for (i, slot) in fds.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(desc);
                return Ok(i as i32);
            }
        }
        Err(FsError::NoSpace)
    }

    /// Retrieves the file descriptor at index `fd`.
    pub fn get(&self, fd: i32) -> Result<Arc<FileDescriptor>, FsError> {
        if fd < 0 || fd as usize >= MAX_FDS {
            return Err(FsError::BadFileDescriptor);
        }
        let fds = self.fds.lock();
        fds[fd as usize].clone().ok_or(FsError::BadFileDescriptor)
    }

    /// Closes and releases the specified file descriptor.
    pub fn close(&self, fd: i32) -> Result<(), FsError> {
        if fd < 0 || fd as usize >= MAX_FDS {
            return Err(FsError::BadFileDescriptor);
        }
        let mut fds = self.fds.lock();
        if fds[fd as usize].is_none() {
            return Err(FsError::BadFileDescriptor);
        }
        fds[fd as usize] = None;
        Ok(())
    }
}

impl Clone for FileDescriptorTable {
    fn clone(&self) -> Self {
        let current_fds = self.fds.lock();
        Self {
            fds: Spinlock::new(current_fds.clone()),
        }
    }
}
