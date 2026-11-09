//! Local APIC (Advanced Programmable Interrupt Controller) & Legacy 8259 PIC Driver.
//!
//! # Architecture & Silicon Overview
//! Modern x86_64 multiprocessor systems replace the legacy IBM PC AT dual-8259 PIC
//! with the Advanced Programmable Interrupt Controller (APIC) architecture:
//! - **Legacy 8259 PIC Disabling**: On IBM PCs, the 8259 PIC vectors `0..7` collide
//!   directly with CPU hardware exceptions (Vectors `8..15`). This driver remaps the PIC
//!   to vectors `0x20..0x2F` and masks all lines (`0xFF`) to completely silence the legacy chip.
//! - **Local APIC (LAPIC)**: An on-die interrupt controller present in each CPU core,
//!   mapped to physical memory at `0xFEE0_0000` (or configured via `IA32_APIC_BASE_MSR`).
//!   Manages local timer interrupts, inter-processor interrupts (IPIs), and device IRQs.
//! - **Local APIC Timer**: High-resolution, on-core timer programmed in Periodic Mode
//!   (Bit 17) to deliver scheduler preemption ticks on Interrupt Vector `32` (`0x20`).
//! - **Interrupt Command Register (ICR)**: Enables the Bootstrap Processor (BSP) to send
//!   INIT and Startup IPI (SIPI) packets to secondary Application Processors (APs) during SMP bootstrap.

use core::sync::atomic::{AtomicU64, Ordering};

// Legacy 8259 PIC I/O port coordinates
const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

// Local APIC Memory-Mapped I/O Register Offsets (Base physical address 0xFEE0_0000)
const IA32_APIC_BASE_MSR: u32 = 0x1B;
#[allow(dead_code)]
const LAPIC_ID: usize = 0x020;
const LAPIC_EOI: usize = 0x0B0;
const LAPIC_SPURIOUS: usize = 0x0F0;
pub const LAPIC_ICR_LOW: usize = 0x300;
pub const LAPIC_ICR_HIGH: usize = 0x310;
pub const LAPIC_TIMER: usize = 0x320;
pub const LAPIC_TIMER_INIT_CNT: usize = 0x380;
#[allow(dead_code)]
pub const LAPIC_TIMER_CURR_CNT: usize = 0x390;
pub const LAPIC_TIMER_DIV: usize = 0x3E0;

/// Target interrupt vector assigned to the Local APIC Periodic Timer (Vector 32).
pub const TIMER_INTERRUPT_VECTOR: u8 = 0x20;

/// Target interrupt vector assigned to Local APIC Spurious Interrupts (Vector 255).
pub const SPURIOUS_INTERRUPT_VECTOR: u8 = 0xFF;

/// Active physical base address of the Local APIC MMIO window.
static mut LAPIC_BASE: u64 = 0xFEE00000;

/// Monotonic system clock counter tracking timer ticks since kernel boot.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Writes an 8-bit value to a port-mapped I/O port.
unsafe fn outb(port: u16, val: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
    }
}

/// Provides a tiny hardware delay between port I/O writes by writing to unused diagnostic port `0x80`.
unsafe fn io_wait() {
    unsafe {
        outb(0x80, 0);
    }
}

/// Remaps and completely masks the legacy dual-8259 Programmable Interrupt Controllers.
///
/// # Hardware Steps
/// 1. Sends Initialization Command Word 1 (ICW1 = `0x11`) to begin initialization sequence.
/// 2. Remaps Master PIC vectors to `0x20..0x27` and Slave PIC to `0x28..0x2F` to move them
///    out of the CPU exception vector collision range (`0..31`).
/// 3. Configures cascading IRQ line 2 connection (ICW3).
/// 4. Configures 8086/88 mode operation (ICW4 = `0x01`).
/// 5. Writes `0xFF` to both PIC data ports to permanently mask all 16 legacy interrupt lines.
pub fn disable_pic() {
    unsafe {
        // Step 1: Initialize both PICs
        outb(PIC1_COMMAND, 0x11);
        io_wait();
        outb(PIC2_COMMAND, 0x11);
        io_wait();

        // Step 2: Set vector offsets
        outb(PIC1_DATA, 0x20); // Master PIC vector offset 32
        io_wait();
        outb(PIC2_DATA, 0x28); // Slave PIC vector offset 40
        io_wait();

        // Step 3: Establish master/slave cascading wiring
        outb(PIC1_DATA, 0x04);
        io_wait();
        outb(PIC2_DATA, 0x02);
        io_wait();

        // Step 4: 8086 mode
        outb(PIC1_DATA, 0x01);
        io_wait();
        outb(PIC2_DATA, 0x01);
        io_wait();

        // Step 5: Mask all interrupts on both PICs
        outb(PIC1_DATA, 0xFF);
        outb(PIC2_DATA, 0xFF);
    }
}

/// Reads a 32-bit register from the Local APIC MMIO space using volatile semantics.
#[allow(dead_code)]
unsafe fn read_lapic(offset: usize) -> u32 {
    unsafe {
        let base = core::ptr::addr_of!(LAPIC_BASE).read();
        let ptr = (base + offset as u64) as *const u32;
        core::ptr::read_volatile(ptr)
    }
}

/// Writes a 32-bit value to a Local APIC MMIO register using volatile semantics.
unsafe fn write_lapic(offset: usize, value: u32) {
    unsafe {
        let base = core::ptr::addr_of!(LAPIC_BASE).read();
        let ptr = (base + offset as u64) as *mut u32;
        core::ptr::write_volatile(ptr, value);
    }
}

/// Signals End of Interrupt (EOI) to the Local APIC.
///
/// Must be written at the conclusion of every hardware interrupt service routine
/// to clear the in-service bit in the APIC and permit further interrupt delivery.
pub fn end_of_interrupt() {
    unsafe {
        write_lapic(LAPIC_EOI, 0);
    }
}

/// Returns the current monotonic kernel tick count.
pub fn get_ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Increments the monotonic tick counter. Invoked on every APIC Timer ISR tick.
pub fn on_timer_tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

/// Initializes the Local APIC and arms the APIC Timer in Periodic Mode on the BSP.
///
/// # Hardware Steps
/// 1. Masks the legacy 8259 PIC.
/// 2. Reads MSR `0x1B` (`IA32_APIC_BASE_MSR`) to verify and enable the Local APIC globally (Bit 11).
/// 3. Configures the Spurious Interrupt Vector register (`0x0F0`), enabling the APIC software flag (Bit 8).
/// 4. Configures the Timer Divide register (`0x3E0`) to divide core clock by 16 (`0x3`).
/// 5. Configures LVT Timer Register (`0x320`) for Periodic Mode (Bit 17) on Vector 32 (`0x20`).
/// 6. Loads the Initial Count Register (`0x380`) with the countdown reload value.
pub fn init() {
    disable_pic();

    unsafe {
        // Read APIC base from MSR 0x1B
        let mut eax: u32;
        let mut edx: u32;
        core::arch::asm!(
            "rdmsr",
            in("ecx") IA32_APIC_BASE_MSR,
            out("eax") eax,
            out("edx") edx,
            options(nomem, nostack)
        );

        let apic_base_msr = ((edx as u64) << 32) | (eax as u64);
        let base_addr = apic_base_msr & 0xFFFFF000;
        LAPIC_BASE = base_addr;

        // Enable APIC by setting bit 11 in MSR 0x1B if not already enabled
        if (apic_base_msr & (1 << 11)) == 0 {
            let new_eax = eax | (1 << 11);
            core::arch::asm!(
                "wrmsr",
                in("ecx") IA32_APIC_BASE_MSR,
                in("eax") new_eax,
                in("edx") edx,
                options(nomem, nostack)
            );
        }

        // Enable Spurious Interrupt Vector register (bit 8 = software enable APIC)
        write_lapic(LAPIC_SPURIOUS, 0x100 | (SPURIOUS_INTERRUPT_VECTOR as u32));

        // Configure APIC Timer:
        // Divide by 16 (0x3)
        write_lapic(LAPIC_TIMER_DIV, 0x3);

        // Configure timer in Periodic mode (bit 17: 0x20000) on Vector 32
        write_lapic(LAPIC_TIMER, 0x20000 | (TIMER_INTERRUPT_VECTOR as u32));
        write_lapic(LAPIC_TIMER_INIT_CNT, 0x00080000); // Periodic counter
    }
}

/// Delays execution by waiting for `ticks` APIC timer intervals.
pub fn sleep(ticks: u64) {
    let start = get_ticks();
    while get_ticks() - start < ticks {
        core::hint::spin_loop();
    }
}

/// Delays execution by busy-spinning for approximately `us` microseconds without requiring interrupts.
pub fn spin_delay_us(us: u64) {
    let iterations = us * 500;
    for _ in 0..iterations {
        core::hint::spin_loop();
    }
}

/// Polls the Interrupt Command Register (ICR) delivery status bit (bit 12) until idle.
unsafe fn wait_icr_idle() {
    unsafe {
        for _ in 0..100_000 {
            if (read_lapic(LAPIC_ICR_LOW) & (1 << 12)) == 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }
}

/// Sends an INIT Inter-Processor Interrupt (IPI) to the specified Local APIC ID.
///
/// Signals an INIT reset to the target core as part of the SMP bootstrap sequence.
pub fn send_init_ipi(apic_id: u8) {
    unsafe {
        wait_icr_idle();
        write_lapic(LAPIC_ICR_HIGH, (apic_id as u32) << 24);
        // Delivery mode 5 (INIT), Edge triggered
        write_lapic(LAPIC_ICR_LOW, 0x00000500);
        wait_icr_idle();
    }
}

/// Sends an INIT De-assert IPI to the target Local APIC ID.
#[allow(dead_code)]
pub fn send_init_deassert(apic_id: u8) {
    unsafe {
        wait_icr_idle();
        write_lapic(LAPIC_ICR_HIGH, (apic_id as u32) << 24);
        // Delivery mode 5 (INIT), De-assert (bit 14 clear, bit 15 level)
        write_lapic(LAPIC_ICR_LOW, 0x00008500);
        wait_icr_idle();
    }
}

/// Sends a Startup IPI (SIPI) to the target Local APIC ID.
///
/// Instructs the target core to begin real-mode execution at physical address `vector * 4096`.
pub fn send_sipi(apic_id: u8, vector: u8) {
    unsafe {
        wait_icr_idle();
        write_lapic(LAPIC_ICR_HIGH, (apic_id as u32) << 24);
        // Delivery mode 6 (Startup), Edge triggered, vector
        write_lapic(LAPIC_ICR_LOW, 0x00000600 | (vector as u32));
        wait_icr_idle();
    }
}

/// Initializes the Local APIC on an Application Processor (AP) core.
///
/// Enables APIC globally in MSR `0x1B`, activates the Spurious Interrupt Register,
/// and masks the local APIC timer so secondary cores do not deliver competing ticks.
pub fn init_ap() {
    unsafe {
        // Read APIC base from MSR 0x1B
        let mut eax: u32;
        let mut edx: u32;
        core::arch::asm!(
            "rdmsr",
            in("ecx") IA32_APIC_BASE_MSR,
            out("eax") eax,
            out("edx") edx,
            options(nomem, nostack)
        );

        let apic_base_msr = ((edx as u64) << 32) | (eax as u64);

        // Enable APIC by setting bit 11 in MSR 0x1B if not already enabled
        if (apic_base_msr & (1 << 11)) == 0 {
            let new_eax = eax | (1 << 11);
            core::arch::asm!(
                "wrmsr",
                in("ecx") IA32_APIC_BASE_MSR,
                in("eax") new_eax,
                in("edx") edx,
                options(nomem, nostack)
            );
        }

        // Enable Spurious Interrupt Vector register (bit 8 = software enable APIC)
        write_lapic(LAPIC_SPURIOUS, 0x100 | (SPURIOUS_INTERRUPT_VECTOR as u32));

        // Mask AP Local APIC timer so it doesn't conflict with BSP scheduler
        write_lapic(LAPIC_TIMER, 0x10000); // Masked (bit 16 = 1)
    }
}

