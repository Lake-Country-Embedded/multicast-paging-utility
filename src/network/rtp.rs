//! RTP (Real-time Transport Protocol) packet parsing and building.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::time::Instant;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RtpError {
    #[error("Packet too short (minimum 12 bytes required, got {0})")]
    TooShort(usize),

    #[error("Invalid RTP version: {0} (expected 2)")]
    InvalidVersion(u8),

    #[error("Packet truncated: expected {expected} bytes, got {actual}")]
    Truncated { expected: usize, actual: usize },

    #[error("Invalid padding length")]
    InvalidPadding,
}

/// RTP header as defined in RFC 3550
#[derive(Debug, Clone)]
pub struct RtpHeader {
    /// RTP version (always 2)
    pub version: u8,
    /// Padding flag
    pub padding: bool,
    /// Extension header present
    pub extension: bool,
    /// CSRC count
    pub csrc_count: u8,
    /// Marker bit
    pub marker: bool,
    /// Payload type (7 bits)
    pub payload_type: u8,
    /// Sequence number (16 bits)
    pub sequence_number: u16,
    /// Timestamp (32 bits)
    pub timestamp: u32,
    /// Synchronization source identifier
    pub ssrc: u32,
    /// Contributing source identifiers
    pub csrc: Vec<u32>,
}

/// Complete RTP packet with parsed header and payload
#[derive(Debug, Clone)]
pub struct RtpPacket {
    pub header: RtpHeader,
    pub payload: Vec<u8>,
    pub received_at: Instant,
    pub source: SocketAddr,
}

impl RtpPacket {
    /// Parse an RTP packet from raw bytes
    pub fn parse(data: &[u8], source: SocketAddr) -> Result<Self, RtpError> {
        Self::parse_with_time(data, source, Instant::now())
    }

    /// Parse an RTP packet with a specific receive time
    pub fn parse_with_time(data: &[u8], source: SocketAddr, received_at: Instant) -> Result<Self, RtpError> {
        if data.len() < 12 {
            return Err(RtpError::TooShort(data.len()));
        }

        // First byte: V(2) P(1) X(1) CC(4)
        let first = data[0];
        let version = (first >> 6) & 0x03;
        if version != 2 {
            return Err(RtpError::InvalidVersion(version));
        }

        let padding = (first >> 5) & 0x01 != 0;
        let extension = (first >> 4) & 0x01 != 0;
        let csrc_count = first & 0x0F;

        // Second byte: M(1) PT(7)
        let second = data[1];
        let marker = (second >> 7) & 0x01 != 0;
        let payload_type = second & 0x7F;

        // Sequence number (bytes 2-3)
        let sequence_number = u16::from_be_bytes([data[2], data[3]]);

        // Timestamp (bytes 4-7)
        let timestamp = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);

        // SSRC (bytes 8-11)
        let ssrc = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

        // Calculate header length
        let mut header_len = 12 + (csrc_count as usize * 4);

        if data.len() < header_len {
            return Err(RtpError::Truncated {
                expected: header_len,
                actual: data.len(),
            });
        }

        // Parse CSRC list
        let mut csrc = Vec::with_capacity(csrc_count as usize);
        for i in 0..csrc_count as usize {
            let offset = 12 + i * 4;
            let val = u32::from_be_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            csrc.push(val);
        }

        // Handle extension header
        if extension {
            if data.len() < header_len + 4 {
                return Err(RtpError::Truncated {
                    expected: header_len + 4,
                    actual: data.len(),
                });
            }
            // Extension header: 2 bytes profile, 2 bytes length (in 32-bit words)
            let ext_length = u16::from_be_bytes([data[header_len + 2], data[header_len + 3]]) as usize * 4;
            header_len += 4 + ext_length;

            if data.len() < header_len {
                return Err(RtpError::Truncated {
                    expected: header_len,
                    actual: data.len(),
                });
            }
        }

        // Handle padding
        // Note: data.len() >= 12 is already validated above, so no empty check needed
        let payload_end = if padding {
            let padding_len = data[data.len() - 1] as usize;
            if padding_len == 0 || padding_len > data.len() - header_len {
                return Err(RtpError::InvalidPadding);
            }
            data.len() - padding_len
        } else {
            data.len()
        };

        let payload = data[header_len..payload_end].to_vec();

        Ok(RtpPacket {
            header: RtpHeader {
                version,
                padding,
                extension,
                csrc_count,
                marker,
                payload_type,
                sequence_number,
                timestamp,
                ssrc,
                csrc,
            },
            payload,
            received_at,
            source,
        })
    }

    /// Build an RTP packet from components
    pub fn build(
        payload_type: u8,
        sequence_number: u16,
        timestamp: u32,
        ssrc: u32,
        payload: &[u8],
        marker: bool,
    ) -> Vec<u8> {
        let mut packet = Vec::with_capacity(12 + payload.len());

        // First byte: V=2, P=0, X=0, CC=0
        packet.push(0x80);

        // Second byte: M + PT
        let second = if marker { 0x80 } else { 0x00 } | (payload_type & 0x7F);
        packet.push(second);

        // Sequence number
        packet.extend_from_slice(&sequence_number.to_be_bytes());

        // Timestamp
        packet.extend_from_slice(&timestamp.to_be_bytes());

        // SSRC
        packet.extend_from_slice(&ssrc.to_be_bytes());

        // Payload
        packet.extend_from_slice(payload);

        packet
    }
}

/// Standard RTP payload types as defined in RFC 3551
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PayloadType {
    /// G.711 u-law (PCMU) - 8kHz, mono
    Pcmu,
    /// G.711 A-law (PCMA) - 8kHz, mono
    Pcma,
    /// G.722 - 8kHz (actually 16kHz wideband), mono
    G722,
    /// L16 stereo - 44.1kHz
    L16Stereo,
    /// L16 mono - 44.1kHz
    L16Mono,
    /// Dynamic payload type (96-127), typically Opus
    Dynamic(u8),
    /// Unknown payload type
    Unknown(u8),
}

impl PayloadType {
    /// Convert from RTP payload type number
    #[must_use]
    pub const fn from_pt(pt: u8) -> Self {
        match pt {
            0 => PayloadType::Pcmu,
            8 => PayloadType::Pcma,
            9 => PayloadType::G722,
            10 => PayloadType::L16Stereo,
            11 => PayloadType::L16Mono,
            96..=127 => PayloadType::Dynamic(pt),
            _ => PayloadType::Unknown(pt),
        }
    }

    /// Get the payload type number
    #[must_use]
    pub const fn to_pt(&self) -> u8 {
        match self {
            PayloadType::Pcmu => 0,
            PayloadType::Pcma => 8,
            PayloadType::G722 => 9,
            PayloadType::L16Stereo => 10,
            PayloadType::L16Mono => 11,
            PayloadType::Dynamic(pt) | PayloadType::Unknown(pt) => *pt,
        }
    }

    /// Get the sample rate for this payload type
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        match self {
            PayloadType::Pcmu | PayloadType::Pcma => 8000,
            PayloadType::G722 => 16000, // Actually 16kHz audio, but RTP clock is 8000
            PayloadType::L16Stereo | PayloadType::L16Mono => 44100,
            PayloadType::Dynamic(_) => 48000, // Assume Opus
            PayloadType::Unknown(_) => 8000,
        }
    }

    /// Get the number of channels
    #[must_use]
    pub const fn channels(&self) -> u8 {
        match self {
            PayloadType::Pcmu | PayloadType::Pcma | PayloadType::G722 | PayloadType::L16Mono => 1,
            PayloadType::L16Stereo => 2,
            PayloadType::Dynamic(_) => 2, // Assume Opus stereo
            PayloadType::Unknown(_) => 1,
        }
    }

    /// Get a human-readable name
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            PayloadType::Pcmu => "G.711 u-law",
            PayloadType::Pcma => "G.711 A-law",
            PayloadType::G722 => "G.722",
            PayloadType::L16Stereo => "L16 Stereo",
            PayloadType::L16Mono => "L16 Mono",
            PayloadType::Dynamic(_) => "Opus",
            PayloadType::Unknown(_) => "Unknown",
        }
    }
}

impl std::fmt::Display for PayloadType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn test_source() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 5004)
    }

    #[test]
    fn test_parse_valid_rtp_packet() {
        // Minimal valid RTP packet: V=2, P=0, X=0, CC=0, M=0, PT=0
        let data = [
            0x80, 0x00, // V=2, P=0, X=0, CC=0, M=0, PT=0
            0x00, 0x01, // Sequence number = 1
            0x00, 0x00, 0x00, 0xA0, // Timestamp = 160
            0x12, 0x34, 0x56, 0x78, // SSRC
            0xAA, 0xBB, // Payload
        ];

        let packet = RtpPacket::parse(&data, test_source()).unwrap();
        assert_eq!(packet.header.version, 2);
        assert!(!packet.header.padding);
        assert!(!packet.header.extension);
        assert_eq!(packet.header.csrc_count, 0);
        assert!(!packet.header.marker);
        assert_eq!(packet.header.payload_type, 0);
        assert_eq!(packet.header.sequence_number, 1);
        assert_eq!(packet.header.timestamp, 160);
        assert_eq!(packet.header.ssrc, 0x12345678);
        assert_eq!(packet.payload, vec![0xAA, 0xBB]);
    }

    #[test]
    fn test_parse_with_csrc() {
        // RTP packet with 2 CSRC entries
        let data = [
            0x82, 0x00, // V=2, P=0, X=0, CC=2
            0x00, 0x01, // Sequence number
            0x00, 0x00, 0x00, 0xA0, // Timestamp
            0x12, 0x34, 0x56, 0x78, // SSRC
            0x11, 0x11, 0x11, 0x11, // CSRC 1
            0x22, 0x22, 0x22, 0x22, // CSRC 2
            0xAA, // Payload
        ];

        let packet = RtpPacket::parse(&data, test_source()).unwrap();
        assert_eq!(packet.header.csrc_count, 2);
        assert_eq!(packet.header.csrc.len(), 2);
        assert_eq!(packet.header.csrc[0], 0x11111111);
        assert_eq!(packet.header.csrc[1], 0x22222222);
        assert_eq!(packet.payload, vec![0xAA]);
    }

    #[test]
    fn test_parse_with_marker() {
        let data = [
            0x80, 0x80, // V=2, M=1, PT=0
            0x00, 0x01, 0x00, 0x00, 0x00, 0xA0, 0x12, 0x34, 0x56, 0x78,
        ];

        let packet = RtpPacket::parse(&data, test_source()).unwrap();
        assert!(packet.header.marker);
    }

    #[test]
    fn test_parse_too_short() {
        let data = [0x80, 0x00, 0x00];
        let result = RtpPacket::parse(&data, test_source());
        assert!(matches!(result, Err(RtpError::TooShort(3))));
    }

    #[test]
    fn test_parse_invalid_version() {
        let data = [
            0x00, 0x00, // V=0 (invalid)
            0x00, 0x01, 0x00, 0x00, 0x00, 0xA0, 0x12, 0x34, 0x56, 0x78,
        ];

        let result = RtpPacket::parse(&data, test_source());
        assert!(matches!(result, Err(RtpError::InvalidVersion(0))));
    }

    #[test]
    fn test_build_rtp_packet() {
        let payload = vec![0xAA, 0xBB, 0xCC];
        let packet = RtpPacket::build(0, 1, 160, 0x12345678, &payload, false);

        assert_eq!(packet.len(), 12 + 3);
        assert_eq!(packet[0], 0x80); // V=2
        assert_eq!(packet[1], 0x00); // PT=0
        assert_eq!(&packet[12..], &[0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn test_roundtrip() {
        let payload = vec![0x01, 0x02, 0x03, 0x04];
        let built = RtpPacket::build(8, 100, 16000, 0xABCDEF00, &payload, true);
        let parsed = RtpPacket::parse(&built, test_source()).unwrap();

        assert_eq!(parsed.header.payload_type, 8);
        assert_eq!(parsed.header.sequence_number, 100);
        assert_eq!(parsed.header.timestamp, 16000);
        assert_eq!(parsed.header.ssrc, 0xABCDEF00);
        assert!(parsed.header.marker);
        assert_eq!(parsed.payload, payload);
    }

    #[test]
    fn test_payload_types() {
        assert_eq!(PayloadType::from_pt(0), PayloadType::Pcmu);
        assert_eq!(PayloadType::from_pt(8), PayloadType::Pcma);
        assert_eq!(PayloadType::from_pt(9), PayloadType::G722);
        assert_eq!(PayloadType::from_pt(96), PayloadType::Dynamic(96));
        assert_eq!(PayloadType::Pcmu.sample_rate(), 8000);
        assert_eq!(PayloadType::G722.sample_rate(), 16000);
    }
}
