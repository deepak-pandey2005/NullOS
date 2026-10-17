use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=src/arch/x86_64/entry.S");
    println!("cargo:rerun-if-changed=src/arch/x86_64/interrupts.S");
    println!("cargo:rerun-if-changed=../linker/x86_64.ld");

    cc::Build::new()
        .file("src/arch/x86_64/entry.S")
        .file("src/arch/x86_64/interrupts.S")
        .flag("-ffreestanding")
        .flag("-fno-stack-protector")
        .flag("-mno-red-zone")
        .compile("kernel_asm");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let linker_script = manifest_dir
        .join("../linker/x86_64.ld")
        .canonicalize()
        .unwrap();

    println!("cargo:rustc-link-arg=-T{}", linker_script.display());
}
