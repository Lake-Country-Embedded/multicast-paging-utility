//! Linear PCM (L16) codec for uncompressed audio.

#![allow(dead_code)]

use super::traits::{AudioDecoder, AudioEncoder, CodecError, CodecType};

/// Linear PCM (L16) codec - big-endian 16-bit signed samples
pub struct L16Codec {
    sample_rate: u32,
    channels: u8,
}

impl L16Codec {
    pub fn new(sample_rate: u32, channels: u8) -> Self {
        Self {
            sample_rate,
            channels,
        }
    }

    /// Create a standard mono 44.1kHz L16 codec
    pub fn standard_mono() -> Self {
        Self::new(44100, 1)
    }

    /// Create a standard stereo 44.1kHz L16 codec
    pub fn standard_stereo() -> Self {
        Self::new(44100, 2)
    }

    /// Create an 8kHz mono codec (common for telephony)
    pub fn telephony() -> Self {
        Self::new(8000, 1)
    }
}

impl Default for L16Codec {
    fn default() -> Self {
        Self::standard_mono()
    }
}

impl AudioDecoder for L16Codec {
    fn decode(&mut self, input: &[u8]) -> Result<Vec<i16>, CodecError> {
        if !input.len().is_multiple_of(2) {
            return Err(CodecError::InvalidFrame(
                "L16 data must have even number of bytes".into(),
            ));
        }

        let samples: Vec<i16> = input
            .chunks_exact(2)
            .map(|chunk| i16::from_be_bytes([chunk[0], chunk[1]]))
            .collect();

        Ok(samples)
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn channels(&self) -> u8 {
        self.channels
    }

    fn codec_type(&self) -> CodecType {
        CodecType::L16
    }
}

impl AudioEncoder for L16Codec {
    fn encode(&mut self, samples: &[i16]) -> Result<Vec<u8>, CodecError> {
        let mut output = Vec::with_capacity(samples.len() * 2);

        for &sample in samples {
            output.extend_from_slice(&sample.to_be_bytes());
        }

        Ok(output)
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn channels(&self) -> u8 {
        self.channels
    }

    fn codec_type(&self) -> CodecType {
        CodecType::L16
    }

    fn frame_size(&self) -> usize {
        // 20ms worth of samples
        (self.sample_rate as usize * 20 / 1000) * self.channels as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_l16_roundtrip() {
        let mut codec = L16Codec::new(8000, 1);

        let original: Vec<i16> = vec![0, 1000, -1000, 32767, -32768, 12345, -12345];

        let encoded = codec.encode(&original).unwrap();
        assert_eq!(encoded.len(), original.len() * 2);

        let decoded = codec.decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_l16_big_endian() {
        let mut codec = L16Codec::new(8000, 1);

        // Test that encoding is big-endian
        let samples = vec![0x1234i16];
        let encoded = codec.encode(&samples).unwrap();
        assert_eq!(encoded, vec![0x12, 0x34]);
    }

    #[test]
    fn test_l16_odd_bytes() {
        let mut codec = L16Codec::new(8000, 1);

        let result = codec.decode(&[0x00, 0x01, 0x02]); // 3 bytes = invalid
        assert!(result.is_err());
    }

    #[test]
    fn test_codec_properties() {
        let mono = L16Codec::standard_mono();
        assert_eq!(AudioDecoder::sample_rate(&mono), 44100);
        assert_eq!(AudioDecoder::channels(&mono), 1);

        let stereo = L16Codec::standard_stereo();
        assert_eq!(AudioDecoder::sample_rate(&stereo), 44100);
        assert_eq!(AudioDecoder::channels(&stereo), 2);

        let telephony = L16Codec::telephony();
        assert_eq!(AudioDecoder::sample_rate(&telephony), 8000);
        assert_eq!(AudioDecoder::channels(&telephony), 1);
    }
}
