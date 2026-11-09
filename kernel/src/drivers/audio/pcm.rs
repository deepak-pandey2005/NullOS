//! PCM (Pulse Code Modulation) Audio Synthesizer & Buffer Management (Phase 12 - M32).
//!
//! Features:
//! - Standard PCM audio format representation (sample rate, bit depth, channels).
//! - Intel HDA 16-bit format descriptor encoder (`SD_FMT`).
//! - Precomputed 256-point fixed-point sine wave lookup table.
//! - Procedural pure tone generator and musical chime synthesizer.

#![allow(dead_code)]

/// 256-sample fixed-point sine lookup table spanning a full $2\pi$ cycle (-32767 to +32767).
const SINE_TABLE: [i16; 256] = [
    0, 804, 1608, 2410, 3212, 4011, 4808, 5602, 6393, 7179, 7962, 8739, 9512, 10278, 11039, 11793,
    12539, 13279, 14010, 14732, 15446, 16151, 16846, 17530, 18204, 18868, 19519, 20159, 20787, 21403,
    22005, 22594, 23170, 23731, 24279, 24811, 25329, 25832, 26319, 26790, 27245, 27683, 28105, 28510,
    28898, 29268, 29621, 29956, 30273, 30571, 30852, 31113, 31356, 31580, 31785, 31971, 32137, 32285,
    32412, 32521, 32609, 32678, 32728, 32757, 32767, 32757, 32728, 32678, 32609, 32521, 32412, 32285,
    32137, 31971, 31785, 31580, 31356, 31113, 30852, 30571, 30273, 29956, 29621, 29268, 28898, 28510,
    28105, 27683, 27245, 26790, 26319, 25832, 25329, 24811, 24279, 23731, 23170, 22594, 22005, 21403,
    20787, 20159, 19519, 18868, 18204, 17530, 16846, 16151, 15446, 14732, 14010, 13279, 12539, 11793,
    11039, 10278, 9512, 8739, 7962, 7179, 6393, 5602, 4808, 4011, 3212, 2410, 1608, 804, 0, -804,
    -1608, -2410, -3212, -4011, -4808, -5602, -6393, -7179, -7962, -8739, -9512, -10278, -11039, -11793,
    -12539, -13279, -14010, -14732, -15446, -16151, -16846, -17530, -18204, -18868, -19519, -20159, -20787,
    -21403, -22005, -22594, -23170, -23731, -24279, -24811, -25329, -25832, -26319, -26790, -27245, -27683,
    -28105, -28510, -28898, -29268, -29621, -29956, -30273, -30571, -30852, -31113, -31356, -31580, -31785,
    -31971, -32137, -32285, -32412, -32521, -32609, -32678, -32728, -32757, -32767, -32757, -32728, -32678,
    -32609, -32521, -32412, -32285, -32137, -31971, -31785, -31580, -31356, -31113, -30852, -30571, -30273,
    -29956, -29621, -29268, -28898, -28510, -28105, -27683, -27245, -26790, -26319, -25832, -25329, -24811,
    -24279, -23731, -23170, -22594, -22005, -21403, -20787, -20159, -19519, -18868, -18204, -17530, -16846,
    -16151, -15446, -14732, -14010, -13279, -12539, -11793, -11039, -10278, -9512, -8739, -7962, -7179,
    -6393, -5602, -4808, -4011, -3212, -2410, -1608, -804,
];

/// Linear PCM Audio Format Descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcmFormat {
    pub sample_rate: u32,
    pub bit_depth: u8,
    pub channels: u8,
}

impl PcmFormat {
    pub const CD_QUALITY: Self = Self {
        sample_rate: 44100,
        bit_depth: 16,
        channels: 2,
    };

    pub const DVD_QUALITY: Self = Self {
        sample_rate: 48000,
        bit_depth: 16,
        channels: 2,
    };

    /// Encodes this PCM format into the Intel HDA 16-bit stream format register (`SD_FMT`).
    ///
    /// Bitfields:
    /// - Bit 14: Base sample rate (0 = 48 kHz, 1 = 44.1 kHz)
    /// - Bits 11..13: Sample rate multiplier (0 = x1)
    /// - Bits 8..10: Sample rate divisor (0 = /1)
    /// - Bits 4..6: Bits per sample (001 = 16-bit PCM)
    /// - Bits 0..3: Channels - 1 (1 = Stereo)
    pub fn to_hda_format(&self) -> u16 {
        let mut fmt = 0u16;

        if self.sample_rate == 44100 {
            fmt |= 1 << 14;
        }

        match self.bit_depth {
            8 => fmt |= 0b000 << 4,
            16 => fmt |= 0b001 << 4,
            20 => fmt |= 0b010 << 4,
            24 => fmt |= 0b011 << 4,
            32 => fmt |= 0b100 << 4,
            _ => fmt |= 0b001 << 4,
        }

        let ch = self.channels.saturating_sub(1).min(15) as u16;
        fmt |= ch;

        fmt
    }
}

/// Generates interleaved stereo 16-bit PCM samples for a pure sine tone.
/// `buffer`: target slice of `i16` samples (length must be an even number: `[L, R, L, R, ...]`).
/// `freq_hz`: frequency in Hertz (e.g. 440 for A4 concert pitch).
/// `sample_rate`: sample rate in Hertz (e.g. 44100 or 48000).
/// `volume`: output scaling factor (0 to 32767).
pub fn generate_sine_tone(
    buffer: &mut [i16],
    freq_hz: u32,
    sample_rate: u32,
    volume: u16,
) {
    if buffer.is_empty() || sample_rate == 0 {
        return;
    }

    // Phase increment: (freq * 256 * 256) / sample_rate (using 16.16 fixed-point phase)
    let phase_step = ((freq_hz as u64 * 256 * 256) / sample_rate as u64) as u32;
    let mut phase: u32 = 0;

    let frames = buffer.len() / 2;
    let vol = volume as i32;

    for i in 0..frames {
        let table_idx = ((phase >> 8) & 0xFF) as usize;
        let raw_sample = SINE_TABLE[table_idx] as i32;
        let scaled_sample = ((raw_sample * vol) / 32767) as i16;

        buffer[i * 2] = scaled_sample;     // Left Channel
        buffer[i * 2 + 1] = scaled_sample; // Right Channel

        phase = phase.wrapping_add(phase_step);
    }
}

/// Generates a pleasant two-tone musical chime into the provided stereo buffer.
pub fn generate_chime(buffer: &mut [i16], sample_rate: u32) {
    let half_frames = (buffer.len() / 4) * 2;
    let (first_half, second_half) = buffer.split_at_mut(half_frames);

    // Tone 1: C5 (523 Hz)
    generate_sine_tone(first_half, 523, sample_rate, 20000);
    // Tone 2: G5 (784 Hz)
    generate_sine_tone(second_half, 784, sample_rate, 20000);
}

