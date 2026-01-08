//! Polycom paging monitor command implementation.
//!
//! Monitors multicast addresses for Polycom PTT/Group Paging traffic
//! and optionally records received pages to WAV files.

use crate::codec::{create_decoder, CodecType};
use crate::network::{
    MulticastSocket, PolycomPacket, PolycomSession, PolycomCodec, PacketType,
};
use crate::utils::range_parser::{parse_range, MulticastEndpoint, RangeParseError};
use std::collections::HashMap;
use std::io;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::{debug, info, warn};

#[derive(Error, Debug)]
pub enum PolycomMonitorError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Multicast error: {0}")]
    Multicast(#[from] crate::network::MulticastError),

    #[error("Codec error: {0}")]
    Codec(#[from] crate::codec::CodecError),

    #[error("Invalid address pattern: {0}")]
    InvalidPattern(#[from] RangeParseError),

    #[error("Invalid channel range: {0}")]
    InvalidChannelRange(String),

    #[error("No endpoints to monitor")]
    NoEndpoints,
}

/// Options for Polycom monitor command
pub struct PolycomMonitorOptions {
    /// Multicast address pattern to monitor (supports ranges like 224.0.{1-10}.116:{5001-5010})
    pub pattern: String,
    /// Default UDP port (used when pattern doesn't include port)
    pub default_port: u16,
    /// Channels to monitor (e.g., "26", "26-50", or "all")
    pub channels: String,
    /// Output directory for recordings
    pub output: Option<PathBuf>,
    /// Timeout (`Duration::MAX` for indefinite)
    pub timeout: Duration,
    /// Output in JSON format
    pub json: bool,
    /// Suppress non-essential output
    pub quiet: bool,
}

/// State for a page being recorded
struct RecordingState {
    session: PolycomSession,
    samples: Vec<i16>,
    decoder: Option<Box<dyn crate::codec::AudioDecoder>>,
}

/// Run the Polycom monitor command
pub async fn run_polycom_monitor(options: PolycomMonitorOptions) -> Result<(), PolycomMonitorError> {
    // Parse channel filter
    let channel_filter = parse_channel_filter(&options.channels)?;

    // Parse address pattern into endpoints
    let endpoints = parse_polycom_pattern(&options.pattern, options.default_port)?;
    if endpoints.is_empty() {
        return Err(PolycomMonitorError::NoEndpoints);
    }

    // Group endpoints by port (we'll create one socket per port)
    let mut ports_to_addresses: HashMap<u16, Vec<Ipv4Addr>> = HashMap::new();
    for endpoint in &endpoints {
        ports_to_addresses
            .entry(endpoint.port)
            .or_default()
            .push(endpoint.address);
    }

    // Create sockets for each port and join all addresses
    let mut sockets: Vec<MulticastSocket> = Vec::new();
    for (&port, addresses) in &ports_to_addresses {
        let mut socket = MulticastSocket::new(port).await?;
        for &addr in addresses {
            socket.join(addr)?;
        }
        socket.set_multicast_loop(true)?;
        sockets.push(socket);
    }

    if !options.quiet && !options.json {
        println!("Polycom Paging Monitor");
        if endpoints.len() == 1 {
            println!("  Address: {}", endpoints[0]);
        } else {
            println!("  Endpoints: {} addresses", endpoints.len());
            for endpoint in &endpoints {
                println!("    - {}", endpoint);
            }
        }
        println!("  Channels: {}", format_channel_filter(&channel_filter));
        if let Some(ref output) = options.output {
            println!("  Output: {}", output.display());
        }
        println!();
        println!("Listening for Polycom pages... (Ctrl+C to stop)");
        println!();
    }

    let start_time = Instant::now();
    let mut buf = vec![0u8; 2048];
    let mut sessions: HashMap<u8, RecordingState> = HashMap::new();
    let mut completed_pages: Vec<PageSummary> = Vec::new();

    // Session timeout (no packets for this long = session ended)
    let session_timeout_ms = 2000u64;

    // For multiple sockets, we need to poll them all
    // Simple approach: use the first socket (most common case is single socket)
    // For multiple sockets, we'd need async select - keeping it simple for now
    let socket = &mut sockets[0];

    loop {
        // Check timeout
        if options.timeout != Duration::MAX && start_time.elapsed() > options.timeout {
            break;
        }

        // Receive with timeout for periodic cleanup
        let recv_result = tokio::time::timeout(
            Duration::from_millis(500),
            socket.recv_from(&mut buf),
        )
        .await;

        match recv_result {
            Ok(Ok((len, source))) => {
                // Try to parse as Polycom packet
                match PolycomPacket::parse(&buf[..len], source) {
                    Ok(packet) => {
                        let channel = packet.header.channel;

                        // Check channel filter
                        if !channel_filter.is_empty() && !channel_filter.contains(&channel) {
                            continue;
                        }

                        match packet.header.packet_type {
                            PacketType::Alert => {
                                handle_alert(&mut sessions, &packet, &options);
                            }
                            PacketType::Transmit => {
                                handle_transmit(&mut sessions, &packet);
                            }
                            PacketType::End => {
                                if let Some(summary) = handle_end(&mut sessions, &packet, &options) {
                                    completed_pages.push(summary);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        debug!("Non-Polycom packet or parse error: {}", e);
                    }
                }
            }
            Ok(Err(e)) => {
                warn!("Receive error: {}", e);
            }
            Err(_) => {
                // Timeout - check for stale sessions
                cleanup_stale_sessions(&mut sessions, session_timeout_ms, &options, &mut completed_pages);
            }
        }
    }

    // Final cleanup
    for (channel, state) in sessions.drain() {
        if let Some(summary) = finalize_session(channel, state, &options) {
            completed_pages.push(summary);
        }
    }

    // Print summary
    if options.json {
        let summary = serde_json::json!({
            "total_pages": completed_pages.len(),
            "pages": completed_pages,
        });
        println!("{}", serde_json::to_string_pretty(&summary).unwrap_or_default());
    } else if !options.quiet {
        println!();
        println!("=== Summary ===");
        println!("Total pages received: {}", completed_pages.len());
        for (i, page) in completed_pages.iter().enumerate() {
            println!(
                "  Page {}: Channel {}, Caller: \"{}\", Duration: {:.1}s, {} audio packets",
                i + 1,
                page.channel,
                page.caller_id,
                page.duration_secs,
                page.audio_packets
            );
        }
    }

    Ok(())
}

/// Page summary for reporting
#[derive(Debug, serde::Serialize)]
struct PageSummary {
    channel: u8,
    caller_id: String,
    codec: String,
    duration_secs: f64,
    audio_packets: u32,
    recording_file: Option<String>,
}

/// Handle an Alert packet (start of new page)
fn handle_alert(
    sessions: &mut HashMap<u8, RecordingState>,
    packet: &PolycomPacket,
    options: &PolycomMonitorOptions,
) {
    let channel = packet.header.channel;

    if sessions.contains_key(&channel) {
        // Already have a session for this channel, update it
        if let Some(state) = sessions.get_mut(&channel) {
            state.session.update(packet);
        }
        return;
    }

    // New session
    let session = PolycomSession::from_alert(packet);

    if !options.quiet && !options.json {
        println!(
            "[Channel {}] Page started from \"{}\"",
            channel, packet.header.caller_id
        );
    }

    sessions.insert(
        channel,
        RecordingState {
            session,
            samples: Vec::new(),
            decoder: None,
        },
    );

    info!(
        "New page on channel {}: caller=\"{}\"",
        channel, packet.header.caller_id
    );
}

/// Handle a Transmit packet (audio data)
fn handle_transmit(sessions: &mut HashMap<u8, RecordingState>, packet: &PolycomPacket) {
    let channel = packet.header.channel;

    let Some(state) = sessions.get_mut(&channel) else {
        // Got audio without an Alert - create session anyway
        debug!("Transmit packet without prior Alert on channel {}", channel);
        return;
    };

    state.session.update(packet);

    // Get codec and create decoder if needed
    if let Some(ref audio_header) = packet.audio_header {
        if state.decoder.is_none() {
            let codec_type = match audio_header.codec {
                PolycomCodec::G711U => CodecType::G711Ulaw,
                PolycomCodec::G711A => CodecType::G711Alaw,
                PolycomCodec::G722 => CodecType::G722,
            };
            match create_decoder(codec_type) {
                Ok(d) => state.decoder = Some(d),
                Err(e) => {
                    warn!("Failed to create decoder: {}", e);
                    return;
                }
            }
        }

        // Decode audio frame (use current frame, ignore redundant)
        if let Some(ref audio_frame) = packet.audio_frame {
            if let Some(ref mut decoder) = state.decoder {
                match decoder.decode(audio_frame) {
                    Ok(samples) => {
                        state.samples.extend(samples);
                    }
                    Err(e) => {
                        warn!("Decode error: {}", e);
                    }
                }
            }
        }
    }
}

/// Handle an End packet (end of page)
fn handle_end(
    sessions: &mut HashMap<u8, RecordingState>,
    packet: &PolycomPacket,
    options: &PolycomMonitorOptions,
) -> Option<PageSummary> {
    let channel = packet.header.channel;

    if let Some(state) = sessions.get_mut(&channel) {
        state.session.update(packet);

        // Check if session is complete (received enough End packets)
        if state.session.is_complete() {
            if let Some(state) = sessions.remove(&channel) {
                return finalize_session(channel, state, options);
            }
        }
    }

    None
}

/// Finalize a session and optionally save recording
#[allow(clippy::unnecessary_wraps)] // Option needed: recording can fail
fn finalize_session(
    channel: u8,
    state: RecordingState,
    options: &PolycomMonitorOptions,
) -> Option<PageSummary> {
    let duration = state.session.duration();
    let codec_name = state
        .session
        .codec
        .map(|c| c.name().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    if !options.quiet && !options.json {
        println!(
            "[Channel {}] Page ended: {:.1}s, {} audio packets",
            channel,
            duration.as_secs_f64(),
            state.session.audio_packet_count
        );
    }

    // Save recording if output is specified and we have samples
    let recording_file = if let Some(ref output_dir) = options.output {
        if state.samples.is_empty() {
            None
        } else {
            let sample_rate = state.session.codec.map(|c| c.sample_rate()).unwrap_or(8000);
            let filename = format!(
                "polycom_ch{}_{}_{}.wav",
                channel,
                state.session.caller_id.replace(|c: char| !c.is_alphanumeric(), "_"),
                chrono::Local::now().format("%Y%m%d_%H%M%S")
            );
            let path = output_dir.join(&filename);

            if let Err(e) = save_wav(&path, &state.samples, sample_rate) {
                warn!("Failed to save recording: {}", e);
                None
            } else {
                info!("Saved recording to {}", path.display());
                if !options.quiet && !options.json {
                    println!("  Saved: {}", path.display());
                }
                Some(filename)
            }
        }
    } else {
        None
    };

    Some(PageSummary {
        channel,
        caller_id: state.session.caller_id,
        codec: codec_name,
        duration_secs: duration.as_secs_f64(),
        audio_packets: state.session.audio_packet_count,
        recording_file,
    })
}

/// Cleanup stale sessions that have timed out
fn cleanup_stale_sessions(
    sessions: &mut HashMap<u8, RecordingState>,
    timeout_ms: u64,
    options: &PolycomMonitorOptions,
    completed_pages: &mut Vec<PageSummary>,
) {
    let stale_channels: Vec<u8> = sessions
        .iter()
        .filter(|(_, s)| s.session.is_timed_out(timeout_ms))
        .map(|(&ch, _)| ch)
        .collect();

    for channel in stale_channels {
        if let Some(state) = sessions.remove(&channel) {
            warn!("Session on channel {} timed out", channel);
            if let Some(summary) = finalize_session(channel, state, options) {
                completed_pages.push(summary);
            }
        }
    }
}

/// Parse channel filter string
fn parse_channel_filter(filter: &str) -> Result<Vec<u8>, PolycomMonitorError> {
    let filter = filter.trim().to_lowercase();

    if filter == "all" || filter.is_empty() {
        return Ok(Vec::new()); // Empty = accept all
    }

    let mut channels = Vec::new();

    for part in filter.split(',') {
        let part = part.trim();
        if part.contains('-') {
            // Range
            let parts: Vec<&str> = part.split('-').collect();
            if parts.len() != 2 {
                return Err(PolycomMonitorError::InvalidChannelRange(part.to_string()));
            }
            let start: u8 = parts[0]
                .parse()
                .map_err(|_| PolycomMonitorError::InvalidChannelRange(part.to_string()))?;
            let end: u8 = parts[1]
                .parse()
                .map_err(|_| PolycomMonitorError::InvalidChannelRange(part.to_string()))?;
            for ch in start..=end {
                if (1..=50).contains(&ch) && !channels.contains(&ch) {
                    channels.push(ch);
                }
            }
        } else {
            // Single channel
            let ch: u8 = part
                .parse()
                .map_err(|_| PolycomMonitorError::InvalidChannelRange(part.to_string()))?;
            if (1..=50).contains(&ch) && !channels.contains(&ch) {
                channels.push(ch);
            }
        }
    }

    channels.sort_unstable();
    Ok(channels)
}

/// Format channel filter for display
fn format_channel_filter(filter: &[u8]) -> String {
    use std::fmt::Write;

    if filter.is_empty() {
        return "all (1-50)".to_string();
    }

    // Try to compress into ranges
    let mut result = String::new();
    let mut i = 0;

    while i < filter.len() {
        let start = filter[i];
        let mut end = start;

        while i + 1 < filter.len() && filter[i + 1] == end + 1 {
            end = filter[i + 1];
            i += 1;
        }

        if !result.is_empty() {
            result.push_str(", ");
        }

        if start == end {
            let _ = write!(result, "{start}");
        } else {
            let _ = write!(result, "{start}-{end}");
        }

        i += 1;
    }

    result
}

/// Save samples to WAV file
fn save_wav(path: &PathBuf, samples: &[i16], sample_rate: u32) -> io::Result<()> {
    use std::fs::File;

    // Create parent directory if needed
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let file = File::create(path)?;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = hound::WavWriter::new(file, spec)
        .map_err(|e| io::Error::other(e.to_string()))?;

    for &sample in samples {
        writer
            .write_sample(sample)
            .map_err(|e| io::Error::other(e.to_string()))?;
    }

    writer
        .finalize()
        .map_err(|e| io::Error::other(e.to_string()))?;

    Ok(())
}

/// Parse address pattern for Polycom monitor
/// Supports range syntax like regular monitor, or simple address without port
fn parse_polycom_pattern(pattern: &str, default_port: u16) -> Result<Vec<MulticastEndpoint>, PolycomMonitorError> {
    let pattern = pattern.trim();

    // If pattern contains a colon with port/range, use the range parser directly
    if pattern.contains(':') {
        Ok(parse_range(pattern)?)
    } else {
        // No port specified - try to parse as simple address or address with ranges
        // Add the default port
        let pattern_with_port = format!("{}:{}", pattern, default_port);
        Ok(parse_range(&pattern_with_port)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_channel_filter_all() {
        let filter = parse_channel_filter("all").unwrap();
        assert!(filter.is_empty());

        let filter = parse_channel_filter("").unwrap();
        assert!(filter.is_empty());
    }

    #[test]
    fn test_parse_channel_filter_single() {
        let filter = parse_channel_filter("26").unwrap();
        assert_eq!(filter, vec![26]);
    }

    #[test]
    fn test_parse_channel_filter_range() {
        let filter = parse_channel_filter("26-30").unwrap();
        assert_eq!(filter, vec![26, 27, 28, 29, 30]);
    }

    #[test]
    fn test_parse_channel_filter_mixed() {
        let filter = parse_channel_filter("1, 26-28, 50").unwrap();
        assert_eq!(filter, vec![1, 26, 27, 28, 50]);
    }

    #[test]
    fn test_format_channel_filter() {
        assert_eq!(format_channel_filter(&[]), "all (1-50)");
        assert_eq!(format_channel_filter(&[26]), "26");
        assert_eq!(format_channel_filter(&[26, 27, 28]), "26-28");
        assert_eq!(format_channel_filter(&[1, 26, 27, 28, 50]), "1, 26-28, 50");
    }
}
