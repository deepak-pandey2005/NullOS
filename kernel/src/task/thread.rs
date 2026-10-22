//! Kernel Thread abstractions, stack frames, and lifecycle management.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use core::sync::atomic::{AtomicU64, Ordering};

pub const DEFAULT_STACK_SIZE: usize = 16 * 1024; // 16 KiB

static NEXT_THREAD_ID: AtomicU64 = AtomicU64::new(1);

/// A unique 64-bit Thread Identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThreadId(pub u64);

impl ThreadId {
    pub fn new() -> Self {
        Self(NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// The current lifecycle state of a kernel thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    Ready,
    Running,
    Blocked,
    Terminated,
}

/// A dedicated kernel execution stack allocated dynamically on the heap.
pub struct KernelStack {
    memory: Box<[u8]>,
}

impl KernelStack {
    pub fn new(size: usize) -> Self {
        let memory = vec![0u8; size].into_boxed_slice();
        Self { memory }
    }

    #[inline]
    pub fn bottom(&self) -> u64 {
        self.memory.as_ptr() as u64
    }

    #[inline]
    pub fn top(&self) -> u64 {
        (self.bottom() + (self.memory.len() as u64)) & !0xF
    }
}

/// Thread Control Block (TCB) representing an independent kernel thread of execution.
pub struct Thread {
    pub id: ThreadId,
    pub name: String,
    pub state: ThreadState,
    pub stack: KernelStack,
    pub rsp: u64,
    pub priority: u8,
    pub total_ticks: u64,
    pub sleep_until: u64,
}

unsafe extern "C" {
    pub fn switch_threads(prev_rsp_ptr: *mut u64, next_rsp: u64);
}

/// Trampoline wrapper executed upon first entry into a newly created thread.
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

impl Thread {
    /// Creates a new Thread with an allocated stack pre-configured for `switch_threads`.
    pub fn new(name: &str, entry: fn(), priority: u8) -> Self {
        let id = ThreadId::new();
        let stack = KernelStack::new(DEFAULT_STACK_SIZE);

        // Pre-configure stack frame so switch_threads can "return" into thread_entry_wrapper.
        // SysV AMD64 stack must be 16-byte aligned before `call`.
        let mut sp = stack.top() & !0xF;

        unsafe {
            // Reserve space for the context frame pushed by switch_threads:
            // 1. Return address (%rip): thread_entry_wrapper
            // 2. Flags: RFLAGS (0x202 = Interrupts Enabled)
            // 3. %rbp: 0
            // 4. %rbx: 0
            // 5. %r12: entry function address (read by thread_entry_wrapper)
            // 6. %r13: 0
            // 7. %r14: 0
            // 8. %r15: 0

            let push = |sp: &mut u64, val: u64| {
                *sp -= 8;
                let ptr = *sp as *mut u64;
                *ptr = val;
            };

            // 1. Thread entry point (popped 8th by `ret` in switch_threads)
            push(&mut sp, thread_entry_wrapper as *const () as usize as u64);

            // 2. Callee-saved registers (popped in reverse order of switch_threads pushes)
            // switch_threads pushes: r15, r14, r13, r12, rbx, rbp, flags
            // switch_threads pops:   flags, rbp, rbx, r12, r13, r14, r15, ret
            push(&mut sp, 0); // %r15 (popped 7th)
            push(&mut sp, 0); // %r14 (popped 6th)
            push(&mut sp, 0); // %r13 (popped 5th)
            push(&mut sp, entry as *const () as usize as u64); // %r12 (popped 4th by switch_threads)
            push(&mut sp, 0); // %rbx (popped 3rd)
            push(&mut sp, 0); // %rbp (popped 2nd)

            // 3. Initial RFLAGS (popped 1st by `popfq`)
            push(&mut sp, 0x202); // RFLAGS with IF enabled
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
        }
    }
}
