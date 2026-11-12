//! # System Call Dispatcher & Ring 3 Execution Engine
//!
//! ## System Call Architecture (x86_64 AMD64 ABI)
//! NullOS implements fast hardware system calls using the x86_64 `SYSCALL` and `SYSRET`
//! instructions. When a Ring 3 user process executes `SYSCALL`:
//! 1. Hardware loads `%rip` from `IA32_LSTAR` (`syscall_entry` in `syscall.S`).
//! 2. Hardware loads `%cs` and `%ss` from `IA32_STAR`.
//! 3. Hardware saves user `%rip` to `%rcx` and user `%rflags` to `%r11`.
//! 4. The kernel stub switches `%rsp` to the current thread's kernel stack (`set_kernel_rsp`),
//!    saves general-purpose registers, maps `%r10` to the 4th argument (replacing `%rcx`),
//!    and invokes [`syscall_dispatcher`].
//! 5. Upon completion, the return value in `%rax` is returned to userspace via `sysretq`.
//!
//! ## System V AMD64 Syscall Calling Conventions:
//! - Syscall Number: `%rax`
//! - Argument 1: `%rdi`
//! - Argument 2: `%rsi`
//! - Argument 3: `%rdx`
//! - Argument 4: `%r10` (converted from `%rcx` which is destroyed by `syscall`)
//! - Argument 5: `%r8`
//! - Argument 6: `%r9`
//! - Return Value: `%rax` (0 or positive on success, `-errno` on failure)
//!
//! ## Security & Buffer Isolation Invariants:
//! All user pointer arguments are validated via [`crate::security::validate_user_buffer`]
//! to ensure user requests cannot read or mutate higher-half kernel memory.

use crate::memory::address::{Page, PhysAddr, PhysFrame, VirtAddr, PAGE_SIZE_4K};
use crate::memory::PageTableFlags;
use crate::{print, println};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// POSIX read(2) syscall number (0).
pub const SYS_READ: u64 = 0;
/// POSIX write(2) syscall number (1).
pub const SYS_WRITE: u64 = 1;
/// POSIX open(2) syscall number (2).
pub const SYS_OPEN: u64 = 2;
/// POSIX close(2) syscall number (3).
pub const SYS_CLOSE: u64 = 3;
/// POSIX stat(2) syscall number (4).
pub const SYS_STAT: u64 = 4;
/// POSIX lseek(2) syscall number (8).
pub const SYS_LSEEK: u64 = 8;
/// POSIX mmap(2) syscall number (9).
pub const SYS_MMAP: u64 = 9;
/// POSIX pipe(2) syscall number (22).
pub const SYS_PIPE: u64 = 22;
/// POSIX sched_yield(2) syscall number (24).
pub const SYS_YIELD: u64 = 24;
/// POSIX getpid(2) syscall number (39).
pub const SYS_GETPID: u64 = 39;
/// Linux clone(2) thread spawn syscall number (56).
pub const SYS_CLONE: u64 = 56;
/// Thread termination syscall number (57).
pub const SYS_THREAD_EXIT: u64 = 57;
/// Thread join syscall number (58).
pub const SYS_THREAD_JOIN: u64 = 58;
/// POSIX exit(2) process exit syscall number (60).
pub const SYS_EXIT: u64 = 60;
/// POSIX waitpid(2) child reap syscall number (61).
pub const SYS_WAITPID: u64 = 61;
/// POSIX kill(2) process signal syscall number (62).
pub const SYS_KILL: u64 = 62;
/// Sleep timer syscall number (100).
pub const SYS_SLEEP: u64 = 100;
/// Linux arch_prctl(2) architecture control syscall number (158).
pub const SYS_ARCH_PRCTL: u64 = 158;
/// POSIX gettid(2) thread identifier syscall number (186).
pub const SYS_GETTID: u64 = 186;
/// Linux futex(2) fast userspace mutex syscall number (202).
pub const SYS_FUTEX: u64 = 202;

/// POSIX poll(2) syscall number (7).
pub const SYS_POLL: u64 = 7;
/// POSIX readv(2) vectored read syscall number (19).
pub const SYS_READV: u64 = 19;
/// POSIX writev(2) vectored write syscall number (20).
pub const SYS_WRITEV: u64 = 20;
/// Linux epoll_create(2) syscall number (213).
pub const SYS_EPOLL_CREATE: u64 = 213;
/// Linux epoll_wait(2) syscall number (232).
pub const SYS_EPOLL_WAIT: u64 = 232;
/// Linux epoll_ctl(2) syscall number (233).
pub const SYS_EPOLL_CTL: u64 = 233;
/// Linux timerfd_create(2) syscall number (283).
pub const SYS_TIMERFD_CREATE: u64 = 283;
/// Linux eventfd(2) syscall number (284).
pub const SYS_EVENTFD: u64 = 284;
/// Linux timerfd_settime(2) syscall number (286).
pub const SYS_TIMERFD_SETTIME: u64 = 286;
/// Linux timerfd_gettime(2) syscall number (287).
pub const SYS_TIMERFD_GETTIME: u64 = 287;
/// Linux eventfd2(2) syscall number (290).
pub const SYS_EVENTFD2: u64 = 290;
/// Linux epoll_create1(2) syscall number (291).
pub const SYS_EPOLL_CREATE1: u64 = 291;
/// Dynamic linker dlopen syscall number (300).
pub const SYS_DLOPEN: u64 = 300;
/// Dynamic linker dlsym syscall number (301).
pub const SYS_DLSYM: u64 = 301;
/// Dynamic linker dlclose syscall number (302).
pub const SYS_DLCLOSE: u64 = 302;
/// GUI compositor: create user window (`title_ptr`, `title_len`, `width`, `height`) -> `win_id`.
pub const SYS_GUI_CREATE_WINDOW: u64 = 310;
/// GUI compositor: present ARGB pixel buffer (`win_id`, `buf_ptr`, `buf_bytes`) -> `0`.
pub const SYS_GUI_PRESENT: u64 = 311;
/// GUI compositor: poll next `GuiUserEvent` (`win_id`, `out_event_ptr`) -> `1 | 0`.
pub const SYS_GUI_POLL_EVENT: u64 = 312;
/// GUI compositor: close user window (`win_id`) -> `0`.
pub const SYS_GUI_CLOSE_WINDOW: u64 = 313;

/// Sets Thread Local Storage (TLS) register `%gs` base address.
pub const ARCH_SET_GS: u64 = 0x1001;
/// Sets Thread Local Storage (TLS) register `%fs` base address.
pub const ARCH_SET_FS: u64 = 0x1002;
/// Reads Thread Local Storage (TLS) register `%fs` base address.
pub const ARCH_GET_FS: u64 = 0x1003;
/// Reads Thread Local Storage (TLS) register `%gs` base address.
pub const ARCH_GET_GS: u64 = 0x1004;

/// Memory page protection: read permission.
pub const PROT_READ: u64 = 1;
/// Memory page protection: write permission.
pub const PROT_WRITE: u64 = 2;
/// Memory page protection: execute permission.
pub const PROT_EXEC: u64 = 4;

const USER_CODE_VADDR: u64 = 0x0040_0000; // 4 MiB
const USER_FAULT_CODE_VADDR: u64 = 0x2000_0000;
const USER_STACK_VADDR: u64 = 0x7000_0000;
const USER_STACK_TOP: u64 = USER_STACK_VADDR + PAGE_SIZE_4K - 16;

static USER_DEMO_PASSED: AtomicBool = AtomicBool::new(false);
static NEXT_USER_MMAP_VADDR: AtomicU64 = AtomicU64::new(0x0000_6000_0000);
static CURRENT_PID: AtomicU64 = AtomicU64::new(1);

/// Returns the currently active user ProcessId.
pub fn current_pid() -> super::process::ProcessId {
    super::process::ProcessId(CURRENT_PID.load(Ordering::SeqCst))
}

/// Sets the currently active user ProcessId.
pub fn set_current_pid(pid: u64) {
    CURRENT_PID.store(pid, Ordering::SeqCst);
    crate::perf::opt::syscall_opt::update_cached_pid(pid);
}

/// POSIX-compatible stat structure matching user/libc/include/sys/stat.h.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StatBuffer {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_mode: u32,
    pub st_nlink: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
}

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

/// Profiled Common Rust System Call Dispatcher invoked from `syscall_entry` in `syscall.S`.
#[unsafe(no_mangle)]
pub extern "C" fn syscall_dispatcher(
    num: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
) -> u64 {
    let t0 = crate::perf::tsc::read_tsc();
    crate::perf::pmu::record_sw_syscall();
    let ret = if num == SYS_GETPID {
        crate::perf::opt::syscall_opt::FAST_SYSCALL_TABLE
            .try_dispatch(num, a1, a2, a3)
            .unwrap_or_else(|| syscall_dispatcher_inner(num, a1, a2, a3, a4, a5))
    } else {
        syscall_dispatcher_inner(num, a1, a2, a3, a4, a5)
    };
    let elapsed = crate::perf::tsc::read_tsc().saturating_sub(t0);
    crate::perf::latency::SYSCALL_LATENCY.record(elapsed);
    crate::tracepoint!(
        crate::perf::trace::TraceCategory::Syscall,
        crate::perf::trace::event_id::SYSCALL_EXIT,
        0,
        num,
        elapsed
    );
    ret
}

fn syscall_dispatcher_inner(
    num: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
) -> u64 {
    match num {
        SYS_READ => {
            let fd = a1 as i32;
            let buf_ptr = a2 as *mut u8;
            let len = a3 as usize;

            if buf_ptr.is_null() || len == 0 {
                return 0;
            }

            if crate::security::validate_user_buffer(buf_ptr as *const u8, len).is_err() {
                return u64::MAX;
            }

            if fd == 0 {
                let mut read_bytes = 0;
                while read_bytes < len {
                    let ch = if let Some(c) = crate::drivers::keyboard::read_char() {
                        Some(c as u8)
                    } else if let Some(b) = crate::drivers::serial::read_byte() {
                        Some(b)
                    } else {
                        None
                    };

                    if let Some(b) = ch {
                        unsafe {
                            *buf_ptr.add(read_bytes) = b;
                        }
                        read_bytes += 1;
                        if b == b'\n' || b == b'\r' {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                return read_bytes as u64;
            }

            let cur_pid = current_pid();
            let proc_table = super::process::PROCESS_TABLE.lock();
            let proc = proc_table.get(cur_pid).or_else(|| proc_table.get(super::process::ProcessId::INIT));
            if let Some(proc) = proc {
                if let Ok(handle) = proc.fd_table.get(fd) {
                    let slice = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) };
                    match handle.read(slice) {
                        Ok(bytes) => bytes as u64,
                        Err(crate::fs::vfs::FsError::WouldBlock) => (-11i64) as u64,
                        Err(_) => u64::MAX,
                    }
                } else {
                    u64::MAX
                }
            } else {
                u64::MAX
            }
        }
        SYS_WRITE => {
            let fd = a1 as i32;
            let buf_ptr = a2 as *const u8;
            let len = a3 as usize;

            if buf_ptr.is_null() || len == 0 {
                return 0;
            }

            if crate::security::validate_user_buffer(buf_ptr, len).is_err() {
                return u64::MAX;
            }

            let slice = unsafe { core::slice::from_raw_parts(buf_ptr, len) };
            if fd == 1 || fd == 2 {
                if let Ok(s) = core::str::from_utf8(slice) {
                    print!("{}", s);
                } else {
                    for &b in slice {
                        unsafe {
                            crate::drivers::serial::write_byte(b);
                        }
                    }
                }
                len as u64
            } else {
                let cur_pid = current_pid();
                let proc_table = super::process::PROCESS_TABLE.lock();
                let proc = proc_table.get(cur_pid).or_else(|| proc_table.get(super::process::ProcessId::INIT));
                if let Some(proc) = proc {
                    if let Ok(handle) = proc.fd_table.get(fd) {
                        match handle.write(slice) {
                            Ok(bytes) => bytes as u64,
                            Err(crate::fs::vfs::FsError::WouldBlock) => (-11i64) as u64,
                            Err(_) => u64::MAX,
                        }
                    } else {
                        u64::MAX
                    }
                } else {
                    u64::MAX
                }
            }
        }
        SYS_OPEN => {
            let path_ptr = a1 as *const u8;
            let flags_bits = a2;
            let path = match crate::security::copy_str_from_user(path_ptr, 256) {
                Ok(p) => p,
                Err(_) => return u64::MAX,
            };

            let flags = crate::fs::vfs::OpenFlags::from_bits(flags_bits);
            match crate::fs::vfs::VFS.open(&path, flags) {
                Ok(inode) => {
                    let desc = alloc::sync::Arc::new(crate::fs::fd::FileDescriptor::new(inode, flags));
                    let cur_pid = current_pid();
                    let mut proc_table = super::process::PROCESS_TABLE.lock();
                    let proc = if proc_table.get_mut(cur_pid).is_some() {
                        proc_table.get_mut(cur_pid)
                    } else {
                        proc_table.get_mut(super::process::ProcessId::INIT)
                    };
                    if let Some(proc) = proc {
                        match proc.fd_table.alloc_fd(desc) {
                            Ok(fd) => fd as u64,
                            Err(_) => u64::MAX,
                        }
                    } else {
                        u64::MAX
                    }
                }
                Err(_) => u64::MAX,
            }
        }
        SYS_CLOSE => {
            let fd = a1 as i32;
            let cur_pid = current_pid();
            let mut proc_table = super::process::PROCESS_TABLE.lock();
            let proc = if proc_table.get_mut(cur_pid).is_some() {
                proc_table.get_mut(cur_pid)
            } else {
                proc_table.get_mut(super::process::ProcessId::INIT)
            };
            if let Some(proc) = proc {
                match proc.fd_table.close(fd) {
                    Ok(_) => 0,
                    Err(_) => u64::MAX,
                }
            } else {
                u64::MAX
            }
        }
        SYS_LSEEK => {
            let fd = a1 as i32;
            let offset = a2 as i64;
            let whence = a3 as i32;
            let cur_pid = current_pid();
            let proc_table = super::process::PROCESS_TABLE.lock();
            let proc = proc_table.get(cur_pid).or_else(|| proc_table.get(super::process::ProcessId::INIT));
            if let Some(proc) = proc {
                if let Ok(handle) = proc.fd_table.get(fd) {
                    match handle.seek(offset, whence) {
                        Ok(new_off) => new_off as u64,
                        Err(_) => u64::MAX,
                    }
                } else {
                    u64::MAX
                }
            } else {
                u64::MAX
            }
        }
        SYS_STAT => {
            let path_ptr = a1 as *const u8;
            let stat_ptr = a2 as *mut StatBuffer;
            if stat_ptr.is_null() || crate::security::validate_user_buffer(stat_ptr as *const u8, core::mem::size_of::<StatBuffer>()).is_err() {
                return u64::MAX;
            }

            let path = match crate::security::copy_str_from_user(path_ptr, 256) {
                Ok(p) => p,
                Err(_) => return u64::MAX,
            };

            match crate::fs::vfs::VFS.resolve_path(&path) {
                Ok(inode) => {
                    let st = inode.stat();
                    unsafe {
                        (*stat_ptr).st_dev = 0;
                        (*stat_ptr).st_ino = st.inode_num;
                        (*stat_ptr).st_mode = st.mode;
                        (*stat_ptr).st_nlink = 1;
                        (*stat_ptr).st_uid = 0;
                        (*stat_ptr).st_gid = 0;
                        (*stat_ptr).st_rdev = 0;
                        (*stat_ptr).st_size = st.size as i64;
                        (*stat_ptr).st_blksize = 512;
                        (*stat_ptr).st_blocks = ((st.size + 511) / 512) as i64;
                    }
                    0
                }
                Err(_) => u64::MAX,
            }
        }
        SYS_EXIT => {
            let status = a1 as i32;
            let cur_pid = current_pid();
            println!("\x1b[32m[USER]\x1b[0m User process #{} finished with exit code: {}", cur_pid.0, status);
            super::process::exit_process(cur_pid, status);
            USER_DEMO_PASSED.store(true, Ordering::SeqCst);
            super::exit();
        }
        SYS_YIELD => {
            super::yield_now();
            0
        }
        SYS_GETPID => {
            current_pid().as_u64()
        }
        SYS_SLEEP => {
            let ticks = a1;
            super::sleep(ticks);
            0
        }
        SYS_WAITPID => {
            let target_pid = if a1 == 0 || a1 == u64::MAX {
                None
            } else {
                Some(super::process::ProcessId(a1))
            };
            let parent_pid = super::process::ProcessId::INIT;
            match super::process::waitpid(parent_pid, target_pid) {
                Ok((reaped_pid, exit_code)) => {
                    let status_ptr = a2 as *mut i32;
                    if !status_ptr.is_null() {
                        if crate::security::validate_user_buffer(status_ptr as *const u8, core::mem::size_of::<i32>()).is_ok() {
                            unsafe {
                                *status_ptr = exit_code;
                            }
                        }
                    }
                    reaped_pid.as_u64()
                }
                Err(_) => u64::MAX,
            }
        }
        SYS_MMAP => {
            let addr = a1;
            let len = a2 as usize;
            let prot = a3;
            let flags = a4;
            sys_mmap(addr, len, prot, flags)
        }
        SYS_PIPE => {
            let pipefd_ptr = a1 as *mut i32;
            if pipefd_ptr.is_null() || crate::security::validate_user_buffer(pipefd_ptr as *const u8, 2 * core::mem::size_of::<i32>()).is_err() {
                return u64::MAX;
            }

            let (read_fd, write_fd) = crate::ipc::create_pipe();
            let cur_pid = current_pid();
            let mut proc_table = super::process::PROCESS_TABLE.lock();
            let proc = if proc_table.get_mut(cur_pid).is_some() {
                proc_table.get_mut(cur_pid)
            } else {
                proc_table.get_mut(super::process::ProcessId::INIT)
            };

            if let Some(proc) = proc {
                let fd0 = match proc.fd_table.alloc_fd(read_fd) {
                    Ok(f) => f,
                    Err(_) => return u64::MAX,
                };
                let fd1 = match proc.fd_table.alloc_fd(write_fd) {
                    Ok(f) => f,
                    Err(_) => {
                        let _ = proc.fd_table.close(fd0);
                        return u64::MAX;
                    }
                };

                unsafe {
                    *pipefd_ptr = fd0;
                    *pipefd_ptr.add(1) = fd1;
                }
                0
            } else {
                u64::MAX
            }
        }
        SYS_KILL => {
            let pid = super::process::ProcessId(a1);
            let sig = a2 as i32;
            if let Some(signal) = crate::ipc::Signal::from_i32(sig) {
                match crate::ipc::send_signal(pid, signal) {
                    Ok(_) => 0,
                    Err(_) => u64::MAX,
                }
            } else {
                u64::MAX
            }
        }
        SYS_CLONE => {
            let entry_point = a1;
            let user_stack_top = a2;
            let arg = a3;
            let tls_base = a4;

            if entry_point >= crate::security::USER_SPACE_CEILING
                || user_stack_top >= crate::security::USER_SPACE_CEILING
            {
                return u64::MAX;
            }
            if tls_base != 0 && tls_base >= crate::security::USER_SPACE_CEILING {
                return u64::MAX;
            }

            let cur_pid = current_pid();
            let tid = crate::task::spawn_user(
                "user_thread",
                entry_point,
                user_stack_top,
                arg,
                tls_base,
                cur_pid,
                1,
            );
            {
                let mut proc_table = super::process::PROCESS_TABLE.lock();
                if let Some(proc) = proc_table.get_mut(cur_pid) {
                    proc.threads.push(tid);
                }
            }
            tid.0
        }
        SYS_THREAD_EXIT => {
            let status = a1 as i32;
            let tid = crate::task::current_thread_id();
            println!(
                "\x1b[32m[USER THREAD]\x1b[0m Thread {:?} exited with code: {}",
                tid, status
            );
            super::exit();
        }
        SYS_THREAD_JOIN => {
            let target_tid = a1;
            let status_ptr = a2 as *mut i32;
            loop {
                let thread_exists = {
                    let list = crate::task::thread_list();
                    list.iter().any(|t| t.id.0 == target_tid)
                };
                if !thread_exists {
                    if !status_ptr.is_null()
                        && crate::security::validate_user_buffer(status_ptr as *const u8, 4).is_ok()
                    {
                        unsafe {
                            *status_ptr = 0;
                        }
                    }
                    return 0;
                }
                super::yield_now();
            }
        }
        SYS_ARCH_PRCTL => {
            let code = a1;
            let addr = a2;
            match code {
                ARCH_SET_FS => {
                    if addr >= crate::security::USER_SPACE_CEILING {
                        return u64::MAX;
                    }
                    unsafe {
                        crate::arch::x86_64::syscall::write_msr(
                            crate::arch::x86_64::syscall::IA32_FS_BASE_MSR,
                            addr,
                        );
                    }
                    0
                }
                ARCH_GET_FS => {
                    let ptr = addr as *mut u64;
                    if ptr.is_null()
                        || crate::security::validate_user_buffer(ptr as *const u8, 8).is_err()
                    {
                        return u64::MAX;
                    }
                    unsafe {
                        let fs = crate::arch::x86_64::syscall::read_msr(
                            crate::arch::x86_64::syscall::IA32_FS_BASE_MSR,
                        );
                        *ptr = fs;
                    }
                    0
                }
                _ => u64::MAX,
            }
        }
        SYS_GETTID => {
            crate::task::current_thread_id().map(|t| t.0).unwrap_or(0)
        }
        SYS_FUTEX => {
            let uaddr = a1;
            let op = a2 as u32;
            let val = a3 as u32;
            let uaddr2 = a4;
            let val3 = a5 as u32;

            let op_type = op & 0x7F;
            match op_type {
                crate::sync::FUTEX_WAIT => match crate::sync::futex_wait(uaddr, val) {
                    Ok(_) => 0,
                    Err("EAGAIN") => (-11i64) as u64,
                    Err(_) => (-14i64) as u64,
                },
                crate::sync::FUTEX_WAKE => match crate::sync::futex_wake(uaddr, val) {
                    Ok(woken) => woken as u64,
                    Err(_) => (-14i64) as u64,
                },
                crate::sync::FUTEX_REQUEUE => {
                    match crate::sync::futex_requeue(uaddr, val, uaddr2, val3) {
                        Ok(woken) => woken as u64,
                        Err(_) => (-14i64) as u64,
                    }
                }
                _ => u64::MAX,
            }
        }
        // SYS_POLL (7): Multiplexed I/O event polling across an array of file descriptors.
        // a1 = struct pollfd *fds
        // a2 = nfds_t nfds
        // a3 = int timeout_ms
        SYS_POLL => {
            let fds_ptr = a1 as *mut crate::task::event::PollFd;
            let nfds = a2 as usize;
            let timeout_ms = a3 as i32;

            if fds_ptr.is_null() && nfds > 0 {
                return (-14i64) as u64;
            }
            if nfds > 1024 {
                return (-22i64) as u64;
            }
            if nfds > 0 && crate::security::validate_user_buffer(fds_ptr as *const u8, nfds * core::mem::size_of::<crate::task::event::PollFd>()).is_err() {
                return (-14i64) as u64;
            }

            let start_ticks = crate::arch::x86_64::apic::get_ticks();
            let timeout_ticks = if timeout_ms < 0 {
                u64::MAX
            } else {
                (timeout_ms as u64) / 10
            };

            let cur_pid = current_pid();
            let poll_slice = if nfds > 0 {
                unsafe { core::slice::from_raw_parts_mut(fds_ptr, nfds) }
            } else {
                &mut []
            };

            loop {
                let mut ready_count = 0usize;
                {
                    let proc_table = super::process::PROCESS_TABLE.lock();
                    let proc = proc_table.get(cur_pid).or_else(|| proc_table.get(super::process::ProcessId::INIT));
                    if let Some(proc) = proc {
                        for pfd in poll_slice.iter_mut() {
                            pfd.revents = 0;
                            if pfd.fd < 0 {
                                continue;
                            }
                            if let Ok(handle) = proc.fd_table.get(pfd.fd) {
                                let mask = handle.inode.poll();
                                let matching = (mask as i16) & (pfd.events | (crate::fs::vfs::POLLERR | crate::fs::vfs::POLLHUP) as i16);
                                if matching != 0 {
                                    pfd.revents = matching;
                                    ready_count += 1;
                                }
                            } else {
                                pfd.revents = crate::fs::vfs::POLLNVAL as i16;
                                ready_count += 1;
                            }
                        }
                    }
                }

                if ready_count > 0 || timeout_ticks == 0 {
                    return ready_count as u64;
                }

                let elapsed = crate::arch::x86_64::apic::get_ticks().saturating_sub(start_ticks);
                if timeout_ticks != u64::MAX && elapsed >= timeout_ticks {
                    return 0;
                }

                super::yield_now();
            }
        }
        // SYS_READV (19): Scatter-gather vectored read from a file descriptor into multiple buffers.
        // a1 = int fd
        // a2 = const struct iovec *iov
        // a3 = int iovcnt
        SYS_READV => {
            let fd = a1 as i32;
            let iov_ptr = a2 as *const crate::task::event::IoVec;
            let iovcnt = a3 as usize;

            if iov_ptr.is_null() && iovcnt > 0 {
                return (-14i64) as u64;
            }
            if iovcnt > 1024 {
                return (-22i64) as u64;
            }
            if iovcnt > 0 && crate::security::validate_user_buffer(iov_ptr as *const u8, iovcnt * core::mem::size_of::<crate::task::event::IoVec>()).is_err() {
                return (-14i64) as u64;
            }

            let iovs = unsafe { core::slice::from_raw_parts(iov_ptr, iovcnt) };
            let cur_pid = current_pid();
            let proc_table = super::process::PROCESS_TABLE.lock();
            let proc = proc_table.get(cur_pid).or_else(|| proc_table.get(super::process::ProcessId::INIT));
            if let Some(proc) = proc {
                if let Ok(handle) = proc.fd_table.get(fd) {
                    let mut total_read = 0usize;
                    for iov in iovs {
                        if iov.iov_len == 0 {
                            continue;
                        }
                        let buf_ptr = iov.iov_base as *mut u8;
                        if crate::security::validate_user_buffer(buf_ptr, iov.iov_len).is_err() {
                            if total_read > 0 { return total_read as u64; }
                            return (-14i64) as u64;
                        }
                        let slice = unsafe { core::slice::from_raw_parts_mut(buf_ptr, iov.iov_len) };
                        match handle.read(slice) {
                            Ok(0) => break,
                            Ok(n) => {
                                total_read += n;
                                if n < iov.iov_len {
                                    break;
                                }
                            }
                            Err(crate::fs::vfs::FsError::WouldBlock) => {
                                if total_read > 0 { return total_read as u64; }
                                return (-11i64) as u64;
                            }
                            Err(_) => {
                                if total_read > 0 { return total_read as u64; }
                                return u64::MAX;
                            }
                        }
                    }
                    total_read as u64
                } else {
                    (-9i64) as u64
                }
            } else {
                u64::MAX
            }
        }
        // SYS_WRITEV (20): Scatter-gather vectored write from multiple buffers to a file descriptor.
        // a1 = int fd
        // a2 = const struct iovec *iov
        // a3 = int iovcnt
        SYS_WRITEV => {
            let fd = a1 as i32;
            let iov_ptr = a2 as *const crate::task::event::IoVec;
            let iovcnt = a3 as usize;

            if iov_ptr.is_null() && iovcnt > 0 {
                return (-14i64) as u64;
            }
            if iovcnt > 1024 {
                return (-22i64) as u64;
            }
            if iovcnt > 0 && crate::security::validate_user_buffer(iov_ptr as *const u8, iovcnt * core::mem::size_of::<crate::task::event::IoVec>()).is_err() {
                return (-14i64) as u64;
            }

            let iovs = unsafe { core::slice::from_raw_parts(iov_ptr, iovcnt) };
            if fd == 1 || fd == 2 {
                let mut total_written = 0usize;
                for iov in iovs {
                    if iov.iov_len == 0 {
                        continue;
                    }
                    let buf_ptr = iov.iov_base as *const u8;
                    if crate::security::validate_user_buffer(buf_ptr, iov.iov_len).is_err() {
                        if total_written > 0 { return total_written as u64; }
                        return (-14i64) as u64;
                    }
                    let slice = unsafe { core::slice::from_raw_parts(buf_ptr, iov.iov_len) };
                    if let Ok(s) = core::str::from_utf8(slice) {
                        print!("{}", s);
                    } else {
                        for &b in slice {
                            unsafe {
                                crate::drivers::serial::write_byte(b);
                            }
                        }
                    }
                    total_written += iov.iov_len;
                }
                total_written as u64
            } else {
                let cur_pid = current_pid();
                let proc_table = super::process::PROCESS_TABLE.lock();
                let proc = proc_table.get(cur_pid).or_else(|| proc_table.get(super::process::ProcessId::INIT));
                if let Some(proc) = proc {
                    if let Ok(handle) = proc.fd_table.get(fd) {
                        let mut total_written = 0usize;
                        for iov in iovs {
                            if iov.iov_len == 0 {
                                continue;
                            }
                            let buf_ptr = iov.iov_base as *const u8;
                            if crate::security::validate_user_buffer(buf_ptr, iov.iov_len).is_err() {
                                if total_written > 0 { return total_written as u64; }
                                return (-14i64) as u64;
                            }
                            let slice = unsafe { core::slice::from_raw_parts(buf_ptr, iov.iov_len) };
                            match handle.write(slice) {
                                Ok(n) => {
                                    total_written += n;
                                    if n < iov.iov_len {
                                        break;
                                    }
                                }
                                Err(crate::fs::vfs::FsError::WouldBlock) => {
                                    if total_written > 0 { return total_written as u64; }
                                    return (-11i64) as u64;
                                }
                                Err(_) => {
                                    if total_written > 0 { return total_written as u64; }
                                    return u64::MAX;
                                }
                            }
                        }
                        total_written as u64
                    } else {
                        (-9i64) as u64
                    }
                } else {
                    u64::MAX
                }
            }
        }
        // SYS_EPOLL_CREATE (213): Creates a new epoll file descriptor.
        // a1 = int size (ignored, must be > 0 for POSIX compatibility)
        SYS_EPOLL_CREATE => {
            let size = a1 as i32;
            if size <= 0 {
                return (-22i64) as u64;
            }
            let epoll = alloc::sync::Arc::new(crate::task::event::EpollInstance::new());
            let flags = crate::fs::vfs::OpenFlags {
                read: true,
                write: true,
                create: false,
                append: false,
                truncate: false,
                nonblock: false,
            };
            let desc = alloc::sync::Arc::new(crate::fs::fd::FileDescriptor::new(epoll, flags));
            let cur_pid = current_pid();
            let mut proc_table = super::process::PROCESS_TABLE.lock();
            let proc = if proc_table.get_mut(cur_pid).is_some() {
                proc_table.get_mut(cur_pid).unwrap()
            } else if proc_table.get_mut(super::process::ProcessId::INIT).is_some() {
                proc_table.get_mut(super::process::ProcessId::INIT).unwrap()
            } else {
                return (-24i64) as u64;
            };

            match proc.fd_table.alloc_fd(desc) {
                Ok(fd) => fd as u64,
                Err(_) => (-24i64) as u64,
            }
        }
        // SYS_EPOLL_CREATE1 (291): Creates an epoll file descriptor with flags (e.g. EPOLL_CLOEXEC).
        // a1 = int flags
        SYS_EPOLL_CREATE1 => {
            let epoll_flags = a1 as i32;
            let epoll = alloc::sync::Arc::new(crate::task::event::EpollInstance::new());
            let flags = crate::fs::vfs::OpenFlags {
                read: true,
                write: true,
                create: false,
                append: false,
                truncate: false,
                nonblock: (epoll_flags & 0o4000) != 0,
            };
            let desc = alloc::sync::Arc::new(crate::fs::fd::FileDescriptor::new(epoll, flags));
            let cur_pid = current_pid();
            let mut proc_table = super::process::PROCESS_TABLE.lock();
            let proc = if proc_table.get_mut(cur_pid).is_some() {
                proc_table.get_mut(cur_pid).unwrap()
            } else if proc_table.get_mut(super::process::ProcessId::INIT).is_some() {
                proc_table.get_mut(super::process::ProcessId::INIT).unwrap()
            } else {
                return (-24i64) as u64;
            };

            match proc.fd_table.alloc_fd(desc) {
                Ok(fd) => fd as u64,
                Err(_) => (-24i64) as u64,
            }
        }
        // SYS_EPOLL_CTL (233): Control interface for an epoll file descriptor (ADD, DEL, MOD).
        // a1 = int epfd
        // a2 = int op
        // a3 = int fd
        // a4 = struct epoll_event *event
        SYS_EPOLL_CTL => {
            let epfd = a1 as i32;
            let op = a2 as i32;
            let fd = a3 as i32;
            let event_ptr = a4 as *const crate::task::event::EpollEvent;

            let cur_pid = current_pid();
            let proc_table = super::process::PROCESS_TABLE.lock();
            let proc = proc_table.get(cur_pid).or_else(|| proc_table.get(super::process::ProcessId::INIT));
            if let Some(proc) = proc {
                if let Ok(handle) = proc.fd_table.get(epfd) {
                    if let Some(epoll) = handle.inode.as_any().and_then(|a| a.downcast_ref::<crate::task::event::EpollInstance>()) {
                        if proc.fd_table.get(fd).is_err() {
                            return (-9i64) as u64;
                        }

                        let event = if op != crate::task::event::EPOLL_CTL_DEL {
                            if event_ptr.is_null() || crate::security::validate_user_buffer(event_ptr as *const u8, core::mem::size_of::<crate::task::event::EpollEvent>()).is_err() {
                                return (-14i64) as u64;
                            }
                            unsafe { *event_ptr }
                        } else {
                            crate::task::event::EpollEvent::default()
                        };

                        match epoll.ctl(op, fd, event) {
                            Ok(()) => 0,
                            Err(crate::fs::vfs::FsError::AlreadyExists) => (-17i64) as u64,
                            Err(crate::fs::vfs::FsError::NotFound) => (-2i64) as u64,
                            Err(crate::fs::vfs::FsError::InvalidInput) => (-22i64) as u64,
                            Err(_) => (-22i64) as u64,
                        }
                    } else {
                        (-9i64) as u64
                    }
                } else {
                    (-9i64) as u64
                }
            } else {
                (-14i64) as u64
            }
        }
        // SYS_EPOLL_WAIT (232): Waits for I/O events on an epoll file descriptor with timeout.
        // a1 = int epfd
        // a2 = struct epoll_event *events
        // a3 = int maxevents
        // a4 = int timeout
        SYS_EPOLL_WAIT => {
            let epfd = a1 as i32;
            let events_ptr = a2 as *mut crate::task::event::EpollEvent;
            let maxevents = a3 as i32;
            let timeout = a4 as i32;

            if maxevents <= 0 || maxevents > 1024 {
                return (-22i64) as u64;
            }
            if events_ptr.is_null() || crate::security::validate_user_buffer(events_ptr as *const u8, (maxevents as usize) * core::mem::size_of::<crate::task::event::EpollEvent>()).is_err() {
                return (-14i64) as u64;
            }

            let start_ticks = crate::arch::x86_64::apic::get_ticks();
            let timeout_ticks = if timeout < 0 {
                u64::MAX
            } else {
                (timeout as u64) / 10
            };

            let cur_pid = current_pid();
            loop {
                let ready_events = {
                    let proc_table = super::process::PROCESS_TABLE.lock();
                    let proc = proc_table.get(cur_pid).or_else(|| proc_table.get(super::process::ProcessId::INIT));
                    if let Some(proc) = proc {
                        if let Ok(handle) = proc.fd_table.get(epfd) {
                            if let Some(epoll) = handle.inode.as_any().and_then(|a| a.downcast_ref::<crate::task::event::EpollInstance>()) {
                                epoll.poll_events(&proc.fd_table, maxevents as usize)
                            } else {
                                return (-9i64) as u64;
                            }
                        } else {
                            return (-9i64) as u64;
                        }
                    } else {
                        return (-14i64) as u64;
                    }
                };

                if !ready_events.is_empty() || timeout_ticks == 0 {
                    let count = ready_events.len();
                    let out_slice = unsafe { core::slice::from_raw_parts_mut(events_ptr, count) };
                    out_slice.copy_from_slice(&ready_events);
                    return count as u64;
                }

                let elapsed = crate::arch::x86_64::apic::get_ticks().saturating_sub(start_ticks);
                if timeout_ticks != u64::MAX && elapsed >= timeout_ticks {
                    return 0;
                }

                super::yield_now();
            }
        }
        // SYS_EVENTFD (284) / SYS_EVENTFD2 (290): Creates a 64-bit event notification descriptor.
        // a1 = unsigned int initval
        // a2 = int flags (EFD_NONBLOCK, EFD_SEMAPHORE, EFD_CLOEXEC)
        SYS_EVENTFD | SYS_EVENTFD2 => {
            let initval = a1 as u32;
            let flags = if num == SYS_EVENTFD2 { a2 as i32 } else { 0 };

            let evfd = alloc::sync::Arc::new(crate::task::event::EventFd::new(initval, flags));
            let desc_flags = crate::fs::vfs::OpenFlags {
                read: true,
                write: true,
                create: false,
                append: false,
                truncate: false,
                nonblock: (flags & crate::task::event::EFD_NONBLOCK) != 0,
            };
            let desc = alloc::sync::Arc::new(crate::fs::fd::FileDescriptor::new(evfd, desc_flags));
            let cur_pid = current_pid();
            let mut proc_table = super::process::PROCESS_TABLE.lock();
            let proc = if proc_table.get_mut(cur_pid).is_some() {
                proc_table.get_mut(cur_pid).unwrap()
            } else if proc_table.get_mut(super::process::ProcessId::INIT).is_some() {
                proc_table.get_mut(super::process::ProcessId::INIT).unwrap()
            } else {
                return (-24i64) as u64;
            };

            match proc.fd_table.alloc_fd(desc) {
                Ok(fd) => fd as u64,
                Err(_) => (-24i64) as u64,
            }
        }
        // SYS_TIMERFD_CREATE (283): Creates a high-resolution timer descriptor.
        // a1 = int clockid (CLOCK_REALTIME, CLOCK_MONOTONIC)
        // a2 = int flags (TFD_NONBLOCK, TFD_CLOEXEC)
        SYS_TIMERFD_CREATE => {
            let clockid = a1 as i32;
            let flags = a2 as i32;

            if clockid != crate::task::event::CLOCK_REALTIME && clockid != crate::task::event::CLOCK_MONOTONIC {
                return (-22i64) as u64;
            }

            let tfd = alloc::sync::Arc::new(crate::task::event::TimerFd::new(clockid, flags));
            let desc_flags = crate::fs::vfs::OpenFlags {
                read: true,
                write: false,
                create: false,
                append: false,
                truncate: false,
                nonblock: (flags & crate::task::event::TFD_NONBLOCK) != 0,
            };
            let desc = alloc::sync::Arc::new(crate::fs::fd::FileDescriptor::new(tfd, desc_flags));
            let cur_pid = current_pid();
            let mut proc_table = super::process::PROCESS_TABLE.lock();
            let proc = if proc_table.get_mut(cur_pid).is_some() {
                proc_table.get_mut(cur_pid).unwrap()
            } else if proc_table.get_mut(super::process::ProcessId::INIT).is_some() {
                proc_table.get_mut(super::process::ProcessId::INIT).unwrap()
            } else {
                return (-24i64) as u64;
            };

            match proc.fd_table.alloc_fd(desc) {
                Ok(fd) => fd as u64,
                Err(_) => (-24i64) as u64,
            }
        }
        // SYS_TIMERFD_SETTIME (286): Arms or disarms the timer referred to by timerfd.
        // a1 = int fd
        // a2 = int flags (TFD_TIMER_ABSTIME)
        // a3 = const struct itimerspec *new_value
        // a4 = struct itimerspec *old_value
        SYS_TIMERFD_SETTIME => {
            let fd = a1 as i32;
            let _flags = a2 as i32;
            let new_value_ptr = a3 as *const crate::task::event::ITimerSpec;
            let old_value_ptr = a4 as *mut crate::task::event::ITimerSpec;

            if new_value_ptr.is_null() || crate::security::validate_user_buffer(new_value_ptr as *const u8, core::mem::size_of::<crate::task::event::ITimerSpec>()).is_err() {
                return (-14i64) as u64;
            }

            let new_val = unsafe { *new_value_ptr };

            let cur_pid = current_pid();
            let proc_table = super::process::PROCESS_TABLE.lock();
            let proc = proc_table.get(cur_pid).or_else(|| proc_table.get(super::process::ProcessId::INIT));
            if let Some(proc) = proc {
                if let Ok(handle) = proc.fd_table.get(fd) {
                    if let Some(timer) = handle.inode.as_any().and_then(|a| a.downcast_ref::<crate::task::event::TimerFd>()) {
                        let old_val = timer.set_time(new_val);
                        if !old_value_ptr.is_null() {
                            if crate::security::validate_user_buffer(old_value_ptr as *const u8, core::mem::size_of::<crate::task::event::ITimerSpec>()).is_err() {
                                return (-14i64) as u64;
                            }
                            unsafe { *old_value_ptr = old_val; }
                        }
                        0
                    } else {
                        (-9i64) as u64
                    }
                } else {
                    (-9i64) as u64
                }
            } else {
                (-14i64) as u64
            }
        }
        // SYS_TIMERFD_GETTIME (287): Queries current setting/remaining time of timerfd.
        // a1 = int fd
        // a2 = struct itimerspec *curr_value
        SYS_TIMERFD_GETTIME => {
            let fd = a1 as i32;
            let curr_value_ptr = a2 as *mut crate::task::event::ITimerSpec;

            if curr_value_ptr.is_null() || crate::security::validate_user_buffer(curr_value_ptr as *const u8, core::mem::size_of::<crate::task::event::ITimerSpec>()).is_err() {
                return (-14i64) as u64;
            }

            let cur_pid = current_pid();
            let proc_table = super::process::PROCESS_TABLE.lock();
            let proc = proc_table.get(cur_pid).or_else(|| proc_table.get(super::process::ProcessId::INIT));
            if let Some(proc) = proc {
                if let Ok(handle) = proc.fd_table.get(fd) {
                    if let Some(_timer) = handle.inode.as_any().and_then(|a| a.downcast_ref::<crate::task::event::TimerFd>()) {
                        unsafe { *curr_value_ptr = crate::task::event::ITimerSpec::default(); }
                        0
                    } else {
                        (-9i64) as u64
                    }
                } else {
                    (-9i64) as u64
                }
            } else {
                (-14i64) as u64
            }
        }
        // SYS_DLOPEN (300): Dynamically loads a shared object library (M34).
        // a1 = const char *path
        // a2 = int flags
        SYS_DLOPEN => {
            let path_ptr = a1 as *const u8;
            let path = match crate::security::copy_str_from_user(path_ptr, 256) {
                Ok(p) => p,
                Err(_) => return 0,
            };

            match crate::task::dynlink::dlopen(&path) {
                Ok(handle) => handle,
                Err(e) => {
                    println!("[SYSCALL] dlopen(\"{}\") failed: {}", path, e);
                    0
                }
            }
        }
        // SYS_DLSYM (301): Resolves symbol address from loaded shared object (M34).
        // a1 = void *handle
        // a2 = const char *symbol
        SYS_DLSYM => {
            let handle = a1;
            let sym_ptr = a2 as *const u8;
            let symbol = match crate::security::copy_str_from_user(sym_ptr, 128) {
                Ok(s) => s,
                Err(_) => return 0,
            };

            match crate::task::dynlink::dlsym(handle, &symbol) {
                Some(addr) => addr,
                None => 0,
            }
        }
        // SYS_DLCLOSE (302): Unloads a dynamically loaded library (M34).
        // a1 = void *handle
        SYS_DLCLOSE => {
            let handle = a1;
            match crate::task::dynlink::dlclose(handle) {
                Ok(()) => 0,
                Err(_) => (-1i64) as u64,
            }
        }
        // SYS_GUI_CREATE_WINDOW (310): (title_ptr, title_len, width, height) -> win_id
        SYS_GUI_CREATE_WINDOW => {
            let title_ptr = a1 as *const u8;
            let title_len = (a2 as usize).min(64);
            let width = a3 as usize;
            let height = a4 as usize;
            if title_ptr.is_null() || title_len == 0 {
                return (-22i64) as u64;
            }
            let mut title_buf = [0u8; 64];
            if crate::security::copy_from_user(&mut title_buf[..title_len], title_ptr).is_err() {
                return (-14i64) as u64;
            }
            let title = core::str::from_utf8(&title_buf[..title_len]).unwrap_or("App");
            let pid = current_pid().0;
            match crate::graphics::user_create_window(pid, title, width, height) {
                Some(id) => id as u64,
                None => (-12i64) as u64,
            }
        }
        // SYS_GUI_PRESENT (311): (win_id, buf_ptr, buf_bytes) -> 0
        SYS_GUI_PRESENT => {
            let win_id = a1 as u32;
            let buf_ptr = a2 as *const u8;
            let buf_bytes = a3 as usize;
            if buf_ptr.is_null() || buf_bytes == 0 || (buf_bytes % 4) != 0 || buf_bytes > 16 * 1024 * 1024 {
                return (-22i64) as u64;
            }
            let mut kbuf = alloc::vec![0u8; buf_bytes];
            if crate::security::copy_from_user(&mut kbuf, buf_ptr).is_err() {
                return (-14i64) as u64;
            }
            let pixel_count = buf_bytes / 4;
            let pixels: &[u32] = unsafe { core::slice::from_raw_parts(kbuf.as_ptr() as *const u32, pixel_count) };
            let pid = current_pid().0;
            if crate::graphics::user_present_window(pid, win_id, pixels) {
                0
            } else {
                (-22i64) as u64
            }
        }
        // SYS_GUI_POLL_EVENT (312): (win_id, out_event_ptr) -> 1 (event) | 0 (empty)
        SYS_GUI_POLL_EVENT => {
            let win_id = a1 as u32;
            let out_ptr = a2 as *mut u8;
            if out_ptr.is_null() {
                return (-22i64) as u64;
            }
            let pid = current_pid().0;
            if let Some(ev) = crate::graphics::user_poll_event(pid, win_id) {
                let bytes: &[u8] = unsafe {
                    core::slice::from_raw_parts(
                        (&ev as *const crate::graphics::event::GuiUserEvent) as *const u8,
                        core::mem::size_of::<crate::graphics::event::GuiUserEvent>(),
                    )
                };
                if crate::security::copy_to_user(out_ptr, bytes).is_err() {
                    return (-14i64) as u64;
                }
                1
            } else {
                0
            }
        }
        // SYS_GUI_CLOSE_WINDOW (313): (win_id) -> 0
        SYS_GUI_CLOSE_WINDOW => {
            let win_id = a1 as u32;
            let pid = current_pid().0;
            if crate::graphics::user_close_window(pid, win_id) {
                0
            } else {
                (-22i64) as u64
            }
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

    let _ = crate::memory::paging::unmap_page(fault_page);

    println!("[USER] User Mode Crash Handling verified: Faulting user thread was terminated cleanly while kernel remained intact!");

    // 6. Process Management & Lifecycle Verification (M14)
    println!();
    println!("----------------------------------------");
    println!(" [PROCESS] Testing Process Lifecycle    ");
    println!("----------------------------------------");

    // Spawn init process (PID 1)
    let init_pid = super::process::spawn_init_process(&test_elf_bytes).expect("Failed to spawn init process");
    assert_eq!(init_pid, super::process::ProcessId::INIT);

    // Create child process (PID 2) with parent PID 1
    let child_pid = super::process::create_process("child_worker", Some(init_pid)).expect("Failed to create child process");
    assert_eq!(child_pid.0, 2);
    assert_eq!(super::process::process_count(), 2);

    // Simulate child process executing, finishing, and transitioning to Zombie
    super::process::exit_process(child_pid, 42);

    // Parent process reaps the zombie child via waitpid
    let (reaped_pid, exit_status) = super::process::waitpid(init_pid, Some(child_pid)).expect("Failed to reap child process");
    assert_eq!(reaped_pid, child_pid);
    assert_eq!(exit_status, 42);

    // Verify child was completely removed from the process table (only init remains)
    assert_eq!(super::process::process_count(), 1);
    println!("[PROCESS] Process lifecycle, PID allocation, parent/child relationships, and waitpid verified successfully!");
    println!("[PROCESS] All Phase 5 milestones (M12 User Mode, M13 Syscalls & M14 Process Management) 100% complete!");
}
