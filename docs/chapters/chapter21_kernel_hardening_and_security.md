# Chapter 21: Operating System Security & Kernel Hardening

### *Architecting Defense-in-Depth: SMEP, SMAP, Write XOR Execute ($W \oplus X$), User Boundary Sanitization, Stack Canaries, and Memory Isolation*

---

## 1. The Threat Model: Operating Systems as the Ultimate Attack Target

An operating system kernel is the most privileged software component in a computer. Operating at **Ring 0 (Supervisor Privilege)**, the kernel has direct, unrestricted access to physical memory, hardware I/O ports, CPU control registers, and peripheral DMA controllers.

Conversely, user-space applications (Ring 3) are inherently untrusted. A malicious user application, a compromised web browser, or an exploited network daemon will attempt to break through the privilege boundary to achieve **Privilege Escalation**—forcing the kernel to execute arbitrary instructions in Ring 0.

```
       +-------------------------------------------------------------+
       |             UNTRUSTED USER SPACE (Ring 3)                   |
       |  Exploited Process / Malicious User / Injected Shellcode    |
       +-------------------------------------------------------------+
                                      │
                   [ SYSCALL / SYSRET / Interrupts ]
                                      │
                                      ▼
       +-------------------------------------------------------------+
       |                  KERNEL HARDENING SHIELD                    |
       |  ├── Pointer Boundary Checks (validate_user_buffer)         |
       |  ├── SMEP (Hardware Execution Prevention of User Code)      |
       |  ├── SMAP (Hardware Access Prevention of User Memory)       |
       |  ├── W^X Policy (No page simultaneously Writable & Exec)    |
       |  └── Stack Canaries (Detection of return address smashing)  |
       +-------------------------------------------------------------+
                                      │
                                      ▼
       +-------------------------------------------------------------+
       |              PROTECTED KERNEL CORE (Ring 0)                 |
       |  CR0.WP, CR3 Kernel Page Tables, Device Drivers, Scheduler  |
       +-------------------------------------------------------------+
```

Historically, operating systems suffered from catastrophic vulnerability classes:
1. **Return-to-User (ret2usr)**: An attacker corrupts a kernel function pointer or return address to point to malicious code mapped in user memory. Because the CPU was in Ring 0, legacy processors would happily fetch and execute instructions located in user space with full kernel privileges!
2. **Confused Deputy / Kernel Pointer Confusion**: An attacker passes a kernel memory address (e.g. `0xFFFF_8000_...`) as the buffer argument to `sys_read(fd, buf, len)`. If the kernel fails to validate the pointer, the filesystem driver reads disk sectors directly into critical kernel data structures.
3. **Stack Smashing**: A buffer overflow on the kernel stack overwrites the saved instruction pointer (`%rip`), hijacking control flow upon function return.
4. **Code Injection ($W \land X$)**: Writing shellcode into memory (e.g. stack, heap) and executing it.

NullOS Phase 10 implements **defense-in-depth** across hardware CPU features and software boundaries to eliminate these entire classes of attack.

---

## 2. Hardware-Enforced Silicon Defenses: SMEP & SMAP

Modern x86_64 silicon provides dedicated hardware protection mechanisms built directly into the CPU Memory Management Unit (MMU) and control registers.

```
CR4 Control Register:
+----+-----+-----+-----+-----------------------------------+
| .. | SMAP| SMEP| ..  | PGE | OSFXSR | OSXMMEXCPT | ..   |
|    |Bit21|Bit20|     |Bit 7| Bit 9  |   Bit 10   |      |
+----+-----+-----+-----+-----------------------------------+
```

### 2.1 Supervisor Mode Execution Prevention (SMEP)
**SMEP** is enabled by setting bit 20 in the `CR4` control register (`CR4.SMEP = 1 << 20`).

When SMEP is active, the silicon CPU MMU continuously monitors instruction fetching. If the CPU is executing in **Supervisor Mode (Ring 0)** and attempts to fetch an instruction from any virtual page marked with the `USER_ACCESSIBLE` flag (`U/S = 1` in page table entries):
- The CPU immediately halts instruction fetch.
- An instant **Page Fault (`#PF`)** exception is raised.
- The Page Fault error code sets bit 4 (`I/D = 1`, instruction fetch violation) with `CPL = 0`.

**Impact**: Even if an attacker successfully hijacks a kernel function pointer, pointing that pointer to user-space shellcode immediately crashes the system rather than executing the attacker's payload.

### 2.2 Supervisor Mode Access Prevention (SMAP)
**SMEP** prevents *executing* user memory, but what if a kernel vulnerability tricks Ring 0 into *reading* or *writing* user memory inadvertently?

**SMAP** is enabled by setting bit 21 in `CR4` (`CR4.SMAP = 1 << 21`).

When SMAP is active, Ring 0 is forbidden from reading or writing any user-space virtual memory page (`U/S = 1`) unless the **Alignment Check (`AC`) flag** in `%rflags` is explicitly asserted.

To facilitate authorized, controlled memory transfers between user and kernel space, x86 provides two specialized assembly instructions:
- **`stac` (Set AC Flag)**: Temporarily allows Ring 0 access to user-space pages.
- **`clac` (Clear AC Flag)**: Re-engages the lock, blocking all further user memory access.

In NullOS, `stac` and `clac` are wrapped in low-level primitives in [`kernel/src/arch/x86_64/cpu.rs`](../../kernel/src/arch/x86_64/cpu.rs):
```rust
#[inline(always)]
pub unsafe fn stac() {
    core::arch::asm!("stac", options(nomem, nostack));
}

#[inline(always)]
pub unsafe fn clac() {
    core::arch::asm!("clac", options(nomem, nostack));
}
```

---

## 3. The Write XOR Execute ($W \oplus X$) Memory Policy

The **$W \oplus X$** (Write XOR Execute) principle dictates that no memory page should ever be simultaneously **Writable** and **Executable**.

```
                   Page Permissions Matrix:
+-------------------+---------+-----------+--------------------+
| Memory Region     | Writable| Executable| Classification     |
+-------------------+---------+-----------+--------------------+
| Kernel Code (.text|   NO    |    YES    | Read-Only Code     |
| Kernel Data/BSS   |   YES   |    NO     | Data (No-Execute)  |
| Kernel Heap       |   YES   |    NO     | Dynamic Data (NX)  |
| Kernel Stack      |   YES   |    NO     | Call Frames (NX)   |
| Framebuffer MMIO  |   YES   |    NO     | Memory-Mapped (NX) |
+-------------------+---------+-----------+--------------------+
```

### Silicon Enforcement: EFER.NXE and CR0.WP
1. **`CR0.WP` (Write-Protect, Bit 16)**:
   In legacy x86 processors, the CPU allowed Ring 0 to write to read-only pages! By enabling `CR0.WP`, the CPU enforces read-only page permissions strictly on Ring 0. Attempting to overwrite `.text` or read-only tables triggers `#PF`.
2. **`EFER.NXE` (No-Execute Enable, MSR `0xC0000080` Bit 11)**:
   Enables bit 63 (`NO_EXECUTE`) in 64-bit page table entries. Any attempt to fetch instructions from stack, heap, or MMIO pages triggers an immediate Page Fault abort.

---

## 4. Safe User Memory Transfer & Pointer Sanitization

In Unix-like systems, system calls such as `read()`, `write()`, and `open()` accept user-space pointers. If a kernel blindly dereferences these raw pointers (`*ptr`), the kernel becomes a **confused deputy**.

### 4.1 Canonical Address Boundary
x86_64 defines a 48-bit canonical virtual address space divided into two halves:
- **Lower Half (User Space)**: `0x0000_0000_0000_0000` to `0x0000_7FFF_FFFF_FFFF`
- **Upper Half (Kernel Space)**: `0xFFFF_8000_0000_0000` to `0xFFFF_FFFF_FFFF_FFFF`

Any pointer with address $\ge$ `0x0000_8000_0000_0000` belongs to kernel space or non-canonical space!

### 4.2 The Validation Primitive: `validate_user_buffer`
NullOS implements strict buffer verification in [`kernel/src/security/usercopy.rs`](../../kernel/src/security/usercopy.rs):
```rust
pub const USER_SPACE_CEILING: u64 = 0x0000_8000_0000_0000;

pub fn validate_user_buffer(ptr: *const u8, len: usize) -> Result<(), SecurityError> {
    if ptr.is_null() {
        return Err(SecurityError::NullPointer);
    }

    let addr = ptr as u64;

    // 1. Reject pointers residing in kernel space
    if addr >= USER_SPACE_CEILING {
        return Err(SecurityError::PointerAboveUserSpace);
    }

    // 2. Reject integer overflow in addr + len
    let end_addr = addr.checked_add(len as u64)
        .ok_or(SecurityError::AddressOverflow)?;

    // 3. Reject buffers extending across the boundary into kernel space
    if end_addr > USER_SPACE_CEILING {
        return Err(SecurityError::BufferExceedsUserSpace);
    }

    Ok(())
}
```

### 4.3 Safe Memory Copy Routines
All user-kernel transfers pass through `copy_from_user` and `copy_to_user`, which wrap the transfer with SMAP protection:
```rust
pub fn copy_from_user(dst: &mut [u8], src: *const u8) -> Result<(), SecurityError> {
    validate_user_buffer(src, dst.len())?;

    unsafe {
        cpu::stac(); // Enable user memory access
        core::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), dst.len());
        cpu::clac(); // Disable user memory access
    }

    Ok(())
}
```

---

## 5. Stack Hardening: Canaries & Guard Pages

Stack buffer overflows remain one of the oldest attack vectors in systems programming. NullOS implements two complementary layers of stack defense:

```
        High Memory (Stack Top)
       +------------------------------------+
       | Return Address (%rip)              |
       | Saved Base Pointer (%rbp)          |
       | Local Variables                    |
       | Buffer [ . . . . . . . . . . . ]   |
       +------------------------------------+
       | STACK CANARY (0xDEADC0DE_FEEDFACE) | ◄── Guard against buffer overflow
       +------------------------------------+
       | Stack Limit (Bottom)               |
       +------------------------------------+
       | 4 KiB UNMAPPED GUARD PAGE (P=0)    | ◄── Instant #PF on stack exhaustion
       +------------------------------------+
        Low Memory (Downwards Stack Growth)
```

### 5.1 Stack Canaries (Stack Smashing Protection)
A **canary** is a known 64-bit secret value placed immediately between local stack variables and control data (saved `%rbp` and `%rip`).
- If an uncontrolled buffer write overflows the local buffer, it must overwrite the canary before it can reach the return address.
- Before returning, the function verifies that the canary value is intact. If modified, the kernel immediately halts execution via `__stack_chk_fail()`:
```rust
#[unsafe(no_mangle)]
pub extern "C" fn __stack_chk_fail() -> ! {
    crate::println!("\n!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
    crate::println!("   FATAL SECURITY ALERT: STACK SMASHING DETECTED! ");
    crate::println!("   Stack canary corruption detected in execution. ");
    crate::println!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
    loop {
        unsafe { core::arch::asm!("cli; hlt"); }
    }
}
```

### 5.2 Stack Guard Pages
Even without an active buffer overflow, deeply nested recursion or large stack allocations can cause **Stack Exhaustion (Stack Overflow)**.
- If stacks are allocated adjacent to heap memory or other thread stacks, a stack overflow silently corrupts adjacent data!
- By leaving a **4 KiB unmapped page (`Present = 0`)** directly below the stack boundary, the very first instruction that pushes past the stack limit triggers an unhandled Page Fault `#PF`. The kernel traps the condition instantly instead of suffering silent corruption.

---

## 6. Interactive Shell Security Diagnostics

NullOS provides the `sec` (or `hardening`) command in its interactive Unix shell `nsh`:

```
nullos> sec
SECURITY FEATURE               STATUS         
----------------------------------------------
CR0.WP (Kernel Write Protect)  Enforced       
EFER.NXE (No-Execute Pages)    Enforced       
CR4.SMEP (Exec Prevention)     Active         
CR4.SMAP (Access Prevention)   Supported      
W^X Memory Permission Policy   Enforced       
Stack Smashing Canaries        Active         
User Pointer Sanitization      Enforced       
```

---

## 7. Summary & Best Practices

| Threat Vector | Mitigation Strategy | Silicon / Software Mechanism |
| :--- | :--- | :--- |
| **ret2usr** | Supervisor Mode Execution Prevention | `CR4.SMEP = 1 << 20` |
| **Confused Deputy** | Supervisor Mode Access Prevention | `CR4.SMAP = 1 << 21`, `stac`, `clac` |
| **Kernel Overwrite** | Ring 0 Write Protection | `CR0.WP = 1 << 16` |
| **Shellcode Injection** | No-Execute Page Table Flag | `EFER.NXE = 1 << 11`, PTE bit 63 |
| **Address Spoofing** | Boundary Sanitization | `validate_user_buffer()`, `USER_SPACE_CEILING` |
| **Stack Smashing** | Tamper-evident Canaries | 64-bit Stack Canary, `__stack_chk_fail` |
| **Stack Overflow** | Memory Protection Boundaries | Unmapped 4 KiB Guard Pages |

Through these hardware and software defenses, NullOS achieves an institutional-grade security foundation, isolating Ring 0 silicon from user-mode untrusted software.
