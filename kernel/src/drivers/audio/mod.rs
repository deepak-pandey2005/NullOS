//! Audio Subsystem & Intel High Definition Audio (HDA) Driver (Phase 12 - M32).
//!
//! Subsystem features:
//! - PCI hardware discovery and 64-bit MMIO initialization.
//! - CORB and RIRB command/response circular ring engines.
//! - Audio Codec discovery and Widget tree traversal.
//! - Linear PCM procedural sound synthesis and ping-pong DMA streaming.
//! - User-facing audio playback API (`beep`, `play_tone`, `stop`).

#![allow(dead_code)]

pub mod hda;
pub mod pcm;

pub use hda::HDA;
use crate::println;

/// Status summary of the Intel HD Audio subsystem.
#[derive(Clone, Debug)]
pub struct AudioStatus {
    pub spec_version_major: u8,
    pub spec_version_minor: u8,
    pub output_streams: u8,
    pub input_streams: u8,
    pub codec_count: usize,
    pub active_dac_nid: u8,
    pub active_pin_nid: u8,
    pub sample_rate: u32,
    pub is_playing: bool,
}

/// Initializes the Intel HD Audio subsystem.
pub fn init() {
    hda::init();
}

/// Checks if an active Intel HD Audio controller is present.
pub fn is_available() -> bool {
    HDA.lock().is_some()
}

/// Plays a pure sine wave tone at `freq_hz` for `duration_ms`.
pub fn play_tone(freq_hz: u32, duration_ms: u32) -> Result<(), &'static str> {
    let mut guard = HDA.lock();
    match guard.as_mut() {
        Some(controller) => controller.play_tone(freq_hz, duration_ms),
        None => Err("No Intel HD Audio controller available"),
    }
}

/// Plays a two-tone musical startup chime.
pub fn beep() -> Result<(), &'static str> {
    let mut guard = HDA.lock();
    match guard.as_mut() {
        Some(controller) => controller.play_chime(),
        None => Err("No Intel HD Audio controller available"),
    }
}

/// Stops any ongoing audio playback.
pub fn stop() {
    if let Some(ref mut controller) = *HDA.lock() {
        controller.stop();
    }
}

/// Retrieves the current status and metrics of the audio controller.
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
