# Phase 5 — User Mode & Syscalls (Milestones M12 - M13)

This phase establishes privilege separation (Ring 0 Kernel vs. Ring 3 User Mode), fast hardware system calls via `SYSCALL` / `SYSRET`, and user-mode execution in NullOS.

---

## Subsystem Architecture

```
kernel/src/
├── arch/x86_64/
│   ├── gdt.rs           # GDT with TSS, Kernel CS/DS (0x8/0x10), User CS/DS (0x20/0x18 | 3)
│   ├── syscall.rs       # MSR initialization (EFER.SCE, STAR, LSTAR, FMASK)
│   └── syscall.S        # Fast syscall_entry, enter_user_mode (iretq), user_test_program
├── memory/
│   └── paging.rs        # 4-level paging with intermediate USER_ACCESSIBLE propagation
└── task/
    └── syscall.rs       # Syscall dispatcher (SYS_WRITE, SYS_EXIT, SYS_YIELD, SYS_GETPID),
                         # User address mapping, Ring 3 launch & self-tests
```

---

## Completed Milestones

### M12 — User Mode (Privilege Separation)
- [x] **GDT Ring 3 Selectors**:
  - `User Data Segment`: Selector `0x18 | 3 = 0x1B` (RPL 3).
  - `User Code Segment (64-bit)`: Selector `0x20 | 3 = 0x23` (RPL 3).
- [x] **TSS RSP0 Dynamic Configuration**:
  - `set_tss_rsp0(rsp0: u64)` updates `TSS.rsp0` so hardware interrupts and privilege switches from Ring 3 safely use the kernel syscall/interrupt stack.
- [x] **Ring 0 -> Ring 3 Transition (`iretq`)**:
  - Hand-crafted `enter_user_mode(entry_point: u64, user_stack_top: u64) -> !` primitive in `syscall.S`.
  - Pushes user SS (`0x1B`), user RSP, user RFLAGS (`0x202` with interrupts enabled), user CS (`0x23`), and user RIP, then executes `iretq`.
- [x] **User Address Space & Page Protection**:
  - User code mapped at `0x0040_0000` with `PRESENT | WRITABLE | USER_ACCESSIBLE`.
  - User stack mapped at `0x7000_0000` with `PRESENT | WRITABLE | USER_ACCESSIBLE | NO_EXECUTE`.
  - Upgraded intermediate table traversal (`PML4`, `PDPT`, `PD`) in `map_page` to automatically grant `USER_ACCESSIBLE` permissions to user address spaces.

### M13 — Fast Syscalls (`SYSCALL` / `SYSRET`)
- [x] **Hardware MSR Configuration**:
  - `EFER.SCE` (System Call Extensions) enabled via MSR `0xC000_0080`.
  - `STAR` MSR (`0xC000_0081`): Configured with `0x0008` (Kernel CS/SS base) and `0x0013` (User CS/SS base for `SYSRET`).
  - `LSTAR` MSR (`0xC000_0082`): Configured with the RIP of `syscall_entry`.
  - `FMASK` MSR (`0xC000_0084`): Configured with `0x200` to mask `IF` (Interrupt Flag) on syscall entry.
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
- [x] **Position-Independent Ring 3 Demo Program**:
  - Assembled user program (`user_test_program_start`) executing purely in Ring 3.
  - Successfully issues `SYS_WRITE`, `SYS_GETPID`, `SYS_YIELD`, and `SYS_EXIT(0)`.

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
[TASK] Spawned thread #3 "user_process" (stack: 16 KiB)
[USER] Configuring TSS RSP0 and switching: Ring 0 -> Ring 3 via iretq...
[USER RING 3] Hello from User Mode via SYSCALL!
[USER] User process finished with exit code: 0
[TASK] Thread #3 "user_process" finished (total ticks: 2).
[USER] Ring 3 User Mode and fast SYSCALL / SYSRET verified successfully!
```
