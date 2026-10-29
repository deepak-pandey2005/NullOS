# Chapter 8: Multitasking, Context Switching & Schedulers
### *How a Single CPU Core Executes Multiple Programs Concurrently*

How can a computer with a single physical CPU core play music, download a file, and respond to your mouse clicks at the exact same second?

In this chapter, we explore the engine of operating system multitasking: **Threads**, the assembly mechanics of **Context Switching (`switch.S`)**, the initial stack trampoline, and how our **Preemptive Round-Robin Scheduler** uses the APIC timer to guarantee fair CPU time.

---

## 1. The Core Illusion: Time Slicing

A CPU core can execute only **one single instruction at any exact nanosecond**.

To create the illusion of multiple programs running simultaneously, the operating system uses **Time Slicing**:

```
Time --->
[ Thread 1: 10 ms ] -> [ Thread 2: 10 ms ] -> [ Thread 3: 10 ms ] -> [ Thread 1: 10 ms ] ...
```

By switching between threads every 10 milliseconds (100 times per second), human eyes and ears perceive all three programs as running completely in parallel!

---

## 2. Anatomy of a Thread: The Thread Control Block (TCB)

In NullOS ([kernel/src/task/thread.rs](../../kernel/src/task/thread.rs)), every thread is represented by a `Thread` struct:

```rust
pub struct Thread {
    pub id: ThreadId,                  // Unique 64-bit integer ID
    pub name: &'static str,            // Human-readable name for logging ("worker_a")
    pub state: ThreadState,            // Ready, Running, Blocked, or Terminated
    pub rsp: u64,                      // Saved Stack Pointer (%rsp) when suspended!
    pub stack: KernelStack,            // Dedicated 16 KiB heap memory stack
    pub priority: u8,
    pub timeslice_remaining: u32,      // Timer ticks left in current turn
    pub sleep_until: u64,              // Target tick count if sleeping
    pub total_ticks: u64,
}
```

### The Thread Lifecycle States:
```
                +--------------+
                |    Spawn     |
                +-------+------+
                        |
                        v
                +--------------+      Scheduler picks it
                |    Ready     | ----------------------------+
                +--------------+                             |
                        ^                                    v
       Voluntary yield  |                             +--------------+
       or timer preempt |                             |   Running    |
                        +---------------------------- | (On the CPU) |
                                                      +-------+------+
                        +----------------------------+        |
       Sleep duration   | Thread calls sleep()                | Thread finishes
       expires          v                                     v
                +--------------+                      +--------------+
                |   Blocked    |                      |  Terminated  |
                +--------------+                      +--------------+
```

---

## 3. The Dedicated Thread Stack (16 KiB)

Every thread **must** have its own private stack in memory.
If Thread A and Thread B shared the same stack, Thread B's function calls and local variables would immediately overwrite and destroy Thread A's local variables!

In NullOS:
- We allocate a 16 KiB buffer on the kernel heap for each thread.
- Remember: on x86_64, **stacks grow downwards** from high addresses to low addresses!
- `stack.top()` is `stack_bottom + 16384`. The thread's stack pointer (`%rsp`) begins at the top and decrements downwards as functions push data.

---

## 4. The Context Switch: Assembly Magic (`switch.S`)

How do you pause Thread A in the middle of a loop and switch to Thread B?

In NullOS, this is handled by a hand-tuned assembly function: `switch_threads` ([kernel/src/arch/x86_64/switch.S](../../kernel/src/arch/x86_64/switch.S)):

```rust
extern "C" {
    pub fn switch_threads(prev_rsp: *mut u64, next_rsp: u64);
}
```
In the System V AMD64 ABI:
- `%rdi` holds `prev_rsp` (pointer to Thread A's `thread.rsp` field).
- `%rsi` holds `next_rsp` (Thread B's saved stack pointer).

### The Complete Assembly Code:
```assembly
.global switch_threads
.type switch_threads, @function

switch_threads:
    # ------------------------------------------------------------------
    # 1. SAVE THREAD A's CONTEXT
    # ------------------------------------------------------------------
    # Note: The 'call switch_threads' instruction already pushed Thread A's
    # return address (RIP) onto Thread A's stack!

    pushfq            # Push RFLAGS (arithmetic condition flags)
    push %rbp         # Push Base Pointer
    push %rbx         # Push RBX
    push %r12         # Push R12
    push %r13         # Push R13
    push %r14         # Push R14
    push %r15         # Push R15

    # Save Thread A's current stack pointer into its Thread struct:
    mov %rsp, (%rdi)

    # ------------------------------------------------------------------
    # 2. THE QUANTUM LEAP: SWITCH THE STACK POINTER TO THREAD B!
    # ------------------------------------------------------------------
    mov %rsi, %rsp

    # ------------------------------------------------------------------
    # 3. RESTORE THREAD B's CONTEXT
    # ------------------------------------------------------------------
    pop %r15
    pop %r14
    pop %r13
    pop %r12
    pop %rbx
    pop %rbp
    popfq

    # ------------------------------------------------------------------
    # 4. RESUME THREAD B!
    # ------------------------------------------------------------------
    ret
```

### Why does this work? (The Mind-Bending Part):
Look at line: `mov %rsi, %rsp`.
- Before that instruction, the CPU was executing on Thread A's stack.
- The instant `mov %rsi, %rsp` executes, **the CPU is suddenly pointing to Thread B's stack!**
- The subsequent `pop` instructions pop Thread B's saved registers!
- Finally, the **`ret` instruction** pops the address from the top of the stack and jumps to it. Because we are on Thread B's stack, it pops **Thread B's return address** and resumes Thread B exactly where it was suspended days, minutes, or milliseconds ago!

### Why do we only save 6 registers instead of all 16?
In the System V AMD64 calling convention, registers are divided into:
- **Caller-Saved** (`RAX, RCX, RDX, RSI, RDI, R8..R11`): The compiler expects these to be clobbered by any function call. It already saved any values it cared about before calling `switch_threads`.
- **Callee-Saved** (`RBX, RBP, RSP, R12..R15`): A function is **required** to preserve these across calls. By saving only these 6 registers plus `RFLAGS`, we achieve maximum switching speed!

---

## 5. The Thread Bootstrap Trampoline: How New Threads Start

When you create a brand new thread, it has never run before. It has never called `switch_threads`, so it doesn't have an existing saved stack!

If `switch_threads` switches to a new thread's stack and executes `pop` and `ret`, what will it pop?
**It would pop garbage and crash!**

### The Solution: The Artificial Initial Stack Frame
When `Thread::new()` creates a thread ([kernel/src/task/thread.rs](../../kernel/src/task/thread.rs#L70-L115)), we manually pre-populate its stack to look **identical** to a thread that was suspended by `switch_threads`:

```
Top of New Thread Stack:
[ rsp + 56 ] -> Pointer to thread_entry_wrapper (The fake return address!)
[ rsp + 48 ] -> RFLAGS: 0x202 (Interrupts Enabled)
[ rsp + 40 ] -> RBP: 0x0
[ rsp + 32 ] -> RBX: 0x0
[ rsp + 24 ] -> R12: Pointer to real entry function (e.g. worker_a)
[ rsp + 16 ] -> R13: 0x0
[ rsp + 8  ] -> R14: 0x0
[ rsp + 0  ] -> R15: 0x0  <-- thread.rsp points here!
```

When `switch_threads` switches to this new thread for the first time:
1. It pops `r15..r12` (loading the function pointer into `%r12`).
2. It pops `rbx, rbp, rflags`.
3. It executes `ret`!
4. The `ret` instruction pops `thread_entry_wrapper` and jumps to it!
5. `thread_entry_wrapper` calls the function in `%r12`, and when finished, cleanly calls `task::exit()`.

---

## 6. Cooperative vs. Preemptive Multitasking

### A. Cooperative Multitasking: `yield_now()`
When a thread voluntarily relinquishes CPU control:
```rust
pub fn yield_now() {
    // 1. Lock scheduler
    // 2. Move current thread to back of ready_queue
    // 3. Pop next thread from ready_queue
    // 4. Call switch_threads()!
}
```

### B. Preemptive Multitasking: The APIC Timer Interrupt
What if a malicious or buggy program enters an infinite loop:
```rust
loop { /* Never calls yield! */ }
```
In a purely cooperative OS, the entire computer freezes forever.

In NullOS, the **APIC Timer** raises Interrupt 32 every 10 milliseconds:
```rust
pub fn timer_tick(ticks: u64) {
    let mut guard = SCHEDULER.lock();
    // 1. Wake up any sleeping threads whose wake_tick <= ticks
    wake_sleeping_threads(ticks);

    // 2. Decrement active thread's timeslice
    if current.timeslice_remaining > 0 {
        current.timeslice_remaining -= 1;
    }

    // 3. If timeslice expired, PREEMPT IT!
    if current.timeslice_remaining == 0 {
        current.timeslice_remaining = DEFAULT_TIMESLICE;
        // Schedule next thread and call switch_threads!
    }
}
```
The CPU pauses the rogue thread mid-instruction and forcibly hands control to another program!

---

## 7. The `IrqSpinlock`: Preventing Deadlocks

Why can't we use a normal spinlock inside the scheduler?

### The Deadlock Nightmare:
1. Thread A is running in the kernel.
2. Thread A acquires `SCHEDULER.lock()`.
3. **Right at that microsecond**, the APIC Timer interrupt fires!
4. The CPU pauses Thread A and enters the `timer_handler` ISR on the same CPU core.
5. `timer_handler` tries to acquire `SCHEDULER.lock()`.
6. But the lock is already held by Thread A! The ISR spins in a loop waiting for Thread A to release the lock.
7. But Thread A can never run because the ISR is hogging the CPU!
8. **Permanent deadlock. The computer freezes.**

### The Solution:
In NullOS ([kernel/src/sync.rs](../../kernel/src/sync/mod.rs)), our scheduler uses an **`IrqSpinlock`**:
- Before acquiring the lock, it executes the `cli` assembly instruction to **disable interrupts on the local CPU core**.
- It acquires the spinlock safely.
- When the lock is released, it restores the previous interrupt state (`sti`).
- Because interrupts cannot fire while the lock is held, the deadlock is physically impossible!

---

## 🧠 Self-Test & Review Questions

1. **Why does each thread need its own separate 16 KiB stack?**
   <details>
   <summary>Click to view answer</summary>
   A stack stores local variables, function parameters, and return addresses. If two threads shared a stack, Thread B would push data over Thread A's active stack frame, causing corrupted variables, incorrect return addresses, and instant memory crashes when Thread A resumed.
   </details>

2. **Why does `switch_threads` only save callee-preserved registers (`rbx, rbp, r12-r15`) instead of all 16 general-purpose registers?**
   <details>
   <summary>Click to view answer</summary>
   Under the System V AMD64 calling convention, caller-saved registers (like RAX, RDX, RSI, RDI) are already saved by the compiler before calling a function if their values are needed. Only callee-saved registers are guaranteed to remain intact across function calls, so saving only those 6 registers minimizes context switch latency.
   </details>

3. **What is the difference between cooperative and preemptive multitasking?**
   <details>
   <summary>Click to view answer</summary>
   In cooperative multitasking, a thread retains CPU control until it voluntarily yields (via <code>yield_now()</code> or I/O). In preemptive multitasking, a hardware timer interrupt periodically interrupts the active thread, allowing the kernel scheduler to forcibly reassign the CPU to another task even if the thread is stuck in an infinite loop.
   </details>

---

*In [Chapter 9: User Mode (Ring 3), System Calls & Crash Resilience](chapter09_usermode_and_syscalls.md), we explore how the kernel transitions to Ring 3 using `iretq`, enables fast `SYSCALL`/`SYSRET`, and safely isolates user crashes.*
