/// Interrupt Descriptor Table (IDT) and Exception Handling for x86_64.
///
/// Configures 256 interrupt gates with ISR stubs and register dump diagnostics.

use core::mem::size_of;

/// IDT Gate Descriptor (16 bytes in x86_64 long mode)
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    _reserved: u32,
}

impl IdtEntry {
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

    /// Create an interrupt gate (type 0xE: 64-bit Interrupt Gate, interrupts disabled on entry)
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

/// IDTR structure loaded with `lidt`
#[repr(C, packed)]
struct IdtDescriptor {
    size: u16,
    offset: u64,
}

/// Saved CPU registers passed from common assembly stub
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ExceptionContext {
    // Pushed by common assembly stub
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

    // Pushed automatically by CPU on interrupt
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

static mut IDT: [IdtEntry; 256] = [IdtEntry::missing(); 256];

// Assembly ISR stub function prototypes
unsafe extern "C" {
    fn isr0();  // #DE Divide-by-zero
    fn isr1();  // #DB Debug
    fn isr2();  // NMI Non-maskable Interrupt
    fn isr3();  // #BP Breakpoint
    fn isr4();  // #OF Overflow
    fn isr5();  // #BR Bound Range Exceeded
    fn isr6();  // #UD Invalid Opcode
    fn isr7();  // #NM Device Not Available
    fn isr8();  // #DF Double Fault
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
    fn isr32(); // IRQ 0 / Vector 32 (Timer)
    fn isr255(); // Spurious Interrupt
}

/// Timer interrupt handler called by isr32
#[unsafe(no_mangle)]
pub extern "C" fn timer_handler() {
    super::apic::on_timer_tick();
    super::apic::end_of_interrupt();
}

/// Common Rust exception dispatcher called by assembly stub
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

    crate::println!();
    crate::println!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
    crate::println!("                CPU EXCEPTION                   ");
    crate::println!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
    crate::println!("Exception: {} (Vector: {})", name, ctx.vector);
    crate::println!("Error Code: {:#018x}", ctx.error_code);

    if ctx.vector == 14 {
        let cr2: u64;
        unsafe {
            core::arch::asm!("mov {}, cr2", out(reg) cr2);
        }
        crate::println!("CR2 Faulting Address: {:#018x}", cr2);
    }

    crate::println!("RIP: {:#018x}   CS:  {:#06x}   RFLAGS: {:#018x}", ctx.rip, ctx.cs, ctx.rflags);
    crate::println!("RSP: {:#018x}   SS:  {:#06x}", ctx.rsp, ctx.ss);
    crate::println!("RAX: {:#018x}   RBX: {:#018x}   RCX: {:#018x}", ctx.rax, ctx.rbx, ctx.rcx);
    crate::println!("RDX: {:#018x}   RSI: {:#018x}   RDI: {:#018x}", ctx.rdx, ctx.rsi, ctx.rdi);
    crate::println!("RBP: {:#018x}   R8:  {:#018x}   R9:  {:#018x}", ctx.rbp, ctx.r8, ctx.r9);
    crate::println!("R10: {:#018x}   R11: {:#018x}   R12: {:#018x}", ctx.r10, ctx.r11, ctx.r12);
    crate::println!("R13: {:#018x}   R14: {:#018x}   R15: {:#018x}", ctx.r13, ctx.r14, ctx.r15);
    crate::println!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");

    loop {
        unsafe {
            core::arch::asm!("cli; hlt");
        }
    }
}

/// Initialize the IDT and load it into the CPU
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
