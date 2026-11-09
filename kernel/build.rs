//! NullOS Kernel Build Script (Phase 2 & Phase 3).
//!
//! # Purpose & Build Configuration
//!
//! This build script is executed by `cargo` on the host system prior to compiling
//! the freestanding x86_64 kernel crate:
//!
//! 1. **Assembly Compilation**:
//!    Uses the [`cc`] crate to compile low-level x86_64 assembly source files (`.S`)
//!    into an object archive (`libkernel_asm.a`) that is linked into the kernel binary:
//!    - `src/arch/x86_64/entry.S`: Early 64-bit kernel entry point and stack initialization.
//!    - `src/arch/x86_64/interrupts.S`: CPU exception and interrupt service routine (ISR) stubs.
//!    - `src/arch/x86_64/switch.S`: Low-level cooperative and preemptive thread context switcher.
//!    - `src/arch/x86_64/syscall.S`: Fast `syscall`/`sysret` entry point stub.
//!    - `src/arch/x86_64/trampoline.S`: 16-bit real mode Application Processor (AP) SMP boot trampoline.
//!
//! 2. **Compiler Flags**:
//!    - `-ffreestanding`: Instructs the compiler not to assume standard C library runtime facilities.
//!    - `-fno-stack-protector`: Disables compiler-inserted stack canaries for low-level assembly stubs.
//!    - `-mno-red-zone`: Prevents the compiler from using the 128-byte x86-64 red zone below the stack pointer
//!      which would otherwise be corrupted by asynchronous hardware interrupts.
//!
//! 3. **Linker Script Injection**:
//!    Passes `-T../linker/x86_64.ld` to the linker, defining memory section layouts (`.text`, `.rodata`,
//!    `.data`, `.bss`) and kernel virtual load addresses.

use std::env;
use std::path::PathBuf;

fn main() {
    // Notify cargo to re-run this build script if any assembly file or linker script changes
    println!("cargo:rerun-if-changed=src/arch/x86_64/entry.S");
    println!("cargo:rerun-if-changed=src/arch/x86_64/interrupts.S");
    println!("cargo:rerun-if-changed=src/arch/x86_64/switch.S");
    println!("cargo:rerun-if-changed=src/arch/x86_64/syscall.S");
    println!("cargo:rerun-if-changed=src/arch/x86_64/trampoline.S");
    println!("cargo:rerun-if-changed=../linker/x86_64.ld");

    // Compile freestanding kernel assembly stubs into a static library archive
    cc::Build::new()
        .file("src/arch/x86_64/entry.S")
        .file("src/arch/x86_64/interrupts.S")
        .file("src/arch/x86_64/switch.S")
        .file("src/arch/x86_64/syscall.S")
        .file("src/arch/x86_64/trampoline.S")
        .flag("-ffreestanding")
        .flag("-fno-stack-protector")
        .flag("-mno-red-zone")
        .compile("kernel_asm");

    // Resolve canonical path to the custom x86_64 ELF linker script
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let linker_script = manifest_dir
        .join("../linker/x86_64.ld")
        .canonicalize()
        .unwrap();

    // Pass the linker script argument to rustc
    println!("cargo:rustc-link-arg=-T{}", linker_script.display());
}
