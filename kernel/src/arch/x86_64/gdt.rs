//! Global Descriptor Table (GDT) and Task State Segment (TSS) for x86_64.
//!
//! # Architecture & Segment Model
//! Although x86_64 Long Mode enforces a flat 64-bit address space where base and limit
//! segmentation checks are disabled for data accesses, the processor strictly relies on
//! the GDT for privilege enforcement and hardware context switching:
//! 1. **Current Privilege Level (CPL)**: In silicon, CPL is determined by the lowest 2 bits
//!    of the `%cs` (Code Segment) register (Ring 0 = `0b00`, Ring 3 = `0b11`).
//! 2. **Ring Transitions**: When interrupts, exceptions, or `iretq` instructions execute,
//!    the CPU validates CPL against Descriptor Privilege Level (DPL) and Requested Privilege
//!    Level (RPL).
//! 3. **Task State Segment (TSS)**: Contains `rsp0` (the kernel stack loaded automatically
//!    when an interrupt fires while in user space) and the Interrupt Stack Table (IST),
//!    which assigns dedicated emergency physical stacks for catastrophic exceptions (e.g., `#DF`).
//!
//! # Segment Selector Table
//! - `0x00`: Null Descriptor (Mandatory hardware placeholder)
//! - `0x08`: Kernel Code 64-bit (Ring 0, Executable, Readable, Long Mode)
//! - `0x10`: Kernel Data 64-bit (Ring 0, Writable)
//! - `0x18 | 3` (`0x1B`): User Data 64-bit (Ring 3, Writable)
//! - `0x20 | 3` (`0x23`): User Code 64-bit (Ring 3, Executable, Readable, Long Mode)
//! - `0x28`: 16-byte System Segment: Task State Segment (TSS)

use core::mem::size_of;

/// Kernel Code Segment Selector (`0x08`, Ring 0, Index 1).
pub const KERNEL_CODE_SELECTOR: u16 = 0x08;

/// Kernel Data Segment Selector (`0x10`, Ring 0, Index 2).
pub const KERNEL_DATA_SELECTOR: u16 = 0x10;

/// User Data Segment Selector (`0x18 | 3 = 0x1B`, Ring 3, Index 3, RPL 3).
#[allow(dead_code)]
pub const USER_DATA_SELECTOR: u16 = 0x18 | 3;

/// User Code Segment Selector (`0x20 | 3 = 0x23`, Ring 3, Index 4, RPL 3).
#[allow(dead_code)]
pub const USER_CODE_SELECTOR: u16 = 0x20 | 3;

/// Task State Segment Selector (`0x28`, Index 5, 16-byte System Descriptor).
pub const TSS_SELECTOR: u16 = 0x28;

/// Index in the TSS IST array for Double Fault handling (IST1, 0-indexed in array).
pub const DOUBLE_FAULT_IST_INDEX: usize = 0;

/// Standard x86_64 Task State Segment structure (104 bytes).
///
/// In 64-bit Long Mode, hardware task switching via TSS is deprecated. The TSS is instead
/// utilized by the CPU for privilege-level stack switching (`rsp0`, `rsp1`, `rsp2`)
/// and dedicated Interrupt Stack Table (`ist1..ist7`) pointer resolution.
#[repr(C, packed)]
pub struct TaskStateSegment {
    _reserved1: u32,
    /// Kernel stack pointer loaded automatically by the CPU when an interrupt or exception
    /// traps from Ring 3 (user mode) into Ring 0 (kernel mode).
    pub rsp0: u64,
    /// Stack pointer loaded on privilege switch to Ring 1 (unused in NullOS).
    pub rsp1: u64,
    /// Stack pointer loaded on privilege switch to Ring 2 (unused in NullOS).
    pub rsp2: u64,
    _reserved2: u64,
    /// Interrupt Stack Table (IST): 7 dedicated emergency stack pointers.
    /// When an IDT gate specifies a non-zero IST index, the CPU automatically pivots `%rsp`
    /// to the designated IST stack before pushing the interrupt frame.
    pub ist: [u64; 7],
    _reserved3: u64,
    _reserved4: u16,
    /// 16-bit offset from the start of the TSS to the I/O Permission Bit Map.
    /// Setting this equal to or greater than the TSS limit disables the I/O bitmap.
    pub iomap_base: u16,
}

impl TaskStateSegment {
    /// Constructs a clean, zeroed TaskStateSegment with the I/O bitmap disabled.
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

/// GDT Register descriptor loaded into CPU silicon via the `lgdt` instruction.
#[repr(C, packed)]
struct GdtDescriptor {
    /// Size of the GDT in bytes minus 1.
    size: u16,
    /// Linear 64-bit base address of the GDT array.
    offset: u64,
}

/// Size of the dedicated emergency stack for Double Fault recovery (16 KiB).
const DOUBLE_FAULT_STACK_SIZE: usize = 16384;

/// Statically allocated physical stack memory for Double Fault (`#DF`) recovery.
static mut DOUBLE_FAULT_STACK: [u8; DOUBLE_FAULT_STACK_SIZE] = [0; DOUBLE_FAULT_STACK_SIZE];

/// Global Task State Segment singleton.
static mut TSS: TaskStateSegment = TaskStateSegment::new();

/// Global Descriptor Table entries:
/// 5 standard 8-byte descriptors + 1 16-byte TSS descriptor (consumes 2 slots) = 7 entries.
static mut GDT: [u64; 7] = [0; 7];

/// Synthesizes an 8-byte segment descriptor bit-pattern with a 4 GiB flat limit.
///
/// Encodes access rights (Present, Ring DPL, Code/Data, Read/Write) and architecture flags
/// (64-bit Long Mode `L=1`, 4 KiB Granularity `G=1`).
const fn create_descriptor(flags: u64, access: u64) -> u64 {
    let limit_low = 0xFFFFu64;
    let limit_high = 0x0Fu64 << 48;
    let flags_shifted = (flags & 0x0F) << 52;
    let access_shifted = (access & 0xFF) << 40;
    flags_shifted | access_shifted | limit_high | limit_low
}

/// Initializes and loads the Global Descriptor Table (GDT) and Task State Segment (TSS) on the BSP.
///
/// # Execution Steps
/// 1. Configures `TSS.ist[0]` to point to the top of `DOUBLE_FAULT_STACK`.
/// 2. Populates standard GDT entries: Null (0), Kernel Code (1), Kernel Data (2), User Data (3), User Code (4).
/// 3. Populates 16-byte TSS system segment across GDT slots 5 and 6 with base address and limit.
/// 4. Executes `lgdt` instruction to load the GDT base and limit into the CPU GDTR register.
/// 5. Executes a 64-bit far return (`lretq`) to reload `%cs` with `KERNEL_CODE_SELECTOR` (`0x08`).
/// 6. Reloads data segment registers (`%ds`, `%es`, `%ss`, `%fs`, `%gs`) with `KERNEL_DATA_SELECTOR` (`0x10`).
/// 7. Executes `ltr` instruction to load the Task Register with `TSS_SELECTOR` (`0x28`).
pub fn init() {
    unsafe {
        // Step 1: Set up IST1 for double fault handling
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

        // Load GDT into CPU GDTR register
        core::arch::asm!(
            "lgdt [{}]",
            in(reg) &gdt_desc,
            options(nostack, readonly)
        );

        // Reload Code Segment via 64-bit far return (lretq) and reload data segments
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

        // Load Task Register (TR) to activate TSS
        core::arch::asm!(
            "ltr {0:x}",
            in(reg) TSS_SELECTOR,
            options(nostack)
        );
    }
}

/// Updates the kernel stack pointer (`RSP0`) in the active TSS.
///
/// Called during process / thread context switches so that if an interrupt interrupts user mode,
/// the CPU will pivot to that process's dedicated kernel stack rather than clobbering another.
pub fn set_tss_rsp0(rsp0: u64) {
    unsafe {
        TSS.rsp0 = rsp0;
    }
}

/// Loads the Global Descriptor Table and reloads segment registers on an Application Processor (AP).
///
/// Ensures secondary multi-core CPUs share the identical GDT configuration and segment registers.
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
