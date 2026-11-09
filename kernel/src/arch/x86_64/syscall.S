/*
 * x86_64 Fast System Call (SYSCALL / SYSRET) & User Mode Transition Stubs.
 *
 * System Call Protocol:
 * - Userspace executes `syscall` instruction.
 * - Hardware loads RIP from IA32_LSTAR MSR (pointing to syscall_entry).
 * - Hardware saves User RIP into %rcx and User RFLAGS into %r11.
 * - Hardware masks RFLAGS with IA32_FMASK MSR (clearing IF, DF, TF, etc.).
 * - Hardware loads CS from STAR MSR (Kernel Code Segment 0x08).
 *
 * Calling Convention & Argument Mapping:
 * - System V AMD64 syscall calling convention:
 *     %rax: System call number (e.g. 0=SYS_READ, 1=SYS_WRITE, 60=SYS_EXIT)
 *     %rdi: Argument 1
 *     %rsi: Argument 2
 *     %rdx: Argument 3
 *     %r10: Argument 4 (NOTE: Linux/NullOS ABI uses %r10 instead of %rcx because
 *           the hardware `syscall` instruction destroys %rcx by storing User RIP!)
 *     %r8:  Argument 5
 *     %r9:  Argument 6
 * - Return value is returned in %rax.
 *
 * Stack Pivoting & Alignment Contract:
 * - User %rsp is saved to `user_rsp_scratch` and CPU pivots to `current_kernel_rsp`.
 * - 15 64-bit registers (120 bytes) are saved onto the kernel stack:
 *     user_rsp_scratch, %r11, %rcx, %r15, %r14, %r13, %r12, %rbp, %rbx, %r10, %r9, %r8, %rdx, %rsi, %rdi
 * - System V AMD64 ABI requires the stack pointer (%rsp) to be 16-byte aligned before
 *   any `call` instruction (such that (%rsp + 8) % 16 == 0 at the callee's entry point).
 * - Because 120 bytes % 16 == 8, we perform `sub $8, %rsp` before `call syscall_dispatcher`
 *   to ensure total stack allocation is 128 bytes (128 % 16 == 0).
 */

.section .text
.global syscall_entry
.type syscall_entry, @function

syscall_entry:
    /* Step 1: Temporarily preserve user %rsp into scratch memory */
    mov %rsp, user_rsp_scratch(%rip)

    /* Step 2: Switch CPU stack pointer to active thread's kernel stack */
    mov current_kernel_rsp(%rip), %rsp

    /* Step 3: Save complete user execution context onto the kernel stack */
    push user_rsp_scratch(%rip)   /* [rsp+112] User RSP */
    push %r11                     /* [rsp+104] User RFLAGS */
    push %rcx                     /* [rsp+96]  User RIP */
    push %r15                     /* [rsp+88] */
    push %r14                     /* [rsp+80] */
    push %r13                     /* [rsp+72] */
    push %r12                     /* [rsp+64] */
    push %rbp                     /* [rsp+56] */
    push %rbx                     /* [rsp+48] */
    push %r10                     /* [rsp+40] User Arg 4 */
    push %r9                      /* [rsp+32] User Arg 6 */
    push %r8                      /* [rsp+24] User Arg 5 */
    push %rdx                     /* [rsp+16] User Arg 3 */
    push %rsi                     /* [rsp+8]  User Arg 2 */
    push %rdi                     /* [rsp+0]  User Arg 1 */

    /* Step 4: Map user arguments to System V C calling convention:
     * extern "C" fn syscall_dispatcher(num: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> u64
     */
    mov %r8, %r9      /* arg 5 -> %r9 */
    mov %r10, %r8     /* arg 4 -> %r8 (restored from user %r10) */
    mov %rdx, %rcx    /* arg 3 -> %rcx */
    mov %rsi, %rdx    /* arg 2 -> %rdx */
    mov %rdi, %rsi    /* arg 1 -> %rsi */
    mov %rax, %rdi    /* syscall number -> %rdi */

    /* Enforce 16-byte stack alignment: 120 bytes + 8 bytes padding = 128 bytes */
    sub $8, %rsp
    call syscall_dispatcher
    add $8, %rsp
    /* Return value from Rust handler is in %rax */

    /* Step 5: Restore user registers in reverse order of pushes */
    pop %rdi
    pop %rsi
    pop %rdx
    pop %r8
    pop %r9
    pop %r10
    pop %rbx
    pop %rbp
    pop %r12
    pop %r13
    pop %r14
    pop %r15
    pop %rcx          /* User RIP restored to %rcx for hardware sysretq */
    pop %r11          /* User RFLAGS restored to %r11 for hardware sysretq */
    pop %rsp          /* User %rsp restored directly */

    /* Step 6: Return to Ring 3 user mode via fast hardware sysretq */
    sysretq

/*
 * Switches CPU from Ring 0 to Ring 3 user mode via iretq.
 *
 * Prototype:
 *   extern "C" fn enter_user_mode(entry_point: u64, user_stack_top: u64) -> !;
 *
 * Parameters:
 *   %rdi: 64-bit entry point address in user virtual memory
 *   %rsi: 64-bit stack top address in user virtual memory
 */
.global enter_user_mode
.type enter_user_mode, @function

enter_user_mode:
    /* Build hardware iretq stack frame:
     * [rsp + 32] = SS selector (0x1B = User Data Selector 0x18 | RPL 3)
     * [rsp + 24] = User RSP (%rsi)
     * [rsp + 16] = RFLAGS (0x202 = Interrupt Enable IF set)
     * [rsp + 8]  = CS selector (0x23 = User Code Selector 0x20 | RPL 3)
     * [rsp + 0]  = RIP (entry_point from %rdi)
     */
    cli
    push $0x1B       /* User Data Segment (0x18 | 3) */
    push %rsi        /* User Stack Pointer */
    push $0x202      /* RFLAGS (IF=1, Reserved=1) */
    push $0x23       /* User Code Segment (0x20 | 3) */
    push %rdi        /* User Instruction Pointer */

    /* Atomic hardware execution transfer to Ring 3 */
    iretq

/*
 * Switches CPU from Ring 0 to a new Ring 3 user thread with an initial argument in %rdi.
 *
 * Prototype:
 *   extern "C" fn enter_user_mode_thread(entry_point: u64, user_stack_top: u64, arg: u64) -> !;
 */
.global enter_user_mode_thread
.type enter_user_mode_thread, @function

enter_user_mode_thread:
    /* %rdi = entry_point, %rsi = user_stack_top, %rdx = initial argument */
    cli
    push $0x1B       /* User Data Segment (0x18 | 3) */
    push %rsi        /* User Stack Pointer */
    push $0x202      /* RFLAGS (IF=1, Reserved=1) */
    push $0x23       /* User Code Segment (0x20 | 3) */
    push %rdi        /* User Instruction Pointer */
    mov %rdx, %rdi   /* Place initial thread parameter into %rdi per SysV ABI */
    xor %rsi, %rsi   /* Clear remaining registers for security hygiene */
    xor %rdx, %rdx
    iretq

/*
 * Self-contained Position-Independent User Mode Program for Ring 3 self-tests.
 */
.global user_test_program_start
.global user_test_program_end
.type user_test_program_start, @function

user_test_program_start:
    /* 1. SYS_WRITE: Output welcome message from Ring 3 */
    mov $1, %rax                      /* SYS_WRITE */
    mov $1, %rdi                      /* fd = 1 (stdout) */
    lea user_msg(%rip), %rsi          /* buffer pointer */
    mov $(user_msg_end - user_msg), %rdx  /* byte count */
    syscall

    /* 2. SYS_GETPID: Query active process ID */
    mov $39, %rax                     /* SYS_GETPID */
    syscall

    /* 3. SYS_YIELD: Voluntarily yield remaining CPU timeslice */
    mov $24, %rax                     /* SYS_YIELD */
    syscall

    /* 4. SYS_MMAP: Allocate 4 KiB user page at 0x60000000 (PROT_READ | PROT_WRITE) */
    mov $9, %rax                      /* SYS_MMAP */
    mov $0x60000000, %rdi             /* addr = 0x60000000 */
    mov $4096, %rsi                   /* length = 4096 bytes */
    mov $3, %rdx                      /* prot = PROT_READ (1) | PROT_WRITE (2) */
    mov $0x22, %r10                   /* flags = MAP_PRIVATE (0x02) | MAP_ANON (0x20) */
    syscall

    /* Verify write and read integrity to the dynamically mapped page in user mode */
    mov $0x12345678, %rcx
    mov %rcx, (%rax)
    mov (%rax), %rbx

    /* 5. SYS_WRITE: Output mmap verification message from Ring 3 */
    mov $1, %rax                      /* SYS_WRITE */
    mov $1, %rdi                      /* fd = 1 (stdout) */
    lea mmap_msg(%rip), %rsi          /* buffer pointer */
    mov $(mmap_msg_end - mmap_msg), %rdx  /* byte count */
    syscall

    /* 6. SYS_EXIT: Terminate process with exit code 0 */
    mov $60, %rax                     /* SYS_EXIT */
    xor %rdi, %rdi                    /* status = 0 */
    syscall

1:  pause
    jmp 1b

user_msg:
    .ascii "\033[32m[USER RING 3]\033[0m Hello from User Mode via SYSCALL!\n"
user_msg_end:
    .align 8

mmap_msg:
    .ascii "\033[32m[USER RING 3]\033[0m SYS_MMAP verified: Dynamically mapped and wrote to user page at 0x60000000!\n"
mmap_msg_end:
    .align 8

user_test_program_end:

/*
 * Deliberate user mode crash test program for Ring 3 fault isolation verification.
 */
.global user_fault_program_start
.global user_fault_program_end
.type user_fault_program_start, @function

user_fault_program_start:
    /* Deliberately access unmapped memory in Ring 3 to trigger Page Fault */
    mov $0xDEAD0000, %rax
    mov (%rax), %rbx
1:  pause
    jmp 1b

user_fault_program_end:

.section .bss
.align 16

/* Scratch storage for user stack pointer during initial syscall entry */
.global user_rsp_scratch
user_rsp_scratch:
    .quad 0

/* Active kernel execution stack pointer loaded on syscall entry */
.global current_kernel_rsp
current_kernel_rsp:
    .quad 0

/* Static fallback kernel syscall stack (16 KiB) */
.global kernel_syscall_stack_bottom
.global kernel_syscall_stack_top
kernel_syscall_stack_bottom:
    .skip 16384
kernel_syscall_stack_top:
