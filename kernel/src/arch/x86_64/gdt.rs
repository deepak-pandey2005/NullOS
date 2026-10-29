/// Global Descriptor Table (GDT) and Task State Segment (TSS) for x86_64.
///
/// Configures:
/// - 0x00: Null Descriptor
/// - 0x08: Kernel Code 64-bit (Ring 0)
/// - 0x10: Kernel Data 64-bit (Ring 0)
/// - 0x18: User Data 64-bit   (Ring 3)
/// - 0x20: User Code 64-bit   (Ring 3)
/// - 0x28: TSS (16-byte System Segment)

use core::mem::size_of;

pub const KERNEL_CODE_SELECTOR: u16 = 0x08;
pub const KERNEL_DATA_SELECTOR: u16 = 0x10;
#[allow(dead_code)]
pub const USER_DATA_SELECTOR: u16 = 0x18 | 3;
#[allow(dead_code)]
pub const USER_CODE_SELECTOR: u16 = 0x20 | 3;
pub const TSS_SELECTOR: u16 = 0x28;

pub const DOUBLE_FAULT_IST_INDEX: usize = 0; // IST1 (0-indexed in array)

/// x86_64 Task State Segment structure (104 bytes)
#[repr(C, packed)]
pub struct TaskStateSegment {
    _reserved1: u32,
    pub rsp0: u64,
    pub rsp1: u64,
    pub rsp2: u64,
    _reserved2: u64,
    pub ist: [u64; 7],
    _reserved3: u64,
    _reserved4: u16,
    pub iomap_base: u16,
}

impl TaskStateSegment {
    pub const fn new() -> Self {
        Self {
            _reserved1: 0,
            rsp0: 0,
            rsp1: 0,
            rsp2: 0,
            _reserved2: 0,
            ist: [0; 7],
            _reserved3: 0,
            _reserved4: 0,
            iomap_base: size_of::<TaskStateSegment>() as u16,
        }
    }
}

/// GDT Pointer structure loaded by `lgdt`
#[repr(C, packed)]
struct GdtDescriptor {
    size: u16,
    offset: u64,
}

const DOUBLE_FAULT_STACK_SIZE: usize = 16384; // 16 KiB
static mut DOUBLE_FAULT_STACK: [u8; DOUBLE_FAULT_STACK_SIZE] = [0; DOUBLE_FAULT_STACK_SIZE];

static mut TSS: TaskStateSegment = TaskStateSegment::new();

// GDT entries: 5 standard 8-byte entries + 1 16-byte TSS descriptor = 7 u64 entries
static mut GDT: [u64; 7] = [0; 7];

/// Build a standard flat segment descriptor with 4-GiB limit (0xFFFFF 4KiB pages)
const fn create_descriptor(flags: u64, access: u64) -> u64 {
    let limit_low = 0xFFFFu64;
    let limit_high = 0x0Fu64 << 48;
    let flags_shifted = (flags & 0x0F) << 52;
    let access_shifted = (access & 0xFF) << 40;
    flags_shifted | access_shifted | limit_high | limit_low
}

/// Initialize and load GDT and TSS
pub fn init() {
    unsafe {
        // Set up IST1 for double fault handling
        let stack_base = core::ptr::addr_of_mut!(DOUBLE_FAULT_STACK) as *mut u8;
        let stack_top = stack_base as u64 + DOUBLE_FAULT_STACK_SIZE as u64;
        TSS.ist[DOUBLE_FAULT_IST_INDEX] = stack_top;

        // Entry 0: Null Descriptor
        GDT[0] = 0;

        // Entry 1 (0x08): Kernel Code 64-bit (Ring 0, Executable, Readable, Long Mode)
        // Access: Present(1) | Ring 0(00) | S=1(Code/Data) | Exec(1) | Direction/Conforming(0) | Readable(1) | Accessed(0) = 0x9A
        // Flags: Granularity(1) | LongMode(1) = 0xA
        GDT[1] = create_descriptor(0xA, 0x9A); // 0x00AF9A000000FFFF

        // Entry 2 (0x10): Kernel Data (Ring 0, Writable)
        // Access: Present(1) | Ring 0(00) | S=1 | Exec(0) | Dir(0) | Writable(1) | Accessed(0) = 0x92
        // Flags: Granularity(1) | Default/Big(1) = 0xC
        GDT[2] = create_descriptor(0xC, 0x92); // 0x00CF92000000FFFF

        // Entry 3 (0x18): User Data (Ring 3, Writable)
        // Access: Present(1) | Ring 3(11) | S=1 | Exec(0) | Dir(0) | Writable(1) | Accessed(0) = 0xF2
        // Flags: Granularity(1) | Default/Big(1) = 0xC
        GDT[3] = create_descriptor(0xC, 0xF2); // 0x00CFF2000000FFFF

        // Entry 4 (0x20): User Code 64-bit (Ring 3, Executable, Readable, Long Mode)
        // Access: Present(1) | Ring 3(11) | S=1 | Exec(1) | Dir(0) | Readable(1) | Accessed(0) = 0xFA
        // Flags: Granularity(1) | LongMode(1) = 0xA
        GDT[4] = create_descriptor(0xA, 0xFA); // 0x00AFFA000000FFFF

        // Entry 5 & 6 (0x28): TSS Descriptor (16 bytes in long mode)
        let tss_addr = core::ptr::addr_of!(TSS) as u64;
        let tss_limit = (size_of::<TaskStateSegment>() - 1) as u64;

        // Low 8 bytes: limit 15..0, base 23..0, type 0x89 (Available 64-bit TSS), limit 19..16, base 31..24
        let tss_low = (tss_limit & 0xFFFF)
            | ((tss_addr & 0xFFFFFF) << 16)
            | (0x89u64 << 40) // Present, System segment, Type 0x9 (Available 64-bit TSS)
            | (((tss_limit >> 16) & 0x0F) << 48)
            | (((tss_addr >> 24) & 0xFF) << 56);

        // High 8 bytes: base 63..32, reserved 0
        let tss_high = tss_addr >> 32;

        GDT[5] = tss_low;
        GDT[6] = tss_high;

        let gdt_ptr = core::ptr::addr_of!(GDT) as *const u64;
        let gdt_desc = GdtDescriptor {
            size: (size_of::<[u64; 7]>() - 1) as u16,
            offset: gdt_ptr as u64,
        };

        // Load GDT
        core::arch::asm!(
            "lgdt [{}]",
            in(reg) &gdt_desc,
            options(nostack, readonly)
        );

        // Reload segment registers
        // In 64-bit mode, far return reloads CS:
        core::arch::asm!(
            "push {cs}",
            "lea 1f(%rip), {tmp}",
            "push {tmp}",
            "lretq",
            "1:",
            "mov {data_sel:x}, %ds",
            "mov {data_sel:x}, %es",
            "mov {data_sel:x}, %ss",
            "xor %eax, %eax",
            "mov %ax, %fs",
            "mov %ax, %gs",
            cs = in(reg) KERNEL_CODE_SELECTOR as u64,
            data_sel = in(reg) KERNEL_DATA_SELECTOR,
            tmp = out(reg) _,
            out("eax") _,
            options(att_syntax)
        );

        // Load Task Register (TSS)
        core::arch::asm!(
            "ltr {0:x}",
            in(reg) TSS_SELECTOR,
            options(nostack)
        );
    }
}

/// Updates the kernel stack pointer (RSP0) in the TSS loaded on privilege transition.
pub fn set_tss_rsp0(rsp0: u64) {
    unsafe {
        TSS.rsp0 = rsp0;
    }
}

/// Loads the Global Descriptor Table on an Application Processor (AP) core.
pub fn load_ap() {
    unsafe {
        let gdt_ptr = core::ptr::addr_of!(GDT) as *const u64;
        let gdt_desc = GdtDescriptor {
            size: (size_of::<[u64; 7]>() - 1) as u16,
            offset: gdt_ptr as u64,
        };

        core::arch::asm!(
            "lgdt [{}]",
            in(reg) &gdt_desc,
            options(nostack, readonly)
        );

        core::arch::asm!(
            "push {cs}",
            "lea 1f(%rip), {tmp}",
            "push {tmp}",
            "lretq",
            "1:",
            "mov {data_sel:x}, %ds",
            "mov {data_sel:x}, %es",
            "mov {data_sel:x}, %ss",
            "xor %eax, %eax",
            "mov %ax, %fs",
            "mov %ax, %gs",
            cs = in(reg) KERNEL_CODE_SELECTOR as u64,
            data_sel = in(reg) KERNEL_DATA_SELECTOR,
            tmp = out(reg) _,
            out("eax") _,
            options(att_syntax)
        );
    }
}
