# Chapter 23: User Threads & Synchronization

### *1:1 Threading Models, Fast Userspace Mutexes (Futex), Thread-Local Storage (TLS), and the POSIX `pthread` Runtime*

---

## 1. Concurrency Within a Single Universe

In Chapter 8, we implemented preemptive multitasking for kernel threads. In Chapter 12 and 14, we created isolated processes, each bound to an independent 4-level PML4 address space (`CR3`).

Processes represent units of **resource ownership and isolation** (memory, file descriptors, credentials). However, many workloads require multiple concurrent paths of execution that share the exact same state without the performance and memory overhead of separate address spaces:
- Web browsers processing HTML parsing, JavaScript execution, and CSS layout simultaneously.
- Game engines updating physics, streaming audio, and preparing GPU command buffers in parallel.
- Database systems executing concurrent transactions over shared buffer pool caches.

A **Thread** is the atomic unit of **CPU scheduling and execution**. In NullOS, threads within the same process share:
- Virtual Address Space (`CR3` / PML4 page directory)
- File Descriptor Table (`stdin`, `stdout`, sockets, pipes)
- Working directory and process credentials (PID, UID, GID)

Each thread possesses its own private:
- Thread Identifier (`ThreadId` / TID)
- User Execution Stack (allocated via `mmap`)
- Dedicated Kernel Stack (16 KiB for syscalls and interrupts)
- CPU Register State (`%rip`, `%rsp`, `%rflags`, callee-preserved registers)
- Thread-Local Storage (TLS) pointer (`IA32_FS_BASE` MSR)

```
                              Process Address Space (PML4)
   0x0000_0000 ┌─────────────────────────────────────────────────────────────┐
               │ .text (Executable Code) & .rodata                           │
   0x0040_0000 ├─────────────────────────────────────────────────────────────┤
               │ .data & .bss (Shared Globals & Static Variables)            │
   0x0060_0000 ├─────────────────────────────────────────────────────────────┤
               │ Dynamic Heap (malloc / free)                                │
               │                                                             │
   0x6002_0000 ├───────────────────────────────┬─────────────────────────────┤
               │ Thread 1 User Stack (64 KiB)  │ Thread 2 User Stack (64 KiB)│
               │           │                   │           │                 │
               │           ▼ (grows down)      │           ▼ (grows down)    │
   0x7000_0000 ├───────────────────────────────┴─────────────────────────────┤
               │ Main Thread User Stack (64 KiB)                             │
   0x7FFF_FFFF └─────────────────────────────────────────────────────────────┘
```

---

## 2. The 1:1 Kernel Threading Model

Operating systems generally implement one of three threading architectures:
1. **N:1 User-Level Threads (Green Threads)**: A userspace runtime multiplexes $N$ user threads onto a single kernel thread. Context switches are fast (no syscalls), but threads cannot execute across multiple physical CPU cores, and one blocking I/O call blocks the entire process.
2. **M:N Hybrid Threads**: $M$ user threads are multiplexed across $N$ kernel worker threads. Highly complex to schedule and prone to priority inversion and signal misdirection.
3. **1:1 Kernel-Managed Threads**: Every userspace thread maps directly to an independent kernel thread of execution scheduled by the OS across all available CPU cores.

NullOS employs the **1:1 Kernel Threading Model**:
- True hardware parallelism: Threads execute concurrently on multiple physical CPU cores.
- Preemptive time-slicing: The Local APIC timer preempts individual user threads without runtime intervention.
- Unified scheduling: The scheduler handles both kernel worker threads and user threads with identical fairness guarantees.

```
       User Space (Ring 3)        [pthread 1]      [pthread 2]      [pthread 3]
                                       │                │                │
       ────────────────────────────────┼────────────────┼────────────────┼────────
       Kernel Space (Ring 0)           ▼                ▼                ▼
                                  [Thread #7]      [Thread #8]      [Thread #9]
                                       │                │                │
       Hardware Cores                  ▼                ▼                ▼
                                   [ CPU 0 ]        [ CPU 1 ]        [ CPU 2 ]
```

---

## 3. User Thread Creation (`SYS_CLONE`)

Creating a user thread requires preparing its hardware execution context before handing control to the scheduler:

```rust
pub fn spawn_user(
    name: &str,
    entry: u64,
    user_stack: u64,
    arg: u64,
    tls_base: u64,
    pid: ProcessId,
    priority: u8,
) -> ThreadId
```

### The Trampoline & Initial Stack Frame

When creating a thread, the kernel crafts an initial stack frame on the new thread's dedicated 16 KiB kernel stack. When `switch_threads` pops this frame, it "returns" into `user_thread_wrapper`:

```rust
extern "C" fn user_thread_wrapper() -> ! {
    let entry: u64;
    let user_stack: u64;
    let arg: u64;
    let tls_base: u64;
    unsafe {
        core::arch::asm!(
            "mov {}, r12",
            "mov {}, r13",
            "mov {}, r14",
            "mov {}, r15",
            out(reg) entry,
            out(reg) user_stack,
            out(reg) arg,
            out(reg) tls_base,
            options(nomem, nostack)
        );
        if tls_base != 0 {
            crate::arch::x86_64::syscall::write_msr(
                crate::arch::x86_64::syscall::IA32_FS_BASE_MSR,
                tls_base,
            );
        }
        crate::arch::x86_64::syscall::enter_user_mode_thread(entry, user_stack, arg);
    }
}
```

The assembly stub `enter_user_mode_thread` in `syscall.S` crafts the hardware `iretq` stack frame, places the thread argument into register `%rdi` (matching the SysV AMD64 C calling convention), and drops privilege into Ring 3:

```assembly
.global enter_user_mode_thread
.type enter_user_mode_thread, @function

enter_user_mode_thread:
    # %rdi = entry_point (u64)
    # %rsi = user_stack_top (u64)
    # %rdx = arg (u64)
    cli
    push $0x1B       # User Data Selector (SS = 0x18 | RPL 3)
    push %rsi        # User Stack Pointer (%rsp)
    push $0x202      # RFLAGS (Interrupts Enabled)
    push $0x23       # User Code Selector (CS = 0x20 | RPL 3)
    push %rdi        # User Instruction Pointer (%rip)
    mov %rdx, %rdi   # 1st argument in SysV AMD64 is %rdi
    xor %rsi, %rsi   # Clear other registers
    xor %rdx, %rdx
    iretq
```

---

## 4. Thread-Local Storage (TLS) & The `%fs_base` ABI

In multi-threaded applications, global variables are shared across all threads. However, certain state must be unique per thread (e.g., `errno`, random number seeds, active thread descriptors).

In 64-bit x86_64, memory segmentation is largely disabled, but the CPU provides two segment base registers for fast thread and CPU localization:
- `%gs`: Used by the operating system kernel for per-CPU control structures.
- `%fs`: Dedicated to user-space **Thread-Local Storage (TLS)**.

Accessing thread-local variables in assembly compiles to an offset relative to `%fs`:
```assembly
mov %fs:0x10, %rax    # Read thread-local variable at offset 16
```

### The `IA32_FS_BASE` MSR (`0xC000_0100`)

The 64-bit base address for `%fs` is programmed via Model-Specific Register `0xC000_0100` (`IA32_FS_BASE`). User applications manipulate this register through the `SYS_ARCH_PRCTL` system call:

```c
#define ARCH_SET_FS 0x1002
#define ARCH_GET_FS 0x1003

syscall(SYS_ARCH_PRCTL, ARCH_SET_FS, tls_pointer);
```

During preemptive context switches, the kernel scheduler preserves and restores each thread's `fs_base`:

```rust
unsafe {
    crate::arch::x86_64::syscall::write_msr(
        crate::arch::x86_64::syscall::IA32_FS_BASE_MSR,
        next.fs_base,
    );
}
```

---

## 5. Fast Userspace Mutexes (Futex)

Prior to Linux 2.5.70, userspace synchronization required entering the kernel via a system call on *every* lock acquisition and release. Because uncontended locks represent over 95% of real-world mutex operations, this incurred massive context-switch overhead.

The **Futex** (Fast Userspace Mutex), invented by Rusty Russell, Ulrich Drepper, and Ingo Molnar, split synchronization into two paths:
1. **Fast Path (Userspace, Zero Syscalls)**: Atomic Compare-and-Swap (CAS) in user space. If the lock is free, it is acquired in a single CPU instruction without entering Ring 0.
2. **Slow Path (Kernel Space, Contention)**: If the lock is already held, the thread calls `SYS_FUTEX` (`FUTEX_WAIT`) to sleep. When the lock is released, the holder calls `SYS_FUTEX` (`FUTEX_WAKE`) to wake blocked waiters.

```
                  Uncontended Mutex Acquisition (Fast Path):
   User Space:    atomic_cas(mutex, 0, 1) ──► Succeeded (0 syscalls!)

                  Contended Mutex Acquisition (Slow Path):
   User Space:    atomic_cas(mutex, 0, 1) ──► Failed (Contended)
                  sys_futex(mutex, FUTEX_WAIT, 2)
                        │
                        ▼ (syscall 202)
   Kernel Space:  verify(*mutex == 2) ──► Block thread in FUTEX_REGISTRY
                  switch_threads() to next ready thread
```

### Kernel Futex Implementation (`kernel/src/sync/futex.rs`)

```rust
pub const FUTEX_WAIT: u32 = 0;
pub const FUTEX_WAKE: u32 = 1;
pub const FUTEX_REQUEUE: u32 = 3;

/// Implements sys_futex(FUTEX_WAIT).
pub fn futex_wait(uaddr: u64, expected: u32) -> Result<i32, &'static str> {
    crate::security::validate_user_buffer(uaddr as *const u8, 4).map_err(|_| "EFAULT")?;

    let actual_val = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
    if actual_val != expected {
        return Err("EAGAIN"); // Value changed; do not sleep!
    }

    let pid = crate::task::syscall::current_pid();
    let tid = crate::task::current_thread_id().ok_or("No active thread")?;

    {
        let mut reg = FUTEX_REGISTRY.lock();
        let queue = reg.waiters.entry((pid, uaddr)).or_insert_with(Vec::new);
        queue.push(tid);
    }

    crate::task::block_current_thread();
    Ok(0)
}

/// Implements sys_futex(FUTEX_WAKE).
pub fn futex_wake(uaddr: u64, max_wake: u32) -> Result<usize, &'static str> {
    crate::security::validate_user_buffer(uaddr as *const u8, 4).map_err(|_| "EFAULT")?;

    let pid = crate::task::syscall::current_pid();
    let mut to_wake: Vec<ThreadId> = Vec::new();

    {
        let mut reg = FUTEX_REGISTRY.lock();
        if let Some(queue) = reg.waiters.get_mut(&(pid, uaddr)) {
            let wake_count = core::cmp::min(max_wake as usize, queue.len());
            for _ in 0..wake_count {
                if !queue.is_empty() {
                    to_wake.push(queue.remove(0));
                }
            }
        }
    }

    let mut woken = 0;
    for tid in to_wake {
        if crate::task::wake_thread(tid) {
            woken += 1;
        }
    }
    Ok(woken)
}
```

---

## 6. Building High-Level Synchronization on Futex

### 1. Mutex (`pthread_mutex_t`)
State is a 32-bit integer:
- `0`: Unlocked
- `1`: Locked without waiters (uncontended)
- `2`: Locked with waiters (contended)

```c
int pthread_mutex_lock(pthread_mutex_t *mutex) {
    uint32_t expected = 0;
    // Fast path: Uncontended lock acquisition
    if (__atomic_compare_exchange_n(&mutex->state, &expected, 1, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) {
        return 0;
    }

    // Slow path: Contended, mark state as 2 and sleep on futex
    while (1) {
        if (__atomic_exchange_n(&mutex->state, 2, __ATOMIC_ACQUIRE) == 0) {
            return 0;
        }
        futex_wait(&mutex->state, 2);
    }
}

int pthread_mutex_unlock(pthread_mutex_t *mutex) {
    uint32_t prev = __atomic_exchange_n(&mutex->state, 0, __ATOMIC_RELEASE);
    if (prev == 2) {
        // Wake up exactly one waiter
        futex_wake(&mutex->state, 1);
    }
    return 0;
}
```

### 2. Counting Semaphore (`sem_t`)

```c
int sem_wait(sem_t *sem) {
    while (1) {
        uint32_t cur = sem->value;
        while (cur > 0) {
            if (__atomic_compare_exchange_n(&sem->value, &cur, cur - 1, 0, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) {
                return 0;
            }
        }
        __atomic_add_fetch(&sem->waiters, 1, __ATOMIC_RELAXED);
        futex_wait(&sem->value, 0);
        __atomic_sub_fetch(&sem->waiters, 1, __ATOMIC_RELAXED);
    }
}

int sem_post(sem_t *sem) {
    __atomic_add_fetch(&sem->value, 1, __ATOMIC_RELEASE);
    if (sem->waiters > 0) {
        futex_wake(&sem->value, 1);
    }
    return 0;
}
```

### 3. Condition Variable (`pthread_cond_t`)

A condition variable enables threads to atomically release an associated mutex and block until another thread signals a state change:

```c
int pthread_cond_wait(pthread_cond_t *cond, pthread_mutex_t *mutex) {
    uint32_t cur_seq = cond->seq;
    pthread_mutex_unlock(mutex);
    futex_wait(&cond->seq, cur_seq);
    pthread_mutex_lock(mutex);
    return 0;
}

int pthread_cond_signal(pthread_cond_t *cond) {
    __atomic_add_fetch(&cond->seq, 1, __ATOMIC_RELEASE);
    futex_wake(&cond->seq, 1);
    return 0;
}
```

---

## 7. Interactive Shell Diagnostics (`threads`)

The `nsh` shell command `threads` inspects all active threads across the operating system in real time:

```
nullos> threads
Active System Threads & Runtime State:
----------------------------------------------------------------------------------
TID    PID    NAME             STATE        PRIO   TICKS      FS_BASE           
----------------------------------------------------------------------------------
0      1      kernel_main      Running      0      254        0x0000000000000000
5      1      bg_job           Terminated   1      1          0x0000000000000000
8      5      THREADS.ELF      Running      1      7          0x0000000000000000
9      5      user_thread      Ready        1      2          0x0000000060020000
10     5      user_thread      Blocked      1      3          0x0000000060030000
```

---

## 8. Summary & Mastery Checklist

In this chapter, we implemented the complete multi-threaded runtime foundation for NullOS:
- [x] **1:1 Threading Architecture**: Direct mapping of userspace POSIX threads to kernel scheduled threads.
- [x] **Hardware User Mode Entry**: Custom `enter_user_mode_thread` passing arguments via `%rdi` and user stacks.
- [x] **x86_64 TLS ABI**: `IA32_FS_BASE` MSR (`0xC000_0100`) preservation and `arch_prctl` control.
- [x] **Fast Userspace Mutex (Futex)**: `FUTEX_WAIT`, `FUTEX_WAKE`, and `FUTEX_REQUEUE` in `kernel/src/sync/futex.rs`.
- [x] **Userspace Synchronization Primitives**: Mutex, counting Semaphore, and Condition Variable built on Futex.
- [x] **POSIX Libc pthread Library**: `pthread_create`, `pthread_join`, `pthread_exit`, `sem_init`, `sem_wait`, `sem_post`.
- [x] **Diagnostic Inspection**: Interactive `threads` command in `nsh`.
