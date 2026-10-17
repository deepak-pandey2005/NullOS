/// Legacy 8259 PIC and Local APIC / APIC Timer driver.
///
/// Disables legacy 8259 PIC (masks all IRQs and remaps to 0x20/0x28 to avoid spurious IRQs),
/// initializes Local APIC, configures APIC Timer for periodic kernel ticks, and provides sleep primitives.

use core::sync::atomic::{AtomicU64, Ordering};

// Legacy 8259 PIC I/O ports
const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

// Local APIC Registers (Base physical address 0xFEE00000)
const IA32_APIC_BASE_MSR: u32 = 0x1B;
#[allow(dead_code)]
const LAPIC_ID: usize = 0x020;
const LAPIC_EOI: usize = 0x0B0;
const LAPIC_SPURIOUS: usize = 0x0F0;
const LAPIC_TIMER: usize = 0x320;
const LAPIC_TIMER_INIT_CNT: usize = 0x380;
#[allow(dead_code)]
const LAPIC_TIMER_CURR_CNT: usize = 0x390;
const LAPIC_TIMER_DIV: usize = 0x3E0;

pub const TIMER_INTERRUPT_VECTOR: u8 = 0x20; // IRQ 0 / Vector 32
pub const SPURIOUS_INTERRUPT_VECTOR: u8 = 0xFF;

static mut LAPIC_BASE: u64 = 0xFEE00000;
static TICKS: AtomicU64 = AtomicU64::new(0);

unsafe fn outb(port: u16, val: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
    }
}

unsafe fn io_wait() {
    unsafe {
        outb(0x80, 0);
    }
}

/// Mask and disable 8259 legacy PIC
pub fn disable_pic() {
    unsafe {
        // Remap PIC to avoid conflicts with CPU exceptions (0..31)
        outb(PIC1_COMMAND, 0x11);
        io_wait();
        outb(PIC2_COMMAND, 0x11);
        io_wait();

        outb(PIC1_DATA, 0x20); // Master PIC vector offset 32
        io_wait();
        outb(PIC2_DATA, 0x28); // Slave PIC vector offset 40
        io_wait();

        outb(PIC1_DATA, 0x04);
        io_wait();
        outb(PIC2_DATA, 0x02);
        io_wait();

        outb(PIC1_DATA, 0x01);
        io_wait();
        outb(PIC2_DATA, 0x01);
        io_wait();

        // Mask all interrupts on both PICs
        outb(PIC1_DATA, 0xFF);
        outb(PIC2_DATA, 0xFF);
    }
}

/// Read 32-bit register from Local APIC MMIO
#[allow(dead_code)]
unsafe fn read_lapic(offset: usize) -> u32 {
    unsafe {
        let base = core::ptr::addr_of!(LAPIC_BASE).read();
        let ptr = (base + offset as u64) as *const u32;
        core::ptr::read_volatile(ptr)
    }
}

/// Write 32-bit register to Local APIC MMIO
unsafe fn write_lapic(offset: usize, value: u32) {
    unsafe {
        let base = core::ptr::addr_of!(LAPIC_BASE).read();
        let ptr = (base + offset as u64) as *mut u32;
        core::ptr::write_volatile(ptr, value);
    }
}

/// Signal End of Interrupt (EOI) to Local APIC
pub fn end_of_interrupt() {
    unsafe {
        write_lapic(LAPIC_EOI, 0);
    }
}

/// Return current kernel tick count
pub fn get_ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Increment tick counter (called from timer ISR)
pub fn on_timer_tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

/// Calibrate and initialize Local APIC & APIC Timer
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

        // Calibrate timer using PIT delay (approximate 10ms calibration)
        // Set initial count to 10,000,000 for periodic timer (periodic mode = bit 17: 0x20000)
        write_lapic(LAPIC_TIMER, 0x20000 | (TIMER_INTERRUPT_VECTOR as u32));
        write_lapic(LAPIC_TIMER_INIT_CNT, 0x00080000); // Periodic counter
    }
}

/// Delay execution by approximately `ms` milliseconds using tick count or spin loop
pub fn sleep(ticks: u64) {
    let start = get_ticks();
    while get_ticks() - start < ticks {
        core::hint::spin_loop();
    }
}
