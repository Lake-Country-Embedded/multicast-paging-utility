//! Polycom PTT/Group Paging protocol implementation.
//!
//! This module implements the Polycom proprietary multicast paging protocol,
//! which is NOT standard RTP. It uses a custom packet format with session
//! control (Alert/End packets) and redundant audio frames.
//!
//! Reference: Polycom Engineering Advisory ea70568

#![allow(dead_code)]

use std::net::SocketAddr;
use std::time::Instant;
use thiserror::Error;

// ============================================================================
// Constants
// ============================================================================

/// Default Polycom multicast address
pub const DEFAULT_ADDRESS: &str = "224.0.1.116";

/// Default Polycom port
pub const DEFAULT_PORT: u16 = 5001;

/// Op code for PTT Alert packet (session start)
pub const OP_ALERT: u8 = 0x0f;

/// Op code for PTT Transmit packet (audio data)
pub const OP_TRANSMIT: u8 = 0x10;

/// Op code for PTT End packet (session end)
pub const OP_END: u8 = 0xff;

/// Codec type for G.711 µ-law
pub const CODEC_G711U: u8 = 0x00;

/// Codec type for G.711 A-law (using RTP payload type value)
pub const CODEC_G711A: u8 = 0x08;

/// Codec type for G.722
pub const CODEC_G722: u8 = 0x09;

/// Number of Alert packets to send when starting a page
pub const ALERT_PACKET_COUNT: u32 = 31;

/// Number of End packets to send when ending a page
pub const END_PACKET_COUNT: u32 = 12;

/// Interval between Alert/End packets in milliseconds
pub const CONTROL_PACKET_INTERVAL_MS: u64 = 30;

/// Delay before sending End packets in milliseconds
pub const END_DELAY_MS: u64 = 50;

/// Frame size for G.711 at 20ms (8000 Hz * 0.020s = 160 samples)
pub const G711_FRAME_SIZE: usize = 160;

/// Frame size for G.722 at 20ms
/// G.722 encodes 16kHz audio at 64kbps = 8000 bytes/sec = 160 bytes per 20ms
/// Note: Polycom uses 160-byte frames for G.722, not 240 bytes
pub const G722_FRAME_SIZE: usize = 160;

/// PTT channel range: 1-25 (24=Priority, 25=Emergency)
pub const PTT_CHANNEL_MIN: u8 = 1;
pub const PTT_CHANNEL_MAX: u8 = 25;
pub const PTT_PRIORITY_CHANNEL: u8 = 24;
pub const PTT_EMERGENCY_CHANNEL: u8 = 25;

/// Group Paging channel range: 26-50 (49=Priority, 50=Emergency)
pub const PAGING_CHANNEL_MIN: u8 = 26;
pub const PAGING_CHANNEL_MAX: u8 = 50;
pub const PAGING_PRIORITY_CHANNEL: u8 = 49;
pub const PAGING_EMERGENCY_CHANNEL: u8 = 50;

/// Minimum caller ID length for Polycom compatibility
/// Polycom phones pad caller ID to 13 bytes with nulls to create a fixed 20-byte header
/// (1 op + 1 channel + 4 serial + 1 len + 13 `caller_id` = 20 bytes)
pub const MIN_CALLER_ID_LEN: usize = 13;

// ============================================================================
// Error Types
// ============================================================================

#[derive(Error, Debug)]
pub enum PolycomError {
    #[error("Packet too short (minimum {expected} bytes, got {actual})")]
    TooShort { expected: usize, actual: usize },

    #[error("Invalid op code: 0x{0:02x}")]
    InvalidOpCode(u8),

    #[error("Invalid channel number: {0} (must be 1-50)")]
    InvalidChannel(u8),

    #[error("Invalid codec type: 0x{0:02x}")]
    InvalidCodec(u8),

    #[error("Packet truncated: expected {expected} bytes, got {actual}")]
    Truncated { expected: usize, actual: usize },

    #[error("Caller ID too long (max 255 bytes)")]
    CallerIdTooLong,
}

// ============================================================================
// Codec Type
// ============================================================================

/// Polycom-supported codec types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolycomCodec {
    /// G.711 µ-law (8kHz, 20ms frames, 160 bytes/frame)
    G711U,
    /// G.711 A-law (8kHz, 20ms frames, 160 bytes/frame)
    G711A,
    /// G.722 (16kHz, 20ms frames, 160 bytes/frame)
    G722,
}

impl PolycomCodec {
    /// Create from codec byte value
    pub const fn from_byte(b: u8) -> Option<Self> {
        match b {
            CODEC_G711U => Some(Self::G711U),
            CODEC_G711A => Some(Self::G711A),
            CODEC_G722 => Some(Self::G722),
            _ => None,
        }
    }

    /// Get the codec byte value
    pub const fn to_byte(&self) -> u8 {
        match self {
            Self::G711U => CODEC_G711U,
            Self::G711A => CODEC_G711A,
            Self::G722 => CODEC_G722,
        }
    }

    /// Get the sample rate for this codec
    pub const fn sample_rate(&self) -> u32 {
        match self {
            Self::G711U | Self::G711A => 8000,
            Self::G722 => 16000,
        }
    }

    /// Get the frame size in bytes for this codec
    pub const fn frame_size(&self) -> usize {
        match self {
            Self::G711U | Self::G711A => G711_FRAME_SIZE,
            Self::G722 => G722_FRAME_SIZE,
        }
    }

    /// Get the frame duration in milliseconds
    /// All Polycom codecs use 20ms frames
    pub const fn frame_duration_ms(&self) -> u32 {
        match self {
            Self::G711U | Self::G711A | Self::G722 => 20,
        }
    }

    /// Get a human-readable name
    pub const fn name(&self) -> &'static str {
        match self {
            Self::G711U => "G.711µ",
            Self::G711A => "G.711A",
            Self::G722 => "G.722",
        }
    }
}

impl std::fmt::Display for PolycomCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ============================================================================
// Packet Types
// ============================================================================

/// Type of Polycom packet
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    /// Alert packet - signals start of page
    Alert,
    /// Transmit packet - contains audio data
    Transmit,
    /// End packet - signals end of page
    End,
}

impl PacketType {
    /// Create from op code byte
    pub const fn from_op_code(op: u8) -> Option<Self> {
        match op {
            OP_ALERT => Some(Self::Alert),
            OP_TRANSMIT => Some(Self::Transmit),
            OP_END => Some(Self::End),
            _ => None,
        }
    }

    /// Get the op code byte
    pub const fn to_op_code(&self) -> u8 {
        match self {
            Self::Alert => OP_ALERT,
            Self::Transmit => OP_TRANSMIT,
            Self::End => OP_END,
        }
    }
}

// ============================================================================
// Packet Header
// ============================================================================

/// Common header for all Polycom packets
#[derive(Debug, Clone)]
pub struct PolycomHeader {
    /// Packet type (Alert, Transmit, End)
    pub packet_type: PacketType,
    /// Channel number (1-50)
    pub channel: u8,
    /// Host serial number (last 4 bytes of MAC address)
    pub host_serial: [u8; 4],
    /// Caller ID string
    pub caller_id: String,
}

impl PolycomHeader {
    /// Create a new header
    pub fn new(packet_type: PacketType, channel: u8, host_serial: [u8; 4], caller_id: String) -> Self {
        Self {
            packet_type,
            channel,
            host_serial,
            caller_id,
        }
    }

    /// Parse header from bytes
    pub fn parse(data: &[u8]) -> Result<(Self, usize), PolycomError> {
        // Minimum header: op(1) + channel(1) + serial(4) + caller_id_len(1) = 7 bytes
        if data.len() < 7 {
            return Err(PolycomError::TooShort {
                expected: 7,
                actual: data.len(),
            });
        }

        let op_code = data[0];
        let packet_type = PacketType::from_op_code(op_code)
            .ok_or(PolycomError::InvalidOpCode(op_code))?;

        let channel = data[1];
        if channel == 0 || channel > 50 {
            return Err(PolycomError::InvalidChannel(channel));
        }

        let host_serial = [data[2], data[3], data[4], data[5]];
        let caller_id_len = data[6] as usize;

        let header_len = 7 + caller_id_len;
        if data.len() < header_len {
            return Err(PolycomError::Truncated {
                expected: header_len,
                actual: data.len(),
            });
        }

        let caller_id = if caller_id_len > 0 {
            // Trim null padding bytes that Polycom phones add for fixed-size headers
            let caller_id_bytes = &data[7..7 + caller_id_len];
            let trimmed_len = caller_id_bytes.iter().position(|&b| b == 0).unwrap_or(caller_id_len);
            String::from_utf8_lossy(&caller_id_bytes[..trimmed_len]).to_string()
        } else {
            String::new()
        };

        Ok((
            Self {
                packet_type,
                channel,
                host_serial,
                caller_id,
            },
            header_len,
        ))
    }

    /// Encode header to bytes
    /// Pads caller ID to `MIN_CALLER_ID_LEN` (13) bytes with nulls to create a fixed 20-byte header
    /// This matches the format used by real Polycom phones
    pub fn encode(&self) -> Result<Vec<u8>, PolycomError> {
        let caller_id_bytes = self.caller_id.as_bytes();
        if caller_id_bytes.len() > 255 {
            return Err(PolycomError::CallerIdTooLong);
        }

        // Pad caller ID to minimum length (13 bytes) to create fixed 20-byte header
        let padded_len = caller_id_bytes.len().max(MIN_CALLER_ID_LEN);

        let mut buf = Vec::with_capacity(7 + padded_len);
        buf.push(self.packet_type.to_op_code());
        buf.push(self.channel);
        buf.extend_from_slice(&self.host_serial);
        buf.push(padded_len as u8);
        buf.extend_from_slice(caller_id_bytes);

        // Pad with nulls to reach minimum length
        if caller_id_bytes.len() < MIN_CALLER_ID_LEN {
            buf.resize(7 + padded_len, 0);
        }

        Ok(buf)
    }

    /// Get the header length in bytes (with padding)
    pub fn len(&self) -> usize {
        7 + self.caller_id.len().max(MIN_CALLER_ID_LEN)
    }

    /// Check if this is an emergency channel
    pub fn is_emergency(&self) -> bool {
        self.channel == PTT_EMERGENCY_CHANNEL || self.channel == PAGING_EMERGENCY_CHANNEL
    }

    /// Check if this is a priority channel
    pub fn is_priority(&self) -> bool {
        self.channel == PTT_PRIORITY_CHANNEL || self.channel == PAGING_PRIORITY_CHANNEL
    }
}

// ============================================================================
// Audio Header (for Transmit packets)
// ============================================================================

/// Audio header for Transmit packets
#[derive(Debug, Clone)]
pub struct AudioHeader {
    /// Codec type
    pub codec: PolycomCodec,
    /// Flags byte (purpose not fully documented)
    pub flags: u8,
    /// Sample count / RTP timestamp
    pub sample_count: u32,
}

impl AudioHeader {
    /// Create a new audio header
    pub fn new(codec: PolycomCodec, flags: u8, sample_count: u32) -> Self {
        Self {
            codec,
            flags,
            sample_count,
        }
    }

    /// Parse audio header from bytes
    pub fn parse(data: &[u8]) -> Result<Self, PolycomError> {
        if data.len() < 6 {
            return Err(PolycomError::TooShort {
                expected: 6,
                actual: data.len(),
            });
        }

        let codec_byte = data[0];
        let codec = PolycomCodec::from_byte(codec_byte)
            .ok_or(PolycomError::InvalidCodec(codec_byte))?;

        let flags = data[1];
        let sample_count = u32::from_be_bytes([data[2], data[3], data[4], data[5]]);

        Ok(Self {
            codec,
            flags,
            sample_count,
        })
    }

    /// Encode audio header to bytes
    pub fn encode(&self) -> Vec<u8> {
        self.encode_with_endian(true)
    }

    /// Encode audio header with configurable endianness
    pub fn encode_with_endian(&self, big_endian: bool) -> Vec<u8> {
        let mut buf = Vec::with_capacity(6);
        buf.push(self.codec.to_byte());
        buf.push(self.flags);
        if big_endian {
            buf.extend_from_slice(&self.sample_count.to_be_bytes());
        } else {
            buf.extend_from_slice(&self.sample_count.to_le_bytes());
        }
        buf
    }

    /// Audio header is always 6 bytes
    pub const fn len() -> usize {
        6
    }
}

// ============================================================================
// Parsed Packet
// ============================================================================

/// A parsed Polycom packet
#[derive(Debug, Clone)]
pub struct PolycomPacket {
    /// Common header
    pub header: PolycomHeader,
    /// Audio header (only for Transmit packets)
    pub audio_header: Option<AudioHeader>,
    /// Redundant audio frame (previous packet's audio, for error recovery)
    pub redundant_frame: Option<Vec<u8>>,
    /// Current audio frame
    pub audio_frame: Option<Vec<u8>>,
    /// Receive timestamp
    pub received_at: Instant,
    /// Source address
    pub source: SocketAddr,
}

impl PolycomPacket {
    /// Parse a Polycom packet from raw bytes
    pub fn parse(data: &[u8], source: SocketAddr) -> Result<Self, PolycomError> {
        Self::parse_with_time(data, source, Instant::now())
    }

    /// Parse with a specific receive time
    pub fn parse_with_time(
        data: &[u8],
        source: SocketAddr,
        received_at: Instant,
    ) -> Result<Self, PolycomError> {
        let (header, header_len) = PolycomHeader::parse(data)?;

        match header.packet_type {
            PacketType::Alert | PacketType::End => {
                // Alert and End packets have no audio payload
                Ok(Self {
                    header,
                    audio_header: None,
                    redundant_frame: None,
                    audio_frame: None,
                    received_at,
                    source,
                })
            }
            PacketType::Transmit => {
                // Transmit packets have audio header + redundant frame + current frame
                let payload = &data[header_len..];

                if payload.len() < AudioHeader::len() {
                    return Err(PolycomError::TooShort {
                        expected: header_len + AudioHeader::len(),
                        actual: data.len(),
                    });
                }

                let audio_header = AudioHeader::parse(payload)?;
                let audio_data = &payload[AudioHeader::len()..];
                let frame_size = audio_header.codec.frame_size();

                // First transmit packet has only one frame, subsequent have redundant + current
                let (redundant_frame, audio_frame) = if audio_data.len() >= frame_size * 2 {
                    // Has redundant frame
                    (
                        Some(audio_data[..frame_size].to_vec()),
                        Some(audio_data[frame_size..frame_size * 2].to_vec()),
                    )
                } else if audio_data.len() >= frame_size {
                    // Only current frame (first packet)
                    (None, Some(audio_data[..frame_size].to_vec()))
                } else {
                    // Incomplete frame
                    (None, None)
                };

                Ok(Self {
                    header,
                    audio_header: Some(audio_header),
                    redundant_frame,
                    audio_frame,
                    received_at,
                    source,
                })
            }
        }
    }
}

// ============================================================================
// Packet Builder
// ============================================================================

/// Builder for creating Polycom packets
#[derive(Debug)]
pub struct PolycomPacketBuilder {
    /// Channel number
    channel: u8,
    /// Host serial number (last 4 bytes of MAC)
    host_serial: [u8; 4],
    /// Caller ID string
    caller_id: String,
    /// Codec to use
    codec: PolycomCodec,
    /// Current sample count (timestamp)
    sample_count: u32,
    /// Previous frame for redundancy
    previous_frame: Option<Vec<u8>>,
    /// Skip redundant frames (for debugging)
    skip_redundant: bool,
    /// Skip audio header (for debugging)
    skip_audio_header: bool,
    /// Use little-endian byte order for sample count
    little_endian: bool,
}

impl PolycomPacketBuilder {
    /// Create a new packet builder
    pub fn new(
        channel: u8,
        host_serial: [u8; 4],
        caller_id: String,
        codec: PolycomCodec,
    ) -> Self {
        Self {
            channel,
            host_serial,
            caller_id,
            codec,
            sample_count: Self::generate_initial_sample_count(),
            previous_frame: None,
            skip_redundant: false,
            skip_audio_header: false,
            little_endian: false,
        }
    }

    /// Generate a random initial sample count (similar to RTP timestamp initialization)
    /// Polycom phones use non-zero starting values, likely for security/uniqueness
    fn generate_initial_sample_count() -> u32 {
        use std::time::SystemTime;
        let seed = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        // Use a simple hash to generate a pseudo-random 32-bit value
        let hash = seed.wrapping_mul(0x517c_c1b7_2722_0a95);
        (hash >> 32) as u32
    }

    /// Create from MAC address (extracts last 4 bytes)
    pub fn from_mac(
        channel: u8,
        mac: [u8; 6],
        caller_id: String,
        codec: PolycomCodec,
    ) -> Self {
        let host_serial = [mac[2], mac[3], mac[4], mac[5]];
        Self::new(channel, host_serial, caller_id, codec)
    }

    /// Set whether to skip redundant frames
    pub fn set_skip_redundant(&mut self, skip: bool) {
        self.skip_redundant = skip;
    }

    /// Set whether to skip audio header
    pub fn set_skip_audio_header(&mut self, skip: bool) {
        self.skip_audio_header = skip;
    }

    /// Set whether to use little-endian byte order for sample count
    pub fn set_little_endian(&mut self, little_endian: bool) {
        self.little_endian = little_endian;
    }

    /// Build an Alert packet
    pub fn build_alert(&self) -> Result<Vec<u8>, PolycomError> {
        let header = PolycomHeader::new(
            PacketType::Alert,
            self.channel,
            self.host_serial,
            self.caller_id.clone(),
        );
        header.encode()
    }

    /// Build a Transmit packet with audio data
    pub fn build_transmit(&mut self, audio_frame: &[u8]) -> Result<Vec<u8>, PolycomError> {
        let header = PolycomHeader::new(
            PacketType::Transmit,
            self.channel,
            self.host_serial,
            self.caller_id.clone(),
        );

        let mut packet = header.encode()?;

        // Add audio header unless skipping
        if !self.skip_audio_header {
            let audio_header = AudioHeader::new(self.codec, 0, self.sample_count);
            packet.extend(audio_header.encode_with_endian(!self.little_endian));

            // Add redundant frame if we have one (not on first packet) and not skipping
            if !self.skip_redundant {
                if let Some(ref prev) = self.previous_frame {
                    packet.extend_from_slice(prev);
                }
            }
        }

        // Add current frame
        packet.extend_from_slice(audio_frame);

        // Update state for next packet (still track for potential future use)
        self.previous_frame = Some(audio_frame.to_vec());
        self.sample_count = self.sample_count.wrapping_add(self.codec.frame_size() as u32);

        Ok(packet)
    }

    /// Build an End packet
    pub fn build_end(&self) -> Result<Vec<u8>, PolycomError> {
        let header = PolycomHeader::new(
            PacketType::End,
            self.channel,
            self.host_serial,
            self.caller_id.clone(),
        );
        header.encode()
    }

    /// Reset the builder state (call between pages)
    pub fn reset(&mut self) {
        self.sample_count = Self::generate_initial_sample_count();
        self.previous_frame = None;
    }

    /// Get the current codec
    pub fn codec(&self) -> PolycomCodec {
        self.codec
    }

    /// Get the channel number
    pub fn channel(&self) -> u8 {
        self.channel
    }
}

// ============================================================================
// Session State
// ============================================================================

/// State of a Polycom paging session
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// No active session
    Idle,
    /// Alert packets being sent/received
    Alerting,
    /// Audio is being transmitted
    Transmitting,
    /// End packets being sent/received
    Ending,
}

/// Tracks the state of an active Polycom session (for receiving)
#[derive(Debug)]
pub struct PolycomSession {
    /// Channel number
    pub channel: u8,
    /// Current state
    pub state: SessionState,
    /// Caller ID from Alert packet
    pub caller_id: String,
    /// Host serial from sender
    pub host_serial: [u8; 4],
    /// Codec being used (determined from first Transmit packet)
    pub codec: Option<PolycomCodec>,
    /// Time session started
    pub started_at: Instant,
    /// Last packet received time
    pub last_packet_at: Instant,
    /// Number of Alert packets received
    pub alert_count: u32,
    /// Number of audio packets received
    pub audio_packet_count: u32,
    /// Number of End packets received
    pub end_count: u32,
}

impl PolycomSession {
    /// Create a new session from an Alert packet
    pub fn from_alert(packet: &PolycomPacket) -> Self {
        Self {
            channel: packet.header.channel,
            state: SessionState::Alerting,
            caller_id: packet.header.caller_id.clone(),
            host_serial: packet.header.host_serial,
            codec: None,
            started_at: packet.received_at,
            last_packet_at: packet.received_at,
            alert_count: 1,
            audio_packet_count: 0,
            end_count: 0,
        }
    }

    /// Update session with a new packet
    pub fn update(&mut self, packet: &PolycomPacket) {
        self.last_packet_at = packet.received_at;

        match packet.header.packet_type {
            PacketType::Alert => {
                self.alert_count += 1;
            }
            PacketType::Transmit => {
                if self.state == SessionState::Alerting {
                    self.state = SessionState::Transmitting;
                }
                if let Some(ref audio_hdr) = packet.audio_header {
                    self.codec = Some(audio_hdr.codec);
                }
                self.audio_packet_count += 1;
            }
            PacketType::End => {
                self.state = SessionState::Ending;
                self.end_count += 1;
            }
        }
    }

    /// Check if the session has timed out
    pub fn is_timed_out(&self, timeout_ms: u64) -> bool {
        self.last_packet_at.elapsed().as_millis() as u64 > timeout_ms
    }

    /// Check if the session is complete (received enough End packets)
    pub fn is_complete(&self) -> bool {
        self.state == SessionState::Ending && self.end_count >= 3
    }

    /// Get the duration of the session
    pub fn duration(&self) -> std::time::Duration {
        self.last_packet_at.duration_since(self.started_at)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn test_source() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 5001)
    }

    #[test]
    fn test_codec_properties() {
        assert_eq!(PolycomCodec::G711U.sample_rate(), 8000);
        assert_eq!(PolycomCodec::G711U.frame_size(), 160);
        assert_eq!(PolycomCodec::G711U.frame_duration_ms(), 20);

        assert_eq!(PolycomCodec::G722.sample_rate(), 16000);
        assert_eq!(PolycomCodec::G722.frame_size(), 160);
        assert_eq!(PolycomCodec::G722.frame_duration_ms(), 20);
    }

    #[test]
    fn test_header_encode_decode() {
        let header = PolycomHeader::new(
            PacketType::Alert,
            26,
            [0xAB, 0xCD, 0xEF, 0x01],
            "Test Caller".to_string(),
        );

        let encoded = header.encode().unwrap();
        let (decoded, len) = PolycomHeader::parse(&encoded).unwrap();

        assert_eq!(decoded.packet_type, PacketType::Alert);
        assert_eq!(decoded.channel, 26);
        assert_eq!(decoded.host_serial, [0xAB, 0xCD, 0xEF, 0x01]);
        assert_eq!(decoded.caller_id, "Test Caller");
        assert_eq!(len, encoded.len());
    }

    #[test]
    fn test_audio_header_encode_decode() {
        let audio_header = AudioHeader::new(PolycomCodec::G711U, 0, 160);
        let encoded = audio_header.encode();
        let decoded = AudioHeader::parse(&encoded).unwrap();

        assert_eq!(decoded.codec, PolycomCodec::G711U);
        assert_eq!(decoded.flags, 0);
        assert_eq!(decoded.sample_count, 160);
    }

    #[test]
    fn test_alert_packet_roundtrip() {
        let builder = PolycomPacketBuilder::new(
            26,
            [0x12, 0x34, 0x56, 0x78],
            "MPS-IP".to_string(),
            PolycomCodec::G711U,
        );

        let packet_data = builder.build_alert().unwrap();
        let parsed = PolycomPacket::parse(&packet_data, test_source()).unwrap();

        assert_eq!(parsed.header.packet_type, PacketType::Alert);
        assert_eq!(parsed.header.channel, 26);
        assert_eq!(parsed.header.caller_id, "MPS-IP");
        assert!(parsed.audio_header.is_none());
        assert!(parsed.audio_frame.is_none());
    }

    #[test]
    fn test_transmit_packet_first_frame() {
        let mut builder = PolycomPacketBuilder::new(
            26,
            [0x12, 0x34, 0x56, 0x78],
            "MPS-IP".to_string(),
            PolycomCodec::G711U,
        );

        // First frame - no redundant data
        let audio = vec![0xAA; 160];
        let packet_data = builder.build_transmit(&audio).unwrap();
        let parsed = PolycomPacket::parse(&packet_data, test_source()).unwrap();

        assert_eq!(parsed.header.packet_type, PacketType::Transmit);
        assert!(parsed.audio_header.is_some());
        assert!(parsed.redundant_frame.is_none());
        assert_eq!(parsed.audio_frame.as_ref().unwrap().len(), 160);
    }

    #[test]
    fn test_transmit_packet_with_redundancy() {
        let mut builder = PolycomPacketBuilder::new(
            26,
            [0x12, 0x34, 0x56, 0x78],
            "MPS-IP".to_string(),
            PolycomCodec::G711U,
        );

        // First frame
        let audio1 = vec![0xAA; 160];
        let _ = builder.build_transmit(&audio1).unwrap();

        // Second frame - should include redundant copy of first
        let audio2 = vec![0xBB; 160];
        let packet_data = builder.build_transmit(&audio2).unwrap();
        let parsed = PolycomPacket::parse(&packet_data, test_source()).unwrap();

        assert!(parsed.redundant_frame.is_some());
        assert_eq!(parsed.redundant_frame.as_ref().unwrap(), &audio1);
        assert_eq!(parsed.audio_frame.as_ref().unwrap(), &audio2);
    }

    #[test]
    fn test_end_packet_roundtrip() {
        let builder = PolycomPacketBuilder::new(
            26,
            [0x12, 0x34, 0x56, 0x78],
            "MPS-IP".to_string(),
            PolycomCodec::G711U,
        );

        let packet_data = builder.build_end().unwrap();
        let parsed = PolycomPacket::parse(&packet_data, test_source()).unwrap();

        assert_eq!(parsed.header.packet_type, PacketType::End);
        assert_eq!(parsed.header.channel, 26);
    }

    #[test]
    fn test_emergency_priority_detection() {
        let header_emergency = PolycomHeader::new(
            PacketType::Alert,
            50,
            [0; 4],
            String::new(),
        );
        assert!(header_emergency.is_emergency());
        assert!(!header_emergency.is_priority());

        let header_priority = PolycomHeader::new(
            PacketType::Alert,
            49,
            [0; 4],
            String::new(),
        );
        assert!(!header_priority.is_emergency());
        assert!(header_priority.is_priority());
    }

    #[test]
    fn test_invalid_channel() {
        let data = [OP_ALERT, 0, 0, 0, 0, 0, 0]; // Channel 0 is invalid
        let result = PolycomHeader::parse(&data);
        assert!(matches!(result, Err(PolycomError::InvalidChannel(0))));

        let data = [OP_ALERT, 51, 0, 0, 0, 0, 0]; // Channel 51 is invalid
        let result = PolycomHeader::parse(&data);
        assert!(matches!(result, Err(PolycomError::InvalidChannel(51))));
    }

    #[test]
    fn test_invalid_op_code() {
        let data = [0x00, 26, 0, 0, 0, 0, 0]; // 0x00 is not a valid op code
        let result = PolycomHeader::parse(&data);
        assert!(matches!(result, Err(PolycomError::InvalidOpCode(0x00))));
    }

    #[test]
    fn test_session_state_transitions() {
        let alert_header = PolycomHeader::new(
            PacketType::Alert,
            26,
            [0; 4],
            "Test".to_string(),
        );
        let alert_packet = PolycomPacket {
            header: alert_header,
            audio_header: None,
            redundant_frame: None,
            audio_frame: None,
            received_at: Instant::now(),
            source: test_source(),
        };

        let mut session = PolycomSession::from_alert(&alert_packet);
        assert_eq!(session.state, SessionState::Alerting);
        assert_eq!(session.alert_count, 1);

        // Simulate receiving a transmit packet
        let transmit_header = PolycomHeader::new(
            PacketType::Transmit,
            26,
            [0; 4],
            "Test".to_string(),
        );
        let transmit_packet = PolycomPacket {
            header: transmit_header,
            audio_header: Some(AudioHeader::new(PolycomCodec::G711U, 0, 0)),
            redundant_frame: None,
            audio_frame: Some(vec![0; 160]),
            received_at: Instant::now(),
            source: test_source(),
        };

        session.update(&transmit_packet);
        assert_eq!(session.state, SessionState::Transmitting);
        assert_eq!(session.audio_packet_count, 1);
        assert_eq!(session.codec, Some(PolycomCodec::G711U));
    }
}
