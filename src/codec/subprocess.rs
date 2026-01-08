//! Subprocess-based audio encoders using ffmpeg
//!
//! Calls ffmpeg as a subprocess to encode audio. This provides access to
//! ffmpeg's high-quality codec implementations without complex library bindings.

use super::traits::{AudioEncoder, CodecError, CodecType};
use std::io::Write;
use std::process::{Command, Stdio};

/// Check that ffmpeg is available
fn check_ffmpeg() -> Result<(), CodecError> {
    Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| CodecError::InitError("ffmpeg not found in PATH".into()))?;
    Ok(())
}

/// Encode samples using ffmpeg with specified codec
fn encode_with_ffmpeg(
    samples: &[i16],
    input_rate: u32,
    codec: &str,
    format: &str,
    output_frame_size: usize,
) -> Result<Vec<Vec<u8>>, CodecError> {
    if samples.is_empty() {
        return Ok(Vec::new());
    }

    // Convert samples to raw PCM bytes (little-endian i16)
    let pcm_bytes: Vec<u8> = samples
        .iter()
        .flat_map(|&s| s.to_le_bytes())
        .collect();

    // Run ffmpeg to encode
    let mut child = Command::new("ffmpeg")
        .args([
            "-f", "s16le",
            "-ar", &input_rate.to_string(),
            "-ac", "1",
            "-i", "pipe:0",
            "-acodec", codec,
            "-f", format,
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| CodecError::EncodeError(format!("Failed to spawn ffmpeg: {}", e)))?;

    // Write PCM data to ffmpeg stdin
    {
        let stdin = child.stdin.as_mut()
            .ok_or_else(|| CodecError::EncodeError("Failed to open ffmpeg stdin".into()))?;
        stdin.write_all(&pcm_bytes)
            .map_err(|e| CodecError::EncodeError(format!("Failed to write to ffmpeg: {}", e)))?;
    }

    // Wait for ffmpeg and read output
    let output = child.wait_with_output()
        .map_err(|e| CodecError::EncodeError(format!("ffmpeg failed: {}", e)))?;

    if !output.status.success() {
        return Err(CodecError::EncodeError("ffmpeg encoding failed".into()));
    }

    // Split output into frames
    let frames: Vec<Vec<u8>> = output.stdout
        .chunks(output_frame_size)
        .map(|chunk| {
            if chunk.len() < output_frame_size {
                let mut padded = chunk.to_vec();
                padded.resize(output_frame_size, 0);
                padded
            } else {
                chunk.to_vec()
            }
        })
        .collect();

    Ok(frames)
}

/// Decode audio using ffmpeg with specified codec
fn decode_with_ffmpeg(
    data: &[u8],
    format: &str,
    output_rate: u32,
) -> Result<Vec<i16>, CodecError> {
    if data.is_empty() {
        return Ok(Vec::new());
    }

    // Run ffmpeg to decode
    // Note: Don't specify input sample rate - ffmpeg infers it from the codec
    let mut child = Command::new("ffmpeg")
        .args([
            "-f", format,
            "-i", "pipe:0",
            "-f", "s16le",
            "-ar", &output_rate.to_string(),
            "-ac", "1",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| CodecError::DecodeError(format!("Failed to spawn ffmpeg: {}", e)))?;

    // Write encoded data to ffmpeg stdin
    {
        let stdin = child.stdin.as_mut()
            .ok_or_else(|| CodecError::DecodeError("Failed to open ffmpeg stdin".into()))?;
        stdin.write_all(data)
            .map_err(|e| CodecError::DecodeError(format!("Failed to write to ffmpeg: {}", e)))?;
    }

    // Wait for ffmpeg and read output
    let output = child.wait_with_output()
        .map_err(|e| CodecError::DecodeError(format!("ffmpeg failed: {}", e)))?;

    if !output.status.success() {
        return Err(CodecError::DecodeError("ffmpeg decoding failed".into()));
    }

    // Convert output bytes to i16 samples
    let samples: Vec<i16> = output.stdout
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    Ok(samples)
}

/// FFmpeg-based G.722 decoder using subprocess
///
/// Decodes G.722 wideband audio to 16kHz PCM.
/// Buffers frames to decode in larger batches for efficiency.
pub struct FfmpegG722Decoder {
    buffer: Vec<u8>,
    // Decode when we have this many bytes (10 frames = 1600 bytes = 200ms)
    decode_threshold: usize,
}

impl FfmpegG722Decoder {
    pub fn new() -> Result<Self, CodecError> {
        check_ffmpeg()?;
        Ok(Self {
            buffer: Vec::new(),
            decode_threshold: 1600, // 10 frames worth
        })
    }
}

impl Default for FfmpegG722Decoder {
    fn default() -> Self {
        Self::new().expect("Failed to create FFmpeg G.722 decoder")
    }
}

impl super::traits::AudioDecoder for FfmpegG722Decoder {
    fn decode(&mut self, input: &[u8]) -> Result<Vec<i16>, CodecError> {
        // Buffer the input
        self.buffer.extend_from_slice(input);

        // Only decode when we have enough data
        if self.buffer.len() < self.decode_threshold {
            return Ok(Vec::new());
        }

        // Decode all buffered data
        let to_decode: Vec<u8> = self.buffer.drain(..).collect();
        decode_with_ffmpeg(&to_decode, "g722", 16000)
    }

    fn sample_rate(&self) -> u32 {
        16000
    }

    fn channels(&self) -> u8 {
        1
    }

    fn codec_type(&self) -> CodecType {
        CodecType::G722
    }
}

/// FFmpeg-based G.722 encoder using subprocess
///
/// Encodes 16kHz PCM to G.722 wideband audio.
pub struct FfmpegG722Encoder {
    buffer: Vec<i16>,
    frame_size: usize,
}

impl FfmpegG722Encoder {
    pub fn new() -> Result<Self, CodecError> {
        check_ffmpeg()?;
        Ok(Self {
            buffer: Vec::new(),
            // G.722: 320 samples (20ms at 16kHz) -> 160 bytes output
            frame_size: 320,
        })
    }

    /// Encode all samples to G.722 using ffmpeg
    /// Returns frames of 160 bytes each
    #[allow(clippy::unused_self)] // &mut self for API consistency with stateful encoders
    pub fn encode_all(&mut self, samples: &[i16]) -> Result<Vec<Vec<u8>>, CodecError> {
        encode_with_ffmpeg(samples, 16000, "g722", "g722", 160)
    }
}

/// FFmpeg-based G.711 µ-law encoder using subprocess
///
/// Encodes 8kHz PCM to G.711 µ-law.
pub struct FfmpegG711UlawEncoder;

impl FfmpegG711UlawEncoder {
    pub fn new() -> Result<Self, CodecError> {
        check_ffmpeg()?;
        Ok(Self)
    }

    /// Encode all samples to G.711 µ-law using ffmpeg
    /// Returns frames of 160 bytes each
    #[allow(clippy::unused_self)] // &mut self for API consistency with stateful encoders
    pub fn encode_all(&mut self, samples: &[i16]) -> Result<Vec<Vec<u8>>, CodecError> {
        encode_with_ffmpeg(samples, 8000, "pcm_mulaw", "mulaw", 160)
    }
}

/// FFmpeg-based G.711 A-law encoder using subprocess
///
/// Encodes 8kHz PCM to G.711 A-law.
pub struct FfmpegG711AlawEncoder;

impl FfmpegG711AlawEncoder {
    pub fn new() -> Result<Self, CodecError> {
        check_ffmpeg()?;
        Ok(Self)
    }

    /// Encode all samples to G.711 A-law using ffmpeg
    /// Returns frames of 160 bytes each
    #[allow(clippy::unused_self)] // &mut self for API consistency with stateful encoders
    pub fn encode_all(&mut self, samples: &[i16]) -> Result<Vec<Vec<u8>>, CodecError> {
        encode_with_ffmpeg(samples, 8000, "pcm_alaw", "alaw", 160)
    }
}

impl Default for FfmpegG722Encoder {
    fn default() -> Self {
        Self::new().expect("Failed to create FFmpeg G.722 encoder")
    }
}

impl AudioEncoder for FfmpegG722Encoder {
    fn encode(&mut self, samples: &[i16]) -> Result<Vec<u8>, CodecError> {
        // For frame-by-frame encoding, buffer samples
        self.buffer.extend_from_slice(samples);

        // Return empty if we don't have enough for a frame
        if self.buffer.len() < self.frame_size {
            return Ok(Vec::new());
        }

        // Encode buffered samples
        let to_encode: Vec<i16> = self.buffer.drain(..).collect();
        let frames = self.encode_all(&to_encode)?;

        // Return concatenated frames
        Ok(frames.into_iter().flatten().collect())
    }

    fn sample_rate(&self) -> u32 {
        16000
    }

    fn channels(&self) -> u8 {
        1
    }

    fn codec_type(&self) -> CodecType {
        CodecType::G722
    }

    fn frame_size(&self) -> usize {
        self.frame_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ffmpeg_g722_encoder() {
        let encoder = FfmpegG722Encoder::new();
        if encoder.is_err() {
            println!("Skipping test: ffmpeg not available");
            return;
        }

        let mut encoder = encoder.unwrap();

        // Generate 320 samples (20ms at 16kHz) of a 1kHz sine wave
        let samples: Vec<i16> = (0..320)
            .map(|i| {
                let t = i as f64 / 16000.0;
                (f64::sin(2.0 * std::f64::consts::PI * 1000.0 * t) * 10000.0) as i16
            })
            .collect();

        let frames = encoder.encode_all(&samples);
        assert!(frames.is_ok(), "Encode failed: {:?}", frames.err());

        let frames = frames.unwrap();
        assert_eq!(frames.len(), 1, "Expected 1 frame, got {}", frames.len());
        assert_eq!(frames[0].len(), 160, "Expected 160 bytes, got {}", frames[0].len());
    }
}
