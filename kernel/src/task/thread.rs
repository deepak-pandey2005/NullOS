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
        let mut memory = vec![0u8; size].into_boxed_slice();
        if memory.len() >= 8 {
            crate::security::place_canary(memory.as_mut_ptr() as *mut u64);
        }
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
    pub id: ThreadId,
    pub name: String,
    pub state: ThreadState,
    pub stack: KernelStack,
    pub rsp: u64,
    pub priority: u8,
    pub total_ticks: u64,
    pub sleep_until: u64,
    pub fs_base: u64,
    pub process_id: Option<super::process::ProcessId>,
    pub is_user: bool,
    pub exit_code: Option<i32>,
}

unsafe extern "C" {
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

            // 1. Thread entry point (popped 8th by `ret` in switch_threads)
            push(&mut sp, thread_entry_wrapper as *const () as usize as u64);

            // 2. Callee-saved registers
            push(&mut sp, 0); // %r15
            push(&mut sp, 0); // %r14
            push(&mut sp, 0); // %r13
            push(&mut sp, entry as *const () as usize as u64); // %r12
            push(&mut sp, 0); // %rbx
            push(&mut sp, 0); // %rbp

            // 3. Initial RFLAGS
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

            // 2. Callee-saved registers
            push(&mut sp, tls_base);   // %r15
            push(&mut sp, arg);        // %r14
            push(&mut sp, user_stack); // %r13
            push(&mut sp, entry);      // %r12
            push(&mut sp, 0);          // %rbx
            push(&mut sp, 0);          // %rbp

            // 3. Initial RFLAGS
            push(&mut sp, 0x202);
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
