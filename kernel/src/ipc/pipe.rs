//! Anonymous Unidirectional Pipes (Phase 10 - M27 IPC).
//!
//! Provides ring-buffered inter-process data streaming conforming to POSIX `pipe(2)`.
//! Reading from `pipefd[0]` retrieves data written to `pipefd[1]`.
//! Supports EOF notification when writers close, and EPIPE error detection when readers close.

use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::fs::fd::FileDescriptor;
use crate::fs::vfs::{DirEntry, FileStat, FileType, FsError, Inode, OpenFlags};
use crate::sync::Spinlock;

/// Default capacity for anonymous pipe ring buffers (4 KiB = 1 physical page).
pub const DEFAULT_PIPE_CAPACITY: usize = 4096;

/// Internal ring buffer holding pipe data and tracking reader/writer reference counts.
pub struct PipeBuffer {
    buffer: Vec<u8>,
    head: usize,
    tail: usize,
    count: usize,
    capacity: usize,
    readers: usize,
    writers: usize,
}

impl PipeBuffer {
    pub fn new(capacity: usize) -> Self {
        let mut buf = Vec::with_capacity(capacity);
        buf.resize(capacity, 0);
        Self {
            buffer: buf,
            head: 0,
            tail: 0,
            count: 0,
            capacity,
            readers: 1,
            writers: 1,
        }
    }

    /// Writes data into the circular buffer.
    pub fn write(&mut self, data: &[u8]) -> Result<usize, FsError> {
        if self.readers == 0 {
            // Broken pipe (EPIPE): no readers remain
            return Err(FsError::IoError);
        }

        let mut written = 0;
        for &byte in data {
            if self.count >= self.capacity {
                // Buffer is full (non-blocking in NullOS for now)
                break;
            }
            self.buffer[self.tail] = byte;
            self.tail = (self.tail + 1) % self.capacity;
            self.count += 1;
            written += 1;
        }

        Ok(written)
    }

    /// Reads data from the circular buffer.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, FsError> {
        if self.count == 0 {
            if self.writers == 0 {
                // All writers closed and buffer empty: EOF
                return Ok(0);
            }
            // Buffer empty but writer still open
            return Ok(0);
        }

        let mut read_bytes = 0;
        for byte_ref in buf.iter_mut() {
            if self.count == 0 {
                break;
            }
            *byte_ref = self.buffer[self.head];
            self.head = (self.head + 1) % self.capacity;
            self.count -= 1;
            read_bytes += 1;
        }

        Ok(read_bytes)
    }

    #[inline]
    pub fn available(&self) -> usize {
        self.count
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline]
    pub fn is_broken(&self) -> bool {
        self.readers == 0
    }

    #[inline]
    pub fn is_eof(&self) -> bool {
        self.writers == 0 && self.count == 0
    }
}

/// Reading end of an anonymous pipe implementing `Inode`.
pub struct PipeReadInode {
    pipe: Arc<Spinlock<PipeBuffer>>,
    inode_num: u64,
}

impl PipeReadInode {
    pub fn new(pipe: Arc<Spinlock<PipeBuffer>>, inode_num: u64) -> Self {
        Self { pipe, inode_num }
    }
}

impl Inode for PipeReadInode {
    fn stat(&self) -> FileStat {
        let guard = self.pipe.lock();
        FileStat {
            inode_num: self.inode_num,
            file_type: FileType::Fifo,
            size: guard.available(),
            mode: 0o600,
        }
    }

    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        let mut guard = self.pipe.lock();
        guard.read(buf)
    }

    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize, FsError> {
        // Read end of pipe does not support writing
        Err(FsError::PermissionDenied)
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

impl Drop for PipeReadInode {
    fn drop(&mut self) {
        let mut guard = self.pipe.lock();
        if guard.readers > 0 {
            guard.readers -= 1;
        }
    }
}

/// Writing end of an anonymous pipe implementing `Inode`.
pub struct PipeWriteInode {
    pipe: Arc<Spinlock<PipeBuffer>>,
    inode_num: u64,
}

impl PipeWriteInode {
    pub fn new(pipe: Arc<Spinlock<PipeBuffer>>, inode_num: u64) -> Self {
        Self { pipe, inode_num }
    }
}

impl Inode for PipeWriteInode {
    fn stat(&self) -> FileStat {
        let guard = self.pipe.lock();
        FileStat {
            inode_num: self.inode_num,
            file_type: FileType::Fifo,
            size: guard.available(),
            mode: 0o600,
        }
    }

    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize, FsError> {
        // Write end of pipe does not support reading
        Err(FsError::PermissionDenied)
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize, FsError> {
        let mut guard = self.pipe.lock();
        guard.write(buf)
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

impl Drop for PipeWriteInode {
    fn drop(&mut self) {
        let mut guard = self.pipe.lock();
        if guard.writers > 0 {
            guard.writers -= 1;
        }
    }
}

static NEXT_PIPE_INODE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0x8000_0000);

/// Creates an anonymous unidirectional pipe, returning a `(read_fd, write_fd)` pair.
pub fn create_pipe() -> (Arc<FileDescriptor>, Arc<FileDescriptor>) {
    let pipe_buffer = Arc::new(Spinlock::new(PipeBuffer::new(DEFAULT_PIPE_CAPACITY)));
    let inode_read_id = NEXT_PIPE_INODE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let inode_write_id = NEXT_PIPE_INODE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    let read_inode = Arc::new(PipeReadInode::new(pipe_buffer.clone(), inode_read_id));
    let write_inode = Arc::new(PipeWriteInode::new(pipe_buffer, inode_write_id));

    let read_fd = Arc::new(FileDescriptor::new(read_inode, OpenFlags::READ_ONLY));
    let write_fd = Arc::new(FileDescriptor::new(write_inode, OpenFlags::WRITE_ONLY));

    (read_fd, write_fd)
}
