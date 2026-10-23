//! Unix Domain Sockets (Phase 10 - M27 IPC).
//!
//! Provides bidirectional local inter-process communication conforming to `AF_UNIX` sockets
//! and `socketpair(2)`.
//! Unlike network TCP/IP sockets, Unix domain sockets bypass Ethernet framing, IP routing,
//! and checksum calculations, streaming data directly through shared in-memory ring buffers.

use alloc::sync::Arc;
use crate::fs::vfs::FsError;
use crate::ipc::pipe::PipeBuffer;
use crate::sync::Spinlock;

/// Default capacity for Unix domain socket ring buffers.
pub const DEFAULT_UNIX_SOCKET_CAPACITY: usize = 8192;

/// A bidirectional Unix domain socket endpoint.
pub struct UnixSocket {
    pub rx_buf: Arc<Spinlock<PipeBuffer>>,
    pub tx_buf: Arc<Spinlock<PipeBuffer>>,
}

impl UnixSocket {
    pub fn new(rx_buf: Arc<Spinlock<PipeBuffer>>, tx_buf: Arc<Spinlock<PipeBuffer>>) -> Self {
        Self { rx_buf, tx_buf }
    }

    /// Transmits data across the local socket.
    pub fn send(&self, data: &[u8]) -> Result<usize, FsError> {
        let mut guard = self.tx_buf.lock();
        guard.write(data)
    }

    /// Receives buffered data from the local socket.
    pub fn recv(&self, buf: &mut [u8]) -> Result<usize, FsError> {
        let mut guard = self.rx_buf.lock();
        guard.read(buf)
    }

    /// Returns the number of bytes available for reading.
    pub fn available(&self) -> usize {
        self.rx_buf.lock().available()
    }
}

/// Creates a connected pair of bidirectional Unix domain sockets (`socketpair`).
pub fn socketpair() -> (UnixSocket, UnixSocket) {
    let a_to_b = Arc::new(Spinlock::new(PipeBuffer::new(DEFAULT_UNIX_SOCKET_CAPACITY)));
    let b_to_a = Arc::new(Spinlock::new(PipeBuffer::new(DEFAULT_UNIX_SOCKET_CAPACITY)));

    let socket_a = UnixSocket::new(b_to_a.clone(), a_to_b.clone());
    let socket_b = UnixSocket::new(a_to_b, b_to_a);

    (socket_a, socket_b)
}
