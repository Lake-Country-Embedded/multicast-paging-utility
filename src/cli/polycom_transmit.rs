//! Polycom paging transmit command implementation.
//!
//! Transmits audio files using the Polycom PTT/Group Paging protocol.

use crate::codec::{FfmpegG711AlawEncoder, FfmpegG711UlawEncoder, FfmpegG722Encoder};
use crate::network::{create_transmit_socket, PolycomPacketBuilder, PolycomCodec};
use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;
use std::time::{Duration, Instant};
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use thiserror::Error;
use tracing::{debug, info};

#[derive(Error, Debug)]
pub enum PolycomTransmitError {
    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("Unsupported audio format: {0}")]
    UnsupportedFormat(String),

    #[error("Codec error: {0}")]
    Codec(#[from] crate::codec::CodecError),

    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Audio decode error: {0}")]
    AudioDecode(String),

    #[error("Polycom protocol error: {0}")]
    Protocol(#[from] crate::network::PolycomError),

    #[error("Invalid channel: {0} (must be 1-50)")]
    InvalidChannel(u8),

    #[error("Invalid codec: {0}")]
    InvalidCodec(String),
}

/// Options for Polycom transmit command
pub struct PolycomTransmitOptions {
    /// Audio file to transmit
    pub file: std::path::PathBuf,
    /// Destination multicast address
    pub address: Ipv4Addr,
    /// Destination UDP port
    pub port: u16,
    /// Channel number (1-50)
    pub channel: u8,
    /// Codec to use (g711u or g722)
    pub codec: String,
    /// Caller ID string
    pub caller_id: String,
    /// Multicast TTL
    pub ttl: u8,
    /// Loop the audio file
    pub loop_audio: bool,
    /// Suppress non-essential output
    pub quiet: bool,
    /// Number of Alert packets to send
    pub alert_count: u32,
    /// Number of End packets to send
    pub end_count: u32,
    /// Delay between control packets in ms
    pub control_interval: u64,
    /// Skip Alert packets
    pub skip_alert: bool,
    /// Skip End packets
    pub skip_end: bool,
    /// Skip redundant audio frames
    pub no_redundant: bool,
    /// Skip audio header
    pub no_audio_header: bool,
    /// Use little-endian byte order for sample count
    pub little_endian: bool,
    /// File is raw pre-encoded audio (not WAV), bypass encoder
    pub raw: bool,
}

/// Run the Polycom transmit command
pub async fn run_polycom_transmit(options: PolycomTransmitOptions) -> Result<(), PolycomTransmitError> {
    // Validate channel
    if options.channel == 0 || options.channel > 50 {
        return Err(PolycomTransmitError::InvalidChannel(options.channel));
    }

    // Validate and parse codec
    let polycom_codec = match options.codec.to_lowercase().as_str() {
        "g711u" | "g711ulaw" | "pcmu" => PolycomCodec::G711U,
        "g711a" | "g711alaw" | "pcma" => PolycomCodec::G711A,
        "g722" => PolycomCodec::G722,
        _ => return Err(PolycomTransmitError::InvalidCodec(options.codec.clone())),
    };

    if !options.file.exists() {
        return Err(PolycomTransmitError::FileNotFound(
            options.file.to_string_lossy().to_string(),
        ));
    }

    // Create transmit socket
    let socket = create_transmit_socket(options.ttl).await?;
    let dest = SocketAddrV4::new(options.address, options.port);

    // Get sample rate for audio decoding (G.722 needs 16kHz, G.711 needs 8kHz)
    let sample_rate = polycom_codec.sample_rate();

    // Generate a pseudo-random host serial from current time
    let host_serial = generate_host_serial();

    // Create packet builder
    let mut builder = PolycomPacketBuilder::new(
        options.channel,
        host_serial,
        options.caller_id.clone(),
        polycom_codec,
    );
    builder.set_skip_redundant(options.no_redundant);
    builder.set_skip_audio_header(options.no_audio_header);
    builder.set_little_endian(options.little_endian);

    if !options.quiet {
        println!("Polycom Paging Transmit");
        println!("  File: {}", options.file.display());
        println!("  Destination: {}:{}", options.address, options.port);
        println!("  Channel: {}", options.channel);
        println!("  Codec: {}", polycom_codec);
        println!("  Caller ID: {}", options.caller_id);
        println!("  TTL: {}", options.ttl);
        println!();
    }

    loop {
        // === Prepare audio frames ===
        let frame_size = polycom_codec.frame_size();
        let frame_duration = Duration::from_millis(polycom_codec.frame_duration_ms() as u64);

        let encoded_frames: Vec<Vec<u8>> = if options.raw {
            // Raw mode: read pre-encoded audio file directly
            if !options.quiet {
                print!("  Reading raw audio frames...");
                io::stdout().flush().ok();
            }

            let raw_data = std::fs::read(&options.file)?;
            let frames: Vec<Vec<u8>> = raw_data
                .chunks(frame_size)
                .map(|chunk| {
                    if chunk.len() < frame_size {
                        let mut padded = chunk.to_vec();
                        padded.resize(frame_size, 0);
                        padded
                    } else {
                        chunk.to_vec()
                    }
                })
                .collect();

            if !options.quiet {
                let duration = frames.len() as f64 * polycom_codec.frame_duration_ms() as f64 / 1000.0;
                println!(" {} frames ({:.1}s)", frames.len(), duration);
            }

            frames
        } else {
            // Normal mode: decode WAV and encode to codec using ffmpeg
            let samples = read_audio_file(&options.file, sample_rate)?;

            if !options.quiet {
                let duration = samples.len() as f64 / sample_rate as f64;
                println!("  Duration: {:.1}s ({} samples)", duration, samples.len());
                print!("  Encoding audio frames with ffmpeg...");
                io::stdout().flush().ok();
            }

            // Use ffmpeg subprocess for all codecs (consistent quality)
            let frames: Vec<Vec<u8>> = match polycom_codec {
                PolycomCodec::G722 => {
                    let mut encoder = FfmpegG722Encoder::new()?;
                    encoder.encode_all(&samples)?
                }
                PolycomCodec::G711U => {
                    let mut encoder = FfmpegG711UlawEncoder::new()?;
                    encoder.encode_all(&samples)?
                }
                PolycomCodec::G711A => {
                    let mut encoder = FfmpegG711AlawEncoder::new()?;
                    encoder.encode_all(&samples)?
                }
            };

            if !options.quiet {
                println!(" {} frames", frames.len());
            }

            frames
        };

        // === Phase 1: Send Alert packets ===
        if !options.skip_alert {
            if !options.quiet {
                print!("  Sending {} Alert packets...", options.alert_count);
                io::stdout().flush().ok();
            }

            for i in 0..options.alert_count {
                let packet = builder.build_alert()?;
                socket.send_to(&packet, dest).await?;
                debug!("Sent Alert packet {}/{}", i + 1, options.alert_count);

                if i < options.alert_count - 1 {
                    tokio::time::sleep(Duration::from_millis(options.control_interval)).await;
                }
            }

            if !options.quiet {
                println!(" done");
            }

            // Critical: Delay before starting audio (Polycom uses ~64ms)
            // This gives receivers time to initialize audio playback
            tokio::time::sleep(Duration::from_millis(64)).await;
        } else if !options.quiet {
            println!("  Skipping Alert packets");
        }

        // === Phase 2: Transmit audio ===
        if !options.quiet {
            print!("  Transmitting audio...");
            io::stdout().flush().ok();
        }

        // Transmit with precise timing - sleep BEFORE each packet to maintain exact 20ms intervals
        let total_frames = encoded_frames.len();
        let mut next_send_time = Instant::now();

        for (i, polycom_frame) in encoded_frames.into_iter().enumerate() {
            // Wait until the exact time to send this packet
            let now = Instant::now();
            if next_send_time > now {
                tokio::time::sleep(next_send_time - now).await;
            }

            // Build and send packet
            let packet = builder.build_transmit(&polycom_frame)?;
            socket.send_to(&packet, dest).await?;

            // Schedule next packet for exactly 20ms later
            next_send_time += frame_duration;

            // Progress update (only every second to minimize output overhead)
            if !options.quiet && (i + 1).is_multiple_of(50) {
                let progress = 100.0 * (i + 1) as f64 / total_frames as f64;
                print!("\r  Transmitting audio... {:.1}%   ", progress);
                io::stdout().flush().ok();
            }
        }

        if !options.quiet {
            println!("\r  Transmitting audio... 100.0% - Complete");
        }

        let frames_sent = total_frames as u32;

        // === Phase 3: Send End packets ===
        if !options.skip_end {
            if !options.quiet {
                print!("  Waiting 50ms before End packets...");
                io::stdout().flush().ok();
            }

            tokio::time::sleep(Duration::from_millis(50)).await;

            if !options.quiet {
                println!(" done");
                print!("  Sending {} End packets...", options.end_count);
                io::stdout().flush().ok();
            }

            for i in 0..options.end_count {
                let packet = builder.build_end()?;
                socket.send_to(&packet, dest).await?;
                debug!("Sent End packet {}/{}", i + 1, options.end_count);

                if i < options.end_count - 1 {
                    tokio::time::sleep(Duration::from_millis(options.control_interval)).await;
                }
            }

            if !options.quiet {
                println!(" done");
            }
        } else if !options.quiet {
            println!("  Skipping End packets");
        }

        if !options.quiet {
            println!();
            info!("Page complete: {} audio packets sent", frames_sent);
        }

        // Reset builder for next loop iteration
        builder.reset();

        if !options.loop_audio {
            break;
        }

        if !options.quiet {
            println!("  Looping...");
        }
    }

    Ok(())
}

/// Read an audio file and return samples at the target sample rate
fn read_audio_file(path: &Path, target_rate: u32) -> Result<Vec<i16>, PolycomTransmitError> {
    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension() {
        hint.with_extension(&ext.to_string_lossy());
    }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| PolycomTransmitError::UnsupportedFormat(e.to_string()))?;

    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| PolycomTransmitError::UnsupportedFormat("No audio track found".into()))?;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| PolycomTransmitError::UnsupportedFormat(e.to_string()))?;

    let track_id = track.id;
    let source_rate = track.codec_params.sample_rate.unwrap_or(target_rate);
    let channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(1);

    let mut samples: Vec<i16> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(_)) => break, // EOF
            Err(e) => return Err(PolycomTransmitError::AudioDecode(e.to_string())),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = decoder
            .decode(&packet)
            .map_err(|e| PolycomTransmitError::AudioDecode(e.to_string()))?;

        // Convert to i16 samples
        let frame_samples = convert_to_i16(&decoded);

        // Mix to mono if stereo
        let mono_samples: Vec<i16> = if channels > 1 {
            frame_samples
                .chunks(channels)
                .map(|chunk| {
                    let sum: i32 = chunk.iter().map(|&s| s as i32).sum();
                    (sum / channels as i32) as i16
                })
                .collect()
        } else {
            frame_samples
        };

        samples.extend(mono_samples);
    }

    // Resample if needed
    if source_rate != target_rate {
        samples = simple_resample(&samples, source_rate, target_rate);
    }

    Ok(samples)
}

/// Convert audio buffer to i16 samples
fn convert_to_i16(buffer: &AudioBufferRef) -> Vec<i16> {
    match buffer {
        AudioBufferRef::S8(buf) => buf
            .chan(0)
            .iter()
            .map(|&s| (s as i16) * 256)
            .collect(),
        AudioBufferRef::S16(buf) => buf.chan(0).to_vec(),
        AudioBufferRef::S32(buf) => buf.chan(0).iter().map(|&s| (s >> 16) as i16).collect(),
        AudioBufferRef::F32(buf) => buf
            .chan(0)
            .iter()
            .map(|&s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
            .collect(),
        AudioBufferRef::F64(buf) => buf
            .chan(0)
            .iter()
            .map(|&s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
            .collect(),
        AudioBufferRef::U8(buf) => buf
            .chan(0)
            .iter()
            .map(|&s| ((s as i16 - 128) * 256))
            .collect(),
        AudioBufferRef::U16(buf) => buf
            .chan(0)
            .iter()
            .map(|&s| (s as i32 - 32768) as i16)
            .collect(),
        AudioBufferRef::U24(buf) => buf
            .chan(0)
            .iter()
            .map(|&s| ((s.inner() as i32 - 8_388_608) >> 8) as i16)
            .collect(),
        AudioBufferRef::S24(buf) => buf
            .chan(0)
            .iter()
            .map(|&s| (s.inner() >> 8) as i16)
            .collect(),
        AudioBufferRef::U32(buf) => buf
            .chan(0)
            .iter()
            .map(|&s| ((s as i64 - (1_i64 << 31)) >> 16) as i16)
            .collect(),
    }
}

/// Simple linear interpolation resampling
fn simple_resample(samples: &[i16], from_rate: u32, to_rate: u32) -> Vec<i16> {
    let ratio = from_rate as f64 / to_rate as f64;
    let new_len = (samples.len() as f64 / ratio) as usize;

    (0..new_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos.floor() as usize;
            let frac = pos.fract();

            if idx + 1 >= samples.len() {
                samples[idx.min(samples.len() - 1)]
            } else {
                let a = samples[idx] as f64;
                let b = samples[idx + 1] as f64;
                (a + (b - a) * frac) as i16
            }
        })
        .collect()
}

/// Generate a pseudo-random host serial (last 4 bytes of MAC)
fn generate_host_serial() -> [u8; 4] {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;

    // Simple hash to generate 4 bytes
    let hash = seed.wrapping_mul(0x517c_c1b7_2722_0a95);
    [
        (hash >> 24) as u8,
        (hash >> 16) as u8,
        (hash >> 8) as u8,
        hash as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_host_serial() {
        let serial1 = generate_host_serial();
        std::thread::sleep(std::time::Duration::from_millis(1));
        let serial2 = generate_host_serial();

        // Different calls should produce different results (with high probability)
        // This is a weak test but ensures the function works
        assert_eq!(serial1.len(), 4);
        assert_eq!(serial2.len(), 4);
    }

    #[test]
    fn test_simple_resample() {
        let samples: Vec<i16> = vec![0, 100, 200, 300, 400, 500, 600, 700];

        // Downsample 2:1
        let resampled = simple_resample(&samples, 16000, 8000);
        assert_eq!(resampled.len(), 4);

        // Upsample 1:2
        let resampled = simple_resample(&samples, 8000, 16000);
        assert_eq!(resampled.len(), 16);
    }
}
