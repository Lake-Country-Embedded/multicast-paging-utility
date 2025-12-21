use super::traits::{AudioDecoder, AudioEncoder, CodecError, CodecType};

/// G.711 u-law decoder/encoder
pub struct G711UlawCodec;

impl G711UlawCodec {
    pub fn new() -> Self {
        Self
    }

    /// Decode a single u-law sample to linear PCM
    #[inline]
    fn decode_sample(ulaw: u8) -> i16 {
        // Invert all bits (u-law is stored inverted)
        let ulaw = !ulaw;

        // Extract sign, exponent (3 bits), and mantissa (4 bits)
        let sign = (ulaw & 0x80) != 0;
        let exponent = ((ulaw >> 4) & 0x07) as i16;
        let mantissa = (ulaw & 0x0F) as i16;

        // Reconstruct the linear value
        // Add bias (33) and shift by exponent
        let mut sample = ((mantissa << 1) + 33) << (exponent + 2);
        sample -= 33 << 2; // Remove bias

        if sign {
            -sample
        } else {
            sample
        }
    }

    /// Encode a linear PCM sample to u-law
    #[inline]
    fn encode_sample(pcm: i16) -> u8 {
        const BIAS: i16 = 0x84;
        const CLIP: i16 = 32635;

        // Get sign and make positive (handle i16::MIN overflow by casting to i32)
        let sign = if pcm < 0 { 0x80u8 } else { 0x00u8 };
        let abs_sample = (pcm as i32).abs().min(CLIP as i32) as i16;
        let mut sample = abs_sample;

        // Add bias
        sample += BIAS;

        // Find exponent (position of highest bit)
        let mut exponent = 7u8;
        let mut exp_mask = 0x4000i16;
        while exponent > 0 && (sample & exp_mask) == 0 {
            exponent -= 1;
            exp_mask >>= 1;
        }

        // Extract mantissa
        let mantissa = ((sample >> (exponent + 3)) & 0x0F) as u8;

        // Combine and invert
        !(sign | (exponent << 4) | mantissa)
    }
}

impl Default for G711UlawCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioDecoder for G711UlawCodec {
    fn decode(&mut self, input: &[u8]) -> Result<Vec<i16>, CodecError> {
        Ok(input.iter().map(|&b| Self::decode_sample(b)).collect())
    }

    fn sample_rate(&self) -> u32 {
        8000
    }

    fn channels(&self) -> u8 {
        1
    }

    fn codec_type(&self) -> CodecType {
        CodecType::G711Ulaw
    }
}

impl AudioEncoder for G711UlawCodec {
    fn encode(&mut self, samples: &[i16]) -> Result<Vec<u8>, CodecError> {
        Ok(samples.iter().map(|&s| Self::encode_sample(s)).collect())
    }

    fn sample_rate(&self) -> u32 {
        8000
    }

    fn channels(&self) -> u8 {
        1
    }

    fn codec_type(&self) -> CodecType {
        CodecType::G711Ulaw
    }

    fn frame_size(&self) -> usize {
        160 // 20ms at 8kHz
    }
}

/// G.711 A-law decoder/encoder
pub struct G711AlawCodec;

impl G711AlawCodec {
    pub fn new() -> Self {
        Self
    }

    /// Decode a single A-law sample to linear PCM
    #[inline]
    fn decode_sample(alaw: u8) -> i16 {
        // XOR with 0x55 to undo bit inversion
        let alaw = alaw ^ 0x55;

        // Extract sign, exponent (3 bits), and mantissa (4 bits)
        let sign = (alaw & 0x80) != 0;
        let exponent = ((alaw >> 4) & 0x07) as i16;
        let mantissa = (alaw & 0x0F) as i16;

        let sample = if exponent == 0 {
            (mantissa << 1) + 1
        } else {
            ((mantissa << 1) + 33) << (exponent - 1)
        };

        let sample = sample << 3; // Scale to 16-bit

        if sign {
            -sample
        } else {
            sample
        }
    }

    /// Encode a linear PCM sample to A-law
    #[inline]
    fn encode_sample(pcm: i16) -> u8 {
        const CLIP: i16 = 32767;

        // Get sign and make positive (handle i16::MIN overflow by casting to i32)
        let sign = if pcm < 0 { 0x80u8 } else { 0x00u8 };
        let abs_sample = (pcm as i32).abs().min(CLIP as i32) as i16;
        let mut sample = abs_sample;

        // Scale down
        sample >>= 3;

        // Find exponent
        let (exponent, mantissa) = if sample < 32 {
            (0u8, (sample >> 1) as u8)
        } else {
            let mut exp = 1u8;
            let mut temp = sample >> 1;
            while temp >= 32 && exp < 7 {
                temp >>= 1;
                exp += 1;
            }
            (exp, ((sample >> exp) & 0x0F) as u8)
        };

        // Combine and XOR
        (sign | (exponent << 4) | mantissa) ^ 0x55
    }
}

impl Default for G711AlawCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioDecoder for G711AlawCodec {
    fn decode(&mut self, input: &[u8]) -> Result<Vec<i16>, CodecError> {
        Ok(input.iter().map(|&b| Self::decode_sample(b)).collect())
    }

    fn sample_rate(&self) -> u32 {
        8000
    }

    fn channels(&self) -> u8 {
        1
    }

    fn codec_type(&self) -> CodecType {
        CodecType::G711Alaw
    }
}

impl AudioEncoder for G711AlawCodec {
    fn encode(&mut self, samples: &[i16]) -> Result<Vec<u8>, CodecError> {
        Ok(samples.iter().map(|&s| Self::encode_sample(s)).collect())
    }

    fn sample_rate(&self) -> u32 {
        8000
    }

    fn channels(&self) -> u8 {
        1
    }

    fn codec_type(&self) -> CodecType {
        CodecType::G711Alaw
    }

    fn frame_size(&self) -> usize {
        160 // 20ms at 8kHz
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ulaw_roundtrip() {
        // Test various sample values
        let samples: Vec<i16> = vec![0, 100, 1000, 10000, -100, -1000, -10000, 32767, -32768];

        for &original in &samples {
            let encoded = G711UlawCodec::encode_sample(original);
            let decoded = G711UlawCodec::decode_sample(encoded);
            // Allow some quantization error (u-law is lossy)
            let error = (original as i32 - decoded as i32).abs();
            assert!(
                error < 1000,
                "Too much error for {}: decoded to {}, error {}",
                original,
                decoded,
                error
            );
        }
    }

    #[test]
    fn test_alaw_roundtrip() {
        // Test various sample values
        let samples: Vec<i16> = vec![0, 100, 1000, 10000, -100, -1000, -10000, 32767, -32768];

        for &original in &samples {
            let encoded = G711AlawCodec::encode_sample(original);
            let decoded = G711AlawCodec::decode_sample(encoded);
            // Allow some quantization error (A-law is lossy)
            let error = (original as i32 - decoded as i32).abs();
            assert!(
                error < 1000,
                "Too much error for {}: decoded to {}, error {}",
                original,
                decoded,
                error
            );
        }
    }

    #[test]
    fn test_ulaw_decode_encode_batch() {
        let mut codec = G711UlawCodec::new();
        let original: Vec<i16> = (0..160).map(|i| (i * 200) as i16).collect();

        let encoded = codec.encode(&original).unwrap();
        assert_eq!(encoded.len(), 160);

        let decoded = codec.decode(&encoded).unwrap();
        assert_eq!(decoded.len(), 160);
    }

    #[test]
    fn test_codec_properties() {
        let ulaw = G711UlawCodec::new();
        assert_eq!(AudioDecoder::sample_rate(&ulaw), 8000);
        assert_eq!(AudioDecoder::channels(&ulaw), 1);
        assert_eq!(AudioDecoder::codec_type(&ulaw), CodecType::G711Ulaw);

        let alaw = G711AlawCodec::new();
        assert_eq!(AudioDecoder::sample_rate(&alaw), 8000);
        assert_eq!(AudioDecoder::channels(&alaw), 1);
        assert_eq!(AudioDecoder::codec_type(&alaw), CodecType::G711Alaw);
    }
}
