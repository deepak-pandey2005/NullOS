//! Interrupt Descriptor Table (IDT), Exception Dispatcher & CPU Diagnostics.
//!
//! # Architecture & Silicon Mechanics
//! The x86_64 processor handles synchronous CPU exceptions (e.g. `#PF`, `#GP`, `#UD`)
//! and asynchronous hardware device interrupts (e.g. APIC Timer, Keyboard) via the
//! Interrupt Descriptor Table (IDT).
//!
//! ### Interrupt Execution Flow
//! 1. Hardware detects an exception or interrupt condition.
//! 2. If transitioning from Ring 3 to Ring 0, the CPU loads `%rsp` from `TSS.rsp0`.
//!    If the IDT gate has an IST index (`1..7`), the CPU loads `%rsp` from `TSS.ist[index-1]`.
//! 3. The CPU pushes the hardware stack frame: `SS`, `RSP`, `RFLAGS`, `CS`, `RIP`,
//!    and an optional architectural `ErrorCode` (for vectors 8, 10..14, 17, 21).
//! 4. For interrupt gates (type `0x8E`), the CPU automatically clears `RFLAGS.IF = 0`.
//! 5. Execution jumps to the corresponding assembly stub in `interrupts.S`, which pushes
//!    all general-purpose registers (`%r15..%rax`), passes a pointer to `ExceptionContext`
//!    in `%rdi` (SysV AMD64 ABI), and invokes `exception_handler()`.

use core::mem::size_of;

/// Standard 64-bit IDT Gate Descriptor (16 bytes).
///
/// In 64-bit mode, an IDT gate is 16 bytes (128 bits) to accommodate the full 64-bit
/// linear handler target address (`offset_high:offset_mid:offset_low`).
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct IdtEntry {
    /// Handler address bits 0..15.
    offset_low: u16,
    /// Target Code Segment Selector (must be `KERNEL_CODE_SELECTOR` = `0x08`).
    selector: u16,
    /// Interrupt Stack Table (IST) index: 3-bit value (`0` = current stack, `1..7` = TSS IST entry).
    ist: u8,
    /// Gate type and attribute flags (Present bit, DPL privilege bits, Gate Type).
    type_attr: u8,
    /// Handler address bits 16..31.
    offset_mid: u16,
    /// Handler address bits 32..63.
    offset_high: u32,
    /// Reserved by AMD64 / Intel 64 specification (must be 0).
    _reserved: u32,
}

impl IdtEntry {
    /// Creates an unpopulated, non-present IDT gate descriptor.
    pub const fn missing() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attr: 0,
            offset_mid: 0,
            offset_high: 0,
            _reserved: 0,
        }
    }

    /// Constructs an active 64-bit Interrupt Gate (`0x8E`).
    ///
    /// # Attributes (`0x8E`)
    /// - Bit 7: Present (`1`)
    /// - Bits 6..5: DPL (`00` = Ring 0 only can trigger via hardware or software)
    /// - Bit 4: Storage Segment (`0` = System descriptor)
    /// - Bits 3..0: Gate Type (`0xE` = 64-bit Interrupt Gate, disables interrupts on entry)
    pub fn new(handler: usize, selector: u16, ist: u8) -> Self {
        let handler_u64 = handler as u64;
        Self {
            offset_low: handler_u64 as u16,
            selector,
            ist,
            type_attr: 0x8E, // Present | DPL 0 | 64-bit Interrupt Gate (0xE)
            offset_mid: (handler_u64 >> 16) as u16,
            offset_high: (handler_u64 >> 32) as u32,
            _reserved: 0,
        }
    }
}

/// IDTR Register descriptor loaded into CPU silicon via the `lidt` instruction.
#[repr(C, packed)]
struct IdtDescriptor {
    /// Size of the IDT in bytes minus 1.
    size: u16,
    /// Linear 64-bit base address of the IDT array.
    offset: u64,
}

/// Comprehensive Saved CPU Register State during an Interrupt or Exception.
///
/// Matches the exact memory layout constructed by `interrupts.S` on the stack.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ExceptionContext {
    // General-purpose registers pushed in reverse order by common assembly stub
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,

    // Vector and error code pushed by individual isr stub
    pub vector: u64,
    pub error_code: u64,

    // Pushed automatically by CPU hardware on interrupt entry
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

/// Global 256-entry Interrupt Descriptor Table singleton.
static mut IDT: [IdtEntry; 256] = [IdtEntry::missing(); 256];

// Assembly ISR stub function prototypes defined in `interrupts.S`
unsafe extern "C" {
    fn isr0();  // #DE Divide-by-zero
    fn isr1();  // #DB Debug
    fn isr2();  // NMI Non-maskable Interrupt
    fn isr3();  // #BP Breakpoint
    fn isr4();  // #OF Overflow
    fn isr5();  // #BR Bound Range Exceeded
    fn isr6();  // #UD Invalid Opcode
    fn isr7();  // #NM Device Not Available
    fn isr8();  // #DF Double Fault (routed to TSS IST1)
    fn isr10(); // #TS Invalid TSS
    fn isr11(); // #NP Segment Not Present
    fn isr12(); // #SS Stack-Segment Fault
    fn isr13(); // #GP General Protection Fault
    fn isr14(); // #PF Page Fault
    fn isr16(); // #MF x87 FPU Floating-Point Error
    fn isr17(); // #AC Alignment Check
    fn isr18(); // #MC Machine Check
    fn isr19(); // #XM SIMD Floating-Point Exception
    fn isr20(); // #VE Virtualization Exception
    fn isr21(); // #CP Control Protection Exception
    fn isr32(); // IRQ 0 / Vector 32 (APIC Timer)
    fn isr255(); // Vector 255 (Spurious Interrupt)
}

/// High-resolution Local APIC periodic timer interrupt handler (Vector 32).
///
/// Increments APIC tick counters, dispatches preemption events to the thread scheduler,
/// and sends an End-Of-Interrupt (EOI) acknowledgment to the local APIC.
#[unsafe(no_mangle)]
pub extern "C" fn timer_handler() {
    super::apic::on_timer_tick();
    super::apic::end_of_interrupt();
    let ticks = super::apic::get_ticks();
    crate::task::timer_tick(ticks);
}

/// Central Rust exception dispatcher invoked by common assembly stubs.
///
/// Decodes the architectural exception vector, analyzes fault conditions (especially `#PF` via `CR2`),
/// displays diagnostic register crash dumps, and differentiates user-mode recoverable faults
/// from fatal kernel panics.
#[unsafe(no_mangle)]
pub extern "C" fn exception_handler(ctx: &ExceptionContext) {
    let name = match ctx.vector {
        0 => "Divide-by-Zero (#DE)",
        1 => "Debug (#DB)",
        2 => "Non-Maskable Interrupt (NMI)",
        3 => "Breakpoint (#BP)",
        4 => "Overflow (#OF)",
        5 => "Bound Range Exceeded (#BR)",
        6 => "Invalid Opcode (#UD)",
        7 => "Device Not Available (#NM)",
        8 => "Double Fault (#DF)",
        10 => "Invalid TSS (#TS)",
        11 => "Segment Not Present (#NP)",
        12 => "Stack-Segment Fault (#SS)",
        13 => "General Protection Fault (#GP)",
        14 => "Page Fault (#PF)",
        16 => "x87 Floating-Point Exception (#MF)",
        17 => "Alignment Check (#AC)",
        18 => "Machine Check (#MC)",
        19 => "SIMD Floating-Point Exception (#XM)",
        20 => "Virtualization Exception (#VE)",
        21 => "Control Protection Exception (#CP)",
        _ => "Unknown Exception",
    };

    let is_user = (ctx.cs & 3) == 3;
    crate::println!();
    if is_user {
        crate::println!("------------------------------------------------");
        crate::println!("        USER MODE CPU EXCEPTION (HANDLED)       ");
        crate::println!("------------------------------------------------");
    } else {
        crate::println!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
        crate::println!("          KERNEL CPU EXCEPTION PANIC            ");
        crate::println!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
    }
    crate::println!("Exception: {} (Vector: {})", name, ctx.vector);
    crate::println!("Error Code: {:#018x}", ctx.error_code);

    // Decode Page Fault details from CR2 linear fault address and architectural error code bits
    if ctx.vector == 14 {
        let cr2: u64;
        unsafe {
            core::arch::asm!("mov {}, cr2", out(reg) cr2);
        }
        let present = (ctx.error_code & (1 << 0)) != 0;
        let write = (ctx.error_code & (1 << 1)) != 0;
        let user = (ctx.error_code & (1 << 2)) != 0;
        let reserved = (ctx.error_code & (1 << 3)) != 0;
        let instruction = (ctx.error_code & (1 << 4)) != 0;

        crate::println!("CR2 Faulting Address: {:#018x}", cr2);
        crate::println!(
            "Page Fault Reason:   {} during {} in {} mode{}{}",
            if present { "Page-protection violation" } else { "Page not present" },
            if instruction { "instruction fetch" } else if write { "write" } else { "read" },
            if user { "user" } else { "kernel" },
            if reserved { " (reserved bit set)" } else { "" },
            if instruction { " (NX/XD violation)" } else { "" }
        );
    }

    // Print full 64-bit general-purpose and segment register crash telemetry
    crate::println!("RIP: {:#018x}   CS:  {:#06x}   RFLAGS: {:#018x}", ctx.rip, ctx.cs, ctx.rflags);
    crate::println!("RSP: {:#018x}   SS:  {:#06x}", ctx.rsp, ctx.ss);
    crate::println!("RAX: {:#018x}   RBX: {:#018x}   RCX: {:#018x}", ctx.rax, ctx.rbx, ctx.rcx);
    crate::println!("RDX: {:#018x}   RSI: {:#018x}   RDI: {:#018x}", ctx.rdx, ctx.rsi, ctx.rdi);
    crate::println!("RBP: {:#018x}   R8:  {:#018x}   R9:  {:#018x}", ctx.rbp, ctx.r8, ctx.r9);
    crate::println!("R10: {:#018x}   R11: {:#018x}   R12: {:#018x}", ctx.r10, ctx.r11, ctx.r12);
    crate::println!("R13: {:#018x}   R14: {:#018x}   R15: {:#018x}", ctx.r13, ctx.r14, ctx.r15);
    if is_user {
        crate::println!("------------------------------------------------");
    } else {
        crate::println!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
    }

    // User mode fault isolation: If fault occurred in Ring 3 (CPL == 3), kill the user thread
    // cleanly and return control to kernel scheduler without triggering a fatal kernel panic.
    if is_user {
        crate::println!();
        crate::println!(
            "\x1b[31;1m[USER CRASH]\x1b[0m User mode process encountered {} (vector {}) at RIP: {:#018x}!",
            name,
            ctx.vector,
            ctx.rip
        );
        crate::println!("\x1b[32m[USER CRASH]\x1b[0m Kernel successfully caught user fault. Terminating user thread cleanly and continuing OS execution...");
        crate::task::exit();
    }

    // Kernel mode panic: Halt execution indefinitely
    loop {
        unsafe {
            core::arch::asm!("cli; hlt");
        }
    }
}

/// Initializes the 256-gate Interrupt Descriptor Table (IDT) and loads it into the CPU.
///
/// Configures exception gates (`0..21`), assigns TSS IST1 for Double Fault `#DF` (vector 8),
/// wires hardware IRQs (Vector 32 for APIC timer, Vector 255 for Spurious), and executes `lidt`.
pub fn init() {
    unsafe {
        let cs = super::gdt::KERNEL_CODE_SELECTOR;

        // Set up CPU exception gates
        IDT[0] = IdtEntry::new(isr0 as *const () as usize, cs, 0);
        IDT[1] = IdtEntry::new(isr1 as *const () as usize, cs, 0);
        IDT[2] = IdtEntry::new(isr2 as *const () as usize, cs, 0);
        IDT[3] = IdtEntry::new(isr3 as *const () as usize, cs, 0);
        IDT[4] = IdtEntry::new(isr4 as *const () as usize, cs, 0);
        IDT[5] = IdtEntry::new(isr5 as *const () as usize, cs, 0);
        IDT[6] = IdtEntry::new(isr6 as *const () as usize, cs, 0);
        IDT[7] = IdtEntry::new(isr7 as *const () as usize, cs, 0);
        // Double fault uses IST1 (ist = 1) from TSS
        IDT[8] = IdtEntry::new(isr8 as *const () as usize, cs, 1);
        IDT[10] = IdtEntry::new(isr10 as *const () as usize, cs, 0);
        IDT[11] = IdtEntry::new(isr11 as *const () as usize, cs, 0);
        IDT[12] = IdtEntry::new(isr12 as *const () as usize, cs, 0);
        IDT[13] = IdtEntry::new(isr13 as *const () as usize, cs, 0);
        IDT[14] = IdtEntry::new(isr14 as *const () as usize, cs, 0);
        IDT[16] = IdtEntry::new(isr16 as *const () as usize, cs, 0);
        IDT[17] = IdtEntry::new(isr17 as *const () as usize, cs, 0);
        IDT[18] = IdtEntry::new(isr18 as *const () as usize, cs, 0);
        IDT[19] = IdtEntry::new(isr19 as *const () as usize, cs, 0);
        IDT[20] = IdtEntry::new(isr20 as *const () as usize, cs, 0);
        IDT[21] = IdtEntry::new(isr21 as *const () as usize, cs, 0);

        // Hardware interrupts
        IDT[32] = IdtEntry::new(isr32 as *const () as usize, cs, 0);
        IDT[255] = IdtEntry::new(isr255 as *const () as usize, cs, 0);

        let idt_ptr = core::ptr::addr_of!(IDT) as *const IdtEntry;
        let idt_desc = IdtDescriptor {
            size: (size_of::<[IdtEntry; 256]>() - 1) as u16,
            offset: idt_ptr as u64,
        };

        core::arch::asm!(
            "lidt [{}]",
            in(reg) &idt_desc,
            options(nostack, readonly)
        );
    }
}

/// Loads the global Interrupt Descriptor Table into an Application Processor (AP).
#[allow(dead_code)]
pub fn load_ap() {
    unsafe {
        let idt_ptr = core::ptr::addr_of!(IDT) as *const IdtEntry;
        let idt_desc = IdtDescriptor {
            size: (size_of::<[IdtEntry; 256]>() - 1) as u16,
            offset: idt_ptr as u64,
        };

        core::arch::asm!(
            "lidt [{}]",
            in(reg) &idt_desc,
            options(nostack, readonly)
        );
    }
}
