//! # Unix Domain Sockets (`AF_UNIX` / `socketpair`)
//!
//! ## Overview
//! Unix domain sockets provide full-duplex, bidirectional inter-process streaming channels.
//! Unlike Internet sockets (`AF_INET`), Unix sockets operate entirely in host memory:
//! - Eliminates TCP state tracking, flow control timers, checksums, and IP encapsulation.
//! - Utilizes paired in-memory ring buffers ([`PipeBuffer`]) for sub-microsecond latency.
//!
//! ## Socket Pairing Topology (`socketpair`):
//! ```text
//!    Socket A                                                     Socket B
//! +------------+       a_to_b Ring Buffer (8 KiB)       +------------+
//! | TX Buffer  | -------------------------------------> | RX Buffer  |
//! |            |                                        |            |
//! | RX Buffer  | <------------------------------------- | TX Buffer  |
//! +------------+       b_to_a Ring Buffer (8 KiB)       +------------+
//! ```

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::fs::vfs::{DirEntry, FileStat, FileType, FsError, Inode, POLLIN, POLLOUT};
use crate::ipc::pipe::PipeBuffer;
use crate::sync::Spinlock;

/// Default capacity for Unix domain socket ring buffers (8 KiB).
pub const DEFAULT_UNIX_SOCKET_CAPACITY: usize = 8192;

/// A bidirectional Unix domain socket endpoint.
pub struct UnixSocket {
    /// Inbound reception ring buffer.
    pub rx_buf: Arc<Spinlock<PipeBuffer>>,
    /// Outbound transmission ring buffer.
    pub tx_buf: Arc<Spinlock<PipeBuffer>>,
}

impl UnixSocket {
    /// Constructs a socket endpoint with the given receive and transmit buffers.
    pub fn new(rx_buf: Arc<Spinlock<PipeBuffer>>, tx_buf: Arc<Spinlock<PipeBuffer>>) -> Self {
        Self { rx_buf, tx_buf }
    }

    /// Transmits data across the local socket into the peer's receive buffer.
    pub fn send(&self, data: &[u8]) -> Result<usize, FsError> {
        let mut guard = self.tx_buf.lock();
        guard.write(data)
    }

    /// Receives buffered data from the local socket's receive buffer.
    pub fn recv(&self, buf: &mut [u8]) -> Result<usize, FsError> {
        let mut guard = self.rx_buf.lock();
        guard.read(buf)
    }

    /// Returns the number of bytes currently buffered and available for reading.
    pub fn available(&self) -> usize {
        self.rx_buf.lock().available()
    }
}

impl Inode for UnixSocket {
    fn stat(&self) -> FileStat {
        FileStat {
            inode_num: 0,
            file_type: FileType::CharDevice,
            size: self.available(),
            mode: 0o666,
        }
    }

    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        self.recv(buf)
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize, FsError> {
        self.send(buf)
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
        let mut mask = 0u32;
        if self.rx_buf.lock().available() > 0 {
            mask |= POLLIN;
        }
        let tx = self.tx_buf.lock();
        if tx.available() < DEFAULT_UNIX_SOCKET_CAPACITY {
            mask |= POLLOUT;
        }
        mask
    }
}

static UNIX_LISTENERS: Spinlock<BTreeMap<String, VecDeque<UnixSocket>>> =
    Spinlock::new(BTreeMap::new());

/// Registers a named `AF_UNIX` listener endpoint (e.g. `/tmp/compositor.sock`).
pub fn unix_listen(path: &str) -> Result<(), FsError> {
    if path.is_empty() {
        return Err(FsError::InvalidInput);
    }
    let mut listeners = UNIX_LISTENERS.lock();
    listeners.entry(String::from(path)).or_insert_with(VecDeque::new);
    Ok(())
}

/// Accepts the next pending client connection on `path` (non-blocking).
pub fn unix_accept(path: &str) -> Result<Option<Arc<UnixSocket>>, FsError> {
    let mut listeners = UNIX_LISTENERS.lock();
    let queue = listeners.get_mut(path).ok_or(FsError::NotFound)?;
    Ok(queue.pop_front().map(Arc::new))
}

/// Connects to a listening `AF_UNIX` endpoint at `path`, returning the client socket.
pub fn unix_connect(path: &str) -> Result<Arc<UnixSocket>, FsError> {
    let mut listeners = UNIX_LISTENERS.lock();
    let queue = listeners.get_mut(path).ok_or(FsError::NotFound)?;
    let (server_end, client_end) = socketpair();
    queue.push_back(server_end);
    Ok(Arc::new(client_end))
}

/// Removes a named `AF_UNIX` listener endpoint.
pub fn unix_close_listener(path: &str) {
    UNIX_LISTENERS.lock().remove(path);
}

/// Creates a connected pair of bidirectional Unix domain sockets conforming to `socketpair(2)`.
///
/// Any data written to `socket_a` can be read from `socket_b`, and vice versa.
pub fn socketpair() -> (UnixSocket, UnixSocket) {
    let a_to_b = Arc::new(Spinlock::new(PipeBuffer::new(DEFAULT_UNIX_SOCKET_CAPACITY)));
    let b_to_a = Arc::new(Spinlock::new(PipeBuffer::new(DEFAULT_UNIX_SOCKET_CAPACITY)));

    let socket_a = UnixSocket::new(b_to_a.clone(), a_to_b.clone());
    let socket_b = UnixSocket::new(a_to_b, b_to_a);

    (socket_a, socket_b)
}
