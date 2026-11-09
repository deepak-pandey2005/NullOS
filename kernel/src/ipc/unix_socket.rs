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

use alloc::sync::Arc;
use crate::fs::vfs::FsError;
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
