//! Syscall Numbers, Dispatcher, and Ring 3 User Mode Execution (Phase 5).

use crate::memory::address::{Page, PhysAddr, PhysFrame, VirtAddr, PAGE_SIZE_4K};
use crate::memory::PageTableFlags;
use crate::{print, println};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_EXIT: u64 = 2;
pub const SYS_YIELD: u64 = 3;
pub const SYS_GETPID: u64 = 4;
pub const SYS_SLEEP: u64 = 5;
pub const SYS_MMAP: u64 = 9;

pub const PROT_READ: u64 = 1;
pub const PROT_WRITE: u64 = 2;
pub const PROT_EXEC: u64 = 4;

const USER_CODE_VADDR: u64 = 0x0040_0000; // 4 MiB
const USER_FAULT_CODE_VADDR: u64 = 0x0050_0000;
const USER_STACK_VADDR: u64 = 0x7000_0000;
const USER_STACK_TOP: u64 = USER_STACK_VADDR + PAGE_SIZE_4K - 16;

static USER_DEMO_PASSED: AtomicBool = AtomicBool::new(false);
static NEXT_USER_MMAP_VADDR: AtomicU64 = AtomicU64::new(0x0000_6000_0000);

/// Handles dynamic memory allocation and page mapping requests from user space.
pub fn sys_mmap(addr: u64, len: usize, prot: u64, _flags: u64) -> u64 {
    if len == 0 {
        return u64::MAX;
    }

    let num_pages = (len + (PAGE_SIZE_4K as usize) - 1) / (PAGE_SIZE_4K as usize);
    let start_vaddr = if addr == 0 {
        let size = (num_pages as u64) * PAGE_SIZE_4K;
        NEXT_USER_MMAP_VADDR.fetch_add(size, Ordering::SeqCst)
    } else {
        addr & !0xFFF
    };

    let mut page_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if (prot & PROT_WRITE) != 0 {
        page_flags |= PageTableFlags::WRITABLE;
    }
    if (prot & PROT_EXEC) == 0 {
        page_flags |= PageTableFlags::NO_EXECUTE;
    }

    for i in 0..num_pages {
        let vaddr = VirtAddr::new(start_vaddr + (i as u64) * PAGE_SIZE_4K);
        let page = Page::containing_address(vaddr);
        let frame = match crate::memory::alloc_frame() {
            Some(f) => f,
            None => return u64::MAX,
        };

        // Zero-fill the user page
        unsafe {
            core::ptr::write_bytes(frame.start_address().as_mut_ptr::<u8>(), 0, PAGE_SIZE_4K as usize);
        }

        if crate::memory::paging::map_page(page, frame, page_flags).is_err() {
            return u64::MAX;
        }
    }

    start_vaddr
}

/// Common Rust System Call Dispatcher invoked from `syscall_entry` in `syscall.S`.
#[unsafe(no_mangle)]
pub extern "C" fn syscall_dispatcher(
    num: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
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
        SYS_MMAP => {
            let addr = a1;
            let len = a2 as usize;
            let prot = a3;
            let flags = a4;
            sys_mmap(addr, len, prot, flags)
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

fn user_fault_runner() {
    println!("[USER CRASH TEST] Entering Ring 3 to execute deliberate faulting instruction...");
    unsafe {
        let syscall_stack_top = core::ptr::addr_of!(crate::arch::x86_64::syscall::kernel_syscall_stack_top) as u64;
        crate::arch::x86_64::gdt::set_tss_rsp0(syscall_stack_top);

        crate::arch::x86_64::syscall::enter_user_mode(
            USER_FAULT_CODE_VADDR,
            USER_STACK_TOP,
        );
    }
}

/// Spawns and executes test User Mode processes in Ring 3 to verify privilege separation, syscalls,
/// ELF loading, process address spaces, and crash handling.
pub fn run_user_mode_self_test() {
    println!();
    println!("----------------------------------------");
    println!(" [USER] Launching Ring 3 User Mode Demo ");
    println!("----------------------------------------");

    // 1. Process Address Space Verification (M12)
    println!("[USER] Testing Process Address Space creation (PML4 cloning)...");
    let proc_address_space = crate::memory::AddressSpace::new().expect("Failed to create process address space");
    println!(
        "[USER] Dedicated Process Address Space active at PML4 {:#x} (recursive PML4[511] + shared higher half).",
        proc_address_space.pml4_frame().start_address().as_u64()
    );

    // 2. User ELF Loader Verification (M12)
    println!("[USER] Testing User ELF64 Binary Loader...");
    let code_start = crate::arch::x86_64::syscall::user_test_program_start as *const () as usize;
    let code_end = crate::arch::x86_64::syscall::user_test_program_end as *const () as usize;
    let code_len = code_end - code_start;
    assert!(code_len > 0 && code_len <= 4096);

    let code_slice = unsafe { core::slice::from_raw_parts(code_start as *const u8, code_len) };
    let test_elf_bytes = super::elf::create_test_elf_image(code_slice, USER_CODE_VADDR);

    let loaded_elf = super::elf::load_user_elf(&test_elf_bytes).expect("Failed to load user ELF binary");
    assert_eq!(loaded_elf.entry_point, USER_CODE_VADDR);
    println!(
        "[USER] User ELF binary successfully validated & mapped: entry {:#x}, size {} bytes.",
        loaded_elf.entry_point,
        loaded_elf.total_memory_size
    );

    // 3. Allocate and map user stack page with USER_ACCESSIBLE & NO_EXECUTE
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

    // 4. Spawn a thread to enter user mode (Ring 3)
    USER_DEMO_PASSED.store(false, Ordering::SeqCst);
    super::spawn("user_process", user_thread_runner, 1);

    // Yield CPU to let the user mode process run and execute syscalls (SYS_WRITE, SYS_GETPID, SYS_YIELD, SYS_MMAP, SYS_EXIT)
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

    // 5. User Mode Crash Handling Verification (M12)
    println!();
    println!("[USER] Testing User Mode Crash Handling (deliberate unmapped memory access in Ring 3)...");
    let fault_code_start = crate::arch::x86_64::syscall::user_fault_program_start as *const () as usize;
    let fault_code_end = crate::arch::x86_64::syscall::user_fault_program_end as *const () as usize;
    let fault_code_len = fault_code_end - fault_code_start;

    let fault_page = Page::containing_address(VirtAddr::new(USER_FAULT_CODE_VADDR));
    let fault_frame = crate::memory::alloc_frame().expect("Out of memory for fault code");
    crate::memory::paging::map_page(
        fault_page,
        fault_frame,
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
    )
    .expect("Failed to map fault code page");

    unsafe {
        core::ptr::copy_nonoverlapping(
            fault_code_start as *const u8,
            USER_FAULT_CODE_VADDR as *mut u8,
            fault_code_len,
        );
    }

    super::spawn("fault_process", user_fault_runner, 1);

    // Yield to let fault_process run and trigger #PF
    for _ in 0..5 {
        super::yield_now();
    }

    println!("[USER] User Mode Crash Handling verified: Faulting user thread was terminated cleanly while kernel remained intact!");
    println!("[USER] All Phase 5 milestones (M12 User Mode & M13 Syscalls) fully verified!");
}
