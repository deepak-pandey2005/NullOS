//! # Kernel Thread Abstractions, Stack Frames, and Lifecycle Management
//!
//! ## Thread Control Block (TCB) Architecture
//! A thread represents an independent sequence of execution. NullOS distinguishes:
//! - **Kernel Threads**: Execute entirely in Ring 0 within the kernel address space.
//! - **User Threads**: Execute in Ring 3 within a parent process's isolated virtual address space.
//!
//! Every thread owns:
//! - A 64 KiB kernel stack ([`KernelStack`]) equipped with security canary protection at its base.
//! - A saved kernel stack pointer (`rsp`) referencing its top-of-stack context switch frame.
//! - An execution state ([`ThreadState`]): `Ready`, `Running`, `Blocked`, or `Terminated`.
//! - Scheduling metadata: Priority, CPU tick runtime accounting, and sleep expiration.
//!
//! ## Context Switch Stack Frame Layout (`switch.S` Contract)
//! When a thread is suspended by [`switch_threads`], its kernel stack contains:
//! ```text
//! Top of Stack (Low Address / RSP)
//!   [RSP + 0x00]: Saved %rbp
//!   [RSP + 0x08]: Saved %rbx
//!   [RSP + 0x10]: Saved %r12 (holds thread entry address)
//!   [RSP + 0x18]: Saved %r13 (holds user stack address for user threads)
//!   [RSP + 0x20]: Saved %r14 (holds user thread argument)
//!   [RSP + 0x28]: Saved %r15 (holds Thread Local Storage %fs base)
//!   [RSP + 0x30]: Saved %rflags (pushed by pushfq; bit 9 IF enabled)
//!   [RSP + 0x38]: Return Address (%rip) -> thread_entry_wrapper or caller
//! Bottom of Stack (High Address)
//! ```

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// Default stack size allocated for kernel and user thread kernel stacks (64 KiB).
pub const DEFAULT_STACK_SIZE: usize = 64 * 1024; // 64 KiB

static NEXT_THREAD_ID: AtomicU64 = AtomicU64::new(1);

/// A unique 64-bit Thread Identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThreadId(pub u64);

impl ThreadId {
    /// Generates a new monotonically increasing thread identifier.
    pub fn new() -> Self {
        Self(NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// The current lifecycle state of an execution thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    /// Runnable and waiting in the scheduler's ready queue.
    Ready,
    /// Currently actively executing on a CPU core.
    Running,
    /// Suspended waiting on an I/O event, mutex, or condition variable.
    Blocked,
    /// Execution complete; awaiting reclamation by scheduler reap or parent `waitpid`.
    Terminated,
}

/// A dedicated kernel execution stack allocated dynamically on the heap with security canaries.
pub struct KernelStack {
    memory: Box<[u8]>,
}

impl KernelStack {
    /// Allocates a new zero-initialized kernel stack of `size` bytes with stack canary.
    pub fn new(size: usize) -> Self {
        let mut memory = vec![0u8; size].into_boxed_slice();
        if memory.len() >= 8 {
            crate::security::place_canary(memory.as_mut_ptr() as *mut u64);
        }
        Self { memory }
    }

    /// Returns the lowest virtual address of the allocated stack buffer.
    #[inline]
    pub fn bottom(&self) -> u64 {
        self.memory.as_ptr() as u64
    }

    /// Returns the 16-byte aligned top virtual address where the stack begins growing downwards.
    #[inline]
    pub fn top(&self) -> u64 {
        (self.bottom() + (self.memory.len() as u64)) & !0xF
    }

    /// Verifies that the stack canary at the bottom of the stack remains uncorrupted.
    #[inline]
    pub fn verify_canary(&self) -> bool {
        if self.memory.len() >= 8 {
            crate::security::verify_canary(self.memory.as_ptr() as *const u64)
        } else {
            true
        }
    }
}

/// Thread Control Block (TCB) representing an independent kernel or user thread of execution.
pub struct Thread {
    /// Unique thread identifier.
    pub id: ThreadId,
    /// Human-readable diagnostic thread name.
    pub name: String,
    /// Current execution state in the scheduler.
    pub state: ThreadState,
    /// Dedicated 64 KiB kernel stack.
    pub stack: KernelStack,
    /// Saved kernel stack pointer (`%rsp`) holding the context frame.
    pub rsp: u64,
    /// Priority level (0 = idle, 1 = normal, 2 = high, 3 = real-time).
    pub priority: u8,
    /// Cumulative CPU ticks spent executing.
    pub total_ticks: u64,
    /// Monotonic tick target when sleeping.
    pub sleep_until: u64,
    /// Thread Local Storage (TLS) register value loaded into `%fs` base.
    pub fs_base: u64,
    /// Process ID of parent process, if associated with a process.
    pub process_id: Option<super::process::ProcessId>,
    /// Whether this thread executes in Ring 3 user mode.
    pub is_user: bool,
    /// Exit status code returned upon termination.
    pub exit_code: Option<i32>,
}

unsafe extern "C" {
    /// Low-level assembly context switch implemented in `switch.S`.
    ///
    /// Saves current CPU callee-saved registers to `prev_rsp_ptr`, switches `%rsp` to `next_rsp`,
    /// restores callee-saved registers, and returns into the next thread.
    pub fn switch_threads(prev_rsp_ptr: *mut u64, next_rsp: u64);
}

/// Trampoline wrapper executed upon first entry into a newly created kernel thread.
extern "C" fn thread_entry_wrapper() -> ! {
    let entry_fn_addr: u64;
    unsafe {
        // Retrieve thread entry function pointer passed in callee-saved register %r12
        core::arch::asm!("mov {}, r12", out(reg) entry_fn_addr, options(nomem, nostack));
        let entry_fn: fn() = core::mem::transmute(entry_fn_addr);
        entry_fn();
    }

    // Thread finished its work, exit cleanly through the scheduler
    super::exit();
}

/// Trampoline wrapper executed upon first entry into a newly created user thread.
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
            crate::arch::x86_64::syscall::write_msr(crate::arch::x86_64::syscall::IA32_FS_BASE_MSR, tls_base);
        }
        crate::arch::x86_64::syscall::enter_user_mode_thread(entry, user_stack, arg);
    }
}

impl Thread {
    /// Creates a new Thread with an allocated stack pre-configured for `switch_threads`.
    ///
    /// Synthesizes a context switch stack frame so that `switch_threads` can pop registers
    /// and `ret` directly into `thread_entry_wrapper`.
    pub fn new(name: &str, entry: fn(), priority: u8) -> Self {
        let id = ThreadId::new();
        let stack = KernelStack::new(DEFAULT_STACK_SIZE);

        let mut sp = stack.top() & !0xF;

        unsafe {
            let push = |sp: &mut u64, val: u64| {
                *sp -= 8;
                let ptr = *sp as *mut u64;
                *ptr = val;
            };

            // 1. Thread entry point (popped by `ret` in switch_threads)
            push(&mut sp, thread_entry_wrapper as *const () as usize as u64);

            // 2. Initial RFLAGS (popped by `popfq` in switch_threads)
            push(&mut sp, 0x202); // RFLAGS with IF enabled

            // 3. Callee-saved registers (popped in order %rbp..%r15)
            push(&mut sp, 0); // %r15
            push(&mut sp, 0); // %r14
            push(&mut sp, 0); // %r13
            push(&mut sp, entry as *const () as usize as u64); // %r12
            push(&mut sp, 0); // %rbx
            push(&mut sp, 0); // %rbp
        }

        Self {
            id,
            name: String::from(name),
            state: ThreadState::Ready,
            stack,
            rsp: sp,
            priority,
            total_ticks: 0,
            sleep_until: 0,
            fs_base: 0,
            process_id: None,
            is_user: false,
            exit_code: None,
        }
    }

    /// Creates a new Ring 3 user thread sharing its parent's process address space.
    pub fn new_user(
        name: &str,
        entry: u64,
        user_stack: u64,
        arg: u64,
        tls_base: u64,
        process_id: super::process::ProcessId,
        priority: u8,
    ) -> Self {
        let id = ThreadId::new();
        let stack = KernelStack::new(DEFAULT_STACK_SIZE);
        let mut sp = stack.top() & !0xF;

        unsafe {
            let push = |sp: &mut u64, val: u64| {
                *sp -= 8;
                let ptr = *sp as *mut u64;
                *ptr = val;
            };

            // 1. Thread entry point
            push(&mut sp, user_thread_wrapper as *const () as usize as u64);

            // 2. Initial RFLAGS
            push(&mut sp, 0x202);

            // 3. Callee-saved registers
            push(&mut sp, tls_base);   // %r15
            push(&mut sp, arg);        // %r14
            push(&mut sp, user_stack); // %r13
            push(&mut sp, entry);      // %r12
            push(&mut sp, 0);          // %rbx
            push(&mut sp, 0);          // %rbp
        }

        Self {
            id,
            name: String::from(name),
            state: ThreadState::Ready,
            stack,
            rsp: sp,
            priority,
            total_ticks: 0,
            sleep_until: 0,
            fs_base: tls_base,
            process_id: Some(process_id),
            is_user: true,
            exit_code: None,
        }
    }

    /// Creates an idle / main kernel thread representation using the existing active stack.
    pub fn new_main() -> Self {
        Self {
            id: ThreadId(0),
            name: String::from("kernel_main"),
            state: ThreadState::Running,
            stack: KernelStack::new(DEFAULT_STACK_SIZE),
            rsp: 0,
            priority: 0,
            total_ticks: 0,
            sleep_until: 0,
            fs_base: 0,
            process_id: Some(super::process::ProcessId::INIT),
            is_user: false,
            exit_code: None,
        }
    }
}
