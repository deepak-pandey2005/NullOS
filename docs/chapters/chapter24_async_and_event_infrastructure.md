# Chapter 24: Async & Event Infrastructure
### *Multiplexed Polling (poll/epoll), Event Notification Descriptors (eventfd), High-Resolution Timers (timerfd), Scatter-Gather Vectored I/O (readv/writev), and SysV ABI Stack Alignment*

---

## 1. The Scaling Problem: From One-Thread-Per-Client to Event-Driven I/O

In Chapters 22 and 23, we developed Inter-Process Communication (IPC) primitives—pipes, sockets, shared memory—and the 1:1 kernel threading subsystem with `pthread` APIs. With these mechanisms, an operating system can handle multiple concurrent connections by spawning a dedicated thread for each client:

```
Client 1 ───► Thread 1 (blocks in read())
Client 2 ───► Thread 2 (blocks in read())
Client 3 ───► Thread 3 (blocks in read())
```

While intuitive, the **thread-per-connection** model suffers from fundamental scalability limitations, historically known as the **$C10K$ problem** (handling 10,000+ simultaneous connections):
1. **Memory Overhead**: Each thread requires a dedicated execution stack (typically 16 KiB to 64 KiB in userspace plus 16 KiB in the kernel). Allocating 10,000 threads consumes hundreds of megabytes of RAM purely for inactive stacks.
2. **Context Switching Overhead**: When thousands of threads are blocked waiting for network packets or user inputs, the CPU scheduler spends excessive clock cycles swapping page tables, saving/restoring registers, and invalidating hardware caches.
3. **Cache Trashing**: Constant preemption between thousands of threads evicts L1/L2 data and instruction caches, severely degrading overall instruction throughput.

To overcome this bottleneck, modern operating systems adopt **event-driven, non-blocking I/O multiplexing**: a single thread can monitor hundreds or thousands of file descriptors simultaneously, putting the thread to sleep until hardware or peer activity makes at least one descriptor ready for non-blocking read or write operations.

```
                  ┌───────────────────────────────┐
                  │ Single Event-Loop User Thread │
                  └───────────────┬───────────────┘
                                  │ epoll_wait() / poll()
                                  ▼
┌─────────────────────────────────┴─────────────────────────────────┐
│                    NullOS Kernel VFS & Event Layer                │
├──────────────────┬──────────────────┬───────────────┬─────────────┤
│ Socket / Pipe FD │ Serial Device FD │  EventFd (42) │ TimerFd (1) │
│   (POLLIN ready) │    (Idle)        │ (POLLIN ready)│  (Expired)  │
└──────────────────┴──────────────────┴───────────────┴─────────────┘
```

Milestone 29 equips NullOS with a complete asynchronous event infrastructure conforming to modern POSIX and Linux specifications:
- **`poll(2)`**: Multiplexed polling across arrays of arbitrary file descriptors.
- **`epoll(7)`**: Scalable $O(1)$ event monitoring with interest-list control.
- **`eventfd(2)`**: 64-bit atomic counter event descriptors for lockless cross-thread/process signaling.
- **`timerfd(2)`**: High-resolution interval and single-shot timer descriptors integrated directly into the file descriptor table.
- **`readv(2)` / `writev(2)`**: Scatter-gather vectored I/O for zero-copy multi-buffer transfers.
- **Non-blocking I/O (`O_NONBLOCK`)**: Returning `-EAGAIN` (`-11`) when I/O would block.

---

## 2. Scatter-Gather Vectored I/O (`readv` and `writev`)

Standard `read()` and `write()` system calls transfer data between a file descriptor and a single contiguous memory buffer. In real-world applications (such as network protocol stacks or database engines), data is frequently partitioned across disparate memory locations:
- A protocol header in buffer A.
- A fixed metadata payload in buffer B.
- A dynamic data payload in buffer C.

Without vectored I/O, user programs must either:
1. Concatenate all segments into a single contiguous buffer using `memcpy` before invoking `write()` (incurring memory copy overhead).
2. Execute multiple sequential `write()` system calls (incurring repeated kernel transition overhead and losing write atomicity).

### The `struct iovec` Architecture
Scatter-gather I/O solves this by allowing applications to pass an array of buffer descriptors in a single system call:

```c
struct iovec {
    void  *iov_base;  /* Starting virtual address of buffer */
    size_t iov_len;   /* Number of bytes to transfer */
};
```

In the NullOS kernel (`kernel/src/task/event.rs`), vectored structures are defined with SysV C representation:

```rust
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct IoVec {
    pub iov_base: u64,
    pub iov_len: usize,
}
```

When handling `SYS_WRITEV` (`kernel/src/task/syscall.rs`):
1. The kernel validates that the `iovec` array resides strictly within userspace via `validate_user_buffer`.
2. The kernel iterates through each `iovec`, validates its individual buffer boundaries, and transfers data sequentially to the target `FileDescriptor`:

```rust
let mut total_written = 0usize;
for iov in iovs {
    if iov.iov_len == 0 { continue; }
    let buf_ptr = iov.iov_base as *const u8;
    validate_user_buffer(buf_ptr, iov.iov_len)?;
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr, iov.iov_len) };
    let n = handle.write(slice)?;
    total_written += n;
    if n < iov.iov_len { break; }
}
```

---

## 3. Multiplexed Event Polling: `poll(2)`

The `poll(2)` system call (`SYS_POLL: 7`) enables a process to monitor a set of file descriptors to see if any can perform read or write operations without blocking.

### The `struct pollfd` Contract
```c
struct pollfd {
    int   fd;       /* File descriptor to inspect */
    short events;   /* Requested events bitmask (input) */
    short revents;  /* Returned events bitmask (output) */
};
```

The standard POSIX event bitmasks supported in NullOS include:
- `POLLIN` (`0x0001`): Data is ready to read without blocking.
- `POLLPRI` (`0x0002`): Urgent or exceptional condition data available.
- `POLLOUT` (`0x0004`): File descriptor can accept writes without blocking.
- `POLLERR` (`0x0008`): Error condition occurred on the descriptor.
- `POLLHUP` (`0x0010`): Peer device or stream hung up / closed.
- `POLLNVAL` (`0x0020`): Invalid request (the file descriptor is not open).

### The Inode Poll Interface
In NullOS, every object accessible via a file descriptor implements the `Inode` trait (`kernel/src/fs/vfs.rs`). Milestone 29 added the `poll()` method to the `Inode` trait:

```rust
pub trait Inode: Send + Sync {
    // ... stat, read_at, write_at, etc. ...
    
    /// Returns the bitmask of currently available poll readiness events.
    fn poll(&self) -> u32 {
        POLLIN | POLLOUT
    }

    /// Downcasting helper for dynamic type inspection.
    fn as_any(&self) -> Option<&dyn core::any::Any> {
        None
    }
}
```

### The Kernel Polling Loop
When `SYS_POLL` is invoked:
1. The kernel validates the userspace `pollfd` slice.
2. It queries each descriptor's underlying `Inode::poll()`.
3. If any descriptor matches its requested `events` (or triggers `POLLERR`/`POLLHUP`), the kernel fills `revents` and increments `ready_count`.
4. If `ready_count > 0` or if `timeout_ticks == 0`, the syscall returns immediately with the number of ready descriptors.
5. If no descriptors are ready and a timeout is specified, the current thread voluntarily yields execution via `super::yield_now()`, allowing other threads or interrupt handlers to run before the next check.

---

## 4. Scalable Event Notification: `epoll(7)`

While `poll(2)` works well for small numbers of descriptors, it exhibits an $O(N)$ algorithmic complexity: on every call, the application must pass the entire array of descriptors from userspace into kernel space, and the kernel must inspect every single descriptor.

Linux introduced `epoll` to achieve $O(1)$ scaling. Instead of passing an array on every call, the kernel maintains an **Epoll Instance** in kernel space.

### The Epoll Instance (`EpollInstance`)
An `EpollInstance` is itself a first-class file descriptor managed by the VFS:

```rust
pub struct EpollItem {
    pub fd: i32,
    pub events: u32,
    pub data: u64,
}

pub struct EpollInstance {
    interest_list: Spinlock<BTreeMap<i32, EpollItem>>,
    inode_num: u64,
}
```

### The Epoll Syscall Triad
1. **`epoll_create1(flags)` (`SYS_EPOLL_CREATE1: 291`)**:
   Allocates a new `EpollInstance` inode, wraps it in a `FileDescriptor`, and registers it into the process's file descriptor table.
2. **`epoll_ctl(epfd, op, fd, event)` (`SYS_EPOLL_CTL: 233`)**:
   Controls the interest list:
   - `EPOLL_CTL_ADD` (`1`): Registers a target file descriptor and its event mask (`EPOLLIN`, `EPOLLOUT`, etc.) along with user-defined 64-bit metadata (`event.data.u64`).
   - `EPOLL_CTL_DEL` (`2`): Removes a file descriptor from the interest list.
   - `EPOLL_CTL_MOD` (`3`): Modifies the monitored events or metadata for an existing descriptor.
3. **`epoll_wait(epfd, events, maxevents, timeout)` (`SYS_EPOLL_WAIT: 232`)**:
   Polls only the registered descriptors in the interest list, populating the user's `events` array with descriptors that are currently ready.

---

## 5. 64-Bit Event Notification Descriptors: `eventfd(2)`

An `eventfd` object (`SYS_EVENTFD: 284`, `SYS_EVENTFD2: 290`) is an in-kernel 64-bit unsigned integer counter used as an event wait/notify mechanism. It is vastly more efficient than an anonymous pipe because it requires zero buffer copies:

```rust
pub struct EventFd {
    counter: Spinlock<u64>,
    flags: i32,
    inode_num: u64,
}
```

### Operational Semantics
- **Writing (`eventfd_write` / `write`)**:
  Writing an 8-byte integer `val` adds `val` to the internal 64-bit counter. If adding `val` would cause an overflow (`0xFFFFFFFFFFFFFFFF`), the call blocks or returns `-EAGAIN`.
- **Reading (`eventfd_read` / `read`)**:
  - In default mode: Reading an 8-byte buffer returns the current counter value and resets the internal counter to 0.
  - In semaphore mode (`EFD_SEMAPHORE`): Reading returns the value `1`, and decrements the internal counter by `1`.
  - If the counter is 0, reading blocks or returns `-EAGAIN` (`FsError::WouldBlock`).
- **Poll Integration**:
  `EventFd::poll()` returns `POLLIN` whenever `counter > 0`, and `POLLOUT` whenever `counter < u64::MAX - 1`.

---

## 6. High-Resolution Timer Descriptors: `timerfd(2)`

The `timerfd` API (`timerfd_create`, `timerfd_settime`, `timerfd_gettime`) allows timers to be read, polled, and multiplexed through epoll just like standard sockets or files.

### The POSIX `struct itimerspec`
```c
struct timespec {
    long tv_sec;   /* Seconds */
    long tv_nsec;  /* Nanoseconds (0 .. 999,999,999) */
};

struct itimerspec {
    struct timespec it_interval;  /* Interval for periodic timer */
    struct timespec it_value;     /* Initial expiration delay */
};
```

### The Kernel `TimerFd` Implementation
In NullOS (`kernel/src/task/event.rs`):
- `target_tick`: The hardware Local APIC tick when the timer will next expire.
- `interval_ticks`: The period in ticks for automatic reloading (or 0 for single-shot).
- `expirations`: A 64-bit counter tracking how many times the timer has expired since the last read.

```rust
pub struct TimerFd {
    pub clockid: i32,
    pub flags: i32,
    target_tick: Spinlock<Option<u64>>,
    interval_ticks: Spinlock<u64>,
    expirations: Spinlock<u64>,
    inode_num: u64,
}
```

### Sub-Tick Safeguard
With the NullOS Local APIC timer ticking at 100 Hz (1 tick = 10 ms), converting small nanosecond delays to ticks via integer division `(nsec * 100) / 1_000_000_000` could yield 0 ticks for intervals under 10 ms. In POSIX semantics, setting `it_value != 0` must arm the timer. NullOS guarantees that any positive non-zero delay is clamped to at least 1 tick:

```rust
let mut val_ticks = (new_val.it_value.tv_sec.max(0) as u64) * hz
    + ((new_val.it_value.tv_nsec.max(0) as u64) * hz / 1_000_000_000);
if val_ticks == 0 && (new_val.it_value.tv_sec > 0 || new_val.it_value.tv_nsec > 0) {
    val_ticks = 1;
}
```

---

## 7. Deep-Dive: System V AMD64 ABI Kernel Stack Alignment

During the implementation of Milestone 29, a subtle hardware exception was uncovered when user threads invoked `SYS_POLL`. Understanding this bug provides a masterclass in CPU architecture, stack frames, and calling conventions.

### The System V AMD64 ABI Rule
The System V AMD64 ABI specification dictates:
> *"The stack pointer `%rsp` shall always be a multiple of 16 immediately prior to executing any `call` instruction. Consequently, upon entry to the called function (after `call` has pushed the 8-byte return address), `(%rsp + 8)` is a multiple of 16."*

Modern compilers generate vector instructions (such as SSE `movaps`, `movdqa`) that assume 16-byte alignment. If `%rsp` is misaligned, an aligned vector store immediately triggers a **General Protection Fault (`#GP`, Vector 13)**.

### The Syscall Context Trap
When a user process executes `syscall`, the CPU enters `syscall_entry` in `kernel/src/arch/x86_64/syscall.S`:
1. The kernel stack pointer `current_kernel_rsp` is loaded into `%rsp`. This stack top is 16-byte aligned (`0 mod 16`).
2. The stub preserves user state by pushing 15 64-bit registers:
   ```assembly
   push user_rsp_scratch(%rip)  # 1
   push %r11                    # 2
   push %rcx                    # 3
   push %r15                    # 4
   push %r14                    # 5
   push %r13                    # 6
   push %r12                    # 7
   push %rbp                    # 8
   push %rbx                    # 9
   push %r10                    # 10
   push %r9                     # 11
   push %r8                     # 12
   push %rdx                    # 13
   push %rsi                    # 14
   push %rdi                    # 15
   ```
3. **The Arithmetic**:
   $$15 \times 8 \text{ bytes} = 120 \text{ bytes}$$
   $$120 \pmod{16} = 8$$

Because $120$ is not a multiple of 16, the stack pointer was misaligned by exactly 8 bytes when calling `syscall_dispatcher`!

### The Consequence: Debug `#DB` Exceptions During Context Switch
In earlier milestones, most system calls executed simple logic and returned immediately without context switching. However, in `SYS_POLL`, when no descriptors are ready, the kernel calls `yield_now()`, triggering `switch_threads`:
```assembly
switch_threads:
    push %r15
    push %r14
    push %r13
    push %r12
    push %rbx
    push %rbp
    pushfq
    mov %rsp, (%rdi)
    mov %rsi, %rsp
    popfq
    ...
```
Because the stack was misaligned upon entering Rust code, the relative offsets where `pushfq` stored RFLAGS and where `switch_threads` restored them were skewed by 8 bytes. As a result, `popfq` loaded a corrupted word into `%rflags` where bit 8 (Trap Flag `TF`) was set to 1, causing the CPU to instantly trigger **Exception 1 (Debug `#DB`)** upon the very next instruction!

### The Solution
By padding the stack with `sub $8, %rsp` before `call syscall_dispatcher` and cleaning it up with `add $8, %rsp` upon return, the total allocated frame becomes $120 + 8 = 128$ bytes ($128 \pmod{16} = 0$), restoring flawless System V ABI conformance:

```assembly
    sub $8, %rsp      # 16-byte stack alignment: 15 pushes (120) + 8 = 128 bytes (0 mod 16)
    call syscall_dispatcher
    add $8, %rsp      # Restore stack pointer to match 15 pop instructions
```

Additionally, `%r8`, `%r9`, and `%r10` are saved and restored to guarantee that caller-saved user variables held in these registers across syscalls are never clobbered.

---

## 8. Userspace C Runtime & Libc Integration

Milestone 29 equips `user/libc` with standard POSIX headers:

1. **`<sys/uio.h>`**: Defines `struct iovec` and `writev(fd, iov, iovcnt)`.
2. **`<sys/poll.h>`**: Defines `struct pollfd` and `poll(fds, nfds, timeout)`.
3. **`<sys/epoll.h>`**: Defines `struct epoll_event`, `epoll_create1()`, `epoll_ctl()`, `epoll_wait()`.
4. **`<sys/eventfd.h>`**: Defines `eventfd()`, `eventfd_read()`, `eventfd_write()`.
5. **`<sys/timerfd.h>`**: Defines `struct itimerspec`, `timerfd_create()`, `timerfd_settime()`.

### Verification Binary: `user/bin/async_test.c`
The user-mode test binary (`ASYNC.ELF`) systematically validates all async components:
- **Phase 1**: Vectored output using `writev` across 3 discrete memory slices.
- **Phase 2**: Atomic signaling using `eventfd` and readiness verification via `poll`.
- **Phase 3**: Epoll registration via `epoll_ctl(EPOLL_CTL_ADD)` and event retrieval with custom user metadata (`0xCAFE`) via `epoll_wait`.
- **Phase 4**: Arming a high-resolution timer with `timerfd_settime` (20 ms expiration) and validating expiration arrival with `poll` and `read`.

---

## 9. Automated Kernel Self-Tests

During kernel boot, automated Test 21 runs in `kernel/src/task/shell.rs`:
```rust
println!("[SHELL] 21. Testing Async & Event Infrastructure (poll, epoll, eventfd, timerfd, writev)...");
let evfd = crate::task::event::EventFd::new(10, 0);
assert_eq!(evfd.poll() & crate::fs::vfs::POLLIN, crate::fs::vfs::POLLIN);
// ...
execute_command("exec /disk/ASYNC.ELF");
```

Upon launching QEMU with `./scripts/run.sh --headless`, all tests pass cleanly:
```
====================================================
   NullOS Async & Event Infrastructure Demo (M29)   
====================================================
[ASYNC] 1. Testing writev(2) scatter-gather output...
Hello from writev(2) vectored I/O!
[ASYNC] writev successfully output 35 bytes.
[ASYNC] 2. Testing eventfd(2) and poll(2)...
[ASYNC] eventfd written and read back value: 42
[ASYNC] 3. Testing epoll_create1(2), epoll_ctl(2), and epoll_wait(2)...
[ASYNC] epoll_wait successfully received event for data: 0xcafe
[ASYNC] 4. Testing timerfd_create(2) and timerfd_settime(2)...
[ASYNC] timerfd successfully expired with 1 expiration tick(s).
[ASYNC] All Async & Event Subsystem tests passed successfully!
```

---

## 10. Summary & Milestone Checklist

| Component | Status | Implementation Details |
| :--- | :---: | :--- |
| **`poll(2)`** | ✅ Done | `SYS_POLL (7)`, `struct pollfd`, `Inode::poll()` interface, yielding loops |
| **`epoll(7)`** | ✅ Done | `SYS_EPOLL_CREATE1 (291)`, `SYS_EPOLL_CTL (233)`, `SYS_EPOLL_WAIT (232)` |
| **`eventfd(2)`** | ✅ Done | `SYS_EVENTFD (284)`, `SYS_EVENTFD2 (290)`, atomic 64-bit counter descriptors |
| **`timerfd(2)`** | ✅ Done | `SYS_TIMERFD_CREATE (283)`, `SYS_TIMERFD_SETTIME (286)`, APIC tick timers |
| **`readv`/`writev`** | ✅ Done | `SYS_READV (19)`, `SYS_WRITEV (20)`, scatter-gather `struct iovec` arrays |
| **Non-blocking I/O** | ✅ Done | `O_NONBLOCK (0o4000)`, `-EAGAIN (-11)` / `FsError::WouldBlock` protocol |
| **SysV ABI Alignment** | ✅ Done | 16-byte `%rsp` kernel stack alignment, register preservation in `syscall.S` |
| **Libc POSIX Headers** | ✅ Done | `<sys/poll.h>`, `<sys/epoll.h>`, `<sys/eventfd.h>`, `<sys/timerfd.h>`, `<sys/uio.h>` |
| **Self-Tests & Demo** | ✅ Done | `user/bin/async_test.c` (`ASYNC.ELF`), automated Shell Test 21 |

