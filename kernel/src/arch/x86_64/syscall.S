/*
 * x86_64 Fast System Call (SYSCALL / SYSRET) & User Mode Transition Stubs.
 */

.section .text
.global syscall_entry
.type syscall_entry, @function

syscall_entry:
    # 1. Save user %rsp into scratch variable
    mov %rsp, user_rsp_scratch(%rip)

    # 2. Switch to the active thread's kernel stack
    mov current_kernel_rsp(%rip), %rsp

    # 3. Save user execution context on kernel stack:
    # Save user %rsp, %r11 (user RFLAGS), and %rcx (user RIP)
    push user_rsp_scratch(%rip)
    push %r11
    push %rcx
    push %r15
    push %r14
    push %r13
    push %r12
    push %rbp
    push %rbx
    push %rdx
    push %rsi
    push %rdi

    # 4. Map registers to SysV C calling convention for:
    # extern "C" fn syscall_dispatcher(num: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> u64
    mov %r8, %r9      # arg 5 -> %r9
    mov %r10, %r8     # arg 4 -> %r8
    mov %rdx, %rcx     # arg 3 -> %rcx
    mov %rsi, %rdx     # arg 2 -> %rdx
    mov %rdi, %rsi     # arg 1 -> %rsi
    mov %rax, %rdi     # num   -> %rdi

    call syscall_dispatcher
    # Return value is in %rax

    # 5. Restore user registers
    pop %rdi
    pop %rsi
    pop %rdx
    pop %rbx
    pop %rbp
    pop %r12
    pop %r13
    pop %r14
    pop %r15
    pop %rcx          # User RIP restored to %rcx for sysretq
    pop %r11          # User RFLAGS restored to %r11 for sysretq
    pop %rsp          # User %rsp restored directly

    # 6. Return to Ring 3 user mode!
    sysretq

/*
 * Switches CPU from Ring 0 to Ring 3 user mode via iretq.
 * Prototype:
 *   extern "C" fn enter_user_mode(entry_point: u64, user_stack_top: u64) -> !;
 */
.global enter_user_mode
.type enter_user_mode, @function

enter_user_mode:
    # %rdi = entry_point (u64)
    # %rsi = user_stack_top (u64)

    # Build iretq stack frame:
    # [rsp + 32] = SS selector (0x1B = User Data | RPL 3)
    # [rsp + 24] = User RSP (%rsi)
    # [rsp + 16] = RFLAGS (0x202 = IF enabled)
    # [rsp + 8]  = CS selector (0x23 = User Code | RPL 3)
    # [rsp + 0]  = RIP (entry_point)

    cli
    push $0x1B       # User Data Selector (0x18 | 3)
    push %rsi        # User Stack Top
    push $0x202      # RFLAGS with Interrupts Enabled
    push $0x23       # User Code Selector (0x20 | 3)
    push %rdi        # User RIP

    iretq

/*
 * Self-contained Position-Independent User Mode Program for Ring 3 testing.
 */
.global user_test_program_start
.global user_test_program_end
.type user_test_program_start, @function

user_test_program_start:
    # 1. SYS_WRITE: Print welcome message from Ring 3
    mov $1, %rax                      # SYS_WRITE
    mov $1, %rdi                      # fd = 1 (stdout)
    lea user_msg(%rip), %rsi          # pointer to string
    mov $(user_msg_end - user_msg), %rdx  # length
    syscall

    # 2. SYS_GETPID: Query PID
    mov $39, %rax                     # SYS_GETPID
    syscall

    # 3. SYS_YIELD: Voluntarily yield CPU
    mov $24, %rax                     # SYS_YIELD
    syscall

    # 4. SYS_MMAP: Allocate 4 KiB user page at 0x60000000 (PROT_READ | PROT_WRITE)
    mov $9, %rax                      # SYS_MMAP
    mov $0x60000000, %rdi             # addr = 0x60000000
    mov $4096, %rsi                   # length = 4096
    mov $3, %rdx                      # prot = PROT_READ (1) | PROT_WRITE (2)
    mov $0x22, %r10                   # flags = MAP_PRIVATE (0x02) | MAP_ANON (0x20)
    syscall

    # Verify write and read to the dynamically mapped page in user mode
    mov $0x12345678, %rcx
    mov %rcx, (%rax)
    mov (%rax), %rbx

    # 5. SYS_WRITE: Print mmap verification message from Ring 3
    mov $1, %rax                      # SYS_WRITE
    mov $1, %rdi                      # fd = 1 (stdout)
    lea mmap_msg(%rip), %rsi          # pointer to string
    mov $(mmap_msg_end - mmap_msg), %rdx  # length
    syscall

    # 6. SYS_EXIT: Terminate process with exit code 0
    mov $60, %rax                     # SYS_EXIT
    xor %rdi, %rdi                    # status = 0
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
 * Deliberate user mode crash test program.
 */
.global user_fault_program_start
.global user_fault_program_end
.type user_fault_program_start, @function

user_fault_program_start:
    # Deliberately access unmapped address in Ring 3 to trigger Page Fault
    mov $0xDEAD0000, %rax
    mov (%rax), %rbx
1:  pause
    jmp 1b

user_fault_program_end:

.section .bss
.align 16

.global user_rsp_scratch
user_rsp_scratch:
    .quad 0

.global current_kernel_rsp
current_kernel_rsp:
    .quad 0

.global kernel_syscall_stack_bottom
.global kernel_syscall_stack_top
kernel_syscall_stack_bottom:
    .skip 16384     # 16 KiB fallback kernel syscall stack
kernel_syscall_stack_top:
