# Phase 4 — Kernel Execution (Milestones M10 - M11)

This phase establishes multitasking, kernel threading, and preemptive round-robin scheduling for NullOS.

---

## Subsystem Architecture (`kernel/src/task/`)

```
kernel/src/
├── arch/x86_64/
│   └── switch.S         # Assembly context switch primitive (switch_threads)
├── task/
│   ├── mod.rs           # Task coordinator, public API, self-tests
│   ├── thread.rs        # Thread, ThreadId, ThreadState, KernelStack, and Trampoline
│   └── scheduler.rs     # Round-Robin Preemptive Scheduler, RunQueue, SleepQueue
```

---

## Completed Milestones

### M10 — Kernel Threads
- [x] **Thread Structure (TCB)**: `Thread` struct with unique `ThreadId`, name, lifecycle state (`Ready`, `Running`, `Blocked`, `Terminated`), priority, and tick counters.
- [x] **Dedicated Kernel Stack per Thread**: 16 KiB heap-allocated `KernelStack` per thread with automatic cleanup on thread termination.
- [x] **Assembly Context Switch**: Hand-tuned SysV AMD64 assembly primitive `switch_threads(prev_rsp: *mut u64, next_rsp: u64)` in `switch.S`:
  - Saves callee-preserved registers (`r15`, `r14`, `r13`, `r12`, `rbx`, `rbp`, `rflags`).
  - Switches `%rsp` to the next thread.
  - Restores registers and flags from the new thread's stack.
- [x] **Thread Bootstrap Trampoline**: Pre-configured initial stack frame with `thread_entry_wrapper` and entry function pointer in `%r12`.
- [x] **Clean Thread Termination**: Finished threads invoke `task::exit()`, transition to `Terminated`, and automatically drop their stack and resources.

### M11 — Scheduler
- [x] **Run Queue**: Thread queue (`VecDeque<Box<Thread>>`) managing ready-to-run threads.
- [x] **Round-Robin Scheduling**: Fair CPU time allocation among ready threads.
- [x] **Cooperative Multitasking**: `task::yield_now()` to voluntarily yield CPU execution.
- [x] **Timer-Driven Preemption**: Integrated with the APIC Timer interrupt (`TIMER_INTERRUPT_VECTOR = 0x20`):
  - Every timer tick decrements the active thread's timeslice and preemptively switches to the next thread.
- [x] **Sleep & Wake Primitives**: `task::sleep(ticks)` moves threads to `sleeping_queue` and wakes them automatically when the target tick count is reached.
- [x] **Interrupt-Safe Locking**: Scheduler protected by `IrqSpinlock`, ensuring locks are never held across context switches.
- [x] **Built-in Verification Self-Tests**:
  - Spawned two concurrent worker threads (`worker_a` and `worker_b`).
  - Interleaved execution across 9 context switches.
  - Verified iteration counts and clean thread termination.

---

## Verification

Run the automated headless test:
```bash
./scripts/run.sh --headless
```

Expected output:
```
[TASK] Preemptive Thread Scheduler initialized (main thread active).
[TASK] Running Kernel Threads verification self-tests...
[TASK] Spawned thread #1 "worker_a" (stack: 16 KiB)
[TASK] Spawned thread #2 "worker_b" (stack: 16 KiB)
[TASK] Spawned test threads #1 and #2 - interleaving execution:
[TASK] Worker A running iteration 1/3 - yielding CPU...
[TASK] Worker B running iteration 1/3 - yielding CPU...
[TASK] Worker A running iteration 2/3 - yielding CPU...
[TASK] Worker B running iteration 2/3 - yielding CPU...
[TASK] Worker A running iteration 3/3 - yielding CPU...
[TASK] Worker B running iteration 3/3 - yielding CPU...
[TASK] Worker A completed.
[TASK] Thread #1 "worker_a" finished (total ticks: 0).
[TASK] Worker B completed.
[TASK] Thread #2 "worker_b" finished (total ticks: 0).
[TASK] All Kernel Threads self-tests passed successfully! (Cooperative multitasking, Context Switch count: 9)
```
