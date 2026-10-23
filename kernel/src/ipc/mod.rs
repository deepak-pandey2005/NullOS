//! Inter-Process Communication (IPC) Subsystem (Phase 10 - M27).
//!
//! Submodules:
//! - `pipe`: Anonymous unidirectional ring-buffered pipes conforming to POSIX `pipe(2)`.
//! - `shm`: Zero-copy physical memory sharing conforming to POSIX `shmget` / `shmat`.
//! - `signal`: Asynchronous process signaling conforming to POSIX `kill(2)` and signals.
//! - `unix_socket`: Bidirectional local Unix Domain sockets (`AF_UNIX`) and `socketpair(2)`.
#![allow(unused_imports, dead_code)]

pub mod pipe;
pub mod shm;
pub mod signal;
pub mod unix_socket;

pub use pipe::{create_pipe, DEFAULT_PIPE_CAPACITY, PipeBuffer};
pub use shm::{shmat, shmget, shm_list, ShmId, ShmInfo};
pub use signal::{send_signal, Signal};
pub use unix_socket::{socketpair, UnixSocket, DEFAULT_UNIX_SOCKET_CAPACITY};

use crate::println;

/// Runs automated self-tests verifying the functionality of all IPC mechanisms.
pub fn run_self_tests() {
    println!("[IPC] Running Inter-Process Communication verification self-tests...");

    // --- Test 1: Anonymous Pipes ---
    let (read_fd, write_fd) = create_pipe();
    let test_msg = b"NullOS IPC Streaming Pipe Test 2026!";
    let written = write_fd.write(test_msg).expect("Failed to write to pipe");
    assert_eq!(written, test_msg.len());

    let mut read_buf = [0u8; 64];
    let read_bytes = read_fd.read(&mut read_buf).expect("Failed to read from pipe");
    assert_eq!(read_bytes, test_msg.len());
    assert_eq!(&read_buf[..read_bytes], test_msg);

    println!("[IPC] 1. Anonymous unidirectional pipe read/write streaming verified.");

    // --- Test 2: Pipe EOF Detection ---
    drop(write_fd);
    let mut eof_buf = [0u8; 8];
    let eof_result = read_fd.read(&mut eof_buf).expect("Read from closed pipe must return EOF");
    assert_eq!(eof_result, 0, "Reading an empty pipe with all writers closed must return EOF (0)");

    println!("[IPC] 2. Pipe EOF propagation on writer closure verified.");

    // --- Test 3: Shared Memory Segments ---
    let shm_key = 0x1337_CAFE;
    let shm_size = 4096;
    let shm_id = shmget(shm_key, shm_size, true).expect("Failed to create shared memory segment");
    assert_eq!(shmget(shm_key, shm_size, false).expect("Lookup by key failed"), shm_id);

    let shm_list = shm_list();
    assert!(shm_list.iter().any(|s| s.key == shm_key), "Shared memory segment must be listed in registry");

    println!("[IPC] 3. Zero-copy shared memory segment allocation & registry verified.");

    // --- Test 4: Signals ---
    let sig = Signal::from_i32(9).expect("SIGKILL must be valid");
    assert_eq!(sig, Signal::SIGKILL);
    assert_eq!(sig.name(), "SIGKILL");
    assert_eq!(Signal::SIGTERM.as_i32(), 15);

    println!("[IPC] 4. POSIX process signals & identifiers verified.");

    // --- Test 5: Unix Domain Sockets ---
    let (sock_a, sock_b) = socketpair();
    let msg_a_to_b = b"Ping from Socket A";
    let msg_b_to_a = b"Pong from Socket B";

    let sent_a = sock_a.send(msg_a_to_b).expect("Failed to send from socket A");
    assert_eq!(sent_a, msg_a_to_b.len());

    let mut buf_b = [0u8; 32];
    let recv_b = sock_b.recv(&mut buf_b).expect("Failed to recv on socket B");
    assert_eq!(&buf_b[..recv_b], msg_a_to_b);

    let sent_b = sock_b.send(msg_b_to_a).expect("Failed to send from socket B");
    assert_eq!(sent_b, msg_b_to_a.len());

    let mut buf_a = [0u8; 32];
    let recv_a = sock_a.recv(&mut buf_a).expect("Failed to recv on socket A");
    assert_eq!(&buf_a[..recv_a], msg_b_to_a);

    println!("[IPC] 5. Bidirectional Unix Domain socketpair messaging verified.");

    println!("[IPC] All Inter-Process Communication self-tests passed successfully!");
}
