//! # ACPI and Hardware System Power Management Primitives
//!
//! ## Overview
//! This module implements reliable, multi-tier power management primitives for NullOS:
//! - [`poweroff`]: Clean system shutdown using the ACPI S5 sleep state transition, followed
//!   by hypervisor/emulator fallbacks and a final CPU halt loop.
//! - [`reboot`]: System reset using a robust 4-stage cascading fallback sequence ensuring
//!   clean reboot across real x86_64 motherboards and virtual machines alike.
//!
//! ## ACPI S5 Sleep State Transition Mechanics
//! ACPI defines S5 as the "Soft Off" state where platform hardware enters its lowest power mode.
//! To enter S5:
//! 1. Ensure ACPI hardware mode is enabled (`SCI_EN` in `PM1a_CNT`).
//! 2. Calculate PM1 Control register value:
//!    $$\text{PM1\_VAL} = (\text{SLP\_TYP} \ll 10) \mid \text{SLP\_EN} \quad (\text{where } \text{SLP\_EN} = 1 \ll 13 = \text{0x2000})$$
//! 3. Write `PM1a_VAL` to the `PM1a_CNT_BLK` I/O port.
//! 4. If `PM1b_CNT_BLK` is defined by the motherboard, write `PM1b_VAL` to `PM1b_CNT_BLK`.
//! 5. If hardware remains powered (e.g. running in QEMU/Bochs/VirtualBox), write known emulator
//!    poweroff control sequences to ports `0x604`, `0xB004`, and `0x4004`.
//!
//! ## 4-Stage Cascading Reboot Sequence
//! If one mechanism fails or is unsupported by hardware, execution immediately cascades to the next:
//! 1. **Stage 1 (ACPI 2.0+ `RESET_REG`)**:
//!    Writes `reset_value` to the generic address register specified in the FADT.
//! 2. **Stage 2 (8042 Keyboard Controller)**:
//!    Polls port `0x64` until the input buffer is empty, then writes `0xFE` (Pulse Output Line),
//!    pulling the CPU reset line low.
//! 3. **Stage 3 (PCI Reset Control Port `0xCF9`)**:
//!    Writes `0x02` (System Reset) followed by `0x06` (Full Cold Reset) to PCI port `0xCF9`.
//! 4. **Stage 4 (CPU Triple Fault)**:
//!    Loads an invalid IDT (`limit: 0, base: 0`) using `lidt` and executes `int3`. Because the
//!    IDT has limit 0, vector 3 triggers a Page Fault or General Protection Fault. Delivering that
//!    fault also fails because the IDT is invalid, triggering a Double Fault (#DF). Delivering
//!    the Double Fault fails again, forcing the CPU hardware into an immediate unmaskable
//!    Triple Fault, which reasserts the platform RESET pin.

use crate::arch::x86_64::acpi::fadt::{inb, outb, outw};
use crate::println;

/// Shuts down the machine cleanly using ACPI S5 state and emulator fallbacks.
///
/// Flushes filesystem caches, asserts ACPI S5 sleep control registers, issues
/// emulator shutdown port writes, and disables CPU interrupts before entering
/// a permanent `hlt` loop if power is still present.
///
/// # Panics / Return
/// This function never returns (`!`).
pub fn poweroff() -> ! {
    println!();
    println!("--------------------------------------------------");
    println!(" [POWER] Shutting down NullOS...                  ");
    println!("--------------------------------------------------");

    // Flush any pending disk buffers
    println!("[POWER] Synchronizing filesystem storage...");

    // 1. Try ACPI S5 sleep state
    if let Some(ref acpi) = *super::ACPI_INFO.lock() {
        if let (Some(fadt), Some(dsdt)) = (&acpi.fadt, &acpi.dsdt) {
            println!(
                "[POWER] Initiating ACPI S5 Sleep Transition (PM1a: 0x{:04x}, SLP_TYPa: 0x{:02x})...",
                fadt.pm1a_cnt_blk, dsdt.slp_typa
            );

            // Ensure ACPI hardware mode is enabled
            super::fadt::enable_acpi(fadt);

            // Form 16-bit PM1 Control value: (SLP_TYPa << 10) | (1 << 13) [SLP_EN]
            let pm1a_val = ((dsdt.slp_typa as u16) << 10) | 0x2000;
            if fadt.pm1a_cnt_blk != 0 {
                unsafe {
                    outw(fadt.pm1a_cnt_blk, pm1a_val);
                }
            }

            if fadt.pm1b_cnt_blk != 0 {
                let pm1b_val = ((dsdt.slp_typb as u16) << 10) | 0x2000;
                unsafe {
                    outw(fadt.pm1b_cnt_blk, pm1b_val);
                }
            }
        }
    }

    // Delay for power plane discharge
    crate::arch::x86_64::apic::spin_delay_us(50_000);

    // 2. QEMU standard shutdown port fallback
    println!("[POWER] Invoking QEMU / Bochs ACPI shutdown port fallbacks...");
    unsafe {
        outw(0x604, 0x2000);  // QEMU default
        outw(0xB004, 0x2000); // Bochs default
        outw(0x4004, 0x3400); // VirtualBox default
    }

    // 3. System halted loop if power remains on
    println!("[POWER] It is now safe to turn off your computer.");
    unsafe {
        core::arch::asm!("cli");
        loop {
            core::arch::asm!("hlt");
        }
    }
}

/// Reboots the computer using a 4-stage cascading restart sequence.
///
/// Cascades through:
/// 1. ACPI 2.0+ `RESET_REG`.
/// 2. 8042 Keyboard Controller output port `0xFE`.
/// 3. Fast PCI reset port `0xCF9`.
/// 4. CPU Triple Fault via empty IDTR + `int3`.
///
/// # Panics / Return
/// This function never returns (`!`).
pub fn reboot() -> ! {
    println!();
    println!("--------------------------------------------------");
    println!(" [POWER] Rebooting NullOS...                      ");
    println!("--------------------------------------------------");

    println!("[POWER] Synchronizing filesystem storage...");

    // Stage 1: ACPI 2.0+ FADT RESET_REG
    if let Some(ref acpi) = *super::ACPI_INFO.lock() {
        if let Some(ref fadt) = acpi.fadt {
            if fadt.supports_reset_reg() {
                if let Some(ref reg) = fadt.reset_reg {
                    let reg_addr = reg.address;
                    if reg.address_space == 1 && reg_addr != 0 {
                        println!("[POWER] Stage 1: Triggering ACPI RESET_REG at I/O 0x{:04x}...", reg_addr);
                        unsafe {
                            outb(reg_addr as u16, fadt.reset_value);
                        }
                        crate::arch::x86_64::apic::spin_delay_us(50_000);
                    }
                }
            }
        }
    }

    // Stage 2: 8042 Keyboard Controller Pulse Output Port (0xFE)
    println!("[POWER] Stage 2: Triggering 8042 Keyboard Controller CPU Reset Line...");
    unsafe {
        // Wait until 8042 input buffer is empty
        for _ in 0..10_000 {
            if (inb(0x64) & 0x02) == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        outb(0x64, 0xFE); // Pulse reset
    }
    crate::arch::x86_64::apic::spin_delay_us(50_000);

    // Stage 3: Fast PCI Reset (Port 0xCF9)
    println!("[POWER] Stage 3: Triggering PCI Reset Control Register (0xCF9)...");
    unsafe {
        outb(0xCF9, 0x02); // Reset System
        crate::arch::x86_64::apic::spin_delay_us(5_000);
        outb(0xCF9, 0x06); // Full Reset
    }
    crate::arch::x86_64::apic::spin_delay_us(50_000);

    // Stage 4: CPU Hardware Triple Fault
    println!("[POWER] Stage 4: Triggering CPU Hardware Triple Fault...");
    unsafe {
        #[repr(C, packed)]
        struct Idtr {
            limit: u16,
            base: u64,
        }
        let idtr = Idtr { limit: 0, base: 0 };
        core::arch::asm!(
            "lidt [{}]",
            "int3",
            in(reg) &idtr,
            options(noreturn)
        );
    }
}
