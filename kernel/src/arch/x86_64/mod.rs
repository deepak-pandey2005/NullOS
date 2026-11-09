//! x86_64 Core Silicon, Interrupts, Multi-Core & Hardware Subsystems.
//!
//! # Subsystem Structure
//! - [`cpu`]: CPUID feature detection, control registers (`CR0`, `CR2`, `CR3`, `CR4`), MSRs.
//! - [`gdt`]: Global Descriptor Table and 16-byte 64-bit Task State Segment (TSS).
//! - [`idt`]: 256-gate Interrupt Descriptor Table, exception handlers, and ISR dispatch.
//! - [`apic`]: Advanced Programmable Interrupt Controller (Local APIC) and periodic timer.
//! - [`smp`]: Symmetric Multiprocessing: INIT-SIPI-SIPI AP multi-core bootstrap trampoline.
//! - [`syscall`]: Fast Ring 3 $\rightarrow$ Ring 0 transitions via hardware `SYSCALL` / `SYSRET` (`MSR_LSTAR`).
//! - [`acpi`]: ACPI table parsing (RSDP, XSDT, MADT, FADT, HPET, DSDT) and power state management.

pub mod acpi;
pub mod apic;
pub mod cpu;
pub mod gdt;
pub mod idt;
pub mod smp;
pub mod syscall;
