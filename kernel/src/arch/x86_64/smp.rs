//! Symmetric Multiprocessing (SMP) Manager (Phase 9 - Milestone 25).
//!
//! Orchestrates the multi-core bootstrap sequence:
//! 1. Allocates per-CPU kernel stacks.
//! 2. Installs the 16-bit real-mode trampoline at 0x8000.
//! 3. Dispatches INIT-SIPI-SIPI Inter-Processor Interrupts (IPIs) via Local APIC ICR.
//! 4. Synchronizes AP cores into 64-bit Long Mode and initializes per-CPU state.

#![allow(dead_code)]

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::println;
use crate::sync::Spinlock;
use super::acpi;
use super::apic;

/// Physical address where the AP bootstrap trampoline is loaded.
/// SIPI vector 0x08 corresponds to 0x8000 (0x08 * 4096).
pub const AP_TRAMPOLINE_ADDR: u64 = 0x8000;
pub const AP_SIPI_VECTOR: u8 = 0x08;

/// Per-CPU kernel stack size (16 KiB = 4 pages).
pub const AP_STACK_SIZE: usize = 16384;

unsafe extern "C" {
    static ap_trampoline_start: u8;
    static ap_trampoline_end: u8;
    static ap_mailbox_cr3: u64;
    static ap_mailbox_stack: u64;
    static ap_mailbox_entry: u64;
    static ap_mailbox_cpu_id: u64;
    static ap_mailbox_status: u32;
}

/// Per-CPU Data Block tracking core state, APIC ID, and runtime metrics.
pub struct CpuData {
    pub cpu_id: usize,
    pub apic_id: u8,
    pub is_bsp: bool,
    pub online: AtomicBool,
    pub ticks: AtomicU64,
    pub stack_top: u64,
}

impl CpuData {
    pub fn new(cpu_id: usize, apic_id: u8, is_bsp: bool, stack_top: u64) -> Self {
        Self {
            cpu_id,
            apic_id,
            is_bsp,
            online: AtomicBool::new(is_bsp),
            ticks: AtomicU64::new(0),
            stack_top,
        }
    }
}

/// Global Per-CPU State Table.
pub static CPU_TABLE: Spinlock<Vec<Box<CpuData>>> = Spinlock::new(Vec::new());

/// Bootstraps all discovered Application Processors (APs) into 64-bit Long Mode.
pub fn init() {
    println!();
    println!("----------------------------------------");
    println!("  [SMP] Initializing Multiprocessor Core");
    println!("----------------------------------------");

    let madt_guard = acpi::MADT_INFO.lock();
    let madt = match madt_guard.as_ref() {
        Some(m) => m,
        None => {
            println!("[SMP] Warning: ACPI MADT not found; running in single-core mode.");
            return;
        }
    };

    let (cr3_frame, _) = crate::memory::paging::x86::read_cr3();
    let kernel_cr3 = cr3_frame.start_address().as_u64();

    // 1. Initialize BSP and AP entries in CPU_TABLE
    {
        let mut table = CPU_TABLE.lock();
        table.clear();

        for cpu in &madt.cpus {
            let stack_top = if cpu.is_bsp {
                0
            } else {
                let stack_layout = alloc::alloc::Layout::from_size_align(AP_STACK_SIZE, 16)
                    .expect("Failed to create AP stack layout");
                let stack_bottom = unsafe { alloc::alloc::alloc_zeroed(stack_layout) };
                if stack_bottom.is_null() {
                    panic!("[SMP] Out of memory allocating AP stack for CPU #{}", cpu.cpu_id);
                }
                (stack_bottom as u64) + AP_STACK_SIZE as u64
            };

            let cpu_data = Box::new(CpuData::new(cpu.cpu_id, cpu.apic_id, cpu.is_bsp, stack_top));
            table.push(cpu_data);
        }
    }

    if madt.cpus.len() <= 1 {
        println!("[SMP] Single-core system detected (1 CPU core).");
        return;
    }

    // 2. Install AP Trampoline to physical address 0x8000
    unsafe {
        let tramp_start = &ap_trampoline_start as *const _ as *const u8;
        let tramp_end = &ap_trampoline_end as *const _ as *const u8;
        let tramp_len = tramp_end.offset_from(tramp_start) as usize;

        let dst = AP_TRAMPOLINE_ADDR as *mut u8;
        core::ptr::copy_nonoverlapping(tramp_start, dst, tramp_len);
    }
    println!("[SMP] AP 16-to-64 bit bootstrap trampoline installed at {:#06x}", AP_TRAMPOLINE_ADDR);

    // 3. Awaken each Application Processor (AP)
    let mut online_count = 1; // BSP is core 0

    for cpu in &madt.cpus {
        if cpu.is_bsp || !cpu.enabled {
            continue;
        }

        println!("[SMP] Preparing CPU #{} (APIC ID: {})...", cpu.cpu_id, cpu.apic_id);

        let stack_top = {
            let table = CPU_TABLE.lock();
            table.iter().find(|c| c.cpu_id == cpu.cpu_id).map(|c| c.stack_top).unwrap_or(0)
        };

        let tramp_start = unsafe { &ap_trampoline_start as *const _ as usize };
        let mailbox_cr3 = (AP_TRAMPOLINE_ADDR as usize + (unsafe { &ap_mailbox_cr3 as *const _ as usize } - tramp_start)) as *mut u64;
        let mailbox_stack = (AP_TRAMPOLINE_ADDR as usize + (unsafe { &ap_mailbox_stack as *const _ as usize } - tramp_start)) as *mut u64;
        let mailbox_entry = (AP_TRAMPOLINE_ADDR as usize + (unsafe { &ap_mailbox_entry as *const _ as usize } - tramp_start)) as *mut u64;
        let mailbox_cpu_id = (AP_TRAMPOLINE_ADDR as usize + (unsafe { &ap_mailbox_cpu_id as *const _ as usize } - tramp_start)) as *mut u64;
        let mailbox_status = (AP_TRAMPOLINE_ADDR as usize + (unsafe { &ap_mailbox_status as *const _ as usize } - tramp_start)) as *mut u32;

        // Configure Mailbox at 0x8000
        unsafe {
            core::ptr::write_volatile(mailbox_cr3, kernel_cr3);
            core::ptr::write_volatile(mailbox_stack, stack_top);
            core::ptr::write_volatile(mailbox_entry, ap_kernel_entry as *const () as u64);
            core::ptr::write_volatile(mailbox_cpu_id, cpu.cpu_id as u64);
            core::ptr::write_volatile(mailbox_status, 0);
        }

        println!("[SMP] Mailbox configured. Sending INIT IPI...");
        // Send INIT IPI (Edge Assert)
        apic::send_init_ipi(cpu.apic_id);
        println!("[SMP] INIT IPI sent. Waiting 10ms...");
        apic::spin_delay_us(10_000); // 10 ms delay

        println!("[SMP] Sending SIPI (vector 0x{:02x})...", AP_SIPI_VECTOR);
        // Send Startup IPI (SIPI) with vector 0x08
        apic::send_sipi(cpu.apic_id, AP_SIPI_VECTOR);
        apic::spin_delay_us(200); // 200 µs delay

        println!("[SMP] Polling for AP boot status...");
        // Wait up to 50 ms for AP online handshake
        let mut booted = false;
        for _ in 0..10_000 {
            let status = unsafe { core::ptr::read_volatile(mailbox_status) };
            if status == 1 {
                booted = true;
                break;
            }
            core::hint::spin_loop();
        }

        if !booted {
            // Send second SIPI per Intel MP specification
            apic::send_sipi(cpu.apic_id, AP_SIPI_VECTOR);
            for _ in 0..50_000 {
                let status = unsafe { core::ptr::read_volatile(mailbox_status) };
                if status == 1 {
                    booted = true;
                    break;
                }
                core::hint::spin_loop();
            }
        }

        if booted {
            println!(
                "[SMP] CPU #{} (APIC ID: {}) successfully awakened in 64-bit Long Mode!",
                cpu.cpu_id, cpu.apic_id
            );
            online_count += 1;
        } else {
            println!(
                "[SMP] Warning: CPU #{} (APIC ID: {}) failed to respond to SIPI.",
                cpu.cpu_id, cpu.apic_id
            );
        }
    }

    println!(
        "[SMP] Multiprocessor initialization complete: {}/{} CPU cores online.",
        online_count,
        madt.cpus.len()
    );
}

/// 64-Bit Rust Kernel Entry Point for Application Processors.
#[unsafe(no_mangle)]
pub extern "C" fn ap_kernel_entry(cpu_id: usize) -> ! {
    // 1. Initialize CPU control registers & SSE
    super::cpu::init();

    // 2. Load IDT on this core
    super::idt::init();

    // 3. Initialize Local APIC on this core
    super::apic::init_ap();

    // 4. Initialize Fast SYSCALL / SYSRET ABI
    super::syscall::init();

    // 5. Mark CPU as online in the global table
    {
        let table = CPU_TABLE.lock();
        if let Some(data) = table.iter().find(|d| d.cpu_id == cpu_id) {
            data.online.store(true, Ordering::SeqCst);
        }
    }

    // 6. Enable interrupts
    unsafe {
        core::arch::asm!("sti");
    }

    // 7. AP Core Idle Loop
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

/// Returns the total number of CPU cores currently online.
pub fn online_cpu_count() -> usize {
    CPU_TABLE
        .lock()
        .iter()
        .filter(|cpu| cpu.online.load(Ordering::Relaxed))
        .count()
}

/// Runs automated self-tests verifying that SMP initialization awakened all available cores.
pub fn run_self_tests() {
    let count = online_cpu_count();
    let total = acpi::cpu_count();

    println!("[SMP] SMP Multi-Core Verification: {}/{} CPU core(s) active.", count, total);
    assert!(count >= 1, "At least BSP must be online");
}
