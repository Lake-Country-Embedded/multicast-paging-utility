//! Opus codec implementation for high-quality audio.

#![allow(dead_code)]

use super::traits::{AudioDecoder, AudioEncoder, CodecError, CodecType};
use audiopus::{coder, Channels, MutSignals, SampleRate};
use audiopus::packet::Packet;
use std::convert::TryInto;

/// Opus decoder
pub struct OpusDecoder {
    decoder: coder::Decoder,
    sample_rate: u32,
    channels: u8,
}

impl OpusDecoder {
    /// Create a new Opus decoder
    pub fn new(sample_rate: u32, channels: u8) -> Result<Self, CodecError> {
        let opus_sr = sample_rate_to_opus(sample_rate)?;
        let opus_ch = channels_to_opus(channels)?;

        let decoder = coder::Decoder::new(opus_sr, opus_ch)
            .map_err(|e| CodecError::InitError(format!("Opus decoder init failed: {}", e)))?;

        Ok(Self {
            decoder,
            sample_rate,
            channels,
        })
    }

    /// Create a stereo 48kHz decoder (most common for Opus)
    pub fn new_stereo() -> Result<Self, CodecError> {
        Self::new(48000, 2)
    }

    /// Create a mono 48kHz decoder
    pub fn new_mono() -> Result<Self, CodecError> {
        Self::new(48000, 1)
    }
}

impl AudioDecoder for OpusDecoder {
    fn decode(&mut self, input: &[u8]) -> Result<Vec<i16>, CodecError> {
        // Opus can decode up to 120ms of audio (5760 samples at 48kHz per channel)
        let max_samples = 5760 * self.channels as usize;
        let mut output = vec![0i16; max_samples];

        // Create packet from input bytes
        let packet: Packet<'_> = input.try_into()
            .map_err(|e| CodecError::DecodeError(format!("Invalid Opus packet: {:?}", e)))?;

        // Create mutable signals view
        let signals: MutSignals<'_, i16> = (&mut output[..]).try_into()
            .map_err(|e| CodecError::DecodeError(format!("Failed to create signals: {:?}", e)))?;

        let samples_decoded = self
            .decoder
            .decode(Some(packet), signals, false)
            .map_err(|e| CodecError::DecodeError(format!("Opus decode error: {}", e)))?;

        output.truncate(samples_decoded * self.channels as usize);
        Ok(output)
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn channels(&self) -> u8 {
        self.channels
    }

    fn codec_type(&self) -> CodecType {
        CodecType::Opus
    }
}

/// Opus encoder
pub struct OpusEncoder {
    encoder: coder::Encoder,
    sample_rate: u32,
    channels: u8,
    frame_size: usize,
}

impl OpusEncoder {
    /// Create a new Opus encoder
    pub fn new(sample_rate: u32, channels: u8, bitrate: u32) -> Result<Self, CodecError> {
        let opus_sr = sample_rate_to_opus(sample_rate)?;
        let opus_ch = channels_to_opus(channels)?;

        let mut encoder = coder::Encoder::new(opus_sr, opus_ch, audiopus::Application::Voip)
            .map_err(|e| CodecError::InitError(format!("Opus encoder init failed: {}", e)))?;

        encoder
            .set_bitrate(audiopus::Bitrate::BitsPerSecond(bitrate as i32))
            .map_err(|e| CodecError::InitError(format!("Failed to set bitrate: {}", e)))?;

        // Frame size: 20ms worth of samples (per channel)
        let frame_size = sample_rate as usize * 20 / 1000;

        Ok(Self {
            encoder,
            sample_rate,
            channels,
            frame_size,
        })
    }

    /// Create a stereo 48kHz encoder with default bitrate
    pub fn new_stereo(bitrate: u32) -> Result<Self, CodecError> {
        Self::new(48000, 2, bitrate)
    }

    /// Create a mono 48kHz encoder with default bitrate
    pub fn new_mono(bitrate: u32) -> Result<Self, CodecError> {
        Self::new(48000, 1, bitrate)
    }
}

impl AudioEncoder for OpusEncoder {
    fn encode(&mut self, samples: &[i16]) -> Result<Vec<u8>, CodecError> {
        // Opus typically produces 1-4KB output for a 20ms frame
        let mut output = vec![0u8; 4000];

        let bytes_written = self
            .encoder
            .encode(samples, &mut output)
            .map_err(|e| CodecError::EncodeError(format!("Opus encode error: {}", e)))?;

        output.truncate(bytes_written);
        Ok(output)
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn channels(&self) -> u8 {
        self.channels
    }

    fn codec_type(&self) -> CodecType {
        CodecType::Opus
    }

    fn frame_size(&self) -> usize {
        self.frame_size
    }
}

fn sample_rate_to_opus(rate: u32) -> Result<SampleRate, CodecError> {
    match rate {
        8000 => Ok(SampleRate::Hz8000),
        12000 => Ok(SampleRate::Hz12000),
        16000 => Ok(SampleRate::Hz16000),
        24000 => Ok(SampleRate::Hz24000),
        48000 => Ok(SampleRate::Hz48000),
        _ => Err(CodecError::InitError(format!(
            "Unsupported sample rate for Opus: {}",
            rate
        ))),
    }
}

fn channels_to_opus(channels: u8) -> Result<Channels, CodecError> {
    match channels {
        1 => Ok(Channels::Mono),
        2 => Ok(Channels::Stereo),
        _ => Err(CodecError::InitError(format!(
            "Unsupported channel count for Opus: {}",
            channels
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opus_decoder_creation() {
        let decoder = OpusDecoder::new_stereo();
        assert!(decoder.is_ok());

        let decoder = decoder.unwrap();
        assert_eq!(decoder.sample_rate(), 48000);
        assert_eq!(decoder.channels(), 2);
    }

    #[test]
    fn test_opus_encoder_creation() {
        let encoder = OpusEncoder::new_mono(24000);
        assert!(encoder.is_ok());

        let encoder = encoder.unwrap();
        assert_eq!(encoder.sample_rate(), 48000);
        assert_eq!(encoder.channels(), 1);
    }

    #[test]
    fn test_opus_encode() {
        let mut encoder = OpusEncoder::new_mono(24000).unwrap();

        // Create a 20ms frame of audio (960 samples at 48kHz mono)
        let samples: Vec<i16> = (0..960).map(|i| ((i * 100) % 32767) as i16).collect();

        let encoded = encoder.encode(&samples).unwrap();
        // Opus typically produces 10-100 bytes for a 20ms frame at 24kbps
        assert!(!encoded.is_empty());
        assert!(encoded.len() < 1000); // Sanity check
    }

    #[test]
    fn test_opus_frame_size() {
        let encoder = OpusEncoder::new_mono(24000).unwrap();
        // 48000 Hz * 20ms = 960 samples
        assert_eq!(encoder.frame_size(), 960);

        let encoder = OpusEncoder::new(48000, 2, 64000).unwrap();
        // 48000 Hz * 20ms = 960 samples (per channel, frame_size is total)
        assert_eq!(encoder.frame_size(), 960);
    }
}
