# Phase 5 — User Mode & Syscalls (Milestones M12 - M13)

This phase establishes privilege separation (Ring 0 Kernel vs. Ring 3 User Mode), fast hardware system calls via `SYSCALL` / `SYSRET`, isolated process address spaces, ELF64 executable loading, dynamic memory mapping (`mmap`), and resilient user-mode crash handling in NullOS.

---

## Subsystem Architecture

```
kernel/src/
├── arch/x86_64/
│   ├── gdt.rs           # GDT with TSS, Kernel CS/DS (0x8/0x10), User CS/DS (0x20/0x18 | 3)
│   ├── idt.rs           # Exception handling with Ring 3 user-mode crash detection & thread exit
│   ├── syscall.rs       # MSR initialization (EFER.SCE, STAR, LSTAR, FMASK)
│   └── syscall.S        # Fast syscall_entry, enter_user_mode (iretq), user test & fault programs
├── memory/
│   ├── address_space.rs # ProcessAddressSpace abstraction (dedicated PML4 cloning & isolation)
│   └── paging.rs        # 4-level paging with intermediate USER_ACCESSIBLE propagation
└── task/
    ├── elf.rs           # ELF64 user binary loader (header/program header parser, PT_LOAD mapping)
    └── syscall.rs       # Syscall dispatcher (SYS_WRITE, SYS_EXIT, SYS_YIELD, SYS_GETPID, SYS_MMAP),
                         # User address mapping, Ring 3 launch, and verification self-tests
```

---

## Completed Milestones

### M12 — User Mode (Privilege Separation & Isolation)
- [x] **GDT Ring 3 Selectors**:
  - `User Data Segment`: Selector `0x18 | 3 = 0x1B` (RPL 3).
  - `User Code Segment (64-bit)`: Selector `0x20 | 3 = 0x23` (RPL 3).
- [x] **TSS RSP0 Dynamic Configuration**:
  - `set_tss_rsp0(rsp0: u64)` updates `TSS.rsp0` so hardware interrupts and privilege switches from Ring 3 safely use the kernel syscall/interrupt stack.
- [x] **Ring 0 -> Ring 3 Transition (`iretq`)**:
  - Hand-crafted `enter_user_mode(entry_point: u64, user_stack_top: u64) -> !` primitive in `syscall.S`.
  - Pushes user SS (`0x1B`), user RSP, user RFLAGS (`0x202` with interrupts enabled), user CS (`0x23`), and user RIP, then executes `iretq`.
- [x] **Process Address Space (`AddressSpace`)**:
  - Creates dedicated PML4 root tables for user processes.
  - Recursively maps `PML4[511]`.
  - Clones higher-half kernel entries (`PML4[256..511]`) while keeping lower-half user space (`PML4[0..256]`, 128 TiB) completely private per process.
- [x] **User ELF64 Binary Loader (`task/elf.rs`)**:
  - Parses 64-bit ELF headers, verifying magic `\x7fELF`, 64-bit class, little-endian, and x86_64 machine type.
  - Iterates over `PT_LOAD` segments, validates canonical user virtual address bounds, maps pages with appropriate permissions (`R`, `W`, `X`), and zero-fills BSS memory.
  - Discovers and returns the binary's entry point (`e_entry`).
- [x] **User-Mode Crash Handling (`arch/x86_64/idt.rs`)**:
  - Checks CPL via `(ctx.cs & 3) == 3`.
  - When a user mode process triggers an exception (e.g. Page Fault #PF from unmapped memory or Invalid Opcode #UD):
    - Diagnostic crash details are logged (`[USER CRASH]`).
    - The faulting user thread is cleanly terminated via `task::exit()`.
    - The kernel and all other processes continue executing without interruption.

### M13 — Fast Syscalls (`SYSCALL` / `SYSRET`)
- [x] **Hardware MSR Configuration**:
  - `EFER.SCE` (System Call Extensions) enabled via MSR `0xC000_0080`.
  - `STAR` MSR (`0xC000_0081`): Configured with `0x0008` (Kernel CS/SS base) and `0x0010` (User CS/SS base for `sysretq`).
  - `LSTAR` MSR (`0xC000_0082`): Configured with the RIP of `syscall_entry`.
  - `FMASK` MSR (`0xC000_0084`): Configured with `0x300` to mask `IF` and `TF` on syscall entry.
- [x] **Low-Overhead `syscall_entry` in `syscall.S`**:
  - Switches from user `%rsp` to `kernel_syscall_stack_top`.
  - Preserves user context (`%rcx` user RIP, `%r11` user RFLAGS, `%rsp`, and general-purpose registers).
  - Remaps registers to standard SysV AMD64 C calling convention and invokes `syscall_dispatcher`.
  - Restores user registers and executes `sysretq` back to Ring 3.
- [x] **Syscall Dispatcher & Core System Calls**:
  - `SYS_WRITE (1)`: Prints user buffers to console / serial output.
  - `SYS_EXIT (2)`: Terminates user process with exit status and signals completion.
  - `SYS_YIELD (3)`: Voluntarily yields CPU to other ready threads.
  - `SYS_GETPID (4)`: Returns the current thread ID / process ID.
  - `SYS_SLEEP (5)`: Puts the calling process to sleep for a specified number of ticks.
  - `SYS_MMAP (9)`: Dynamically allocates and maps 4 KiB user pages with requested permissions (`PROT_READ`, `PROT_WRITE`, `PROT_EXEC`).

---

## Verification

Run the automated headless test:
```bash
./scripts/run.sh --headless
```

Expected output:
```
----------------------------------------
 [USER] Launching Ring 3 User Mode Demo 
----------------------------------------
[USER] Testing Process Address Space creation (PML4 cloning)...
[USER] Dedicated Process Address Space active at PML4 0x3e3000 (recursive PML4[511] + shared higher half).
[USER] Testing User ELF64 Binary Loader...
[USER] User ELF binary successfully validated & mapped: entry 0x400000, size 307 bytes.
[TASK] Spawned thread #3 "user_process" (stack: 16 KiB)
[USER] Configuring TSS RSP0 and switching: Ring 0 -> Ring 3 via iretq...
[USER RING 3] Hello from User Mode via SYSCALL!
[USER RING 3] SYS_MMAP verified: Dynamically mapped and wrote to user page at 0x60000000!
[USER] User process finished with exit code: 0
[TASK] Thread #3 "user_process" finished (total ticks: 2).

[USER] Testing User Mode Crash Handling (deliberate unmapped memory access in Ring 3)...
[TASK] Spawned thread #4 "fault_process" (stack: 16 KiB)
[USER CRASH TEST] Entering Ring 3 to execute deliberate faulting instruction...
Exception: Page Fault (#PF) (Vector: 14)
CR2 Faulting Address: 0x00000000dead0000
Page Fault Reason:   Page not present during read in user mode
[USER CRASH] User mode process crashed with Page Fault (#PF) (vector 14) at RIP: 0x000000000050000a!
[USER CRASH] Terminating faulting user thread cleanly and continuing OS execution...
[TASK] Thread #4 "fault_process" finished (total ticks: 1).
[USER] User Mode Crash Handling verified: Faulting user thread was terminated cleanly while kernel remained intact!
[USER] All Phase 5 milestones (M12 User Mode & M13 Syscalls) fully verified!
```
