# NullOS

*A custom, bare-metal 64-bit operating system for the `x86_64` architecture, built using `C++20` and Assembly. NullOS utilizes the modern Limine Bootloader to transition safely into 64-bit long mode, initializing essential kernel subsystems from scratch.*

Features
- Pure 64-Bit Entry: Boots straight into 64-bit long mode via the Limine boot protocol.

Prerequisites
- To build and emulate NullOS on Arch Linux, make sure you have your cross-compiler toolchain and system utilities ready:

`sudo pacman -S base-devel nasm qemu-desktop xorriso`

Toolchain Expectation
- The build configuration defaults to looking for an ELF cross-compiler path to prevent pollution from host operating system symbols:

C++ Compiler: `x86_64-elf-g++`

Linker: `x86_64-elf-ld` or `x86_64-elf-g++`

