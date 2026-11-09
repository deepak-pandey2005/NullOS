//! Architecture-Specific Abstractions and Hardware Interfaces.
//!
//! # Overview
//! This module isolates CPU architecture specifics from generic operating system logic.
//! NullOS currently targets the 64-bit AMD64 / Intel 64 (`x86_64`) architecture,
//! managing CPU registers, segmentation (GDT/TSS), interrupts (IDT), Local APIC,
//! multi-core bootstrap (SMP), system call dispatch, and ACPI power management.

pub mod x86_64;
