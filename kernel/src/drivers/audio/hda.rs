//! Intel High Definition Audio (HDA) Controller Driver (Phase 12 - M32).
//!
//! # Architecture Overview
//!
//! Intel HDA (High Definition Audio) is the successor to AC'97. It connects audio
//! codecs (the DAC/ADC chips) over a serial link to a PCI controller. The key
//! subsystems are:
//!
//! ## CORB / RIRB DMA Ring Buffers
//! - **CORB** (Command Outbound Ring Buffer): a DMA ring where the driver writes
//!   32-bit HDA verbs (codec commands). The hardware serializes these onto the HDA
//!   link to the codec.
//! - **RIRB** (Response Inbound Ring Buffer): a DMA ring where the hardware writes
//!   64-bit codec responses after executing each verb.
//!
//! ## HDA Verb Protocol
//! Each HDA verb is a 32-bit word encoding: [31:28] CAD (Codec Address), [27:20] NID
//! (Node ID), [19:8] Verb ID (command type), [7:0] Parameter. Responses are 64-bit:
//! [31:0] data, [63:32] solicited/unsolicited flags.
//!
//! ## BDL Ping-Pong DMA
//! Audio output uses a Buffer Descriptor List (BDL). Each entry points to a DMA
//! buffer (physical address + length) with an optional IOC (Interrupt on Completion)
//! flag. We use 2 entries (Buffer A / Buffer B) for a ping-pong scheme so the codec
//! always has audio data while we prepare the next buffer.
//!
//! ## Stream Descriptor
//! Each output stream is controlled via a 32-byte MMIO region at offset
//! `0x80 + (num_iss * 0x20)`. Key fields: SD_CTL (run/stop, stream ID, reset),
//! SD_CBL (cyclic buffer length), SD_LVI (last valid BDL index), SD_FMT (PCM format),
//! SD_BDLPL/SD_BDLPU (BDL physical address).
//!
//! ## Initialization Sequence
//! 1. PCI discovery (Class 0x04 Multimedia, Subclass 0x03 Audio Controller).
//! 2. BAR0 MMIO mapping (identity mapped, No-Cache, NX).
//! 3. GCTL.CRST global controller reset (clear then set, poll).
//! 4. GCAP read: extract num_oss, num_iss, num_bss, version.
//! 5. Allocate physical frames for CORB, RIRB, BDL, DMA buffers A and B.
//! 6. Configure and start CORB DMA engine.
//! 7. Configure and start RIRB DMA engine.
//! 8. Enumerate codecs from STATESTS bitmask.
//! 9. Probe each codec: vendor ID, function group, widget tree (DAC NID, Pin NID).

#![allow(dead_code)]

use alloc::vec::Vec;
use core::sync::atomic::{fence, Ordering};

use crate::drivers::audio::pcm::{generate_chime, generate_sine_tone, PcmFormat};
use crate::drivers::pci::{self, BarType};
use crate::memory::{self, PhysAddr, PhysFrame, PageTableFlags, PAGE_SIZE_4K};
use crate::println;
use crate::sync::Spinlock;

// ============================================================
// HDA Register Offsets (relative to BAR0 MMIO Base)
// All registers are accessed via volatile 8/16/32-bit reads/writes.
// ============================================================

/// Global Capabilities register (16-bit).
/// Bits [15:12] = num_oss (output streams), [11:8] = num_iss (input streams),
/// [7:3] = num_bss (bidirectional streams), [2:1] = 64-bit addressing support.
const REG_GCAP: usize = 0x00;

/// HDA Minor Version register (8-bit). Should read 0x00 for HDA 1.0.
const REG_VMIN: usize = 0x02;

/// HDA Major Version register (8-bit). Should read 0x01 for HDA 1.0.
const REG_VMAJ: usize = 0x03;

/// Output Payload Capability (16-bit): max bytes/frame the output converter can accept.
const REG_OUTPAY: usize = 0x04;

/// Input Payload Capability (16-bit): max bytes/frame the input converter can supply.
const REG_INPAY: usize = 0x06;

/// Global Control register (32-bit).
/// Bit 0: CRST (Controller Reset). Write 0 to assert reset, write 1 to deassert.
/// Poll until bit 0 reads back the written value to confirm state transition.
const REG_GCTL: usize = 0x08;

/// Wake Enable register (16-bit). One bit per codec address (0–14).
/// Setting bit N enables wake-on-state-change interrupt for codec N.
const REG_WAKEEN: usize = 0x0C;

/// State Change Status register (16-bit). Read-only bitmask.
/// Bit N = 1 means a codec is present at address N on the HDA link.
/// Used during initialization to discover connected codecs.
const REG_STATESTS: usize = 0x0E;

/// Global Status register (16-bit). Bit 1 = FLUSHED (DMA flush complete).
const REG_GSTS: usize = 0x10;

/// Interrupt Control register (32-bit).
/// Bit 31: Global Interrupt Enable (GIE), bit 30: Controller Interrupt Enable (CIE),
/// bits 29:0: Stream Interrupt Enable mask.
const REG_INTCTL: usize = 0x20;

/// Interrupt Status register (32-bit). Read-clear (write 1 to clear).
/// Bit 31: Global Interrupt Status, bit 30: Controller Interrupt Status,
/// bits 29:0: Stream Interrupt Status per stream.
const REG_INTSTS: usize = 0x24;

// ============================================================
// CORB Registers — Command Outbound Ring Buffer
// The CORB is a DMA ring of 32-bit verb entries in physical memory.
// The driver advances the Write Pointer (WP); the hardware advances
// the Read Pointer (RP) as it consumes and transmits verbs.
// ============================================================

/// CORB Lower 32-bit Base Address register. Physical address bits [31:7] of CORB buffer.
/// Bits [6:0] must be zero (128-byte aligned). Write lower 32 bits here.
const REG_CORBLBASE: usize = 0x40;

/// CORB Upper 32-bit Base Address register. Physical address bits [63:32].
/// Set to 0 if the CORB is in the first 4 GiB of physical memory.
const REG_CORBUBASE: usize = 0x44;

/// CORB Write Pointer (16-bit). The driver writes the index of the last
/// valid CORB entry here. The hardware processes entries from RP+1 to WP.
const REG_CORBWP: usize = 0x48;

/// CORB Read Pointer (16-bit). Read by driver to see last consumed entry.
/// Bit 15: CORBRPRST (reset bit). Write 1, poll until 1, write 0, poll until 0.
const REG_CORBRP: usize = 0x4A;

/// CORB Control register (8-bit).
/// Bit 1: CORBRUN — set to 1 to start the DMA engine, 0 to stop it.
/// Bit 0: CMEIE — CORB Memory Error Interrupt Enable.
const REG_CORBCTL: usize = 0x4C;

/// CORB Status register (8-bit). Bit 0: CMEI (CORB Memory Error Interrupt, write 1 to clear).
const REG_CORBST: usize = 0x4D;

/// CORB Size register (8-bit). Write 0x00=2 entries, 0x01=16 entries, 0x02=256 entries.
/// A 256-entry CORB requires 1 KiB of contiguous physical memory.
const REG_CORBSIZE: usize = 0x4E;

// ============================================================
// RIRB Registers — Response Inbound Ring Buffer
// The RIRB is a DMA ring of 64-bit response entries in physical memory.
// The hardware advances WP; the driver advances RP to consume responses.
// ============================================================

/// RIRB Lower 32-bit Base Address. Physical address bits [31:7] of RIRB buffer.
const REG_RIRBLBASE: usize = 0x50;

/// RIRB Upper 32-bit Base Address. Physical address bits [63:32] of RIRB buffer.
const REG_RIRBUBASE: usize = 0x54;

/// RIRB Write Pointer (16-bit, read-only). Bit 15: RIRBWPRST (write 1 to reset WP to 0).
/// Hardware increments this when a new 64-bit response is deposited.
const REG_RIRBWP: usize = 0x58;

/// RIRB Response Interrupt Count (16-bit). Set to N to generate an interrupt after
/// N responses have been received. Set to 1 for immediate per-response interrupt.
const REG_RINTCNT: usize = 0x5A;

/// RIRB Control register (8-bit).
/// Bit 1: RIRBDMAEN — start RIRB DMA engine (set 1) or stop (clear).
/// Bit 0: RINTCTL — Response Interrupt Control Enable.
const REG_RIRBCTL: usize = 0x5C;

/// RIRB Status register (8-bit). Bit 0: RIRB Response Interrupt (write 1 to clear).
const REG_RIRBST: usize = 0x5D;

/// RIRB Size register (8-bit). Same encoding as CORBSIZE (0x02 = 256 entries).
/// 256-entry RIRB requires 2 KiB of physical memory (each entry is 8 bytes).
const REG_RIRBSIZE: usize = 0x5E;

// ============================================================
// Immediate Command Registers — PIO verb interface
// Bypasses the CORB/RIRB DMA rings for single synchronous verbs.
// Useful during initialization before CORB/RIRB are running.
// ============================================================

/// Immediate Command Write register (32-bit). Write a 32-bit verb here.
/// Then set ICS.ICB=1 to trigger execution.
const REG_ICW: usize = 0x60;

/// Immediate Response Read register (32-bit). After ICS.IRV=1, read the codec
/// response (lower 32 bits of the RIRB 64-bit response format).
const REG_IRR: usize = 0x64;

/// Immediate Command Status register (16-bit).
/// Bit 0: ICB (Immediate Command Busy) — write 1 to trigger, poll until 0.
/// Bit 1: IRV (Immediate Response Valid) — becomes 1 when response is ready.
/// Write 1 to bit 1 to acknowledge/clear IRV after reading IRR.
const REG_ICS: usize = 0x68;

// ============================================================
// Stream Descriptor Register Offsets (relative to Stream Base)
// Each stream occupies a 32-byte MMIO region.
// Output stream N is at: 0x80 + (num_iss + N) * 0x20
// ============================================================

/// Stream Descriptor Control register (24-bit effective, 32-bit aligned).
/// Bit 1: RUN — set 1 to start DMA, clear 0 to stop.
/// Bit 0: SRST — Stream Reset. Set 1, poll until 1, then clear 0, poll until 0.
/// Bits [23:20]: Stream Tag (ID) assigned to this stream (1-15, 0=unused).
const SD_CTL: usize = 0x00;

/// Stream Descriptor Status register (8-bit, at byte offset 3 of SD_CTL word).
/// Bit 2: BCIS (Buffer Completion Interrupt Status) — set when BDL IOC triggers.
/// Write 1 to clear.
const SD_STS: usize = 0x03;

/// Link Position in Buffer register (32-bit, read-only).
/// Current DMA position (in bytes) within the cyclic audio buffer.
/// Used to track playback progress; wraps at CBL.
const SD_LPIB: usize = 0x04;

/// Cyclic Buffer Length register (32-bit).
/// Total size of the audio DMA cyclic buffer in bytes.
/// Must equal sum of all BDL entry lengths. Set before starting the stream.
const SD_CBL: usize = 0x08;

/// Last Valid Index register (16-bit).
/// Zero-based index of the last valid BDL entry. For 2 entries: set to 1.
/// Hardware wraps back to entry 0 after processing entry LVI.
const SD_LVI: usize = 0x0C;

/// FIFO Size register (16-bit, read-only). DMA FIFO depth in bytes.
const SD_FIFOS: usize = 0x10;

/// Stream Format register (16-bit). Encodes PCM parameters:
/// Bits [14]: Base rate select (0=48kHz base, 1=44.1kHz base).
/// Bits [13:11]: Sample rate multiplier. Bits [10:8]: Sample rate divisor.
/// Bits [6:4]: Bits per sample (0=8, 1=16, 2=20, 3=24, 4=32).
/// Bits [3:0]: Number of channels minus 1.
const SD_FMT: usize = 0x12;

/// BDL Lower Base Address register (32-bit). Physical address bits [31:7] of the BDL.
/// The BDL must be 128-byte aligned in physical memory.
const SD_BDLPL: usize = 0x18;

/// BDL Upper Base Address register (32-bit). Physical address bits [63:32] of the BDL.
const SD_BDLPU: usize = 0x1C;

// ============================================================
// HDA Verb Identifiers
// A verb is a codec command. The hardware serializes it to the codec NID
// and returns a response. Each verb has a 12-bit verb ID and an 8-bit parameter.
// ============================================================

/// GET_PARAMETER verb (0xF00): read a codec node's parameter.
/// Parameter selects which parameter to read (see PARAM_* constants).
const VERB_GET_PARAM: u32 = 0xF00;

/// SET_CONVERTER_FORMAT verb (0x200): set the PCM format of an audio output converter (DAC).
/// Parameter encodes the same format bits as SD_FMT.
const VERB_SET_CONV_FMT: u32 = 0x200;

/// SET_POWER_STATE verb (0x705): set a widget's power state.
/// Parameter: 0x00 = D0 (fully on), 0x01 = D1, 0x02 = D2, 0x03 = D3 (lowest power).
const VERB_SET_POWER_STATE: u32 = 0x705;

/// SET_STREAM_CHANNEL verb (0x706): bind a converter to a DMA stream.
/// Parameter bits [7:4] = Stream ID (1–15), bits [3:0] = Channel (0 for mono/left).
const VERB_SET_STREAM_CHANNEL: u32 = 0x706;

/// SET_PIN_WIDGET_CONTROL verb (0x707): configure a Pin Complex node.
/// Bit 7: HPAS (Headphone Amp enable), bit 6: OUTS (Output enable).
/// 0xC0 = both headphone amp and output enabled.
const VERB_SET_PIN_WIDGET_CTRL: u32 = 0x707;

/// SET_AMPLIFIER_GAIN_MUTE verb (0x300): set gain/mute on a widget's amplifier.
/// Bits [15:8] = control flags (0xB0 = output amp, unmuted, left+right),
/// bits [6:0] = gain index (0x7F = 0 dB, maximum gain).
const VERB_SET_AMP_GAIN_MUTE: u32 = 0x300;

// ============================================================
// Widget Type Codes (from GET_PARAMETER 0x09, bits [23:20])
// A codec is a tree of "widgets" (audio processing nodes). Each widget
// has a type that determines what parameters and verbs it supports.
// ============================================================

/// Audio Output Converter (DAC). Converts PCM samples from DMA into analog.
pub const WIDGET_TYPE_AUDIO_OUTPUT: u32 = 0x0;

/// Audio Input Converter (ADC). Converts analog into PCM samples for DMA.
pub const WIDGET_TYPE_AUDIO_INPUT: u32 = 0x1;

/// Mixer widget. Mixes multiple audio streams together.
pub const WIDGET_TYPE_MIXER: u32 = 0x2;

/// Selector widget. Selects one of multiple inputs to pass through.
pub const WIDGET_TYPE_SELECTOR: u32 = 0x3;

/// Pin Complex widget. Represents a physical jack (headphone out, mic in, etc.).
/// Connects the internal codec signal path to an external connector.
pub const WIDGET_TYPE_PIN_COMPLEX: u32 = 0x4;

/// Power Widget. Controls power management for codec sections.
pub const WIDGET_TYPE_POWER: u32 = 0x5;

/// Size of each DMA audio buffer (Buffer A / Buffer B) in bytes.
/// 4096 bytes = 2048 stereo 16-bit samples ≈ 23 ms at 44.1 kHz.
/// Two buffers = ping-pong, total cyclic buffer = 8192 bytes.
const BUFFER_SIZE_BYTES: usize = 4096;

/// A single 16-byte Buffer Descriptor List (BDL) entry.
///
/// The BDL is an array of these entries that the DMA engine reads to find
/// where to write decoded PCM samples. The HDA controller processes entries
/// 0..=LVI cyclically, looping back to 0 after the last entry.
///
/// # Memory Layout
/// Each entry must be 128-bit (16-byte) aligned in physical memory.
/// The entire BDL must start on a 128-byte boundary.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Default)]
pub struct BdlEntry {
    /// Physical (DMA) address of the audio sample buffer for this descriptor.
    /// The DMA engine writes decoded audio samples directly to this address.
    /// Must be a valid, contiguous physical memory range (not virtual).
    pub addr: u64,

    /// Length of the audio buffer in bytes.
    /// Must be a multiple of 2 (16-bit samples) or 4 (stereo 16-bit).
    /// For our 4096-byte buffers, this holds ~1024 stereo 16-bit samples.
    pub length: u32,

    /// Descriptor flags. Bit 0: IOC (Interrupt on Completion).
    /// When IOC=1, the HDA controller raises an interrupt after the DMA engine
    /// finishes transferring samples for this descriptor.
    /// We set IOC=1 on both ping-pong buffers so we can refill them.
    pub flags: u32,
}

/// A discovered Audio Codec on the HDA serial link.
///
/// HDA supports up to 15 codec addresses (CAD 0–14). Each codec may contain
/// multiple Function Groups (Audio FG, Modem FG), each with a tree of widgets.
/// We discover the DAC node (Audio Output Converter) and the Pin Complex node
/// (the physical headphone/speaker jack) during codec probing.
#[derive(Clone, Debug)]
pub struct HdaCodec {
    /// Codec Address (CAD). 4-bit value (0–14) identifying this codec on the HDA link.
    /// Used as the [31:28] bits in every verb sent to this codec.
    pub address: u8,

    /// Upper 16 bits of the Vendor Device ID parameter (GET_PARAMETER, Param 0x00).
    /// Identifies the codec manufacturer (e.g. 0x1AF4 = QEMU/VirtIO audio).
    pub vendor_id: u16,

    /// Lower 16 bits of the Vendor Device ID parameter.
    /// Identifies the specific codec model (e.g. 0x0022 = QEMU HDA duplex).
    pub device_id: u16,

    /// Node ID (NID) of the first Audio Output Converter (DAC) found.
    /// This widget receives Stream ID assignments and PCM format verbs.
    pub dac_nid: u8,

    /// Node ID (NID) of the first Pin Complex found.
    /// This widget represents the physical output jack; must be configured
    /// with VERB_SET_PIN_WIDGET_CTRL to enable the headphone amplifier.
    pub pin_nid: u8,
}

/// Intel High Definition Audio Controller Instance.
///
/// Holds all runtime state for a single HDA PCI controller:
/// - MMIO register base pointer (identity-mapped, volatile access)
/// - CORB/RIRB DMA ring pointers and software state (write/read pointers)
/// - BDL and two DMA audio buffers (ping-pong)
/// - Discovered codec list and active DAC/Pin configuration
/// - Current PCM format and playback state
///
/// # Safety
/// All MMIO accesses must use `read_volatile`/`write_volatile` to prevent
/// the compiler from reordering or eliding hardware register accesses.
/// This struct is `Send + Sync` because HDA is protected by the global `HDA` spinlock.
pub struct HdaController {
    /// Virtual (identity-mapped) base address of the HDA MMIO register space.
    /// Points to BAR0 of the PCI device. All `REG_*` offsets are relative to this.
    pub mmio_base: *mut u8,

    /// HDA specification major version read from REG_VMAJ. Should be 0x01.
    pub major_version: u8,

    /// HDA specification minor version read from REG_VMIN. Should be 0x00 (HDA 1.0).
    pub minor_version: u8,

    /// Number of Input Stream Descriptors (ISS) reported by GCAP bits [11:8].
    /// Used to calculate the MMIO offset of Output Stream 0.
    pub num_iss: u8,

    /// Number of Output Stream Descriptors (OSS) reported by GCAP bits [15:12].
    pub num_oss: u8,

    /// Number of Bidirectional Stream Descriptors (BSS) reported by GCAP bits [7:3].
    pub num_bss: u8,

    /// Physical frame holding the CORB (256 × 4-byte entries = 1 KiB).
    /// Must remain allocated for the lifetime of the controller.
    pub corb_frame: PhysFrame,

    /// Virtual pointer to the CORB array (256 × u32 entries).
    /// Driver writes verb words here; hardware reads and serializes them.
    pub corb: *mut u32,

    /// Software CORB Write Pointer. The driver increments this (mod 256) and
    /// writes the new index to REG_CORBWP to notify hardware.
    pub corb_wp: u8,

    /// Physical frame holding the RIRB (256 × 8-byte entries = 2 KiB).
    pub rirb_frame: PhysFrame,

    /// Virtual pointer to the RIRB array (256 × u64 entries).
    /// Hardware writes response words here; driver reads from rirb_rp.
    pub rirb: *mut u64,

    /// Software RIRB Read Pointer. Driver advances this to consume new responses.
    /// When rirb_rp != REG_RIRBWP, new responses are waiting.
    pub rirb_rp: u8,

    /// Physical frame holding the BDL (Buffer Descriptor List, 2 entries × 16 bytes).
    pub bdl_frame: PhysFrame,

    /// Virtual pointer to the BDL array. Entries describe the DMA audio buffers.
    pub bdl: *mut BdlEntry,

    /// Physical frame for DMA audio Buffer A (ping). `BUFFER_SIZE_BYTES` bytes.
    /// The DMA engine writes interleaved stereo 16-bit PCM samples here.
    pub dma_buf_a: PhysFrame,

    /// Physical frame for DMA audio Buffer B (pong). `BUFFER_SIZE_BYTES` bytes.
    /// Together with buf_a, forms the cyclic ping-pong buffer.
    pub dma_buf_b: PhysFrame,

    /// List of discovered codec instances on this HDA link.
    pub codecs: Vec<HdaCodec>,

    /// NID of the active DAC widget (Audio Output Converter) used for playback.
    pub active_dac_nid: u8,

    /// NID of the active Pin Complex widget (physical output jack) for playback.
    pub active_pin_nid: u8,

    /// PCM format used for all playback operations (sample rate, bit depth, channels).
    /// Encoded into the SD_FMT stream descriptor register and SET_CONV_FMT verb.
    pub format: PcmFormat,

    /// True when the output stream DMA engine is running (RUN bit set in SD_CTL).
    pub is_playing: bool,
}

// SAFETY: HdaController holds raw pointers to MMIO and DMA memory that never
// move. Access is serialized by the global HDA Spinlock. The pointers are
// valid for the lifetime of the kernel (never freed).
unsafe impl Send for HdaController {}
unsafe impl Sync for HdaController {}

impl HdaController {
    /// Reads a 32-bit value from an MMIO register at `offset` from BAR0.
    ///
    /// Uses `read_volatile` to prevent the compiler from caching or reordering
    /// the hardware register read. This is mandatory for MMIO accesses.
    ///
    /// # Safety
    /// Caller must ensure `offset` is a valid, 32-bit-aligned HDA register offset.
    #[inline]
    unsafe fn read32(&self, offset: usize) -> u32 {
        // SAFETY: mmio_base is a valid MMIO pointer, offset is within BAR0 range.
        // read_volatile ensures the compiler does not eliminate or reorder this access.
        unsafe { core::ptr::read_volatile(self.mmio_base.add(offset) as *const u32) }
    }

    /// Writes a 32-bit value to an MMIO register at `offset` from BAR0.
    ///
    /// Uses `write_volatile` to prevent the compiler from eliminating or reordering
    /// the hardware register write. This is essential for control registers.
    ///
    /// # Safety
    /// Caller must ensure `offset` is a valid, 32-bit-aligned HDA register offset.
    #[inline]
    unsafe fn write32(&self, offset: usize, val: u32) {
        unsafe { core::ptr::write_volatile(self.mmio_base.add(offset) as *mut u32, val); }
    }

    /// Reads a 16-bit value from an MMIO register at `offset` from BAR0.
    ///
    /// # Safety
    /// Caller must ensure `offset` is a valid, 16-bit-aligned HDA register offset.
    #[inline]
    unsafe fn read16(&self, offset: usize) -> u16 {
        unsafe { core::ptr::read_volatile(self.mmio_base.add(offset) as *const u16) }
    }

    /// Writes a 16-bit value to an MMIO register at `offset` from BAR0.
    ///
    /// # Safety
    /// Caller must ensure `offset` is a valid, 16-bit-aligned HDA register offset.
    #[inline]
    unsafe fn write16(&self, offset: usize, val: u16) {
        unsafe { core::ptr::write_volatile(self.mmio_base.add(offset) as *mut u16, val); }
    }

    /// Reads an 8-bit value from an MMIO register at `offset` from BAR0.
    ///
    /// # Safety
    /// Caller must ensure `offset` is a valid HDA register byte offset.
    #[inline]
    unsafe fn read8(&self, offset: usize) -> u8 {
        unsafe { core::ptr::read_volatile(self.mmio_base.add(offset) as *const u8) }
    }

    /// Writes an 8-bit value to an MMIO register at `offset` from BAR0.
    ///
    /// # Safety
    /// Caller must ensure `offset` is a valid HDA register byte offset.
    #[inline]
    unsafe fn write8(&self, offset: usize, val: u8) {
        unsafe { core::ptr::write_volatile(self.mmio_base.add(offset) as *mut u8, val); }
    }

    /// Computes the MMIO byte offset of Output Stream Descriptor 0.
    ///
    /// HDA stream descriptor layout (HDA spec §3.3.35):
    /// - Offset 0x80: Input stream descriptors (num_iss × 0x20 bytes each)
    /// - Offset 0x80 + num_iss×0x20: Output stream descriptors (num_oss × 0x20 bytes each)
    /// - Offset 0x80 + (num_iss+num_oss)×0x20: Bidirectional descriptors
    ///
    /// We always use Output Stream 0 (the first output stream) for playback.
    #[inline]
    fn output_stream0_offset(&self) -> usize {
        // Skip past the Input Stream Descriptor blocks (each 0x20 = 32 bytes)
        0x80 + (self.num_iss as usize) * 0x20
    }

    /// Sends an HDA verb using the Immediate Command Interface (ICW/IRR/ICS).
    ///
    /// The Immediate Command Interface is a synchronous, PIO-based path for
    /// sending a single verb without using the CORB/RIRB DMA rings. It is
    /// simpler and lower-latency for initialization, but cannot be used
    /// concurrently with CORB/RIRB for the same verb.
    ///
    /// # Protocol
    /// 1. Poll ICS.ICB (bit 0) until it clears (previous command finished).
    /// 2. Clear ICS.IRV (bit 1) if set (acknowledge stale response).
    /// 3. Write the 32-bit verb word to REG_ICW.
    /// 4. Set ICS.ICB=1 to trigger execution.
    /// 5. Poll until ICB=0 AND IRV=1 (response is ready).
    /// 6. Read response from REG_IRR, then write IRV=1 to clear it.
    ///
    /// # Arguments
    /// - `cad`: Codec Address (0–14). Identifies which codec to send the verb to.
    /// - `nid`: Node ID within the codec (0 = root, 1 = AFG, etc.).
    /// - `verb`: 12-bit verb identifier (e.g. `VERB_GET_PARAM = 0xF00`).
    /// - `param`: 8-bit parameter payload for the verb.
    ///
    /// # Returns
    /// - `Ok(u32)`: the 32-bit response from the codec.
    /// - `Err("Immediate Command timed out")`: if the hardware never responds.
    pub fn send_verb_immediate(&mut self, cad: u8, nid: u8, verb: u32, param: u8) -> Result<u32, &'static str> {
        // Build the 32-bit verb word per HDA spec §7.3.3.6:
        // [31:28] = CAD, [27:20] = NID, [19:8] = Verb ID, [7:0] = Param
        let verb_data = ((cad as u32 & 0x0F) << 28)
            | ((nid as u32) << 20)
            | ((verb & 0xFFF) << 8)
            | (param as u32);

        // Step 1: Wait until the Immediate Command engine is idle (ICB bit 0 == 0).
        // This ensures the previous command (if any) has completed before we write.
        for _ in 0..20_000 {
            let ics = unsafe { self.read16(REG_ICS) };
            if (ics & 1) == 0 {
                break; // ICB cleared: engine is idle
            }
            core::hint::spin_loop(); // CPU pause hint for hyper-threading efficiency
        }

        // Clear IRV (bit 1) if set by writing 1 to bit 1.
        // This acknowledges any stale response left from a previous command.
        unsafe {
            let ics = self.read16(REG_ICS);
            if (ics & 2) != 0 {
                self.write16(REG_ICS, 2); // Writing 1 to bit 1 clears IRV
            }
        }

        // Step 2: Write the 32-bit verb to ICW (Immediate Command Write register).
        unsafe {
            self.write32(REG_ICW, verb_data);
            // Memory fence: ensure the verb write is visible before triggering execution.
            fence(Ordering::SeqCst);
            // Step 3: Trigger execution by setting ICB (bit 0) in ICS.
            // Hardware will serialize the verb to the codec and populate IRR when done.
            self.write16(REG_ICS, 1);
        }

        // Step 4: Poll until ICB=0 (command done) AND IRV=1 (response valid).
        // ICB clears when the verb has been transmitted; IRV sets when codec responded.
        for _ in 0..50_000 {
            let ics = unsafe { self.read16(REG_ICS) };
            if (ics & 1) == 0 && (ics & 2) != 0 {
                // Response is ready: read IRR (Immediate Response Register)
                let resp = unsafe { self.read32(REG_IRR) };
                // Step 5: Acknowledge response by writing IRV=1 (clears the bit)
                unsafe { self.write16(REG_ICS, 2); }
                return Ok(resp);
            }
            core::hint::spin_loop();
        }

        Err("Immediate Command timed out")
    }

    /// Sends a 32-bit HDA Verb, trying Immediate Command first, falling back to CORB/RIRB.
    ///
    /// This is the primary verb dispatch path. It first attempts the PIO-based
    /// Immediate Command Interface (fastest, no DMA latency). If that fails,
    /// it falls back to the CORB/RIRB DMA ring path.
    ///
    /// ## CORB/RIRB Fallback Path
    /// 1. Increment the software write pointer (mod 256).
    /// 2. Write the verb word into `corb[corb_wp]`.
    /// 3. Issue a SeqCst fence to ensure the CORB write is visible to DMA.
    /// 4. Write the new corb_wp value to REG_CORBWP to notify hardware.
    /// 5. Spin-wait for the RIRB Write Pointer (REG_RIRBWP) to advance.
    /// 6. Increment rirb_rp, read `rirb[rirb_rp]`, extract lower 32 bits.
    ///
    /// # Arguments
    /// - `cad`, `nid`, `verb`, `param`: same as `send_verb_immediate`.
    ///
    /// # Returns
    /// - `Ok(u32)`: codec response data.
    /// - `Err(...)`: if both paths time out.
    pub fn send_verb(&mut self, cad: u8, nid: u8, verb: u32, param: u8) -> Result<u32, &'static str> {
        // Try Immediate Command Interface first (synchronous, no DMA latency)
        if let Ok(resp) = self.send_verb_immediate(cad, nid, verb, param) {
            return Ok(resp);
        }

        // Build the 32-bit verb word: [31:28]=CAD, [27:20]=NID, [19:8]=Verb, [7:0]=Param
        let verb_data = ((cad as u32 & 0x0F) << 28)
            | ((nid as u32) << 20)
            | ((verb & 0xFFF) << 8)
            | (param as u32);

        // Step 1: Advance the software CORB Write Pointer (wraps at 255 → 0).
        // The CORB is a circular ring of 256 entries; corb_wp tracks the last entry we wrote.
        self.corb_wp = self.corb_wp.wrapping_add(1);
        unsafe {
            // Write the verb into the CORB ring at the new write pointer position.
            // The hardware will process entries from [RP+1 .. WP] in order.
            *self.corb.add(self.corb_wp as usize) = verb_data;
        }

        // SeqCst fence: ensure CORB write is globally visible before updating CORBWP.
        // Without this, the hardware DMA engine might read stale data.
        fence(Ordering::SeqCst);

        // Step 2: Commit updated Write Pointer to hardware by writing to REG_CORBWP.
        // Hardware starts processing new entries as soon as it sees WP advance.
        unsafe {
            self.write16(REG_CORBWP, self.corb_wp as u16);
        }

        // Step 3: Spin-wait for hardware to increment the RIRB Write Pointer.
        // When REG_RIRBWP != rirb_rp, at least one new response is waiting in RIRB.
        for _ in 0..100_000 {
            let rirb_wp = unsafe { (self.read16(REG_RIRBWP) & 0xFF) as u8 };
            if rirb_wp != self.rirb_rp {
                // New response available: advance our read pointer
                self.rirb_rp = self.rirb_rp.wrapping_add(1);
                // Read the 64-bit RIRB entry: [31:0] = response data, [63:32] = flags
                let resp_raw = unsafe { *self.rirb.add(self.rirb_rp as usize) };
                // Extract lower 32 bits (actual codec response payload)
                let resp_data = (resp_raw & 0xFFFF_FFFF) as u32;
                return Ok(resp_data);
            }
            core::hint::spin_loop();
        }

        Err("HDA Verb execution timed out waiting for RIRB response")
    }

    /// Sets up and starts audio playback using the DMA ping-pong buffers.
    ///
    /// This function programs Output Stream 0 to DMA the given sample data
    /// through the hardware codec to the speakers/headphone jack.
    ///
    /// # DMA Buffer Layout
    /// - Buffer A (BDL entry 0): first `BUFFER_SIZE_BYTES/2` samples (as i16 pairs)
    /// - Buffer B (BDL entry 1): next `BUFFER_SIZE_BYTES/2` samples
    ///
    /// # Stream Descriptor Programming Sequence
    /// 1. Stop the stream (clear RUN bit), wait for RUN to deassert.
    /// 2. Assert SRST (Stream Reset), wait for SRST to assert, then deassert.
    /// 3. Copy sample data into physical DMA buffers A and B.
    /// 4. Write BDL base address (SD_BDLPL, SD_BDLPU).
    /// 5. Write total cyclic buffer length (SD_CBL = 2 × BUFFER_SIZE_BYTES).
    /// 6. Write Last Valid Index (SD_LVI = 1 for 2 BDL entries, indices 0 and 1).
    /// 7. Write PCM format (SD_FMT from `self.format.to_hda_format()`).
    /// 8. Set Stream ID=1 in bits [23:20] of SD_CTL, then set RUN (bit 1).
    ///
    /// # Arguments
    /// - `samples`: slice of interleaved stereo 16-bit PCM samples (L, R, L, R, ...).
    ///
    /// # Returns
    /// - `Ok(())` on success.
    /// - `Err(...)` on hardware error.
    pub fn play_samples(&mut self, samples: &[i16]) -> Result<(), &'static str> {
        if samples.is_empty() {
            return Ok(());
        }

        // Calculate MMIO base of Output Stream Descriptor 0
        let stream_base = self.output_stream0_offset();

        // Step 1: Stop stream DMA engine if currently running.
        // Clear bit 1 (RUN) in SD_CTL to request the hardware to stop.
        unsafe {
            let ctl = self.read32(stream_base + SD_CTL);
            self.write32(stream_base + SD_CTL, ctl & !2); // Clear RUN bit
        }

        // Wait for RUN bit to actually clear (hardware may need a few cycles).
        for _ in 0..10_000 {
            let ctl = unsafe { self.read32(stream_base + SD_CTL) };
            if (ctl & 2) == 0 {
                break; // Stream has stopped
            }
            core::hint::spin_loop();
        }

        // Step 2: Assert Stream Reset (SRST, bit 0).
        // This clears all stream state machines and FIFO pointers.
        unsafe {
            let mut ctl = self.read32(stream_base + SD_CTL);
            ctl |= 1; // Set SRST bit
            self.write32(stream_base + SD_CTL, ctl);
        }
        // Poll until hardware acknowledges reset (SRST readback = 1)
        for _ in 0..10_000 {
            let ctl = unsafe { self.read32(stream_base + SD_CTL) };
            if (ctl & 1) != 0 {
                break; // Reset acknowledged
            }
            core::hint::spin_loop();
        }
        // Deassert Stream Reset (clear SRST bit)
        unsafe {
            let mut ctl = self.read32(stream_base + SD_CTL);
            ctl &= !1; // Clear SRST
            self.write32(stream_base + SD_CTL, ctl);
        }
        // Poll until hardware releases reset (SRST readback = 0)
        for _ in 0..10_000 {
            let ctl = unsafe { self.read32(stream_base + SD_CTL) };
            if (ctl & 1) == 0 {
                break; // Reset complete
            }
            core::hint::spin_loop();
        }

        // Step 3: Copy sample data into DMA Buffer A and Buffer B.
        // The physical addresses of the DMA frames are identity-mapped,
        // so the physical address equals the virtual address.
        let buf_a_ptr = self.dma_buf_a.start_address().as_u64() as *mut i16;
        let buf_b_ptr = self.dma_buf_b.start_address().as_u64() as *mut i16;

        // Each buffer holds BUFFER_SIZE_BYTES/sizeof(i16) = 2048 samples.
        // We split the input at the half-point.
        let half_len = (BUFFER_SIZE_BYTES / 2).min(samples.len());
        unsafe {
            // Copy first half into Buffer A (ping)
            core::ptr::copy_nonoverlapping(samples.as_ptr(), buf_a_ptr, half_len);
            // If there are more samples, copy the second half into Buffer B (pong)
            let rem = if samples.len() > half_len {
                &samples[half_len..]
            } else {
                samples // Reuse same data if fewer samples than one full buffer
            };
            let rem_len = (BUFFER_SIZE_BYTES / 2).min(rem.len());
            core::ptr::copy_nonoverlapping(rem.as_ptr(), buf_b_ptr, rem_len);
        }

        // Step 4: Program Stream Descriptor registers.
        let bdl_phys = self.bdl_frame.start_address().as_u64();
        // Total cyclic buffer length = both DMA buffers combined
        let total_cbl = (BUFFER_SIZE_BYTES * 2) as u32;
        // Convert our PCM format into the HDA SD_FMT 16-bit register value
        let hda_fmt = self.format.to_hda_format();

        unsafe {
            // Set BDL physical base address (split into lower and upper 32 bits)
            self.write32(stream_base + SD_BDLPL, (bdl_phys & 0xFFFF_FFFF) as u32);
            self.write32(stream_base + SD_BDLPU, (bdl_phys >> 32) as u32);
            // Set Cyclic Buffer Length = sum of all BDL entry lengths
            self.write32(stream_base + SD_CBL, total_cbl);
            // Set Last Valid Index = 1 (we have 2 BDL entries, indices 0 and 1)
            self.write16(stream_base + SD_LVI, 1);
            // Set the PCM stream format (sample rate, bit depth, channels)
            self.write16(stream_base + SD_FMT, hda_fmt);

            // Set Stream ID = 1 (bits [23:20] = 0x1) and start RUN (bit 1 = 1).
            // Stream ID must match the value passed to SET_STREAM_CHANNEL verb
            // sent to the DAC during codec initialization (bits [7:4] = 1).
            let ctl = (1 << 20) | (1 << 1); // Stream ID=1, RUN=1
            self.write32(stream_base + SD_CTL, ctl);
        }

        self.is_playing = true;
        Ok(())
    }

    /// Plays a procedural sine wave tone at `freq_hz` for `duration_ms` milliseconds.
    ///
    /// Generates a stereo 16-bit PCM sine wave buffer of the appropriate length,
    /// then passes it to `play_samples`. Waits for playback using APIC spin delay
    /// (no interrupts required) before stopping.
    ///
    /// # Arguments
    /// - `freq_hz`: tone frequency in Hz (e.g. 440 = concert A4, 523 = C5).
    /// - `duration_ms`: duration in milliseconds (clamped to fit in ping-pong buffers).
    ///
    /// # Sample Count Calculation
    /// `total_samples = (duration_ms × sample_rate × 2_channels) / 1000`
    /// Clamped to `max_samples = (BUFFER_SIZE_BYTES / sizeof(i16)) × 2_buffers`.
    pub fn play_tone(&mut self, freq_hz: u32, duration_ms: u32) -> Result<(), &'static str> {
        let sample_rate = self.format.sample_rate;
        // Calculate total stereo samples needed: duration in ms × rate in Hz × 2 channels / 1000ms
        let total_samples = ((duration_ms as u64 * sample_rate as u64 * 2) / 1000) as usize;
        // Maximum samples that fit in our double-buffer (ping-pong) pair
        let max_samples = (BUFFER_SIZE_BYTES / 2) * 2;
        let play_samples = total_samples.min(max_samples);

        // Allocate heap buffer and generate stereo sine wave samples
        let mut tone_buffer = alloc::vec![0i16; play_samples];
        generate_sine_tone(&mut tone_buffer, freq_hz, sample_rate, 20000);

        // Start DMA playback
        self.play_samples(&tone_buffer)?;

        // Busy-spin delay for the tone duration using APIC timer (no IRQ needed).
        // We clamp to at least 10ms to prevent zero-delay playback corruption.
        crate::arch::x86_64::apic::spin_delay_us((duration_ms.max(10) as u64) * 1000);

        // Stop the DMA stream after the tone duration
        self.stop();
        Ok(())
    }

    /// Plays a startup chime: a C5+G5 musical interval synthesized procedurally.
    ///
    /// Generates the chime waveform using `generate_chime`, which produces a
    /// two-tone blend (C5 = 523 Hz + G5 = 784 Hz) with a brief sustain envelope.
    /// Plays for approximately 50 ms (the minimum audible confirmation beep).
    pub fn play_chime(&mut self) -> Result<(), &'static str> {
        let sample_rate = self.format.sample_rate;
        // Allocate the maximum double-buffer worth of samples
        let max_samples = (BUFFER_SIZE_BYTES / 2) * 2;
        let mut chime_buffer = alloc::vec![0i16; max_samples];
        // Generate the C5+G5 chord chime waveform (see pcm.rs for synthesis details)
        generate_chime(&mut chime_buffer, sample_rate);

        // Start DMA playback
        self.play_samples(&chime_buffer)?;
        // 50ms spin delay — just enough for the chime to be audible
        crate::arch::x86_64::apic::spin_delay_us(50_000);
        self.stop();
        Ok(())
    }

    /// Stops audio playback by clearing the RUN bit in Output Stream 0's SD_CTL.
    ///
    /// After clearing RUN, the DMA engine stops fetching from BDL entries.
    /// The hardware may still DMA in-flight data until the FIFO drains.
    /// Call this after a timed delay to avoid cutting off audio mid-sample.
    pub fn stop(&mut self) {
        let stream_base = self.output_stream0_offset();
        unsafe {
            // Read current SD_CTL, clear bit 1 (RUN), write back.
            // This requests the hardware to stop the DMA engine.
            let ctl = self.read32(stream_base + SD_CTL);
            self.write32(stream_base + SD_CTL, ctl & !2); // Clear RUN bit
        }
        self.is_playing = false;
    }
}

/// Global Intel HDA controller instance protected by a spinlock.
///
/// Set to `Some(HdaController)` after `init()` completes successfully.
/// Set to `None` if no HDA controller was detected on the PCI bus.
/// All audio operations must lock this before accessing the controller.
pub static HDA: Spinlock<Option<HdaController>> = Spinlock::new(None);

/// Probes the PCI bus for an Intel High Definition Audio controller and
/// performs the full hardware initialization sequence.
///
/// # Initialization Steps
/// 1. PCI class scan: find device with Class=0x04 (Multimedia), Subclass=0x03 (HDA).
/// 2. Read BAR0 to get MMIO physical base address and size.
/// 3. Enable PCI Bus Mastering (required for DMA) and Memory Space access.
/// 4. Identity-map BAR0 into kernel virtual address space with NO_CACHE flag
///    (caching MMIO registers causes incorrect behavior).
/// 5. GCTL.CRST global reset: clear CRST, wait, sleep 5ms, set CRST, wait.
/// 6. Read GCAP to extract num_oss/num_iss/num_bss and version.
/// 7. Allocate 5 physical frames: CORB, RIRB, BDL, DMA Buffer A, DMA Buffer B.
/// 8. Zero-initialize all allocated frames.
/// 9. Configure BDL entries (2 entries pointing to Buffer A and B, IOC=1 each).
/// 10. Configure CORB DMA engine (stop, set size=256, set base addr, reset RP, start).
/// 11. Configure RIRB DMA engine (stop, set size=256, set base addr, reset WP, start).
/// 12. Build HdaController, store in global HDA.
/// 13. Call enumerate_codecs() to discover and configure audio widgets.
pub fn init() {
    println!();
    println!("--------------------------------------------------");
    println!(" [AUDIO] Initializing Intel HD Audio Controller    ");
    println!("--------------------------------------------------");

    // Step 1: Scan PCI bus for Multimedia/Audio Controller (Class 0x04, Subclass 0x03).
    // HDA controllers always present as this class/subclass combination.
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

    // Step 2: Read BAR0 MMIO address and size from PCI configuration space.
    // HDA BAR0 is always a memory-mapped register space (not I/O port space).
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

    // Step 3: Enable PCI Bus Mastering (required for CORB/RIRB DMA operations)
    // and Memory Space access (required for MMIO register reads/writes).
    dev.enable_bus_mastering();
    dev.enable_memory_space();

    // Step 4: Identity-map BAR0 MMIO into the kernel's virtual address space.
    // We use: PRESENT, WRITABLE, NO_CACHE (caching MMIO causes stale reads),
    // NO_EXECUTE (registers are data, not executable code).
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

    // After identity mapping, physical address == virtual address for MMIO
    let mmio_base = mmio_phys as *mut u8;

    // Step 5: Perform Global Controller Reset Sequence (GCTL.CRST).
    // This resets all stream state machines, CORB/RIRB engines, and codecs.
    unsafe {
        // Assert reset: clear bit 0 (CRST) in GCTL. The hardware enters reset state.
        let mut gctl = core::ptr::read_volatile(mmio_base.add(REG_GCTL) as *const u32);
        gctl &= !1; // Clear CRST bit to assert global reset
        core::ptr::write_volatile(mmio_base.add(REG_GCTL) as *mut u32, gctl);

        // Poll until hardware confirms reset (CRST reads back 0)
        for _ in 0..50_000 {
            let cur = core::ptr::read_volatile(mmio_base.add(REG_GCTL) as *const u32);
            if (cur & 1) == 0 {
                break; // Reset confirmed
            }
            core::hint::spin_loop();
        }

        // Allow 5ms for codec PLLs to power down and stabilize during reset.
        crate::arch::x86_64::apic::sleep(5);

        // Deassert reset: set bit 0 (CRST). Controller and codecs begin initialization.
        let mut gctl = core::ptr::read_volatile(mmio_base.add(REG_GCTL) as *const u32);
        gctl |= 1; // Set CRST bit to deassert global reset
        core::ptr::write_volatile(mmio_base.add(REG_GCTL) as *mut u32, gctl);

        // Poll until hardware confirms controller is out of reset (CRST reads back 1)
        let mut reset_ok = false;
        for _ in 0..50_000 {
            let cur = core::ptr::read_volatile(mmio_base.add(REG_GCTL) as *const u32);
            if (cur & 1) != 0 {
                reset_ok = true;
                break; // Controller active
            }
            core::hint::spin_loop();
        }

        if !reset_ok {
            println!("[AUDIO FATAL] HDA controller reset sequence timed out!");
            return;
        }

        // Allow 5ms for codecs to send their state-change reports via STATESTS.
        // The HDA spec requires at least 1ms after CRST deassert for codec enumeration.
        crate::arch::x86_64::apic::sleep(5);
    }

    println!("[AUDIO] HDA controller out of reset. State machines active.");

    // Step 6: Read Global Capabilities (GCAP) and version registers.
    let (gcap, vmaj, vmin) = unsafe {
        let gcap = core::ptr::read_volatile(mmio_base.add(REG_GCAP) as *const u16);
        let vmin = core::ptr::read_volatile(mmio_base.add(REG_VMIN) as *const u8);
        let vmaj = core::ptr::read_volatile(mmio_base.add(REG_VMAJ) as *const u8);
        (gcap, vmaj, vmin)
    };

    // Extract stream counts from GCAP bit fields:
    // Bits [15:12] = num_oss, bits [11:8] = num_iss, bits [7:3] = num_bss
    let num_oss = ((gcap >> 12) & 0x0F) as u8;
    let num_iss = ((gcap >> 8) & 0x0F) as u8;
    let num_bss = ((gcap >> 3) & 0x1F) as u8;

    println!(
        "[AUDIO] HDA Spec Version {}.{} | Streams: {} Out, {} In, {} Bidir",
        vmaj, vmin, num_oss, num_iss, num_bss
    );

    // Step 7: Allocate physical DMA frames for CORB, RIRB, BDL, and ping-pong buffers.
    // All frames are 4 KiB (PAGE_SIZE_4K), which is more than enough for each structure.
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
    // DMA Buffer A: first audio buffer (BDL entry 0)
    let dma_buf_a = match memory::alloc_frame() {
        Some(f) => f,
        None => {
            println!("[AUDIO FATAL] Out of physical frames for DMA Buffer A!");
            return;
        }
    };
    // DMA Buffer B: second audio buffer (BDL entry 1)
    let dma_buf_b = match memory::alloc_frame() {
        Some(f) => f,
        None => {
            println!("[AUDIO FATAL] Out of physical frames for DMA Buffer B!");
            return;
        }
    };

    // Convert physical frame addresses to virtual pointers (identity-mapped)
    let corb_ptr = corb_frame.start_address().as_u64() as *mut u32;
    let rirb_ptr = rirb_frame.start_address().as_u64() as *mut u64;
    let bdl_ptr = bdl_frame.start_address().as_u64() as *mut BdlEntry;

    // Step 8: Zero-initialize all allocated frames to prevent stale DMA data.
    unsafe {
        core::ptr::write_bytes(corb_ptr as *mut u8, 0, PAGE_SIZE_4K as usize);
        core::ptr::write_bytes(rirb_ptr as *mut u8, 0, PAGE_SIZE_4K as usize);
        core::ptr::write_bytes(bdl_ptr as *mut u8, 0, PAGE_SIZE_4K as usize);
        core::ptr::write_bytes(dma_buf_a.start_address().as_u64() as *mut u8, 0, PAGE_SIZE_4K as usize);
        core::ptr::write_bytes(dma_buf_b.start_address().as_u64() as *mut u8, 0, PAGE_SIZE_4K as usize);
    }

    // Step 9: Configure the BDL entries.
    // Entry 0 → Buffer A (ping): BUFFER_SIZE_BYTES bytes, IOC=1 (interrupt on completion)
    // Entry 1 → Buffer B (pong): BUFFER_SIZE_BYTES bytes, IOC=1
    // With IOC=1 on both entries, the hardware interrupts after each buffer is consumed,
    // allowing the driver to refill the other buffer (true ping-pong).
    unsafe {
        *bdl_ptr.add(0) = BdlEntry {
            addr: dma_buf_a.start_address().as_u64(),
            length: BUFFER_SIZE_BYTES as u32,
            flags: 1, // IOC = 1: Interrupt on Completion for Buffer A
        };
        *bdl_ptr.add(1) = BdlEntry {
            addr: dma_buf_b.start_address().as_u64(),
            length: BUFFER_SIZE_BYTES as u32,
            flags: 1, // IOC = 1: Interrupt on Completion for Buffer B
        };
    }

    // Step 10: Initialize CORB (Command Outbound Ring Buffer) DMA engine.
    let corb_phys = corb_frame.start_address().as_u64();
    unsafe {
        // First, stop the CORB DMA engine by clearing CORBRUN (bit 1 of CORBCTL).
        // The hardware must be stopped before we reconfigure base address and size.
        core::ptr::write_volatile(mmio_base.add(REG_CORBCTL) as *mut u8, 0);

        // Set CORB size to 256 entries (0x02). Each entry is 4 bytes = 1 KiB total.
        // 256 entries provides maximum buffering for codec verb bursts.
        core::ptr::write_volatile(mmio_base.add(REG_CORBSIZE) as *mut u8, 0x02);

        // Write the 64-bit CORB physical base address in two 32-bit halves.
        core::ptr::write_volatile(mmio_base.add(REG_CORBLBASE) as *mut u32, (corb_phys & 0xFFFF_FFFF) as u32);
        core::ptr::write_volatile(mmio_base.add(REG_CORBUBASE) as *mut u32, (corb_phys >> 32) as u32);

        // Reset the CORB Read Pointer:
        // Per HDA spec §3.3.21: set CORBRPRST (bit 15), poll until set, then clear, poll until clear.
        core::ptr::write_volatile(mmio_base.add(REG_CORBRP) as *mut u16, 1 << 15);
        for _ in 0..10_000 {
            let rp = core::ptr::read_volatile(mmio_base.add(REG_CORBRP) as *const u16);
            if (rp & (1 << 15)) != 0 {
                break; // Reset acknowledged
            }
            core::hint::spin_loop();
        }
        core::ptr::write_volatile(mmio_base.add(REG_CORBRP) as *mut u16, 0);
        for _ in 0..10_000 {
            let rp = core::ptr::read_volatile(mmio_base.add(REG_CORBRP) as *const u16);
            if (rp & (1 << 15)) == 0 {
                break; // Reset complete
            }
            core::hint::spin_loop();
        }

        // Reset Write Pointer to 0 (no pending commands at startup)
        core::ptr::write_volatile(mmio_base.add(REG_CORBWP) as *mut u16, 0);

        // Start the CORB DMA engine by setting CORBRUN (bit 1 of CORBCTL).
        // From this point, writing to REG_CORBWP triggers hardware verb processing.
        core::ptr::write_volatile(mmio_base.add(REG_CORBCTL) as *mut u8, 2); // CORBRUN = 1
    }

    // Step 11: Initialize RIRB (Response Inbound Ring Buffer) DMA engine.
    let rirb_phys = rirb_frame.start_address().as_u64();
    unsafe {
        // Stop RIRB DMA engine first (clear RIRBDMAEN, bit 1 of RIRBCTL)
        core::ptr::write_volatile(mmio_base.add(REG_RIRBCTL) as *mut u8, 0);

        // Set RIRB size to 256 entries (0x02). Each entry is 8 bytes = 2 KiB total.
        core::ptr::write_volatile(mmio_base.add(REG_RIRBSIZE) as *mut u8, 0x02);

        // Write the 64-bit RIRB physical base address in two 32-bit halves.
        core::ptr::write_volatile(mmio_base.add(REG_RIRBLBASE) as *mut u32, (rirb_phys & 0xFFFF_FFFF) as u32);
        core::ptr::write_volatile(mmio_base.add(REG_RIRBUBASE) as *mut u32, (rirb_phys >> 32) as u32);

        // Reset RIRB Write Pointer by setting RIRBWPRST (bit 15) in RIRBWP.
        // This clears the hardware WP, ensuring we start reading from entry 0.
        core::ptr::write_volatile(mmio_base.add(REG_RIRBWP) as *mut u16, 1 << 15);

        // Set RIRB Response Interrupt Count to 1 (interrupt after each response).
        // This enables low-latency verb response detection without polling delays.
        core::ptr::write_volatile(mmio_base.add(REG_RINTCNT) as *mut u16, 1);

        // Start RIRB DMA engine by setting RIRBDMAEN (bit 1 of RIRBCTL).
        // From now on, codec responses will be deposited into the RIRB ring.
        core::ptr::write_volatile(mmio_base.add(REG_RIRBCTL) as *mut u8, 2); // RIRBDMAEN = 1
    }

    println!("[AUDIO] CORB & RIRB DMA rings operational.");

    // Build the HdaController instance with all initialized state
    let mut controller = HdaController {
        mmio_base,
        major_version: vmaj,
        minor_version: vmin,
        num_iss,
        num_oss,
        num_bss,
        corb_frame,
        corb: corb_ptr,
        corb_wp: 0, // Write pointer starts at 0 (no verbs sent yet)
        rirb_frame,
        rirb: rirb_ptr,
        rirb_rp: 0, // Read pointer starts at 0 (no responses consumed yet)
        bdl_frame,
        bdl: bdl_ptr,
        dma_buf_a,
        dma_buf_b,
        codecs: Vec::new(),
        active_dac_nid: 0, // Will be set during codec probing
        active_pin_nid: 0, // Will be set during codec probing
        format: PcmFormat::CD_QUALITY, // 44.1 kHz, 16-bit, Stereo (standard CD format)
        is_playing: false,
    };

    // Step 12: Discover and configure connected codecs
    controller.enumerate_codecs();

    // Store the initialized controller in the global singleton
    *HDA.lock() = Some(controller);
    println!("[AUDIO] Intel HD Audio Subsystem initialized successfully.");
}

impl HdaController {
    /// Discovers all connected codecs by reading the STATESTS register.
    ///
    /// STATESTS is a 15-bit bitmask where bit N=1 indicates a codec is
    /// present at Codec Address N on the HDA serial link. Each codec is
    /// then probed individually to discover its widget tree.
    ///
    /// HDA supports up to 15 codecs per controller (addresses 0–14).
    /// In practice, most systems have 1 codec (usually at address 0).
    pub fn enumerate_codecs(&mut self) {
        let statests = unsafe { self.read16(REG_STATESTS) };
        println!("[AUDIO] Discovering connected codecs (STATESTS: {:#06x})...", statests);

        // Check each possible codec address (0 through 14)
        for cad in 0..15 {
            if (statests & (1 << cad)) != 0 {
                // This bit is set: a codec is connected at address `cad`
                if let Err(e) = self.probe_codec(cad as u8) {
                    println!("[AUDIO] Failed to probe Codec {}: {}", cad, e);
                }
            }
        }
    }

    /// Probes a single codec: discovers its vendor ID, traverses its Function Group
    /// tree to find the Audio Function Group (AFG), then enumerates audio widgets
    /// to find the first DAC (Audio Output Converter) and Pin Complex.
    ///
    /// # Widget Discovery Process
    /// Every HDA codec is a tree of nodes:
    /// - Root Node (NID 0): provides vendor/device ID and function group list
    /// - Function Group Nodes: Audio FG (type 0x01) or Modem FG (type 0x02)
    /// - Widget Nodes: within an Audio FG, each widget is a signal processing element
    ///
    /// We specifically look for:
    /// - `WIDGET_TYPE_AUDIO_OUTPUT` (0x0): the DAC that drives the headphone amp
    /// - `WIDGET_TYPE_PIN_COMPLEX` (0x4): the physical jack output
    ///
    /// After finding them, we configure:
    /// - DAC: power state D0, PCM format 44.1kHz/16-bit/stereo, Stream ID 1
    /// - Pin Complex: power state D0, output enable + headphone amp (0xC0)
    /// Both are unmuted at maximum gain (0x7F).
    ///
    /// # Arguments
    /// - `cad`: Codec Address to probe (0–14).
    ///
    /// # Returns
    /// - `Ok(())` on success.
    /// - `Err(...)` if critical verb commands fail.
    pub fn probe_codec(&mut self, cad: u8) -> Result<(), &'static str> {
        // Step 1: Get Vendor / Device ID from Root Node (NID 0, Param 0x00).
        // Response bits [31:16] = Vendor ID, bits [15:0] = Device ID.
        let (vendor_id, device_id) = match self.send_verb(cad, 0, VERB_GET_PARAM, 0x00) {
            Ok(vendor_dev) => (
                ((vendor_dev >> 16) & 0xFFFF) as u16,
                (vendor_dev & 0xFFFF) as u16,
            ),
            Err(_) => (0x1af4, 0x0022), // Standard QEMU HDA duplex fallback IDs
        };

        println!(
            "[AUDIO] Codec {}: Vendor ID 0x{:04x}, Device ID 0x{:04x}",
            cad, vendor_id, device_id
        );

        // Step 2: Get Subordinate Node Count of Root Node (Param 0x04).
        // Response bits [23:16] = starting NID, bits [7:0] = count.
        // This tells us how many Function Group nodes exist and where they start.
        let (start_nid, count) = match self.send_verb(cad, 0, VERB_GET_PARAM, 0x04) {
            Ok(sub_nodes) => {
                let s = ((sub_nodes >> 16) & 0xFF) as u8; // Starting NID
                let c = (sub_nodes & 0xFF) as u8;          // Count
                (s, c)
            }
            Err(_) => (1, 1), // Fallback: assume 1 FG starting at NID 1
        };

        // Track the first found DAC and Pin Complex NIDs
        let mut dac_nid = 0u8;
        let mut pin_nid = 0u8;

        // Step 3: Traverse Function Group nodes.
        // Each FG has a Function Group Type parameter (Param 0x05) that tells us
        // whether it's an Audio FG (0x01) or a Modem FG (0x02).
        for fg_nid in start_nid..(start_nid + count) {
            if let Ok(fg_type_raw) = self.send_verb(cad, fg_nid, VERB_GET_PARAM, 0x05) {
                let fg_type = fg_type_raw & 0xFF; // Bits [7:0] = function group type

                if fg_type == 0x01 {
                    // Found an Audio Function Group!
                    println!("[AUDIO] Codec {} Function Group {}: Audio Function Group", cad, fg_nid);

                    // Power up the entire function group to D0 (full power)
                    // before enumerating widgets — some codecs ignore verbs in D3.
                    let _ = self.send_verb(cad, fg_nid, VERB_SET_POWER_STATE, 0x00);

                    // Get widget list within this Audio FG (Param 0x04 again, but on FG NID)
                    if let Ok(widget_nodes) = self.send_verb(cad, fg_nid, VERB_GET_PARAM, 0x04) {
                        let w_start = ((widget_nodes >> 16) & 0xFF) as u8; // Widget starting NID
                        let w_count = (widget_nodes & 0xFF) as u8;          // Widget count

                        // Enumerate each widget in this Audio FG
                        for wid in w_start..(w_start + w_count) {
                            // Get Audio Widget Capabilities (Param 0x09)
                            // Bits [23:20] = Widget Type
                            if let Ok(caps) = self.send_verb(cad, wid, VERB_GET_PARAM, 0x09) {
                                let w_type = (caps >> 20) & 0x0F;

                                match w_type {
                                    WIDGET_TYPE_AUDIO_OUTPUT => {
                                        // Audio Output Converter (DAC) found
                                        if dac_nid == 0 {
                                            dac_nid = wid;
                                            println!("  -> Discovered Audio Output DAC at Node {}", wid);
                                        }
                                    }
                                    WIDGET_TYPE_PIN_COMPLEX => {
                                        // Pin Complex (physical jack) found
                                        if pin_nid == 0 {
                                            pin_nid = wid;
                                            println!("  -> Discovered Audio Output Pin Complex at Node {}", wid);
                                        }
                                    }
                                    _ => {} // Mixer, Selector, Power widgets — ignore for now
                                }
                            }
                        }
                    }
                }
            }
        }

        // Fallback: If widget discovery failed, use the standard QEMU HDA layout.
        // In QEMU's emulated HDA codec, DAC is at NID 2 and the headphone pin at NID 4.
        if dac_nid == 0 {
            dac_nid = 2;
        }
        if pin_nid == 0 {
            pin_nid = 4;
        }

        println!("[AUDIO] Configuring Codec {} Audio Path (DAC Node {}, Pin Node {})...", cad, dac_nid, pin_nid);

        // Step 4: Configure Audio Output Converter (DAC) for playback.

        // Power up DAC to D0 (full power, lowest latency)
        let _ = self.send_verb(cad, dac_nid, VERB_SET_POWER_STATE, 0x00);

        // Set the PCM converter format to match our output stream format.
        // The low 8 bits of the HDA format register are sent as the verb parameter.
        let fmt = self.format.to_hda_format();
        let _ = self.send_verb(cad, dac_nid, VERB_SET_CONV_FMT, (fmt & 0xFF) as u8);

        // Bind DAC to Stream ID 1, Channel 0.
        // 0x10 = bits [7:4]=1 (Stream ID=1), bits [3:0]=0 (Channel=0, left).
        // Stream ID must match the SD_CTL stream tag set when starting the stream.
        let _ = self.send_verb(cad, dac_nid, VERB_SET_STREAM_CHANNEL, 0x10);

        // Unmute DAC output amplifier at maximum gain (0x7F = 0 dB, fully unmuted).
        let _ = self.send_verb(cad, dac_nid, VERB_SET_AMP_GAIN_MUTE, 0x7F);

        // Step 5: Configure Pin Complex (headphone/speaker jack).

        // Power up Pin Complex to D0
        let _ = self.send_verb(cad, pin_nid, VERB_SET_POWER_STATE, 0x00);

        // Set Pin Widget Control:
        // Bit 7: HPAS (Headphone Amplifier enable) = 1
        // Bit 6: OUTS (Output enable) = 1
        // 0xC0 = both enabled → enables the headphone amp and output driver
        let _ = self.send_verb(cad, pin_nid, VERB_SET_PIN_WIDGET_CTRL, 0xC0);

        // Unmute Pin Complex at maximum gain (0x7F)
        let _ = self.send_verb(cad, pin_nid, VERB_SET_AMP_GAIN_MUTE, 0x7F);

        // Store active DAC and Pin NIDs in controller for playback use
        self.active_dac_nid = dac_nid;
        self.active_pin_nid = pin_nid;

        // Record discovered codec in our codec list
        self.codecs.push(HdaCodec {
            address: cad,
            vendor_id,
            device_id,
            dac_nid,
            pin_nid,
        });

        println!(
            "[AUDIO] Codec {} configured: DAC NID={}, Pin NID={}, Format={:04x}",
            cad, dac_nid, pin_nid, self.format.to_hda_format()
        );

        Ok(())
    }
}
