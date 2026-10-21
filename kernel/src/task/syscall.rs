//! Syscall Numbers, Dispatcher, and Ring 3 User Mode Execution (Phase 5).

use crate::memory::address::{Page, PhysAddr, PhysFrame, VirtAddr, PAGE_SIZE_4K};
use crate::memory::PageTableFlags;
use crate::{print, println};
use core::sync::atomic::{AtomicBool, Ordering};

pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_EXIT: u64 = 2;
pub const SYS_YIELD: u64 = 3;
pub const SYS_GETPID: u64 = 4;
pub const SYS_SLEEP: u64 = 5;

const USER_CODE_VADDR: u64 = 0x0040_0000; // 4 MiB
const USER_STACK_VADDR: u64 = 0x7000_0000;
const USER_STACK_TOP: u64 = USER_STACK_VADDR + PAGE_SIZE_4K - 16;

static USER_DEMO_PASSED: AtomicBool = AtomicBool::new(false);

/// Common Rust System Call Dispatcher invoked from `syscall_entry` in `syscall.S`.
#[unsafe(no_mangle)]
pub extern "C" fn syscall_dispatcher(
    num: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    _a4: u64,
    _a5: u64,
) -> u64 {
    match num {
        SYS_READ => 0,
        SYS_WRITE => {
            let buf_ptr = a2 as *const u8;
            let len = a3 as usize;

            if buf_ptr.is_null() || len == 0 {
                return 0;
            }

            let slice = unsafe { core::slice::from_raw_parts(buf_ptr, len) };
            if let Ok(s) = core::str::from_utf8(slice) {
                print!("{}", s);
            }

            len as u64
        }
        SYS_EXIT => {
            let status = a1;
            println!("\x1b[32m[USER]\x1b[0m User process finished with exit code: {}", status);
            USER_DEMO_PASSED.store(true, Ordering::SeqCst);
            super::exit();
        }
        SYS_YIELD => {
            super::yield_now();
            0
        }
        SYS_GETPID => {
            1337
        }
        SYS_SLEEP => {
            let ticks = a1;
            super::sleep(ticks);
            0
        }
        unknown => {
            println!("[SYSCALL] Unknown syscall number: {}", unknown);
            u64::MAX
        }
    }
}

fn user_thread_runner() {
    println!("[USER] Configuring TSS RSP0 and switching: Ring 0 -> Ring 3 via iretq...");
    unsafe {
        let syscall_stack_top = core::ptr::addr_of!(crate::arch::x86_64::syscall::kernel_syscall_stack_top) as u64;
        crate::arch::x86_64::gdt::set_tss_rsp0(syscall_stack_top);

        crate::arch::x86_64::syscall::enter_user_mode(
            USER_CODE_VADDR,
            USER_STACK_TOP,
        );
    }
}

/// Spawns and executes a test User Mode process in Ring 3 to verify privilege separation and syscalls.
pub fn run_user_mode_self_test() {
    println!();
    println!("----------------------------------------");
    println!(" [USER] Launching Ring 3 User Mode Demo ");
    println!("----------------------------------------");

    // 1. Allocate and map user code page with USER_ACCESSIBLE
    let user_code_vaddr = VirtAddr::new(USER_CODE_VADDR);
    let code_page = Page::containing_address(user_code_vaddr);
    let code_frame = crate::memory::alloc_frame().expect("Out of memory for user code");

    crate::memory::paging::map_page(
        code_page,
        code_frame,
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
    )
    .expect("Failed to map user code page");

    // Copy position-independent assembly user program into the user code page
    let code_start = crate::arch::x86_64::syscall::user_test_program_start as *const () as usize;
    let code_end = crate::arch::x86_64::syscall::user_test_program_end as *const () as usize;
    let code_len = code_end - code_start;
    assert!(code_len > 0 && code_len <= 4096);

    unsafe {
        core::ptr::copy_nonoverlapping(
            code_start as *const u8,
            user_code_vaddr.as_mut_ptr::<u8>(),
            code_len,
        );
    }

    // 2. Allocate and map user stack page with USER_ACCESSIBLE & NO_EXECUTE
    let user_stack_vaddr = VirtAddr::new(USER_STACK_VADDR);
    let stack_page = Page::containing_address(user_stack_vaddr);
    let stack_frame = crate::memory::alloc_frame().expect("Out of memory for user stack");

    crate::memory::paging::map_page(
        stack_page,
        stack_frame,
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE
            | PageTableFlags::NO_EXECUTE,
    )
    .expect("Failed to map user stack page");

    // 3. Spawn a thread to enter user mode
    super::spawn("user_process", user_thread_runner, 1);

    // Yield CPU to let the user mode process run and execute syscalls
    for _ in 0..10 {
        if USER_DEMO_PASSED.load(Ordering::SeqCst) {
            break;
        }
        super::yield_now();
    }

    assert!(
        USER_DEMO_PASSED.load(Ordering::SeqCst),
        "User mode demo program did not complete successfully"
    );

    println!("[USER] Ring 3 User Mode and fast SYSCALL / SYSRET verified successfully!");
}
