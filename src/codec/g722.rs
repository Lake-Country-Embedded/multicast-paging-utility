//! G.722 Sub-Band ADPCM encoder/decoder
//!
//! G.722 is a 7 kHz wideband audio codec operating at 64 kbit/s.
//! It splits the signal into two sub-bands (low and high) and applies
//! ADPCM coding to each.
//!
//! Note: This native implementation is kept as a reference but is superseded
//! by the ffmpeg subprocess encoder in `subprocess.rs` which produces
//! better audio quality.

// Reference implementation - superseded by ffmpeg subprocess
#![allow(dead_code)]
#![allow(clippy::unused_self)]
#![allow(clippy::bool_to_int_with_if)]
#![allow(clippy::let_and_return)]

use super::traits::{AudioEncoder, CodecError, CodecType};

/// G.722 encoder state
pub struct G722Encoder {
    /// Lower band quantizer state
    band_low: G722BandState,
    /// Upper band quantizer state
    band_high: G722BandState,
}

/// State for each sub-band
#[derive(Default)]
struct G722BandState {
    s: i32,        // Reconstructed signal
    sp: i32,       // Predicted signal
    sz: i32,       // Zero section prediction
    r: [i32; 3],   // Quantized difference signal
    p: [i32; 3],   // Partial reconstruction signal
    a: [i32; 3],   // Second order predictor coefficients
    b: [i32; 7],   // Sixth order predictor coefficients
    d: [i32; 7],   // Quantized difference signal
    nb: i32,       // Step size multiplier
    det: i32,      // Quantizer step size
}

impl G722BandState {
    fn new(det: i32) -> Self {
        Self {
            det,
            ..Default::default()
        }
    }
}

impl G722Encoder {
    pub fn new() -> Self {
        Self {
            band_low: G722BandState::new(32),
            band_high: G722BandState::new(8),
        }
    }

    /// Encode 16kHz PCM samples to G.722
    /// Input: 16-bit PCM samples at 16kHz
    /// Output: G.722 encoded bytes (2 samples per byte)
    pub fn encode_frame(&mut self, samples: &[i16]) -> Vec<u8> {
        // G.722 encodes 2 samples per output byte
        let mut output = Vec::with_capacity(samples.len() / 2);

        // Process samples in pairs
        for chunk in samples.chunks(2) {
            if chunk.len() < 2 {
                break;
            }

            // QMF analysis filter - split into low and high bands
            let (x_low, x_high) = self.qmf_analyze(chunk[0], chunk[1]);

            // Encode low band (6 bits)
            let i_low = self.encode_low_band(x_low);

            // Encode high band (2 bits)
            let i_high = self.encode_high_band(x_high);

            // Pack into output byte: high bits in MSB, low bits in LSB
            let out_byte = ((i_high & 0x03) << 6) | (i_low & 0x3F);
            output.push(out_byte);
        }

        output
    }

    /// QMF analysis filter - splits signal into low and high sub-bands
    fn qmf_analyze(&self, sample1: i16, sample2: i16) -> (i32, i32) {
        // Simplified QMF filter
        let x1 = sample1 as i32;
        let x2 = sample2 as i32;

        // Low band = sum (0-4kHz)
        let x_low = (x1 + x2) >> 1;

        // High band = difference (4-8kHz)
        let x_high = (x1 - x2) >> 1;

        (x_low, x_high)
    }

    /// Encode low sub-band with 6-bit ADPCM
    fn encode_low_band(&mut self, x_low: i32) -> u8 {
        let band = &mut self.band_low;

        // Compute difference signal
        let d_low = x_low.saturating_sub(band.sp);

        // Quantize with adaptive step size
        let i_low = quantize_low(d_low, band.det);

        // Inverse quantize for predictor update
        let d_low_x = inverse_quantize_low(i_low, band.det);

        // Update predictor
        self.update_predictor_low(d_low_x);

        // Update step size
        self.adapt_step_low(i_low);

        i_low
    }

    /// Encode high sub-band with 2-bit ADPCM
    fn encode_high_band(&mut self, x_high: i32) -> u8 {
        let band = &mut self.band_high;

        // Compute difference signal
        let d_high = x_high.saturating_sub(band.sp);

        // Quantize with adaptive step size
        let i_high = quantize_high(d_high, band.det);

        // Inverse quantize for predictor update
        let d_high_x = inverse_quantize_high(i_high, band.det);

        // Update predictor
        self.update_predictor_high(d_high_x);

        // Update step size
        self.adapt_step_high(i_high);

        i_high
    }

    fn update_predictor_low(&mut self, d_low_x: i32) {
        let band = &mut self.band_low;

        // Shift delay lines
        band.r[2] = band.r[1];
        band.r[1] = band.r[0];
        band.r[0] = d_low_x;

        band.p[2] = band.p[1];
        band.p[1] = band.p[0];
        band.p[0] = d_low_x.saturating_add(band.sz);

        // Simple first-order predictor update
        band.sp = band.p[0].clamp(-32768, 32767);
    }

    fn update_predictor_high(&mut self, d_high_x: i32) {
        let band = &mut self.band_high;

        // Simple predictor update for high band
        band.sp = d_high_x.clamp(-16384, 16383);
    }

    fn adapt_step_low(&mut self, i_low: u8) {
        // Step size adaptation table for low band (6-bit)
        const ADAPTATION: [i32; 32] = [
            -60, -60, -60, -60, -52, -44, -36, -28,
            -20, -12,  -4,   4,  12,  20,  28,  36,
             44,  52,  60,  68,  76,  84,  92, 100,
            108, 116, 124, 132, 140, 148, 156, 164,
        ];

        let band = &mut self.band_low;
        let index = (i_low & 0x1F) as usize;

        band.nb = (band.nb + ADAPTATION[index]).clamp(0, 22528);
        band.det = (band.det * DET_MULTIPLIER[band.nb as usize >> 8]) >> 15;
        band.det = band.det.max(32);
    }

    fn adapt_step_high(&mut self, i_high: u8) {
        // Step size adaptation for high band (2-bit)
        const ADAPTATION: [i32; 4] = [-214, 798, 798, -214];

        let band = &mut self.band_high;
        let index = (i_high & 0x03) as usize;

        band.nb = (band.nb + ADAPTATION[index]).clamp(0, 22528);
        band.det = (band.det * DET_MULTIPLIER[band.nb as usize >> 8]) >> 15;
        band.det = band.det.max(8);
    }
}

impl Default for G722Encoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Quantize low band difference signal (6-bit output)
fn quantize_low(d: i32, det: i32) -> u8 {
    let sign = if d < 0 { 0x20u8 } else { 0 };
    let abs_d = d.abs();

    // Find quantization level
    let mut level = 0u8;
    let mut threshold = det >> 1;

    for i in 1..30 {
        if abs_d > threshold {
            level = i;
            threshold += det;
        } else {
            break;
        }
    }

    sign | (level & 0x1F)
}

/// Inverse quantize low band
fn inverse_quantize_low(i: u8, det: i32) -> i32 {
    const QUANT_TABLE: [i32; 32] = [
           0,   -66,   -66,   -66,   -66,  -136,  -136,  -136,
        -136,  -264,  -264,  -264,  -264,  -440,  -440,  -440,
        -440,  -680,  -680,  -680,  -680, -1000, -1000, -1000,
       -1000, -1420, -1420, -1420, -1420, -1960, -1960, -1960,
    ];

    let sign = (i & 0x20) != 0;
    let level = (i & 0x1F) as usize;

    let mut value = (det * QUANT_TABLE[level.min(31)]) >> 15;

    if sign {
        value = -value;
    }

    value
}

/// Quantize high band difference signal (2-bit output)
fn quantize_high(d: i32, det: i32) -> u8 {
    let sign = if d < 0 { 0x02u8 } else { 0 };
    let abs_d = d.abs();

    let level = if abs_d > det * 3 {
        1
    } else {
        0
    };

    sign | level
}

/// Inverse quantize high band
fn inverse_quantize_high(i: u8, det: i32) -> i32 {
    const QUANT_TABLE: [i32; 4] = [0, -2048, 0, 2048];

    let value = (det * QUANT_TABLE[(i & 0x03) as usize]) >> 15;
    value
}

/// Step size adaptation multiplier table
const DET_MULTIPLIER: [i32; 89] = [
    32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47,
    48, 49, 51, 52, 54, 55, 57, 58, 60, 62, 64, 66, 68, 70, 72, 74,
    76, 79, 81, 84, 86, 89, 92, 95, 98, 101, 104, 108, 111, 115, 118, 122,
    126, 130, 134, 138, 143, 148, 152, 157, 162, 168, 173, 179, 185, 191, 197, 204,
    211, 218, 225, 233, 241, 249, 257, 266, 275, 284, 294, 304, 315, 326, 337, 349,
    361, 373, 386, 400, 414, 428, 444, 459, 476,
];

impl AudioEncoder for G722Encoder {
    fn encode(&mut self, samples: &[i16]) -> Result<Vec<u8>, CodecError> {
        Ok(self.encode_frame(samples))
    }

    fn sample_rate(&self) -> u32 {
        16000 // G.722 operates at 16kHz
    }

    fn channels(&self) -> u8 {
        1
    }

    fn codec_type(&self) -> CodecType {
        CodecType::G722
    }

    fn frame_size(&self) -> usize {
        320 // 20ms at 16kHz = 320 samples, encodes to 160 bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_g722_encoder_basic() {
        let mut encoder = G722Encoder::new();

        // 320 samples (20ms at 16kHz)
        let samples: Vec<i16> = (0..320)
            .map(|i| ((i as f64 * 0.1).sin() * 10000.0) as i16)
            .collect();

        let encoded = encoder.encode(&samples).unwrap();

        // Should produce 160 bytes (2 samples per byte)
        assert_eq!(encoded.len(), 160);
    }

    #[test]
    fn test_g722_silence() {
        let mut encoder = G722Encoder::new();

        // Silence should encode to mostly zeros
        let silence: Vec<i16> = vec![0; 320];
        let encoded = encoder.encode(&silence).unwrap();

        assert_eq!(encoded.len(), 160);
    }
}
