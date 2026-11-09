//! File Descriptor Table & File Handles (Phase 6 - M15 & M16).
//!
//! # Architecture Overview
//!
//! POSIX processes interact with files, pipes, sockets, and character devices through
//! integer indices called **File Descriptors (FDs)**. This module implements:
//! - [`FileDescriptor`]: The open file description holding an [`Inode`] reference,
//!   access permissions ([`OpenFlags`]), and current read/write byte offset.
//! - [`FileDescriptorTable`]: The per-process table mapping integer descriptors (0..64)
//!   to shared open file descriptions ([`Arc<FileDescriptor>`]).
//!
//! # Standard Streams
//! Upon creation of a new process or thread table, slots 0, 1, and 2 are pre-populated:
//! - **FD 0 (stdin)**: Bound to [`ConsoleDevice`] in read-only mode.
//! - **FD 1 (stdout)**: Bound to [`ConsoleDevice`] in write-only mode.
//! - **FD 2 (stderr)**: Bound to [`ConsoleDevice`] in write-only mode.

use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::fs::initramfs::ConsoleDevice;
use crate::fs::vfs::{FsError, Inode, OpenFlags};
use crate::sync::Spinlock;

/// Seek from the beginning of the file (`SEEK_SET`). Offset is treated as an absolute position.
pub const SEEK_SET: i32 = 0;
/// Seek relative to the current file offset (`SEEK_CUR`).
pub const SEEK_CUR: i32 = 1;
/// Seek relative to the end of the file (`SEEK_END`). Offset is added to the file size.
pub const SEEK_END: i32 = 2;

/// Maximum number of simultaneously open file descriptors permitted per process.
pub const MAX_FDS: usize = 64;

/// An open file description holding an Inode, access flags, and current read/write cursor.
///
/// In POSIX semantics, multiple file descriptors (e.g., after `dup` or `fork`) can reference
/// the exact same open file description, sharing the file offset and access mode.
pub struct FileDescriptor {
    /// Underlying VFS inode representing the opened file, directory, or device.
    pub inode: Arc<dyn Inode>,
    /// Access mode flags (read, write, non-blocking, append) set when opened.
    pub flags: OpenFlags,
    /// Current read/write byte offset within the file stream, protected by a spinlock.
    pub offset: Spinlock<usize>,
}

impl FileDescriptor {
    /// Constructs a new open file description with the offset initialized to byte 0.
    pub fn new(inode: Arc<dyn Inode>, flags: OpenFlags) -> Self {
        Self {
            inode,
            flags,
            offset: Spinlock::new(0),
        }
    }

    /// Reads up to `buf.len()` bytes at the current offset and advances the offset.
    ///
    /// # Semantics
    /// - Verifies `flags.read` is true (`EACCES`).
    /// - Reads data from the underlying inode at `*offset`.
    /// - If non-blocking and 0 bytes are immediately readable, polls the inode and
    ///   returns [`FsError::WouldBlock`] if unready.
    /// - Advances the internal offset by the number of bytes successfully read.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, FsError> {
        if !self.flags.read {
            return Err(FsError::PermissionDenied);
        }
        let mut offset = self.offset.lock();
        let bytes_read = self.inode.read_at(*offset, buf)?;
        if bytes_read == 0 && self.flags.nonblock && !buf.is_empty() {
            let p = self.inode.poll();
            if (p & crate::fs::vfs::POLLIN) == 0 {
                return Err(FsError::WouldBlock);
            }
        }
        *offset += bytes_read;
        Ok(bytes_read)
    }

    /// Writes `buf.len()` bytes at the current offset and advances the offset.
    ///
    /// # Semantics
    /// - Verifies `flags.write` is true (`EACCES`).
    /// - If `flags.nonblock`, verifies readiness via `poll()`.
    /// - If `flags.append`, forces write position to current file size (`O_APPEND`).
    /// - Dispatches write to the underlying inode and updates the file offset.
    pub fn write(&self, buf: &[u8]) -> Result<usize, FsError> {
        if !self.flags.write {
            return Err(FsError::PermissionDenied);
        }
        if self.flags.nonblock && !buf.is_empty() {
            let p = self.inode.poll();
            if (p & crate::fs::vfs::POLLOUT) == 0 {
                return Err(FsError::WouldBlock);
            }
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
    ///
    /// Returns the new resulting byte offset from the beginning of the file.
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
///
/// Encapsulates a fixed-capacity vector of descriptor slots. Open slots contain
/// `Some(Arc<FileDescriptor>)`, while closed/unallocated slots contain `None`.
pub struct FileDescriptorTable {
    fds: Spinlock<Vec<Option<Arc<FileDescriptor>>>>,
}

impl FileDescriptorTable {
    /// Creates an empty table pre-populated with standard I/O (stdin: 0, stdout: 1, stderr: 2).
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
    ///
    /// Complies with POSIX requirements that the lowest unallocated descriptor number must be selected.
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

    /// Retrieves a cloned reference to the open file description at index `fd`.
    pub fn get(&self, fd: i32) -> Result<Arc<FileDescriptor>, FsError> {
        if fd < 0 || fd as usize >= MAX_FDS {
            return Err(FsError::BadFileDescriptor);
        }
        let fds = self.fds.lock();
        fds[fd as usize].clone().ok_or(FsError::BadFileDescriptor)
    }

    /// Closes and releases the specified file descriptor slot, decrementing the description's refcount.
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
    /// Clones the descriptor table (as during a `fork()` operation), sharing the underlying
    /// open file descriptions across parent and child processes.
    fn clone(&self) -> Self {
        let current_fds = self.fds.lock();
        Self {
            fds: Spinlock::new(current_fds.clone()),
        }
    }
}
