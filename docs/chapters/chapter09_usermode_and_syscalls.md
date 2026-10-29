# Chapter 9: User Mode (Ring 3), System Calls & Crash Resilience
### *The Ultimate Frontier: Privilege Separation, Fast Hardware SYSCALL, and Process Isolation*

In the previous chapters, every line of code executed in **Ring 0 (Supervisor Mode)**. The kernel had unlimited, god-mode access to the hardware.

In this chapter, we step across the most important line in computer science: **The User/Kernel Barrier**.

We will explore how the kernel uses `iretq` to drop privilege down to **Ring 3 (User Mode)**, how modern CPUs execute system calls at hardware speed via **`SYSCALL`/`SYSRET`**, how **Process Address Spaces** isolate memory, and how **User-Mode Crash Handling** prevents a buggy or crashed application from ever crashing the operating system.

---

## 1. Why Can't We Just `jmp` to User Code?

Suppose you compile a user application into memory at address `0x0040_0000`.
Why can't the kernel simply execute:
```assembly
jmp 0x00400000
```

Because **a normal jump instruction does NOT change the CPU's Current Privilege Level (CPL)!**
- If the CPU was in Ring 0, executing `jmp` keeps the CPU in Ring 0.
- The user program would run with full kernel privileges and could wipe your hard drive, disable interrupts, or overwrite page tables.

The x86_64 CPU hardware strictly forbids arbitrary privilege changes. There are only two ways the CPU allows changing privilege:
1. Returning from an interrupt via **`iretq`**.
2. Executing the fast system call return instruction: **`sysretq`**.

---

## 2. The `iretq` Privilege Drop (Ring 0 $\rightarrow$ Ring 3)

To transition into User Mode for the first time, we use a clever architectural technique: **The Fake Interrupt Stack Frame** ([kernel/src/arch/x86_64/syscall.S](../../kernel/src/arch/x86_64/syscall.S#L65-L87)).

Normally, `iretq` is used to return from an interrupt. The CPU pops 5 values from the stack: `RIP`, `CS`, `RFLAGS`, `RSP`, `SS`.

**The Kernel's Trick:**
We manually push five crafted values onto the stack before running `iretq`:

```
Top of Stack -> [ rsp + 0  ] = Entry Point Address (0x0040_0000 - User RIP)
                [ rsp + 8  ] = User Code Segment: 0x23 (Index 4, DPL 3, RPL 3)
                [ rsp + 16 ] = RFLAGS: 0x202 (Interrupts Enabled)
                [ rsp + 24 ] = User Stack Top (0x7000_0FF0 - User RSP)
                [ rsp + 32 ] = User Data Segment: 0x1B (Index 3, DPL 3, RPL 3)
```

### The Assembly Primitive (`enter_user_mode`):
```assembly
.global enter_user_mode
enter_user_mode:
    # %rdi = entry_point (0x0040_0000)
    # %rsi = user_stack_top (0x7000_0FF0)

    cli              # Disable interrupts during frame assembly
    push $0x1B       # Push User Data Selector (SS = 0x18 | 3)
    push %rsi        # Push User Stack Pointer (RSP)
    push $0x202      # Push RFLAGS with Interrupt Flag enabled
    push $0x23       # Push User Code Selector (CS = 0x20 | 3)
    push %rdi        # Push User Instruction Pointer (RIP)

    iretq            # THE SWITCH HAPPENS HERE!
```

### What the CPU Does When `iretq` Executes:
1. Pops `RIP` $\rightarrow$ loads `0x0040_0000` into `%rip`.
2. Pops `CS` $\rightarrow$ inspects the lowest two bits of `0x23` (`RPL = 3`).
   **The CPU hardware changes its internal CPL state to Ring 3!**
3. Pops `RFLAGS` $\rightarrow$ enables interrupts.
4. Pops `RSP` $\rightarrow$ sets the user stack pointer.
5. Pops `SS` $\rightarrow$ loads user data selector `0x1B`.

The CPU is now executing in **sandboxed User Mode**! If this code tries to execute a privileged instruction like `cli` or `mov cr3, rax`, the CPU instantly halts it with a General Protection Fault (`#GP`).

---

## 3. Fast System Calls: `SYSCALL` & `SYSRET`

Now that your user application is sandboxed in Ring 3, how does it print text, read files, or allocate memory?
It must ask the kernel for help via a **System Call**.

### Why Not Use `int 0x80`?
In older 32-bit Linux, programs invoked system calls via the software interrupt `int 0x80`.
However, software interrupts are slow on modern CPUs:
- The CPU flushes pipeline stages.
- It performs memory table lookups in the IDT.
- It checks gate descriptors and privilege levels.
- This takes **hundreds of CPU cycles**.

Modern x86_64 CPUs introduce two dedicated hardware instructions: **`syscall`** and **`sysretq`**.
In **one instruction**, the CPU transitions from Ring 3 to Ring 0 at lightning speed!

---

### Hardware MSR Configuration (The Setup)
To enable `syscall`, the kernel configures Model Specific Registers (MSRs) during boot ([kernel/src/arch/x86_64/syscall.rs](../../kernel/src/arch/x86_64/syscall.rs)):

1. **`IA32_EFER` (MSR `0xC000_0080`)**: Set Bit 0 (`SCE` - System Call Extension) to `1`.
2. **`STAR` MSR (`0xC000_0081`)**: Tells the CPU which GDT segment selectors to load:
   - Bits 47:32 = Kernel CS (`0x08`).
   - Bits 63:48 = User CS/SS base (`0x10`). On `sysretq`, the CPU automatically loads User SS = `0x18 | 3 = 0x1B` and User CS = `0x20 | 3 = 0x23`!
3. **`LSTAR` MSR (`0xC000_0082`)**: Stores the 64-bit address of our kernel's assembly entry point: `syscall_entry`.
4. **`FMASK` MSR (`0xC000_0084`)**: Masks condition flags. Configured with `0x300` to automatically clear `RFLAGS.IF` (disabling interrupts atomically on syscall entry).

---

### What Happens When User Code Runs `syscall`:
When user space executes `syscall`:
1. The CPU saves user `%rip` into register **`%rcx`**.
2. The CPU saves user `%rflags` into register **`%r11`**.
3. The CPU loads `%cs` and `%ss` with Kernel Selectors (`0x08` and `0x10`), changing privilege to **Ring 0**.
4. The CPU jumps directly to the address stored in `LSTAR` (`syscall_entry`)!

---

### The `syscall_entry` Assembly Stub ([syscall.S](../../kernel/src/arch/x86_64/syscall.S)):
```assembly
syscall_entry:
    # 1. Save user RSP into scratch variable
    mov %rsp, user_rsp_scratch(%rip)

    # 2. Switch to dedicated kernel syscall stack!
    lea kernel_syscall_stack_top(%rip), %rsp

    # 3. Save user context: user RSP, %r11 (user RFLAGS), %rcx (user RIP)
    push user_rsp_scratch(%rip)
    push %r11
    push %rcx
    push %r15
    push %r14
    # ... (push remaining general-purpose registers)

    # 4. Map registers to SysV C calling convention for Rust:
    # extern "C" fn syscall_dispatcher(num, a1, a2, a3, a4, a5) -> u64
    mov %r8, %r9      # arg 5 -> %r9
    mov %r10, %r8     # arg 4 -> %r8
    mov %rdx, %rcx     # arg 3 -> %rcx
    mov %rsi, %rdx     # arg 2 -> %rdx
    mov %rdi, %rsi     # arg 1 -> %rsi
    mov %rax, %rdi     # syscall number -> %rdi

    call syscall_dispatcher
    # Return value is placed in %rax

    # 5. Restore user registers
    pop %rcx          # User RIP restored to %rcx for sysretq
    pop %r11          # User RFLAGS restored to %r11 for sysretq
    pop %rsp          # User RSP restored

    # 6. Return to Ring 3 User Mode!
    sysretq
```

---

## 4. The NullOS System Call ABI

In NullOS ([kernel/src/task/syscall.rs](../../kernel/src/task/syscall.rs)), user applications invoke system calls using these standardized numbers:

| Number | Name | Arguments | Description |
| :---: | :--- | :--- | :--- |
| **`0`** | `SYS_READ` | `fd, buf, len` | Read input from console/device. |
| **`1`** | `SYS_WRITE` | `fd, buf, len` | Write buffer text to the console/serial port. |
| **`2`** | `SYS_EXIT` | `status` | Terminate the process cleanly. |
| **`3`** | `SYS_YIELD` | *(none)* | Voluntarily yield the CPU timeslice. |
| **`4`** | `SYS_GETPID` | *(none)* | Return current process/thread ID. |
| **`5`** | `SYS_SLEEP` | `ticks` | Put the process to sleep for $N$ timer ticks. |
| **`9`** | `SYS_MMAP` | `addr, len, prot, flags` | Dynamically allocate and map user memory pages. |

---

## 5. Process Address Space Isolation (`AddressSpace`)

How do we guarantee that Process A cannot spy on Process B?
In NullOS ([kernel/src/memory/address_space.rs](../../kernel/src/memory/address_space.rs)), every process receives its own dedicated **`AddressSpace`**:

```
+=============================================================================+
|                      PROCESS VIRTUAL ADDRESS SPACE (256 TiB)                |
|                                                                             |
|  [0x0000_0000_0000_0000 .. 0x0000_7FFF_FFFF_FFFF] (Lower Half - 128 TiB)    |
|   PRIVATE TO THIS PROCESS!                                                  |
|   - Code (0x0040_0000)                                                      |
|   - Data / BSS                                                              |
|   - Dynamic Mapped Pages (0x6000_0000 via SYS_MMAP)                         |
|   - Stack (0x7000_0000)                                                     |
|                                                                             |
|-----------------------------------------------------------------------------|
|                                                                             |
|  [0xFFFF_8000_0000_0000 .. 0xFFFF_FFFF_FFFF_FFFF] (Higher Half - 128 TiB)   |
|   SHARED KERNEL SPACE (Identical in every process!)                         |
|   - Kernel code and data segments                                           |
|   - Kernel Heap (0x4444_4444_0000)                                          |
|   - Local APIC MMIO (0xFEE0_0000)                                           |
|   - Framebuffer Video Memory (0x8000_0000)                                  |
|   - Recursive Page Mapping at PML4[511]                                     |
+=============================================================================+
```

### Why Must the Kernel Be Mapped in Every Process?
When a user application executes `syscall` or an interrupt occurs, the CPU jumps to the kernel's code **without switching CR3**. If the kernel were not mapped in the higher half, the CPU would encounter a missing page and immediately crash!

---

## 6. User-Mode Crash Handling: Resilience

What happens if a user application has a bug (e.g. divides by zero, executes an invalid instruction, or accesses an unmapped pointer `0xDEAD0000`)?

In older or naive kernels, any unhandled exception halted the entire operating system.

In NullOS, we implemented **User-Mode Crash Resilience** ([kernel/src/arch/x86_64/idt.rs](../../kernel/src/arch/x86_64/idt.rs#L188-L200)):
When an exception arrives:
```rust
// Check if the fault occurred in Ring 3 User Mode (CPL == 3)
if (ctx.cs & 3) == 3 {
    crate::println!();
    crate::println!(
        "\x1b[31;1m[USER CRASH]\x1b[0m User mode process crashed with {} (vector {}) at RIP: {:#018x}!",
        name,
        ctx.vector,
        ctx.rip
    );
    crate::println!("\x1b[33m[USER CRASH]\x1b[0m Terminating faulting user thread cleanly and continuing OS execution...");
    
    // Cleanly terminate the user thread and switch to the next ready task!
    crate::task::exit();
}
```

The faulting application is cleanly killed, its resources are freed, and the rest of the operating system continues running with zero downtime!

---

## 🧠 Self-Test & Review Questions

1. **Why does the `iretq` instruction enable dropping CPU privilege to Ring 3, whereas a normal `jmp` cannot?**
   <details>
   <summary>Click to view answer</summary>
   The x86_64 CPU hardware checks the Requested Privilege Level (RPL) in the Code Segment selector popped during <code>iretq</code>. If the RPL is 3, the CPU's internal microcode updates its Current Privilege Level (CPL) to 3. Normal <code>jmp</code> instructions do not modify CPL.
   </details>

2. **Why does `syscall_entry` immediately switch the stack pointer (`%rsp`) to a dedicated kernel stack?**
   <details>
   <summary>Click to view answer</summary>
   When user mode invokes <code>syscall</code>, <code>%rsp</code> is still pointing to the user application's stack. The kernel cannot trust this stack (it could be exhausted, corrupted, unmapped, or monitored by an exploit). Switching to <code>kernel_syscall_stack_top</code> ensures the kernel executes with a guaranteed, private, secure stack.
   </details>

3. **In 64-bit mode, where does the `syscall` instruction save the user program's return instruction address (`RIP`), and where does it save `RFLAGS`?**
   <details>
   <summary>Click to view answer</summary>
   The <code>syscall</code> hardware instruction automatically saves the user's return <code>RIP</code> into register <strong><code>%rcx</code></strong>, and saves the user's <code>RFLAGS</code> into register <strong><code>%r11</code></strong>.
   </details>

---

*In [Chapter 10: Bare-Metal Hardware & Real PC Deployment](chapter10_bare_metal_and_debugging.md), we explore how to test your OS on physical computers, navigate BIOS quirks, and deploy via USB.*
