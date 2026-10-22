# Chapter 17: The C Standard Library (libc) & Userspace Runtime

> *"A kernel without a C runtime is an orchestra without musicians. The operating system provides the stage, the hardware, and the acoustics—but it is the C library that gives application programmers the instruments to compose real software."*

---

## 1. The Bridge Between Kernel and Userland

In **Phase 5 (M12, M13, M14)**, we implemented the hardware and software mechanisms for privilege separation:
- Transitioning the CPU from Ring 0 to Ring 3 via `iretq`.
- Catching system calls at the hardware boundary with the fast x86_64 `syscall` / `sysretq` instructions.
- Isolating each process in its own 4-level PML4 address space with a private user stack.

However, raw system calls are hostile to software development. A programmer writing an application should not have to manually write inline assembly with `%rax = 1`, `%rdi = 1`, and `syscall` just to print `"Hello, World!"`. They should not have to write custom page-mapping logic to allocate 64 bytes of memory.

This is the purpose of the **C Standard Library (`libc`)**:
1. **The C Runtime (`crt0.s`)**: The assembly startup code that receives control from the ELF loader, aligns the stack according to the System V AMD64 ABI, extracts `argc` and `argv`, invokes `int main(int argc, char **argv)`, and passes the return code to `exit(status)`.
2. **System Call Wrappers**: Thin C wrappers (`read`, `write`, `open`, `close`, `lseek`, `getpid`, `exit`) that marshal C arguments into machine registers and execute the `syscall` instruction.
3. **Standard String & Memory APIs (`string.h`)**: Fundamental primitives (`strlen`, `strcpy`, `strcmp`, `memcpy`, `memset`, `memmove`) implemented without relying on compiler builtins.
4. **Dynamic Heap Memory Allocator (`stdlib.h`)**: A robust block allocator (`malloc`, `free`, `calloc`, `realloc`) requesting 4 KiB virtual pages from the kernel via `SYS_MMAP` and managing free lists in userspace.
5. **Formatted Input/Output (`stdio.h`)**: Streams (`stdin`, `stdout`, `stderr`), character I/O, file streams (`fopen`, `fclose`, `fread`, `fwrite`), and formatted string generation (`printf`, `snprintf`, `vsnprintf`).

---

## 2. The C Runtime Startup (`crt0.s`)

When the kernel's ELF loader loads an executable and executes `iretq`, the CPU does **not** jump directly to `main()`. It jumps to the entry point specified in the ELF header: `_start`.

The System V AMD64 ABI imposes strict conventions upon `_start`:
- `%rsp + 8` must be a multiple of 16 prior to a `call` instruction (ensuring SSE/AVX vector instructions on the stack do not crash with alignment faults).
- The outermost stack frame base pointer (`%rbp`) must be zeroed to terminate backtraces.
- Command-line arguments (`argc`, `argv`) are positioned on the user stack.

```assembly
/*
 * user/libc/src/crt0.s - C Runtime Startup for NullOS x86_64 User Mode
 */

.section .text
.global _start
.type _start, @function

_start:
    # 1. Clear frame pointer to mark the outermost frame
    xorq %rbp, %rbp

    # 2. Extract argc and argv from stack
    movq (%rsp), %rdi         # %rdi = argc
    leaq 8(%rsp), %rsi        # %rsi = argv

    # 3. Align stack to 16 bytes before calling main
    andq $-16, %rsp
    subq $8, %rsp             # 8-byte padding for 16-byte alignment after 'call'

    # 4. Invoke user entry point: int main(int argc, char **argv)
    call main

    # 5. Call exit(status) with main's return value (in %rax)
    movq %rax, %rdi
    call exit

    # 6. Fallback hang loop if exit ever returns
.Lhang:
    movq $60, %rax            # SYS_EXIT = 60
    movq $1, %rdi             # exit status = 1
    syscall
    hlt
    jmp .Lhang

.size _start, . - _start
```

---

## 3. Fast System Call ABI & Inline Assembly

NullOS uses the standard 64-bit x86_64 System Call ABI:

| Register | Role in Syscall | C Calling Convention Register |
| :--- | :--- | :--- |
| **`RAX`** | System Call Number (`SYS_READ = 0`, `SYS_WRITE = 1`, etc.) | Return value |
| **`RDI`** | Argument 1 | Argument 1 |
| **`RSI`** | Argument 2 | Argument 2 |
| **`RDX`** | Argument 3 | Argument 3 |
| **`R10`** | Argument 4 *(Note: R10 instead of RCX; CPU destroys RCX with RIP)* | Argument 4 (`RCX` in SysV ABI) |
| **`R8`** | Argument 5 | Argument 5 |
| **`R9`** | Argument 6 | Argument 6 |
| **`RCX`** | User RIP (saved by hardware) | Clobbered |
| **`R11`** | User RFLAGS (saved by hardware) | Clobbered |

In `user/libc/include/sys/syscall.h`, we provide fast inline assembly macros that let the compiler place arguments directly into the designated hardware registers without memory overhead:

```c
static inline long syscall3(long n, long a1, long a2, long a3) {
    long ret;
    __asm__ __volatile__(
        "syscall"
        : "=a"(ret)
        : "a"(n), "D"(a1), "S"(a2), "d"(a3)
        : "rcx", "r11", "memory"
    );
    return ret;
}
```

Every standard POSIX function in `unistd.c` maps directly to these primitives:

```c
ssize_t write(int fd, const void *buf, size_t count) {
    return (ssize_t)syscall3(SYS_WRITE, fd, (long)buf, (long)count);
}

ssize_t read(int fd, void *buf, size_t count) {
    return (ssize_t)syscall3(SYS_READ, fd, (long)buf, (long)count);
}

int open(const char *pathname, int flags, ...) {
    return (int)syscall2(SYS_OPEN, (long)pathname, (long)flags);
}

off_t lseek(int fd, off_t offset, int whence) {
    return (off_t)syscall3(SYS_LSEEK, fd, (long)offset, (long)whence);
}
```

---

## 4. Userspace Dynamic Heap Allocator (`malloc` & `free`)

Applications rarely know their memory footprint at compile time. In a kernel without a virtual paging abstraction, programs must pre-allocate fixed arrays. In NullOS, our libc implements a true dynamic memory allocator powered by `SYS_MMAP`.

### The Boundary Tag Allocator Architecture
Each allocated and free memory chunk is prefixed by a 16-byte aligned header:

```c
typedef struct block_header {
    size_t size;                 /* Payload size in bytes */
    int is_free;                 /* 1 = Free, 0 = In Use */
    struct block_header *next;   /* Doubly linked list pointer */
    struct block_header *prev;
} block_header_t;
```

1. **Allocation (`malloc`)**:
   - Searches the free list for a block with `size >= requested`.
   - If found, it splits the block if the remaining capacity exceeds `sizeof(block_header_t) + 16` bytes.
   - If no free block fits, it invokes `mmap(NULL, chunk_size, PROT_READ | PROT_WRITE, MAP_ANONYMOUS, -1, 0)` to obtain a new 64 KiB chunk of physical memory from the kernel.
2. **Deallocation (`free`)**:
   - Marks the header `is_free = 1`.
   - **Immediate Coalescing**: Inspects the previous and next adjacent memory addresses. If contiguous blocks are also free, it merges them into a single larger block, preventing memory fragmentation!
3. **Reallocation (`realloc`)**:
   - If the existing block capacity is sufficient, returns the existing pointer.
   - Otherwise, allocates a new block, copies the old payload with `memcpy`, and frees the old block.

---

## 5. Formatted Stream I/O (`stdio.h`)

Our `stdio.c` provides full formatted printing without depending on any external host libraries:

- **`vsnprintf(char *buf, size_t size, const char *format, va_list ap)`**:
  - Parses format specifiers: `%s` (strings), `%c` (characters), `%d` / `%i` (signed integers), `%u` (unsigned integers), `%x` / `%X` (hexadecimal), `%p` (pointers), and `%%` (literal percent).
  - Handles minimum field widths (e.g. `%8d`) and zero-padding (e.g. `%02x` or `%016p`).
  - Supports length modifiers (`%ld`, `%llu`).
- **`printf(...)`**: Formats into a buffer and writes directly to file descriptor 1 (`stdout`).
- **File Streams (`FILE *`)**: Encapsulates open file descriptors with seek cursors, EOF detection, and error flags. `fopen`, `fclose`, `fread`, and `fwrite` provide a POSIX-compliant file API.

---

## 6. Standalone Executable Compilation & Linker Script

User programs must be completely decoupled from host Linux libc and runtime symbols.

### Compiler Flags
We compile user source files with:
```bash
gcc -Wall -Wextra -O2 -nostdlib -fno-builtin -fno-stack-protector -mno-red-zone -Ilibc/include -mcmodel=small -c app.c
```
- `-nostdlib`: Do not link default system startup files or standard libraries.
- `-fno-builtin`: Prevent the compiler from generating calls to glibc builtins.
- `-fno-stack-protector`: Avoid unresolved dependencies on `__stack_chk_fail`.
- `-mno-red-zone`: Ensures stack safety above `%rsp`.

### The Linker Script (`user/linker.ld`)
User mode executables in NullOS are linked at base virtual address `0x400000` (4 MiB):

```ld
OUTPUT_FORMAT("elf64-x86-64")
ENTRY(_start)

SECTIONS
{
    . = 0x00400000;

    .text : ALIGN(4K) {
        *(.text._start)
        *(.text .text.*)
    }

    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(4K) {
        *(.data .data.*)
    }

    .bss : ALIGN(4K) {
        *(COMMON)
        *(.bss .bss.*)
    }
}
```

The linker output is a standalone static 64-bit ELF binary:
```bash
ld -m elf_x86_64 -static -nostdlib -T linker.ld crt0.o app.o libc.a -o app.elf
```

---

## 7. Execution & Kernel Lifecycle

When an executable is launched via the shell (`exec /bin/hello.elf` or `exec /disk/HELLO.ELF`):

```
+-------------------------------------------------------------------------------+
|                       User Program Execution Lifecycle                        |
+-------------------------------------------------------------------------------+
| 1. VFS open & read ELF binary bytes from RamFS or Persistent FAT32 Disk       |
| 2. create_process("hello.elf", parent=INIT) -> dedicated AddressSpace (PML4)  |
| 3. load_user_elf: Map .text (R-E), .rodata (R--), .data/.bss (RW-) at 0x400000 |
| 4. Map user stack at 0x7000_0000 (4 KiB, USER_ACCESSIBLE | NO_EXECUTE)        |
| 5. Configure TSS RSP0 with kernel stack address                               |
| 6. enter_user_mode: iretq switches CS to 0x23 (Ring 3), SS to 0x1B, RIP 0x400000|
| 7. crt0.s _start calls main() -> executes user logic, printf, malloc, I/O     |
| 8. main() returns status -> calls exit() -> SYS_EXIT (syscall 60)             |
| 9. Kernel marks process Zombie(status) -> waitpid reaps child -> shell reports|
+-------------------------------------------------------------------------------+
```

### Verification Output:
```
[SHELL] Testing 'exec /disk/HELLO.ELF' (Persistent FAT32 Ring 3 C Binary)...

--------------------------------------------------
 [EXEC] Spawning User ELF Process: "HELLO.ELF"
--------------------------------------------------
[PROCESS] Created process #4 "HELLO.ELF" with dedicated AddressSpace (PML4 0x468000)
[EXEC] Process #4 "HELLO.ELF" ELF mapped: entry 0x400000, memory size 20504 bytes
[TASK] Spawned thread #6 "HELLO.ELF" (stack: 16 KiB)
[EXEC] Process #4 transitioning to Ring 3 (RIP 0x400000, RSP 0x70000ff0)...
====================================================
   Hello from Ring 3 C Userspace on NullOS!        
====================================================
[USER HELLO] Running in isolated process with PID: 4 (argc=0)
[USER HELLO] Testing strlen("NullOS Minimal Libc C Runtime") = 29 bytes
[USER HELLO] Testing strcpy & strcmp: MATCH SUCCESS (result=0)
[USER HELLO] Testing malloc() dynamic heap allocation...
[USER HELLO] Heap contents: Dynamic heap memory allocated at pointer 0x0000000060010020 via SYS_MMAP!
[USER HELLO] Memory freed successfully.
[USER HELLO] Testing file open & read on /disk/README.TXT...
[USER HELLO] Read 44 bytes from /disk/README.TXT:
  >> Welcome to NullOS Persistent FAT32 Storage!

====================================================
   Ring 3 C Userspace Program Completed Successfully!
====================================================
[USER] User process #4 finished with exit code: 0
[PROCESS] Process #4 "HELLO.ELF" exited with code: 0. Transitioning to Zombie state.
[TASK] Thread #6 "HELLO.ELF" finished (total ticks: 9).
[PROCESS] Parent #1 reaped zombie child #4, exit status: 0
[EXEC] User process #4 "HELLO.ELF" finished with exit status: 0
Process 'HELLO.ELF' exited with status 0
```

With **Phase 7: M21 — Minimal libc** complete, NullOS is no longer just a standalone kernel; it is a full-fledged operating platform capable of hosting real-world, high-level software written in C!
