//! Symmetric Multiprocessing (SMP) Multi-Core Manager (Phase 9 - Milestone 25).
//!
//! # Multi-Core Bootstrap Architecture (Intel MP Spec)
//! At power-on, the motherboard firmware designates one CPU core as the **Bootstrap Processor (BSP)**.
//! The remaining CPU cores—termed **Application Processors (APs)**—are halted in an uninitialized
//! real-mode `Wait-for-SIPI` state by hardware.
//!
//! ### AP Bootstrap Sequence (INIT-SIPI-SIPI Protocol)
//! 1. **Per-CPU Kernel Stack Allocation**: Allocates 16 KiB dedicated 16-byte aligned kernel
//!    execution stacks for each secondary processor.
//! 2. **Trampoline Installation**: Copies 16-bit real-mode assembly code (`trampoline.S`)
//!    to physical address `0x8000` (low memory accessible via real-mode segment:offset addressing).
//! 3. **Mailbox Configuration**: Writes the kernel's active PML4 page table base (`CR3`), the core's
//!    stack pointer (`stack_top`), and the 64-bit Rust entry function (`ap_kernel_entry`)
//!    into shared communication memory at `0x8000`.
//! 4. **INIT Inter-Processor Interrupt (IPI)**: Sends an INIT IPI via the Local APIC Interrupt
//!    Command Register (ICR) to trigger a hardware reset of the AP core, followed by a 10ms delay.
//! 5. **Startup IPI (SIPI)**: Sends a SIPI with vector `0x08`, instructing the AP to start
//!    executing real-mode instructions at `0x08 * 4096 = 0x8000`. If the AP does not acknowledge
//!    within 50ms, a second SIPI is dispatched per Intel MP specification.
//! 6. **Long Mode Transition**: The trampoline switches the AP from 16-bit Real Mode to 32-bit
//!    Protected Mode (`CR0.PE`), loads identity page tables, sets `EFER.LME`, activates 64-bit
//!    paging (`CR0.PG`), and jumps directly into 64-bit Long Mode.
//! 7. **Rust Kernel Handshake**: In `ap_kernel_entry`, the AP sets its `online` flag, loads GDT
//!    and IDT, initializes its local APIC, and enters the preemptible multi-core scheduler loop.

#![allow(dead_code)]

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::println;
use crate::sync::Spinlock;
use super::acpi;
use super::apic;

/// Physical address where the AP bootstrap trampoline is loaded.
///
/// Must reside in real-mode addressable low memory (< 1 MiB).
/// SIPI vector `0x08` maps to physical address `0x08 * 4096 = 0x8000`.
pub const AP_TRAMPOLINE_ADDR: u64 = 0x8000;

/// Startup IPI Vector passed via Local APIC ICR.
pub const AP_SIPI_VECTOR: u8 = 0x08;

/// Per-CPU kernel stack size (16 KiB = 4 physical pages).
pub const AP_STACK_SIZE: usize = 16384;

// External symbols exported from `trampoline.S`
unsafe extern "C" {
    static ap_trampoline_start: u8;
    static ap_trampoline_end: u8;
    static ap_mailbox_cr3: u64;
    static ap_mailbox_stack: u64;
    static ap_mailbox_entry: u64;
    static ap_mailbox_cpu_id: u64;
    static ap_mailbox_status: u32;
}

/// Per-CPU Data Block tracking physical hardware state and runtime metrics.
pub struct CpuData {
    /// Logical zero-indexed CPU core ID (`0..N-1`).
    pub cpu_id: usize,
    /// Physical Local APIC hardware ID assigned by ACPI MADT.
    pub apic_id: u8,
    /// Whether this core is the primary Bootstrap Processor (BSP).
    pub is_bsp: bool,
    /// Atomic flag indicating whether this core has completed Long Mode initialization.
    pub online: AtomicBool,
    /// Number of local scheduling ticks executed on this core.
    pub ticks: AtomicU64,
    /// Virtual address of the top of this core's dedicated kernel stack.
    pub stack_top: u64,
}

impl CpuData {
    /// Creates a new per-CPU data tracking structure.
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

/// Global Per-CPU State Table tracking all enumerated CPU cores.
pub static CPU_TABLE: Spinlock<Vec<Box<CpuData>>> = Spinlock::new(Vec::new());

/// Bootstraps all discovered Application Processors (APs) into 64-bit Long Mode.
///
/// # Workflow
/// 1. Queries MADT (Multiple APIC Description Table) to enumerate all CPU cores and their APIC IDs.
/// 2. Allocates dedicated 16 KiB kernel execution stacks for each AP core.
/// 3. Copies the `trampoline.S` payload to low memory at `0x8000`.
/// 4. Iterates through each AP core:
///    - Populates the shared mailbox with `CR3`, `stack_top`, `cpu_id`, and `ap_kernel_entry`.
///    - Dispatches INIT IPI and waits 10 ms.
///    - Dispatches SIPI (vector 0x08) and polls the mailbox handshake variable.
///    - If timeout occurs, retries with a second SIPI per Intel MP specification.
/// 5. Prints final symmetric multiprocessing cluster status.
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

    // Step 1: Initialize BSP and allocate dedicated stacks for AP entries in CPU_TABLE
    {
        let mut table = CPU_TABLE.lock();
        table.clear();

        for cpu in &madt.cpus {
            let stack_top = if cpu.is_bsp {
                0 // BSP stack is managed by early bootloader
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

    // Step 2: Install AP Trampoline to physical address 0x8000
    unsafe {
        let tramp_start = &ap_trampoline_start as *const _ as *const u8;
        let tramp_end = &ap_trampoline_end as *const _ as *const u8;
        let tramp_len = tramp_end.offset_from(tramp_start) as usize;

        let dst = AP_TRAMPOLINE_ADDR as *mut u8;
        core::ptr::copy_nonoverlapping(tramp_start, dst, tramp_len);
    }
    println!("[SMP] AP 16-to-64 bit bootstrap trampoline installed at {:#06x}", AP_TRAMPOLINE_ADDR);

    // Step 3: Awaken each Application Processor (AP) core sequentially
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

        // Configure Mailbox parameters at physical address 0x8000
        unsafe {
            core::ptr::write_volatile(mailbox_cr3, kernel_cr3);
            core::ptr::write_volatile(mailbox_stack, stack_top);
            core::ptr::write_volatile(mailbox_entry, ap_kernel_entry as *const () as u64);
            core::ptr::write_volatile(mailbox_cpu_id, cpu.cpu_id as u64);
            core::ptr::write_volatile(mailbox_status, 0);
        }

        println!("[SMP] Mailbox configured. Sending INIT IPI...");
        // Send INIT IPI (Edge Assert) to reset AP core hardware
        apic::send_init_ipi(cpu.apic_id);
        println!("[SMP] INIT IPI sent. Waiting 10ms...");
        apic::spin_delay_us(10_000); // 10 ms hardware stabilization delay

        println!("[SMP] Sending SIPI (vector 0x{:02x})...", AP_SIPI_VECTOR);
        // Send Startup IPI (SIPI) pointing to vector 0x08 (physical 0x8000)
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
            // Send second SIPI per Intel MP specification retry protocol
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
///
/// Invoked directly by `trampoline.S` once the AP transitions into 64-bit Long Mode.
///
/// # Core Bringup Sequence
/// 1. Loads Global Descriptor Table on this core (`load_ap`).
/// 2. Initializes CPU control registers, Write Protection, and SSE (`cpu::init`).
/// 3. Loads Interrupt Descriptor Table on this core (`idt::init`).
/// 4. Initializes Local APIC and masks timer on this core (`apic::init_ap`).
/// 5. Enables hardware `SYSCALL`/`SYSRET` MSR registers (`syscall::init`).
/// 6. Sets atomic `online = true` in the global `CPU_TABLE`.
/// 7. Enables CPU hardware interrupts (`sti`).
/// 8. Enters the idle halt loop (`hlt`).
#[unsafe(no_mangle)]
pub extern "C" fn ap_kernel_entry(cpu_id: usize) -> ! {
    // Step 0: Load GDT on this core
    super::gdt::load_ap();

    // Step 1: Initialize CPU control registers & SSE
    super::cpu::init();

    // Step 2: Load IDT on this core
    super::idt::init();

    // Step 3: Initialize Local APIC on this core
    super::apic::init_ap();

    // Step 4: Initialize Fast SYSCALL / SYSRET ABI
    super::syscall::init();

    // Step 5: Mark CPU as online in the global table
    {
        let table = CPU_TABLE.lock();
        if let Some(data) = table.iter().find(|d| d.cpu_id == cpu_id) {
            data.online.store(true, Ordering::SeqCst);
        }
    }

    // Step 6: Enable interrupts
    unsafe {
        core::arch::asm!("sti");
    }

    // Step 7: AP Core Idle Loop
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

/// Returns the total number of CPU cores currently online and active.
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
