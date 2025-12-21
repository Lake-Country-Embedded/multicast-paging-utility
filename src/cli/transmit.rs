use crate::codec::{create_encoder, CodecType};
use crate::network::{create_transmit_socket, RtpPacket};
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

#[derive(Error, Debug)]
pub enum TransmitError {
    #[error("Invalid address: {0}")]
    #[allow(dead_code)]
    InvalidAddress(String),

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
}

pub struct TransmitOptions {
    pub file: std::path::PathBuf,
    pub address: Ipv4Addr,
    pub port: u16,
    pub codec: CodecType,
    pub ttl: u8,
    pub loop_audio: bool,
    pub quiet: bool,
}

/// Run the transmit command
pub async fn run_transmit(options: TransmitOptions) -> Result<(), TransmitError> {
    if !options.file.exists() {
        return Err(TransmitError::FileNotFound(
            options.file.to_string_lossy().to_string(),
        ));
    }

    // Create transmit socket
    let socket = create_transmit_socket(options.ttl).await?;
    let dest = SocketAddrV4::new(options.address, options.port);

    // Create encoder
    let mut encoder = create_encoder(options.codec)?;
    let frame_size = encoder.frame_size();
    let sample_rate = encoder.sample_rate();

    if !options.quiet {
        println!("Transmitting {} to {}:{}", options.file.display(), options.address, options.port);
        println!("  Codec: {}", options.codec.name());
        println!("  TTL: {}", options.ttl);
        println!();
    }

    // Generate a random SSRC
    let ssrc: u32 = rand_ssrc();

    loop {
        // Read and decode the audio file
        let samples = read_audio_file(&options.file, sample_rate)?;

        if !options.quiet {
            let duration = samples.len() as f64 / sample_rate as f64;
            println!("  Duration: {:.1}s ({} samples)", duration, samples.len());
        }

        // Transmit
        let mut sequence: u16 = 0;
        let mut timestamp: u32 = 0;
        let mut samples_sent = 0;
        let start = Instant::now();

        // Frame duration in seconds (for potential future pacing)
        let _frame_duration = Duration::from_secs_f64(frame_size as f64 / sample_rate as f64);

        for chunk in samples.chunks(frame_size) {
            // Pad last chunk if needed
            let frame: Vec<i16> = if chunk.len() < frame_size {
                let mut padded = chunk.to_vec();
                padded.resize(frame_size, 0);
                padded
            } else {
                chunk.to_vec()
            };

            // Encode
            let encoded = encoder.encode(&frame)?;

            // Build RTP packet
            let packet = RtpPacket::build(
                options.codec.payload_type(),
                sequence,
                timestamp,
                ssrc,
                &encoded,
                false,
            );

            // Send
            socket.send_to(&packet, dest).await?;

            sequence = sequence.wrapping_add(1);
            timestamp = timestamp.wrapping_add(frame_size as u32);
            samples_sent += chunk.len();

            // Rate limiting - sleep to maintain real-time pace
            let expected_time = Duration::from_secs_f64(samples_sent as f64 / sample_rate as f64);
            let elapsed = start.elapsed();
            if expected_time > elapsed {
                tokio::time::sleep(expected_time - elapsed).await;
            }

            // Progress update
            if !options.quiet && sequence.is_multiple_of(50) {
                let progress = 100.0 * samples_sent as f64 / samples.len() as f64;
                print!("\r  Progress: {:.1}%   ", progress);
                io::stdout().flush().ok();
            }
        }

        if !options.quiet {
            println!("\r  Progress: 100.0% - Complete");
        }

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
fn read_audio_file(path: &Path, target_rate: u32) -> Result<Vec<i16>, TransmitError> {
    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension() {
        hint.with_extension(&ext.to_string_lossy());
    }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| TransmitError::UnsupportedFormat(e.to_string()))?;

    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| TransmitError::UnsupportedFormat("No audio track found".into()))?;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| TransmitError::UnsupportedFormat(e.to_string()))?;

    let track_id = track.id;
    let source_rate = track.codec_params.sample_rate.unwrap_or(target_rate);
    let channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(1);

    let mut samples: Vec<i16> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(_)) => break, // EOF
            Err(e) => return Err(TransmitError::AudioDecode(e.to_string())),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = decoder
            .decode(&packet)
            .map_err(|e| TransmitError::AudioDecode(e.to_string()))?;

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
            // Convert unsigned 32-bit to signed 16-bit: subtract 2^31 to center, then shift
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

/// Generate a random SSRC
fn rand_ssrc() -> u32 {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u32;

    // Simple LCG (constants from glibc)
    seed.wrapping_mul(1_103_515_245).wrapping_add(12345)
}

#[cfg(test)]
mod tests {
    use super::*;

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
