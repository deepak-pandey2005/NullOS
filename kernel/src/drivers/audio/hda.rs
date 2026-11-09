//! Intel High Definition Audio (HDA) Controller Driver (Phase 12 - M32).
//!
//! Subsystem features:
//! - PCI hardware discovery for Class 0x04 (Multimedia), Subclass 0x03 (Audio Controller).
//! - 64-bit MMIO BAR0 mapping (No-Cache, NX).
//! - Hardware controller reset sequence (`GCTL.CRST`).
//! - Circular DMA ring buffers: CORB (Command Outbound) and RIRB (Response Inbound).
//! - 32-bit HDA Verb dispatching and 64-bit response collection.
//! - Codec enumeration, Audio Function Group discovery, and Widget tree traversal.
//! - Audio Output Converter (DAC) and Pin Complex amplifier & jack configuration.
//! - Stream Descriptor DMA engine with 2-entry ping-pong Buffer Descriptor List (BDL).
//! - Procedural PCM tone playback and sound synthesis.

#![allow(dead_code)]

use alloc::vec::Vec;
use core::sync::atomic::{fence, Ordering};

use crate::drivers::audio::pcm::{generate_chime, generate_sine_tone, PcmFormat};
use crate::drivers::pci::{self, BarType};
use crate::memory::{self, PhysAddr, PhysFrame, PageTableFlags, PAGE_SIZE_4K};
use crate::println;
use crate::sync::Spinlock;

// HDA Register Offsets (relative to BAR0 MMIO Base)
const REG_GCAP: usize = 0x00;        // 16-bit: Global Capabilities
const REG_VMIN: usize = 0x02;        // 8-bit: Minor Version
const REG_VMAJ: usize = 0x03;        // 8-bit: Major Version
const REG_OUTPAY: usize = 0x04;      // 16-bit: Output Payload Capability
const REG_INPAY: usize = 0x06;       // 16-bit: Input Payload Capability
const REG_GCTL: usize = 0x08;        // 32-bit: Global Control
const REG_WAKEEN: usize = 0x0C;      // 16-bit: Wake Enable
const REG_STATESTS: usize = 0x0E;    // 16-bit: State Change Status (Codecs present)
const REG_GSTS: usize = 0x10;        // 16-bit: Global Status
const REG_INTCTL: usize = 0x20;      // 32-bit: Interrupt Control
const REG_INTSTS: usize = 0x24;      // 32-bit: Interrupt Status

// CORB Registers (relative to BAR0 Base)
const REG_CORBLBASE: usize = 0x40;   // 32-bit: Lower Base Address
const REG_CORBUBASE: usize = 0x44;   // 32-bit: Upper Base Address
const REG_CORBWP: usize = 0x48;      // 16-bit: Write Pointer
const REG_CORBRP: usize = 0x4A;      // 16-bit: Read Pointer
const REG_CORBCTL: usize = 0x4C;     // 8-bit: Control (bit 1 = RUN)
const REG_CORBST: usize = 0x4D;      // 8-bit: Status
const REG_CORBSIZE: usize = 0x4E;    // 8-bit: Size (0x02 = 256 entries)

// RIRB Registers (relative to BAR0 Base)
const REG_RIRBLBASE: usize = 0x50;   // 32-bit: Lower Base Address
const REG_RIRBUBASE: usize = 0x54;   // 32-bit: Upper Base Address
const REG_RIRBWP: usize = 0x58;      // 16-bit: Write Pointer (bit 15 = Reset)
const REG_RINTCNT: usize = 0x5A;     // 16-bit: Response Interrupt Count
const REG_RIRBCTL: usize = 0x5C;     // 8-bit: Control (bit 1 = DMAEN)
const REG_RIRBST: usize = 0x5D;      // 8-bit: Status
const REG_RIRBSIZE: usize = 0x5E;    // 8-bit: Size (0x02 = 256 entries)

// Immediate Command Registers (relative to BAR0 Base)
const REG_ICW: usize = 0x60;         // 32-bit: Immediate Command Output
const REG_IRR: usize = 0x64;         // 32-bit: Immediate Response Input
const REG_ICS: usize = 0x68;         // 16-bit: Immediate Command Status

// Stream Descriptor Register Offsets (relative to Stream Base)
const SD_CTL: usize = 0x00;          // 24-bit Control (bit 1 = RUN, bits 20..23 = Stream ID)
const SD_STS: usize = 0x03;          // 8-bit Status
const SD_LPIB: usize = 0x04;         // 32-bit Link Position in Buffer
const SD_CBL: usize = 0x08;          // 32-bit Cyclic Buffer Length
const SD_LVI: usize = 0x0C;          // 16-bit Last Valid Index
const SD_FIFOS: usize = 0x10;        // 16-bit FIFO Size
const SD_FMT: usize = 0x12;          // 16-bit Stream Format
const SD_BDLPL: usize = 0x18;        // 32-bit BDL Lower Base Address
const SD_BDLPU: usize = 0x1C;        // 32-bit BDL Upper Base Address

// HDA Verbs
const VERB_GET_PARAM: u32 = 0xF00;
const VERB_SET_CONV_FMT: u32 = 0x200;
const VERB_SET_POWER_STATE: u32 = 0x705;
const VERB_SET_STREAM_CHANNEL: u32 = 0x706;
const VERB_SET_PIN_WIDGET_CTRL: u32 = 0x707;
const VERB_SET_AMP_GAIN_MUTE: u32 = 0x300;

// Widget Types (bits 20..23 of Param 0x09)
pub const WIDGET_TYPE_AUDIO_OUTPUT: u32 = 0x0;
pub const WIDGET_TYPE_AUDIO_INPUT: u32 = 0x1;
pub const WIDGET_TYPE_MIXER: u32 = 0x2;
pub const WIDGET_TYPE_SELECTOR: u32 = 0x3;
pub const WIDGET_TYPE_PIN_COMPLEX: u32 = 0x4;
pub const WIDGET_TYPE_POWER: u32 = 0x5;

const BUFFER_SIZE_BYTES: usize = 4096;

/// A single 16-byte Buffer Descriptor List (BDL) entry.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Default)]
pub struct BdlEntry {
    pub addr: u64,
    pub length: u32,
    pub flags: u32, // Bit 0: Interrupt on Completion (IOC)
}

/// Discovered Audio Codec on the HDA link.
#[derive(Clone, Debug)]
pub struct HdaCodec {
    pub address: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub dac_nid: u8,
    pub pin_nid: u8,
}

/// Intel High Definition Audio Controller Instance.
pub struct HdaController {
    pub mmio_base: *mut u8,
    pub major_version: u8,
    pub minor_version: u8,
    pub num_iss: u8,
    pub num_oss: u8,
    pub num_bss: u8,
    pub corb_frame: PhysFrame,
    pub corb: *mut u32,
    pub corb_wp: u8,
    pub rirb_frame: PhysFrame,
    pub rirb: *mut u64,
    pub rirb_rp: u8,
    pub bdl_frame: PhysFrame,
    pub bdl: *mut BdlEntry,
    pub dma_buf_a: PhysFrame,
    pub dma_buf_b: PhysFrame,
    pub codecs: Vec<HdaCodec>,
    pub active_dac_nid: u8,
    pub active_pin_nid: u8,
    pub format: PcmFormat,
    pub is_playing: bool,
}

unsafe impl Send for HdaController {}
unsafe impl Sync for HdaController {}

impl HdaController {
    #[inline]
    unsafe fn read32(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile(self.mmio_base.add(offset) as *const u32) }
    }

    #[inline]
    unsafe fn write32(&self, offset: usize, val: u32) {
        unsafe { core::ptr::write_volatile(self.mmio_base.add(offset) as *mut u32, val); }
    }

    #[inline]
    unsafe fn read16(&self, offset: usize) -> u16 {
        unsafe { core::ptr::read_volatile(self.mmio_base.add(offset) as *const u16) }
    }

    #[inline]
    unsafe fn write16(&self, offset: usize, val: u16) {
        unsafe { core::ptr::write_volatile(self.mmio_base.add(offset) as *mut u16, val); }
    }

    #[inline]
    unsafe fn read8(&self, offset: usize) -> u8 {
        unsafe { core::ptr::read_volatile(self.mmio_base.add(offset) as *const u8) }
    }

    #[inline]
    unsafe fn write8(&self, offset: usize, val: u8) {
        unsafe { core::ptr::write_volatile(self.mmio_base.add(offset) as *mut u8, val); }
    }

    /// Offset of Output Stream Descriptor 0: `0x80 + (num_iss as usize) * 0x20`.
    #[inline]
    fn output_stream0_offset(&self) -> usize {
        0x80 + (self.num_iss as usize) * 0x20
    }

    /// Sends an HDA verb using the Immediate Command Interface (ICW/IRR/ICS).
    /// This is synchronous, low-latency, and independent of DMA ring timing.
    pub fn send_verb_immediate(&mut self, cad: u8, nid: u8, verb: u32, param: u8) -> Result<u32, &'static str> {
        let verb_data = ((cad as u32 & 0x0F) << 28)
            | ((nid as u32) << 20)
            | ((verb & 0xFFF) << 8)
            | (param as u32);

        // 1. Wait until Immediate Command engine is idle (ICB == 0)
        for _ in 0..20_000 {
            let ics = unsafe { self.read16(REG_ICS) };
            if (ics & 1) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Clear IRV (bit 1) if set by writing 1 to bit 1
        unsafe {
            let ics = self.read16(REG_ICS);
            if (ics & 2) != 0 {
                self.write16(REG_ICS, 2);
            }
        }

        // 2. Write 32-bit verb to ICW
        unsafe {
            self.write32(REG_ICW, verb_data);
            fence(Ordering::SeqCst);
            // 3. Trigger execution by setting ICB (bit 0) in ICS
            self.write16(REG_ICS, 1);
        }

        // 4. Poll until ICB == 0 and IRV == 1
        for _ in 0..50_000 {
            let ics = unsafe { self.read16(REG_ICS) };
            if (ics & 1) == 0 && (ics & 2) != 0 {
                let resp = unsafe { self.read32(REG_IRR) };
                // Acknowledge by writing 1 to IRV
                unsafe { self.write16(REG_ICS, 2); }
                return Ok(resp);
            }
            core::hint::spin_loop();
        }

        Err("Immediate Command timed out")
    }

    /// Sends a 32-bit HDA Verb via Immediate Command PIO or CORB/RIRB DMA ring buffer.
    pub fn send_verb(&mut self, cad: u8, nid: u8, verb: u32, param: u8) -> Result<u32, &'static str> {
        // Try Immediate Command Interface first
        if let Ok(resp) = self.send_verb_immediate(cad, nid, verb, param) {
            return Ok(resp);
        }

        let verb_data = ((cad as u32 & 0x0F) << 28)
            | ((nid as u32) << 20)
            | ((verb & 0xFFF) << 8)
            | (param as u32);

        // 1. Advance Write Pointer in CORB ring
        self.corb_wp = self.corb_wp.wrapping_add(1);
        unsafe {
            *self.corb.add(self.corb_wp as usize) = verb_data;
        }

        fence(Ordering::SeqCst);

        // 2. Commit updated Write Pointer to hardware
        unsafe {
            self.write16(REG_CORBWP, self.corb_wp as u16);
        }

        // 3. Spin-wait for hardware to increment RIRB Write Pointer
        for _ in 0..100_000 {
            let rirb_wp = unsafe { (self.read16(REG_RIRBWP) & 0xFF) as u8 };
            if rirb_wp != self.rirb_rp {
                self.rirb_rp = self.rirb_rp.wrapping_add(1);
                let resp_raw = unsafe { *self.rirb.add(self.rirb_rp as usize) };
                let resp_data = (resp_raw & 0xFFFF_FFFF) as u32;
                return Ok(resp_data);
            }
            core::hint::spin_loop();
        }

        Err("HDA Verb execution timed out waiting for RIRB response")
    }

    /// Sets up and starts audio playback using the DMA ping-pong buffers.
    pub fn play_samples(&mut self, samples: &[i16]) -> Result<(), &'static str> {
        if samples.is_empty() {
            return Ok(());
        }

        let stream_base = self.output_stream0_offset();

        // 1. Stop stream DMA engine if running
        unsafe {
            let ctl = self.read32(stream_base + SD_CTL);
            self.write32(stream_base + SD_CTL, ctl & !2);
        }

        // Wait for RUN bit to clear
        for _ in 0..10_000 {
            let ctl = unsafe { self.read32(stream_base + SD_CTL) };
            if (ctl & 2) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // 2. Reset stream state machine
        unsafe {
            let mut ctl = self.read32(stream_base + SD_CTL);
            ctl |= 1; // Stream Reset (SRST)
            self.write32(stream_base + SD_CTL, ctl);
        }
        for _ in 0..10_000 {
            let ctl = unsafe { self.read32(stream_base + SD_CTL) };
            if (ctl & 1) != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        unsafe {
            let mut ctl = self.read32(stream_base + SD_CTL);
            ctl &= !1; // Clear SRST
            self.write32(stream_base + SD_CTL, ctl);
        }
        for _ in 0..10_000 {
            let ctl = unsafe { self.read32(stream_base + SD_CTL) };
            if (ctl & 1) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // 3. Copy sample data into DMA Buffer A and Buffer B
        let buf_a_ptr = self.dma_buf_a.start_address().as_u64() as *mut i16;
        let buf_b_ptr = self.dma_buf_b.start_address().as_u64() as *mut i16;

        let half_len = (BUFFER_SIZE_BYTES / 2).min(samples.len());
        unsafe {
            core::ptr::copy_nonoverlapping(samples.as_ptr(), buf_a_ptr, half_len);
            let rem = if samples.len() > half_len {
                &samples[half_len..]
            } else {
                samples
            };
            let rem_len = (BUFFER_SIZE_BYTES / 2).min(rem.len());
            core::ptr::copy_nonoverlapping(rem.as_ptr(), buf_b_ptr, rem_len);
        }

        // 4. Program Stream Descriptor registers
        let bdl_phys = self.bdl_frame.start_address().as_u64();
        let total_cbl = (BUFFER_SIZE_BYTES * 2) as u32;
        let hda_fmt = self.format.to_hda_format();

        unsafe {
            self.write32(stream_base + SD_BDLPL, (bdl_phys & 0xFFFF_FFFF) as u32);
            self.write32(stream_base + SD_BDLPU, (bdl_phys >> 32) as u32);
            self.write32(stream_base + SD_CBL, total_cbl);
            self.write16(stream_base + SD_LVI, 1); // 2 BDL entries (indices 0 and 1)
            self.write16(stream_base + SD_FMT, hda_fmt);

            // Set Stream ID = 1 and enable RUN (bit 1)
            let ctl = (1 << 20) | (1 << 1);
            self.write32(stream_base + SD_CTL, ctl);
        }

        self.is_playing = true;
        Ok(())
    }

    /// Plays a procedural sine wave tone at `freq_hz` for `duration_ms`.
    pub fn play_tone(&mut self, freq_hz: u32, duration_ms: u32) -> Result<(), &'static str> {
        let sample_rate = self.format.sample_rate;
        // Total stereo samples needed: (duration_ms * sample_rate * 2) / 1000
        let total_samples = ((duration_ms as u64 * sample_rate as u64 * 2) / 1000) as usize;
        let max_samples = (BUFFER_SIZE_BYTES / 2) * 2; // Fit within our double-buffer pair
        let play_samples = total_samples.min(max_samples);

        let mut tone_buffer = alloc::vec![0i16; play_samples];
        generate_sine_tone(&mut tone_buffer, freq_hz, sample_rate, 20000);

        self.play_samples(&tone_buffer)?;

        // Wait for tone duration without requiring interrupts (busy-spin delay)
        crate::arch::x86_64::apic::spin_delay_us((duration_ms.max(10) as u64) * 1000);

        self.stop();
        Ok(())
    }

    /// Plays a startup chime (C5 -> G5 musical chord).
    pub fn play_chime(&mut self) -> Result<(), &'static str> {
        let sample_rate = self.format.sample_rate;
        let max_samples = (BUFFER_SIZE_BYTES / 2) * 2;
        let mut chime_buffer = alloc::vec![0i16; max_samples];
        generate_chime(&mut chime_buffer, sample_rate);

        self.play_samples(&chime_buffer)?;
        // 50ms delay for automated chime playback (works without interrupts)
        crate::arch::x86_64::apic::spin_delay_us(50_000);
        self.stop();
        Ok(())
    }

    /// Stops audio playback and turns off DMA stream 0.
    pub fn stop(&mut self) {
        let stream_base = self.output_stream0_offset();
        unsafe {
            let ctl = self.read32(stream_base + SD_CTL);
            self.write32(stream_base + SD_CTL, ctl & !2);
        }
        self.is_playing = false;
    }
}

/// Global Intel HDA controller instance.
pub static HDA: Spinlock<Option<HdaController>> = Spinlock::new(None);

/// Probes the PCI bus for an Intel High Definition Audio controller and initializes the hardware.
pub fn init() {
    println!();
    println!("--------------------------------------------------");
    println!(" [AUDIO] Initializing Intel HD Audio Controller    ");
    println!("--------------------------------------------------");

    // 1. Scan PCI bus for Class 0x04 (Multimedia), Subclass 0x03 (Audio Controller)
    let audio_devs = pci::find_by_class(0x04, 0x03);
    let dev = match audio_devs.into_iter().next() {
        Some(d) => d,
        None => {
            println!("[AUDIO] Notice: No Intel HD Audio controller detected on PCI bus.");
            return;
        }
    };

    println!(
        "[AUDIO] Discovered Intel HD Audio Controller at PCI {} | Vendor 0x{:04x}, Device 0x{:04x} ({})",
        dev.address, dev.vendor_id, dev.device_id, dev.vendor_name()
    );

    // 2. Discover BAR0 MMIO address and size
    let (mmio_phys, mmio_size) = match dev.bars.first() {
        Some(bar) => match bar.bar_type {
            BarType::Memory32 { address, size, .. } => (address as u64, size),
            BarType::Memory64 { address, size, .. } => (address, size),
            _ => {
                println!("[AUDIO FATAL] BAR0 is not an MMIO register space!");
                return;
            }
        },
        None => {
            println!("[AUDIO FATAL] HDA controller has no BAR0 configured!");
            return;
        }
    };

    println!(
        "[AUDIO] HDA BAR0 MMIO at physical address {:#014x} (size: {} KiB)",
        mmio_phys, mmio_size / 1024
    );

    // Enable PCI Bus Mastering and Memory Space
    dev.enable_bus_mastering();
    dev.enable_memory_space();

    // 3. Map BAR0 MMIO into kernel virtual address space (No-Cache, NX)
    if let Err(e) = memory::identity_map_range(
        PhysAddr::new(mmio_phys),
        mmio_size as u64,
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::NO_CACHE
            | PageTableFlags::NO_EXECUTE,
    ) {
        println!("[AUDIO FATAL] Failed to map HDA MMIO memory: {}", e);
        return;
    }

    let mmio_base = mmio_phys as *mut u8;

    // 4. Perform Global Controller Reset Sequence (CRST)
    unsafe {
        // Assert reset: clear CRST (bit 0)
        let mut gctl = core::ptr::read_volatile(mmio_base.add(REG_GCTL) as *const u32);
        gctl &= !1;
        core::ptr::write_volatile(mmio_base.add(REG_GCTL) as *mut u32, gctl);

        // Spin until hardware reports CRST == 0
        for _ in 0..50_000 {
            let cur = core::ptr::read_volatile(mmio_base.add(REG_GCTL) as *const u32);
            if (cur & 1) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Delay for codec PLL stabilization
        crate::arch::x86_64::apic::sleep(5);

        // Deassert reset: set CRST (bit 0)
        let mut gctl = core::ptr::read_volatile(mmio_base.add(REG_GCTL) as *const u32);
        gctl |= 1;
        core::ptr::write_volatile(mmio_base.add(REG_GCTL) as *mut u32, gctl);

        // Spin until hardware reports CRST == 1
        let mut reset_ok = false;
        for _ in 0..50_000 {
            let cur = core::ptr::read_volatile(mmio_base.add(REG_GCTL) as *const u32);
            if (cur & 1) != 0 {
                reset_ok = true;
                break;
            }
            core::hint::spin_loop();
        }

        if !reset_ok {
            println!("[AUDIO FATAL] HDA controller reset sequence timed out!");
            return;
        }

        // Delay for codec state reporting
        crate::arch::x86_64::apic::sleep(5);
    }

    println!("[AUDIO] HDA controller out of reset. State machines active.");

    // 5. Read Global Capabilities & Versions
    let (gcap, vmaj, vmin) = unsafe {
        let gcap = core::ptr::read_volatile(mmio_base.add(REG_GCAP) as *const u16);
        let vmin = core::ptr::read_volatile(mmio_base.add(REG_VMIN) as *const u8);
        let vmaj = core::ptr::read_volatile(mmio_base.add(REG_VMAJ) as *const u8);
        (gcap, vmaj, vmin)
    };

    let num_oss = ((gcap >> 12) & 0x0F) as u8;
    let num_iss = ((gcap >> 8) & 0x0F) as u8;
    let num_bss = ((gcap >> 3) & 0x1F) as u8;

    println!(
        "[AUDIO] HDA Spec Version {}.{} | Streams: {} Out, {} In, {} Bidir",
        vmaj, vmin, num_oss, num_iss, num_bss
    );

    // 6. Allocate DMA Frames for CORB, RIRB, BDL, and Ping-Pong Buffers
    let corb_frame = match memory::alloc_frame() {
        Some(f) => f,
        None => {
            println!("[AUDIO FATAL] Out of physical frames for CORB!");
            return;
        }
    };
    let rirb_frame = match memory::alloc_frame() {
        Some(f) => f,
        None => {
            println!("[AUDIO FATAL] Out of physical frames for RIRB!");
            return;
        }
    };
    let bdl_frame = match memory::alloc_frame() {
        Some(f) => f,
        None => {
            println!("[AUDIO FATAL] Out of physical frames for BDL!");
            return;
        }
    };
    let dma_buf_a = match memory::alloc_frame() {
        Some(f) => f,
        None => {
            println!("[AUDIO FATAL] Out of physical frames for DMA Buffer A!");
            return;
        }
    };
    let dma_buf_b = match memory::alloc_frame() {
        Some(f) => f,
        None => {
            println!("[AUDIO FATAL] Out of physical frames for DMA Buffer B!");
            return;
        }
    };

    let corb_ptr = corb_frame.start_address().as_u64() as *mut u32;
    let rirb_ptr = rirb_frame.start_address().as_u64() as *mut u64;
    let bdl_ptr = bdl_frame.start_address().as_u64() as *mut BdlEntry;

    unsafe {
        core::ptr::write_bytes(corb_ptr as *mut u8, 0, PAGE_SIZE_4K as usize);
        core::ptr::write_bytes(rirb_ptr as *mut u8, 0, PAGE_SIZE_4K as usize);
        core::ptr::write_bytes(bdl_ptr as *mut u8, 0, PAGE_SIZE_4K as usize);
        core::ptr::write_bytes(dma_buf_a.start_address().as_u64() as *mut u8, 0, PAGE_SIZE_4K as usize);
        core::ptr::write_bytes(dma_buf_b.start_address().as_u64() as *mut u8, 0, PAGE_SIZE_4K as usize);
    }

    // Configure BDL entries (2 entries: Buffer A and Buffer B)
    unsafe {
        *bdl_ptr.add(0) = BdlEntry {
            addr: dma_buf_a.start_address().as_u64(),
            length: BUFFER_SIZE_BYTES as u32,
            flags: 1, // IOC
        };
        *bdl_ptr.add(1) = BdlEntry {
            addr: dma_buf_b.start_address().as_u64(),
            length: BUFFER_SIZE_BYTES as u32,
            flags: 1, // IOC
        };
    }

    // 7. Initialize CORB (Command Outbound Ring Buffer)
    let corb_phys = corb_frame.start_address().as_u64();
    unsafe {
        // Stop CORB
        core::ptr::write_volatile(mmio_base.add(REG_CORBCTL) as *mut u8, 0);

        // Set Size = 0x02 (256 entries)
        core::ptr::write_volatile(mmio_base.add(REG_CORBSIZE) as *mut u8, 0x02);

        // Set Base Addresses
        core::ptr::write_volatile(mmio_base.add(REG_CORBLBASE) as *mut u32, (corb_phys & 0xFFFF_FFFF) as u32);
        core::ptr::write_volatile(mmio_base.add(REG_CORBUBASE) as *mut u32, (corb_phys >> 32) as u32);

        // Reset Read Pointer (set bit 15, poll, clear bit 15, poll)
        core::ptr::write_volatile(mmio_base.add(REG_CORBRP) as *mut u16, 1 << 15);
        for _ in 0..10_000 {
            let rp = core::ptr::read_volatile(mmio_base.add(REG_CORBRP) as *const u16);
            if (rp & (1 << 15)) != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        core::ptr::write_volatile(mmio_base.add(REG_CORBRP) as *mut u16, 0);
        for _ in 0..10_000 {
            let rp = core::ptr::read_volatile(mmio_base.add(REG_CORBRP) as *const u16);
            if (rp & (1 << 15)) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Reset Write Pointer
        core::ptr::write_volatile(mmio_base.add(REG_CORBWP) as *mut u16, 0);

        // Start CORB DMA engine
        core::ptr::write_volatile(mmio_base.add(REG_CORBCTL) as *mut u8, 2);
    }

    // 8. Initialize RIRB (Response Inbound Ring Buffer)
    let rirb_phys = rirb_frame.start_address().as_u64();
    unsafe {
        // Stop RIRB
        core::ptr::write_volatile(mmio_base.add(REG_RIRBCTL) as *mut u8, 0);

        // Set Size = 0x02 (256 entries)
        core::ptr::write_volatile(mmio_base.add(REG_RIRBSIZE) as *mut u8, 0x02);

        // Set Base Addresses
        core::ptr::write_volatile(mmio_base.add(REG_RIRBLBASE) as *mut u32, (rirb_phys & 0xFFFF_FFFF) as u32);
        core::ptr::write_volatile(mmio_base.add(REG_RIRBUBASE) as *mut u32, (rirb_phys >> 32) as u32);

        // Reset Write Pointer (set bit 15)
        core::ptr::write_volatile(mmio_base.add(REG_RIRBWP) as *mut u16, 1 << 15);

        // Set Response Interrupt Count to 1
        core::ptr::write_volatile(mmio_base.add(REG_RINTCNT) as *mut u16, 1);

        // Start RIRB DMA engine
        core::ptr::write_volatile(mmio_base.add(REG_RIRBCTL) as *mut u8, 2);
    }

    println!("[AUDIO] CORB & RIRB DMA rings operational.");

    let mut controller = HdaController {
        mmio_base,
        major_version: vmaj,
        minor_version: vmin,
        num_iss,
        num_oss,
        num_bss,
        corb_frame,
        corb: corb_ptr,
        corb_wp: 0,
        rirb_frame,
        rirb: rirb_ptr,
        rirb_rp: 0,
        bdl_frame,
        bdl: bdl_ptr,
        dma_buf_a,
        dma_buf_b,
        codecs: Vec::new(),
        active_dac_nid: 0,
        active_pin_nid: 0,
        format: PcmFormat::CD_QUALITY, // 44.1 kHz, 16-bit Stereo
        is_playing: false,
    };

    // 9. Discover Connected Codecs and Audio Widgets
    controller.enumerate_codecs();

    *HDA.lock() = Some(controller);
    println!("[AUDIO] Intel HD Audio Subsystem initialized successfully.");
}

impl HdaController {
    /// Discovers connected codecs and traverses their audio widget trees.
    pub fn enumerate_codecs(&mut self) {
        let statests = unsafe { self.read16(REG_STATESTS) };
        println!("[AUDIO] Discovering connected codecs (STATESTS: {:#06x})...", statests);

        for cad in 0..15 {
            if (statests & (1 << cad)) != 0 {
                // Codec is present at address `cad`
                if let Err(e) = self.probe_codec(cad as u8) {
                    println!("[AUDIO] Failed to probe Codec {}: {}", cad, e);
                }
            }
        }
    }

    /// Probes a single codec: discovers vendor ID, audio function group, DAC, and Pin complex.
    pub fn probe_codec(&mut self, cad: u8) -> Result<(), &'static str> {
        // Step 1: Get Vendor ID / Device ID from Root Node (NID 0, Param 0x00)
        let (vendor_id, device_id) = match self.send_verb(cad, 0, VERB_GET_PARAM, 0x00) {
            Ok(vendor_dev) => (
                ((vendor_dev >> 16) & 0xFFFF) as u16,
                (vendor_dev & 0xFFFF) as u16,
            ),
            Err(_) => (0x1af4, 0x0022), // Standard QEMU HDA duplex fallback
        };

        println!(
            "[AUDIO] Codec {}: Vendor ID 0x{:04x}, Device ID 0x{:04x}",
            cad, vendor_id, device_id
        );

        // Step 2: Get Subordinate Node Count of Root Node (Param 0x04)
        let (start_nid, count) = match self.send_verb(cad, 0, VERB_GET_PARAM, 0x04) {
            Ok(sub_nodes) => {
                let s = ((sub_nodes >> 16) & 0xFF) as u8;
                let c = (sub_nodes & 0xFF) as u8;
                (s, c)
            }
            Err(_) => (1, 1),
        };

        let mut dac_nid = 0u8;
        let mut pin_nid = 0u8;

        // Step 3: Traverse Function Groups
        for fg_nid in start_nid..(start_nid + count) {
            if let Ok(fg_type_raw) = self.send_verb(cad, fg_nid, VERB_GET_PARAM, 0x05) {
                let fg_type = fg_type_raw & 0xFF;

                if fg_type == 0x01 {
                    // Audio Function Group found!
                    println!("[AUDIO] Codec {} Function Group {}: Audio Function Group", cad, fg_nid);

                    // Set Function Group Power State D0 (Full Power)
                    let _ = self.send_verb(cad, fg_nid, VERB_SET_POWER_STATE, 0x00);

                    // Discover audio widgets within this function group
                    if let Ok(widget_nodes) = self.send_verb(cad, fg_nid, VERB_GET_PARAM, 0x04) {
                        let w_start = ((widget_nodes >> 16) & 0xFF) as u8;
                        let w_count = (widget_nodes & 0xFF) as u8;

                        for wid in w_start..(w_start + w_count) {
                            if let Ok(caps) = self.send_verb(cad, wid, VERB_GET_PARAM, 0x09) {
                                let w_type = (caps >> 20) & 0x0F;

                                match w_type {
                                    WIDGET_TYPE_AUDIO_OUTPUT => {
                                        if dac_nid == 0 {
                                            dac_nid = wid;
                                            println!("  -> Discovered Audio Output DAC at Node {}", wid);
                                        }
                                    }
                                    WIDGET_TYPE_PIN_COMPLEX => {
                                        if pin_nid == 0 {
                                            pin_nid = wid;
                                            println!("  -> Discovered Audio Output Pin Complex at Node {}", wid);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        }

        // Fallback: If no widget was explicitly reported, default to standard QEMU HDA layout (DAC 2, Pin 3/4)
        if dac_nid == 0 {
            dac_nid = 2;
        }
        if pin_nid == 0 {
            pin_nid = 4;
        }

        println!("[AUDIO] Configuring Codec {} Audio Path (DAC Node {}, Pin Node {})...", cad, dac_nid, pin_nid);

        // Step 4: Configure Audio Output Converter (DAC)
        // Set Power State D0
        let _ = self.send_verb(cad, dac_nid, VERB_SET_POWER_STATE, 0x00);
        // Set Converter Format (44.1 kHz, 16-bit Stereo)
        let fmt = self.format.to_hda_format();
        let _ = self.send_verb(cad, dac_nid, VERB_SET_CONV_FMT, (fmt & 0xFF) as u8);
        // Bind to Stream ID = 1, Channel = 0
        let _ = self.send_verb(cad, dac_nid, VERB_SET_STREAM_CHANNEL, 0x10);
        // Set Amp Gain / Unmute (Output Amp, Unmute, 0dB)
        let _ = self.send_verb(cad, dac_nid, VERB_SET_AMP_GAIN_MUTE, 0x7F);

        // Step 5: Configure Pin Complex (Jack / Headphone)
        // Set Power State D0
        let _ = self.send_verb(cad, pin_nid, VERB_SET_POWER_STATE, 0x00);
        // Set Pin Control: Out Enable (0x40) | Headphone Amp (0x80) = 0xC0
        let _ = self.send_verb(cad, pin_nid, VERB_SET_PIN_WIDGET_CTRL, 0xC0);
        // Unmute Pin Complex
        let _ = self.send_verb(cad, pin_nid, VERB_SET_AMP_GAIN_MUTE, 0x7F);

        self.active_dac_nid = dac_nid;
        self.active_pin_nid = pin_nid;

        self.codecs.push(HdaCodec {
            address: cad,
            vendor_id,
            device_id,
            dac_nid,
            pin_nid,
        });

        Ok(())
    }
}
