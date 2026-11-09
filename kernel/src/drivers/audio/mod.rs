//! # Audio Subsystem & Intel High Definition Audio (HDA) Driver
//!
//! Provides a modern audio subsystem centered around the Intel High Definition Audio specification:
//! - **PCI Discovery & MMIO Mapping**: Scans for PCI Class 0x04 (Multimedia), Subclass 0x03 (HD Audio),
//!   identity-mapping the 64-bit BAR0 MMIO space.
//! - **CORB & RIRB Ring Engines**: Implements the Command Output Ring Buffer (CORB) and Response Input Ring Buffer
//!   (RIRB) to send verbs to audio codecs and receive responses asynchronously.
//! - **Codec & Widget Graph Traversal**: Discovers audio codecs on the link, iterates widget trees
//!   to locate Audio Output (DAC) widgets and Pin Complex widgets configured for headphone/line-out jacks.
//! - **Linear PCM Sound Synthesis**: In-kernel integer sine wave and square wave synthesis with fixed-point
//!   trigonometric tables, tone generators, and multi-tone chimes.
//! - **Direct Memory Access (DMA) Audio Streams**: Buffer Descriptor List (BDL) cyclic DMA engines with
//!   ping-pong buffers for glitch-free audio output.
//! - **Kernel Audio API**: High-level primitives ([`play_tone`], [`beep`], [`stop`], [`get_status`])
//!   for notification sounds and music synthesis.

#![allow(dead_code)]

pub mod hda;
pub mod pcm;

pub use hda::HDA;
use crate::println;

/// Status summary and capabilities of the Intel HD Audio subsystem.
#[derive(Clone, Debug)]
pub struct AudioStatus {
    /// HDA specification major version (typically 1).
    pub spec_version_major: u8,
    /// HDA specification minor version (typically 0).
    pub spec_version_minor: u8,
    /// Number of output stream descriptors supported by the controller.
    pub output_streams: u8,
    /// Number of input stream descriptors supported by the controller.
    pub input_streams: u8,
    /// Total count of audio codecs discovered on the link.
    pub codec_count: usize,
    /// Node ID of the active Digital-to-Analog Converter (DAC) widget.
    pub active_dac_nid: u8,
    /// Node ID of the active Pin Complex (speaker/headphone) widget.
    pub active_pin_nid: u8,
    /// Active sample rate in Hertz (e.g. 48000 Hz or 44100 Hz).
    pub sample_rate: u32,
    /// True if an audio stream is currently actively playing.
    pub is_playing: bool,
}

/// Initializes the Intel HD Audio subsystem and probes the PCI bus.
pub fn init() {
    hda::init();
}

/// Checks if an active Intel HD Audio controller is present and initialized.
pub fn is_available() -> bool {
    HDA.lock().is_some()
}

/// Plays a pure sine wave tone at `freq_hz` for `duration_ms`.
///
/// Synthesizes 16-bit signed PCM samples and streams them through the primary output DMA engine.
pub fn play_tone(freq_hz: u32, duration_ms: u32) -> Result<(), &'static str> {
    let mut guard = HDA.lock();
    match guard.as_mut() {
        Some(controller) => controller.play_tone(freq_hz, duration_ms),
        None => Err("No Intel HD Audio controller available"),
    }
}

/// Plays a pleasant two-tone musical startup chime.
pub fn beep() -> Result<(), &'static str> {
    let mut guard = HDA.lock();
    match guard.as_mut() {
        Some(controller) => controller.play_chime(),
        None => Err("No Intel HD Audio controller available"),
    }
}

/// Stops any ongoing audio playback and halts the DMA stream engine.
pub fn stop() {
    if let Some(ref mut controller) = *HDA.lock() {
        controller.stop();
    }
}

/// Retrieves the current status, metrics, and active node configuration of the audio controller.
pub fn get_status() -> Option<AudioStatus> {
    let guard = HDA.lock();
    guard.as_ref().map(|c| AudioStatus {
        spec_version_major: c.major_version,
        spec_version_minor: c.minor_version,
        output_streams: c.num_oss,
        input_streams: c.num_iss,
        codec_count: c.codecs.len(),
        active_dac_nid: c.active_dac_nid,
        active_pin_nid: c.active_pin_nid,
        sample_rate: c.format.sample_rate,
        is_playing: c.is_playing,
    })
}

/// Runs automated verification self-tests for Milestone 32 (Intel HD Audio).
///
/// Asserts:
/// 1. HDA specification version is valid ($\ge$ 1.0).
/// 2. Stream capabilities (output stream descriptors) are non-zero.
/// 3. CORB, RIRB, BDL, and Ping-Pong DMA buffer allocations are valid non-zero physical addresses.
/// 4. Discovered codecs expose valid Vendor ID, DAC node ID, and Pin node ID.
/// 5. Procedural PCM tone generation produces time-varying samples and activates the DMA stream.
pub fn run_self_tests() {
    println!();
    println!("--------------------------------------------------");
    println!(" [AUDIO] Running Milestone 32 Intel HDA Self-Tests ");
    println!("--------------------------------------------------");

    let mut guard = HDA.lock();
    let controller = match guard.as_mut() {
        Some(c) => c,
        None => {
            println!("[AUDIO] Notice: No HDA controller detected. Skipping audio hardware tests.");
            return;
        }
    };

    // Test 1: HDA Specification Version
    println!(
        "[AUDIO] Test 1: Verifying HDA Specification Version {}.{}...",
        controller.major_version, controller.minor_version
    );
    assert!(controller.major_version >= 1, "HDA Major Version must be at least 1");

    // Test 2: Stream capabilities
    println!(
        "[AUDIO] Test 2: Verifying Stream Descriptors (Output: {}, Input: {})...",
        controller.num_oss, controller.num_iss
    );
    assert!(controller.num_oss > 0, "Controller must expose at least 1 output stream descriptor");

    // Test 3: Ring buffers and DMA frame allocations
    println!("[AUDIO] Test 3: Verifying CORB, RIRB, BDL, and Ping-Pong Buffer allocations...");
    assert!(controller.corb_frame.start_address().as_u64() != 0, "CORB DMA base must be non-zero");
    assert!(controller.rirb_frame.start_address().as_u64() != 0, "RIRB DMA base must be non-zero");
    assert!(controller.bdl_frame.start_address().as_u64() != 0, "BDL base must be non-zero");
    assert!(controller.dma_buf_a.start_address().as_u64() != 0, "Buffer A DMA base must be non-zero");
    assert!(controller.dma_buf_b.start_address().as_u64() != 0, "Buffer B DMA base must be non-zero");

    // Test 4: Codec discovery
    println!("[AUDIO] Test 4: Discovered codecs count: {}", controller.codecs.len());
    for codec in &controller.codecs {
        println!(
            "  -> Codec CAD {}: Vendor 0x{:04x}, Device 0x{:04x}, DAC Node {}, Pin Node {}",
            codec.address, codec.vendor_id, codec.device_id, codec.dac_nid, codec.pin_nid
        );
        assert!(codec.vendor_id != 0, "Codec vendor ID must be non-zero");
        assert!(codec.dac_nid != 0, "DAC node ID must be non-zero");
        assert!(codec.pin_nid != 0, "Pin node ID must be non-zero");
    }

    // Test 5: Procedural PCM tone generation & DMA stream activation
    println!("[AUDIO] Test 5: Testing PCM tone generation and DMA stream playback...");
    let sample_rate = controller.format.sample_rate;
    let mut test_samples = alloc::vec![0i16; 512];
    pcm::generate_sine_tone(&mut test_samples, 440, sample_rate, 16000);
    assert_ne!(test_samples[0], test_samples[20], "Generated sine samples must vary over time");

    let play_res = controller.play_samples(&test_samples);
    assert!(play_res.is_ok(), "Stream descriptor DMA activation failed");
    assert!(controller.is_playing, "Controller should be in playing state");

    controller.stop();
    assert!(!controller.is_playing, "Controller should be stopped");

    println!("[AUDIO] All Milestone 32 Intel HD Audio self-tests passed successfully!");
}
