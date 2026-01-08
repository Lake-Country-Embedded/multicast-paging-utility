pub mod g711;
pub mod g722;
pub mod opus;
pub mod pcm;
pub mod subprocess;
pub mod traits;

pub use g711::{G711AlawCodec, G711UlawCodec};
pub use opus::{OpusDecoder, OpusEncoder};
pub use pcm::L16Codec;
pub use subprocess::{FfmpegG711AlawEncoder, FfmpegG711UlawEncoder, FfmpegG722Decoder, FfmpegG722Encoder};
pub use traits::{AudioDecoder, AudioEncoder, CodecError, CodecType};

/// Create a decoder for the given codec type
pub fn create_decoder(codec_type: CodecType) -> Result<Box<dyn AudioDecoder>, CodecError> {
    match codec_type {
        CodecType::G711Ulaw => Ok(Box::new(G711UlawCodec::new())),
        CodecType::G711Alaw => Ok(Box::new(G711AlawCodec::new())),
        CodecType::G722 => Ok(Box::new(FfmpegG722Decoder::new()?)),
        CodecType::Opus => Ok(Box::new(OpusDecoder::new_stereo()?)),
        CodecType::L16 => Ok(Box::new(L16Codec::standard_mono())),
    }
}

/// Create a decoder based on RTP payload type
pub fn create_decoder_for_payload_type(pt: u8) -> Result<Box<dyn AudioDecoder>, CodecError> {
    match CodecType::from_payload_type(pt) {
        Some(codec_type) => create_decoder(codec_type),
        None => Err(CodecError::UnsupportedPayloadType(pt)),
    }
}

/// Create an encoder for the given codec type
pub fn create_encoder(codec_type: CodecType) -> Result<Box<dyn AudioEncoder>, CodecError> {
    match codec_type {
        CodecType::G711Ulaw => Ok(Box::new(G711UlawCodec::new())),
        CodecType::G711Alaw => Ok(Box::new(G711AlawCodec::new())),
        CodecType::G722 => Ok(Box::new(FfmpegG722Encoder::new()?)),
        CodecType::Opus => Ok(Box::new(OpusEncoder::new_mono(24000)?)),
        CodecType::L16 => Ok(Box::new(L16Codec::telephony())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_decoder_ulaw() {
        let decoder = create_decoder(CodecType::G711Ulaw);
        assert!(decoder.is_ok());
        assert_eq!(decoder.unwrap().codec_type(), CodecType::G711Ulaw);
    }

    #[test]
    fn test_create_decoder_alaw() {
        let decoder = create_decoder(CodecType::G711Alaw);
        assert!(decoder.is_ok());
        assert_eq!(decoder.unwrap().codec_type(), CodecType::G711Alaw);
    }

    #[test]
    fn test_create_decoder_by_payload_type() {
        // PCMU
        let decoder = create_decoder_for_payload_type(0);
        assert!(decoder.is_ok());
        assert_eq!(decoder.unwrap().codec_type(), CodecType::G711Ulaw);

        // PCMA
        let decoder = create_decoder_for_payload_type(8);
        assert!(decoder.is_ok());
        assert_eq!(decoder.unwrap().codec_type(), CodecType::G711Alaw);
    }

    #[test]
    fn test_create_encoder() {
        let encoder = create_encoder(CodecType::G711Ulaw);
        assert!(encoder.is_ok());
    }
}
