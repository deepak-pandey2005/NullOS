# Chapter 22: Inter-Process Communication (IPC)

### *Connecting Isolated Worlds: Anonymous Pipes, Shared Memory, POSIX Signals, and Unix Domain Sockets*

---

## 1. The Fundamental Paradox: Isolation vs. Cooperation

In Chapters 5, 6, and 12, we engineered total architectural isolation:
- The CPU hardware privilege boundary (Ring 0 vs. Ring 3).
- Dedicated 4-level page tables (`CR3` switches) giving each process its own private 128 TiB virtual universe.
- Memory protection bits (`USER_ACCESSIBLE`, `WRITABLE`, `NO_EXECUTE`).

This isolation is essential for operating system stability and security. A bug or exploit in a web browser cannot corrupt an adjacent spreadsheet application or read private cryptographic keys.

However, modern software is not a monolith—it is an ecosystem of cooperating programs:
- A shell pipeline connects utilities: `cat access.log | grep "404" | wc -l`.
- A database background worker shares cached table pages with query planner threads.
- A parent process controls worker pools and monitors child lifecycles via signals.
- A graphical desktop compositor receives window draw buffers from client applications.

**Inter-Process Communication (IPC)** provides controlled, high-performance mechanisms allowing processes to exchange data, synchronize state, and notify events across memory boundaries.

```
       +--------------------+                    +--------------------+
       |    Process A       |                    |    Process B       |
       |  (PID 1, PML4 A)   |                    |  (PID 2, PML4 B)   |
       +--------------------+                    +--------------------+
                 │                                         ▲
                 │              [ IPC Channel ]            │
                 ├──────────► 1. Anonymous Pipe  ──────────┤
                 │            (Ring-Buffered Byte Stream)  │
                 │                                         │
                 │◄═════════► 2. Shared Memory   ═════════►│
                 │            (Zero-Copy Physical Frames)  │
                 │                                         │
                 ├──────────► 3. POSIX Signals   ──────────┤
                 │            (Asynchronous Notifications) │
                 │                                         │
                 │◄─────────► 4. Unix Sockets    ─────────►│
                 │            (Bidirectional Socketpairs)  │
                 ▼                                         ▼
       +--------------------------------------------------------------+
       |                     NullOS Kernel (Ring 0)                   |
       +--------------------------------------------------------------+
```

---

## 2. Anonymous Unidirectional Pipes (`pipe`)

The **pipe** is one of the most celebrated abstractions in computing history, invented by Douglas McIlroy for Unix in 1973. A pipe is a unidirectional, FIFO (First-In, First-Out) byte stream connecting a writing process to a reading process.

```
                               Anonymous Pipe Architecture:
       +------------------+                                      +------------------+
       |   Writer Process |                                      |   Reader Process |
       +------------------+                                      +------------------+
                │                                                          ▲
                ▼ write(pipefd[1], buf, len)                               │ read(pipefd[0], buf, len)
       +-------------------------------------------------------------------+--------+
       |                         Circular Ring Buffer                       |
       |  [ Byte 0 | Byte 1 | Byte 2 | ... | Byte N | Unused | ... ]       |
       |       ▲                                  ▲                         |
       |       │ Head (Read Position)             │ Tail (Write Position)   |
       +----------------------------------------------------------------------------+
```

### 2.1 The Circular Ring Buffer (`PipeBuffer`)
In NullOS, a pipe is backed by a thread-safe circular ring buffer ([`kernel/src/ipc/pipe.rs`](file:///home/deepak/OPENSOURCE/NullOS/kernel/src/ipc/pipe.rs)):
```rust
pub struct PipeBuffer {
    buffer: Vec<u8>,
    head: usize,      // Next byte to read
    tail: usize,      // Next byte to write
    count: usize,     // Total bytes currently stored
    capacity: usize,  // Default: 4,096 bytes (1 page)
    readers: usize,   // Open read descriptors
    writers: usize,   // Open write descriptors
}
```

### 2.2 Wrap-Around Modulo Arithmetic
When pushing or popping bytes, indices wrap seamlessly using modulo math:
- **Writing**:
  ```rust
  self.buffer[self.tail] = byte;
  self.tail = (self.tail + 1) % self.capacity;
  self.count += 1;
  ```
- **Reading**:
  ```rust
  let byte = self.buffer[self.head];
  self.head = (self.head + 1) % self.capacity;
  self.count -= 1;
  ```

### 2.3 Broken Pipes & End-of-File (EOF) Lifecycle
A pipe's lifecycle depends strictly on reader and writer reference counts:
1. **End-of-File (EOF)**: When all writing processes close their write descriptor (`writers == 0`), subsequent reads on `pipefd[0]` drain remaining buffered bytes. Once empty, `read()` returns `Ok(0)`, signaling EOF to the reader.
2. **Broken Pipe (`EPIPE`)**: If a process writes to `pipefd[1]` when all reading processes have closed their read descriptor (`readers == 0`), the kernel immediately aborts the write with `FsError::IoError` and generates `SIGPIPE`.

### 2.4 The `pipe()` System Call
The `pipe()` syscall allocates two file descriptors in the process's file descriptor table:
```rust
pub fn sys_pipe(pipefd_ptr: *mut i32) -> u64 {
    let (read_fd, write_fd) = crate::ipc::create_pipe();
    let fd0 = proc.fd_table.alloc_fd(read_fd)?;
    let fd1 = proc.fd_table.alloc_fd(write_fd)?;

    unsafe {
        *pipefd_ptr = fd0;
        *pipefd_ptr.add(1) = fd1;
    }
    0
}
```

Because `PipeReadInode` and `PipeWriteInode` implement the generic VFS [`Inode`](file:///home/deepak/OPENSOURCE/NullOS/kernel/src/fs/vfs.rs) trait, standard system calls (`SYS_READ`, `SYS_WRITE`, `SYS_CLOSE`) operate on pipes without modification!

---

## 3. Zero-Copy Shared Memory (SHM)

Pipes and sockets require copying data across memory boundaries:
1. User buffer $\rightarrow$ Kernel ring buffer.
2. Kernel ring buffer $\rightarrow$ User buffer.

For high-throughput systems (e.g. video processing, shared caches), double-copying gigabytes of data saturates the memory bus. **Shared Memory** eliminates copying altogether by mapping the **exact same physical memory frames** into two or more distinct process address spaces.

```
       Process A Virtual Address Space (CR3 = PML4_A)
       +--------------------------------------------+
       | Virtual Page 0x0000_5000_0000              | ──┐
       +--------------------------------------------+   │
                                                        │
                                                        ▼ (Direct Hardware Translation)
                                         +---------------------------------+
                                         | Physical Frame 0x1F4000 (RAM)   |
                                         +---------------------------------+
                                                        ▲
                                                        │ (Direct Hardware Translation)
       Process B Virtual Address Space (CR3 = PML4_B)   │
       +--------------------------------------------+   │
       | Virtual Page 0x0000_6000_0000              | ──┘
       +--------------------------------------------+
```

### 3.1 Paging Mechanics of Shared Memory
In [`kernel/src/ipc/shm.rs`](file:///home/deepak/OPENSOURCE/NullOS/kernel/src/ipc/shm.rs):
1. `shmget(key, size, create)` allocates physical frames via the Physical Memory Manager (PMM):
   ```rust
   let frame = crate::memory::alloc_frame()?;
   ```
2. `shmat(shm_id, address_space, vaddr)` maps those frames into the calling process's page table:
   ```rust
   let flags = PageTableFlags::PRESENT
       | PageTableFlags::WRITABLE
       | PageTableFlags::USER_ACCESSIBLE
       | PageTableFlags::NO_EXECUTE;

   paging::map_page(page, frame, flags)?;
   ```

### 3.2 Instantaneous Visibility
When Process A writes a value to `0x0000_5000_0000`:
- The write travels straight through CPU L1/L2 caches to physical frame `0x1F4000`.
- When Process B reads from `0x0000_6000_0000`, cache coherency protocols (MESI) provide the updated value immediately.
- **Kernel overhead during data transfer: ZERO.**

---

## 4. Asynchronous Process Signals

A **signal** is a software interrupt sent to a process to notify it that an asynchronous event has occurred.

```
Signal Delivery Pipeline:
+-------------------+                          +-------------------+
|  Sender (Kernel   |                          |  Target Process   |
|  or `kill` cmd)   |                          |  (e.g. PID 3)     |
+-------------------+                          +-------------------+
          │                                              │
          ▼ send_signal(pid, sig)                        │
+------------------------------------+                   │
| NullOS Signal Dispatcher           |                   │
|   ├── Validate target PID          |                   │
|   ├── Check permissions            |                   │
|   └── Inspect signal type:         |                   │
|       ├── SIGKILL (9)  ────────────┼───────────────────► Instantly aborts process
|       ├── SIGTERM (15) ────────────┼───────────────────► Clean termination exit
|       └── SIGCHLD (17) ────────────┴───────────────────► Notify parent of state change
+--------------------------------------------------------+
```

### 4.1 Standard POSIX Signals in NullOS
Defined in [`kernel/src/ipc/signal.rs`](file:///home/deepak/OPENSOURCE/NullOS/kernel/src/ipc/signal.rs):

| Signal | Number | Default Action | Description |
| :--- | :---: | :--- | :--- |
| **SIGHUP** | 1 | Terminate | Terminal hangup or controlling process exit |
| **SIGINT** | 2 | Terminate | Interactive terminal interrupt (Ctrl+C) |
| **SIGQUIT** | 3 | Terminate | Terminal quit request (Ctrl+\) |
| **SIGILL** | 4 | Terminate | Illegal CPU instruction (#UD) |
| **SIGTRAP** | 5 | Trace Trap | Debugger breakpoint (#BP) |
| **SIGABRT** | 6 | Terminate | Process abort triggered by `abort()` |
| **SIGBUS** | 7 | Terminate | Hardware bus memory alignment error |
| **SIGFPE** | 8 | Terminate | Arithmetic floating-point exception (#DE/#MF) |
| **SIGKILL** | 9 | Force Terminate | Uncatchable, unignorable process kill |
| **SIGSEGV** | 11 | Terminate | Segmentation violation / Page Fault (#PF) |
| **SIGPIPE** | 13 | Terminate | Write to a pipe with no readers |
| **SIGALRM** | 14 | Terminate | Real-time hardware timer alarm expired |
| **SIGTERM** | 15 | Terminate | Standard software termination request |
| **SIGCHLD** | 17 | Ignore | Child process status changed (exit/stop) |
| **SIGCONT** | 18 | Continue | Resume execution if stopped |
| **SIGSTOP** | 19 | Stop | Uncatchable process suspension |

---

## 5. Unix Domain Sockets (`AF_UNIX`) & `socketpair`

While anonymous pipes are strictly unidirectional, **Unix Domain Sockets** provide bidirectional, full-duplex communication channels between processes on the same machine.

Unlike network Internet sockets (`AF_INET`), Unix sockets completely bypass:
- Layer 2 Ethernet framing and MAC lookups.
- Layer 3 IPv4 routing and one's complement checksum calculation.
- Layer 4 TCP 3-way handshakes and retransmission timers.

```
       Socket A                                                 Socket B
+--------------------+                                   +--------------------+
| UnixSocket Endpt A |                                   | UnixSocket Endpt B |
+--------------------+                                   +--------------------+
          │                                                         ▲
          │ transmit (send)                       receive (recv)    │
          ├────────────────► [ Ring Buffer A -> B ] ────────────────┤
          │                                                         │
          │ receive (recv)                        transmit (send)   │
          ├◄──────────────── [ Ring Buffer B -> A ] ◄───────────────┤
```

### 5.1 The `socketpair()` Primitive
Implemented in [`kernel/src/ipc/unix_socket.rs`](file:///home/deepak/OPENSOURCE/NullOS/kernel/src/ipc/unix_socket.rs):
```rust
pub fn socketpair() -> (UnixSocket, UnixSocket) {
    let a_to_b = Arc::new(Spinlock::new(PipeBuffer::new(8192)));
    let b_to_a = Arc::new(Spinlock::new(PipeBuffer::new(8192)));

    let socket_a = UnixSocket::new(b_to_a.clone(), a_to_b.clone());
    let socket_b = UnixSocket::new(a_to_b, b_to_a);

    (socket_a, socket_b)
}
```

---

## 6. Shell IPC Inspection Tool

NullOS provides the `ipc` diagnostic command in its interactive Unix shell `nsh`:

```
nullos> ipc
Inter-Process Communication (IPC) Resources:
----------------------------------------------
  Anonymous Pipes:      Circular ring buffers (default cap: 4096 bytes)
  Unix Domain Sockets:  Bidirectional socketpairs (default cap: 8192 bytes)
  Process Signals:      POSIX SIGHUP, SIGINT, SIGKILL, SIGSEGV, SIGTERM, SIGCHLD

Shared Memory Segments (SHM):
  SHMID  KEY          SIZE       PAGES    ATTACHMENTS 
  1      0x1337cafe   4096       1        0           
```

---

## 7. Architectural Comparison of IPC Mechanisms

| Mechanism | Directionality | Data Format | Kernel Copy Overhead | Primary Use Case |
| :--- | :---: | :---: | :---: | :--- |
| **Anonymous Pipe** | Unidirectional | Byte stream | Single copy | Shell pipelines, parent-child stdout |
| **Shared Memory** | Bidirectional | Arbitrary memory | **Zero copy** | Video streams, database shared buffers |
| **Signals** | Asynchronous | Integer notification | Zero payload | Process lifecycle, Ctrl+C interrupt |
| **Unix Sockets** | Bidirectional | Byte/Datagram stream | Single copy | Local client-server daemons |

With Phase 10 Milestone 27 complete, NullOS processes possess full capabilities to stream data, share memory, coordinate across CPU cores, and respond to asynchronous system events.
