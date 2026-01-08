//! Audio codec traits and type definitions.

#![allow(dead_code)]

use thiserror::Error;

#[derive(Error, Debug)]
pub enum CodecError {
    #[error("Unsupported payload type: {0}")]
    UnsupportedPayloadType(u8),

    #[error("Invalid frame data: {0}")]
    InvalidFrame(String),

    #[error("Encode error: {0}")]
    EncodeError(String),

    #[error("Decode error: {0}")]
    DecodeError(String),

    #[error("Initialization error: {0}")]
    InitError(String),

    #[error("Invalid frame size: expected {expected}, got {got}")]
    InvalidFrameSize { expected: usize, got: usize },
}

/// Supported codec types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodecType {
    G711Ulaw,
    G711Alaw,
    G722,
    Opus,
    L16,
}

impl CodecType {
    /// Get the RTP payload type for this codec
    #[must_use]
    pub const fn payload_type(&self) -> u8 {
        match self {
            CodecType::G711Ulaw => 0,
            CodecType::G711Alaw => 8,
            CodecType::G722 => 9,
            CodecType::Opus => 96, // Dynamic, typically 96
            CodecType::L16 => 11,  // Mono
        }
    }

    /// Get the native sample rate for this codec
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        match self {
            CodecType::G711Ulaw | CodecType::G711Alaw => 8000,
            CodecType::G722 => 16000,
            CodecType::Opus => 48000,
            CodecType::L16 => 44100,
        }
    }

    /// Get the number of channels
    #[must_use]
    pub const fn channels(&self) -> u8 {
        match self {
            CodecType::G711Ulaw | CodecType::G711Alaw | CodecType::G722 | CodecType::L16 => 1,
            CodecType::Opus => 2,
        }
    }

    /// Get a human-readable name
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            CodecType::G711Ulaw => "G.711 u-law",
            CodecType::G711Alaw => "G.711 A-law",
            CodecType::G722 => "G.722",
            CodecType::Opus => "Opus",
            CodecType::L16 => "Linear PCM",
        }
    }

    /// Parse from string (case-insensitive)
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        // Use case-insensitive comparison without allocating
        if s.eq_ignore_ascii_case("g711ulaw") || s.eq_ignore_ascii_case("pcmu") || s.eq_ignore_ascii_case("ulaw") {
            Some(CodecType::G711Ulaw)
        } else if s.eq_ignore_ascii_case("g711alaw") || s.eq_ignore_ascii_case("pcma") || s.eq_ignore_ascii_case("alaw") {
            Some(CodecType::G711Alaw)
        } else if s.eq_ignore_ascii_case("g722") {
            Some(CodecType::G722)
        } else if s.eq_ignore_ascii_case("opus") {
            Some(CodecType::Opus)
        } else if s.eq_ignore_ascii_case("l16") || s.eq_ignore_ascii_case("pcm") || s.eq_ignore_ascii_case("linear") {
            Some(CodecType::L16)
        } else {
            None
        }
    }

    /// Detect codec from RTP payload type
    #[must_use]
    pub const fn from_payload_type(pt: u8) -> Option<Self> {
        match pt {
            0 => Some(CodecType::G711Ulaw),
            8 => Some(CodecType::G711Alaw),
            9 => Some(CodecType::G722),
            10 | 11 => Some(CodecType::L16),
            96..=127 => Some(CodecType::Opus), // Assume Opus for dynamic types
            _ => None,
        }
    }
}

impl std::fmt::Display for CodecType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Trait for audio decoders
pub trait AudioDecoder: Send {
    /// Decode compressed audio to PCM samples (i16)
    fn decode(&mut self, input: &[u8]) -> Result<Vec<i16>, CodecError>;

    /// Get the native sample rate of decoded audio
    fn sample_rate(&self) -> u32;

    /// Get the number of channels
    fn channels(&self) -> u8;

    /// Get the codec type
    fn codec_type(&self) -> CodecType;
}

/// Trait for audio encoders
pub trait AudioEncoder: Send {
    /// Encode PCM samples to compressed audio
    fn encode(&mut self, samples: &[i16]) -> Result<Vec<u8>, CodecError>;

    /// Get the sample rate expected for input
    fn sample_rate(&self) -> u32;

    /// Get the number of channels expected
    fn channels(&self) -> u8;

    /// Get the codec type
    fn codec_type(&self) -> CodecType;

    /// Get the frame size in samples (per channel)
    fn frame_size(&self) -> usize;
}

/// Codec information (for future use in codec negotiation)
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CodecInfo {
    pub codec_type: CodecType,
    pub sample_rate: u32,
    pub channels: u8,
    pub bitrate: Option<u32>,
}
