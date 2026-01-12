use crate::codec::{create_decoder_for_payload_type, AudioDecoder, CodecType};
use crate::network::{MulticastSocket, RtpPacket, PayloadType};
use crate::cli::audio_analyzer::{AudioAnalyzer, AudioStats, AudioAnalysis, format_frequency, format_db};
use crate::cli::recorder::WavRecorder;
use crate::utils::range_parser::{parse_range, MulticastEndpoint, RangeParseError};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{self, Write};
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum MonitorError {
    #[error("Invalid address: {0}")]
    InvalidAddress(String),

    #[error("Invalid address pattern: {0}")]
    InvalidPattern(#[from] RangeParseError),

    #[error("Network error: {0}")]
    Network(#[from] crate::network::MulticastError),

    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Codec error: {0}")]
    Codec(#[from] crate::codec::CodecError),

    #[error("Recorder error: {0}")]
    Recorder(#[from] super::recorder::RecorderError),

    #[error("No endpoints to monitor")]
    NoEndpoints,
}

/// Statistics for a monitored page
#[derive(Debug, Clone, Default, Serialize)]
pub struct PageStats {
    pub packets_received: u64,
    pub bytes_received: u64,
    pub packets_lost: u64,
    pub jitter_ms: f64,
    pub duration_secs: f64,
    #[serde(skip)]
    last_sequence: Option<u16>,
    #[serde(skip)]
    last_timestamp: Option<u32>,
    #[serde(skip)]
    last_arrival: Option<Instant>,
    #[serde(skip)]
    jitter_accumulator: f64,
}

impl PageStats {
    pub fn update(&mut self, packet: &RtpPacket) {
        self.packets_received += 1;
        self.bytes_received += packet.payload.len() as u64;

        // Calculate packet loss
        if let Some(last_seq) = self.last_sequence {
            let expected = last_seq.wrapping_add(1);
            if packet.header.sequence_number != expected {
                let gap = packet.header.sequence_number.wrapping_sub(last_seq);
                if gap > 1 && gap < 1000 {
                    self.packets_lost += (gap - 1) as u64;
                }
            }
        }

        // Calculate jitter (RFC 3550 algorithm)
        if let (Some(last_ts), Some(last_arrival)) = (self.last_timestamp, self.last_arrival) {
            let arrival_diff = packet.received_at.duration_since(last_arrival).as_secs_f64() * 8000.0;
            let ts_diff = packet.header.timestamp.wrapping_sub(last_ts) as f64;
            let d = (arrival_diff - ts_diff).abs();
            self.jitter_accumulator += (d - self.jitter_accumulator) / 16.0;
            self.jitter_ms = self.jitter_accumulator / 8.0; // Convert to ms
        }

        self.last_sequence = Some(packet.header.sequence_number);
        self.last_timestamp = Some(packet.header.timestamp);
        self.last_arrival = Some(packet.received_at);
    }

    pub fn loss_percent(&self) -> f64 {
        if self.packets_received + self.packets_lost == 0 {
            0.0
        } else {
            100.0 * self.packets_lost as f64 / (self.packets_received + self.packets_lost) as f64
        }
    }
}

/// JSON event types for automated testing
#[derive(Debug, Serialize)]
#[serde(tag = "event")]
pub enum JsonEvent {
    #[serde(rename = "monitoring_started")]
    MonitoringStarted {
        address: String,
        port: u16,
        timestamp: DateTime<Utc>,
        #[serde(skip_serializing_if = "Option::is_none")]
        endpoint_count: Option<usize>,
    },
    #[serde(rename = "page_started")]
    PageStarted {
        timestamp: DateTime<Utc>,
        address: String,
        port: u16,
        source: String,
        codec: String,
        ssrc: u32,
    },
    #[serde(rename = "stats")]
    Stats {
        address: String,
        port: u16,
        duration_secs: f64,
        packets: u64,
        bytes: u64,
        jitter_ms: f64,
        loss_percent: f64,
        // Audio analysis
        rms_db: f64,
        peak_db: f64,
        dominant_freq_hz: f64,
        glitches: u64,
        clipped: u64,
    },
    #[serde(rename = "page_ended")]
    PageEnded {
        address: String,
        port: u16,
        duration_secs: f64,
        total_packets: u64,
        total_bytes: u64,
        // Audio analysis summary
        peak_rms_db: f64,
        avg_rms_db: f64,
        max_peak_db: f64,
        dominant_freq_hz: f64,
        total_glitches: u64,
        total_clipped: u64,
        clipping_percent: f64,
        avg_zero_crossing_rate: f64,
    },
    #[serde(rename = "recording_saved")]
    RecordingSaved {
        address: String,
        port: u16,
        path: String,
    },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(rename = "timeout")]
    Timeout,
}

/// Options for monitoring a single endpoint (for future API use)
#[allow(dead_code)]
pub struct MonitorOptions {
    pub address: Ipv4Addr,
    pub port: u16,
    pub interface: Option<Ipv4Addr>,
    pub codec: Option<CodecType>,
    pub output: Option<PathBuf>,
    pub timeout: Duration,
    pub json: bool,
    pub quiet: bool,
}

/// Options for monitoring with range support
pub struct MonitorRangeOptions {
    pub pattern: String,
    pub default_port: u16,
    pub interface: Option<Ipv4Addr>,
    pub codec: Option<CodecType>,
    pub output: Option<PathBuf>,
    pub timeout: Duration,
    pub json: bool,
    pub quiet: bool,
}

/// State for a single monitored endpoint
struct EndpointState {
    address: Ipv4Addr,
    port: u16,
    stats: PageStats,
    audio_stats: AudioStats,
    audio_analyzer: Option<AudioAnalyzer>,
    current_audio: AudioAnalysis,
    decoder: Option<Box<dyn AudioDecoder>>,
    recorder: Option<WavRecorder>,
    page_active: bool,
    page_start: Option<Instant>,
    last_packet: Option<Instant>,
    ssrc: Option<u32>,
    output_path: Option<PathBuf>,
}

impl EndpointState {
    fn new(address: Ipv4Addr, port: u16, output_path: Option<PathBuf>) -> Self {
        Self {
            address,
            port,
            stats: PageStats::default(),
            audio_stats: AudioStats::new(),
            audio_analyzer: None,
            current_audio: AudioAnalysis::default(),
            decoder: None,
            recorder: None,
            page_active: false,
            page_start: None,
            last_packet: None,
            ssrc: None,
            output_path,
        }
    }

    fn reset_page(&mut self) {
        self.page_active = false;
        self.stats = PageStats::default();
        self.audio_stats = AudioStats::new();
        self.current_audio = AudioAnalysis::default();
        if let Some(ref mut analyzer) = self.audio_analyzer {
            analyzer.reset();
        }
        self.decoder = None;
        self.recorder = None;
        self.page_start = None;
        self.ssrc = None;
    }
}

/// Run the monitor command with range support
pub async fn run_monitor_range(options: MonitorRangeOptions) -> Result<(), MonitorError> {
    // Parse the pattern - if it contains ':', use it as-is, otherwise append default port
    let pattern = if options.pattern.contains(':') {
        options.pattern.clone()
    } else {
        format!("{}:{}", options.pattern, options.default_port)
    };

    let endpoints = parse_range(&pattern)?;

    if endpoints.is_empty() {
        return Err(MonitorError::NoEndpoints);
    }

    let endpoint_count = endpoints.len();
    let single_endpoint = endpoint_count == 1;

    // Group endpoints by port (we need one socket per port)
    let mut ports: HashMap<u16, Vec<Ipv4Addr>> = HashMap::new();
    for ep in &endpoints {
        ports.entry(ep.port).or_default().push(ep.address);
    }

    // Create sockets and join multicast groups
    // Use specified interface if provided, otherwise default to INADDR_ANY
    let interface = options.interface.unwrap_or(Ipv4Addr::UNSPECIFIED);
    let mut sockets: HashMap<u16, MulticastSocket> = HashMap::new();
    for (&port, addresses) in &ports {
        let mut socket = MulticastSocket::with_interface(port, interface).await?;
        for &addr in addresses {
            socket.join(addr)?;
        }
        sockets.insert(port, socket);
    }

    // Create endpoint states
    let mut endpoint_states: HashMap<(Ipv4Addr, u16), EndpointState> = HashMap::new();
    for ep in &endpoints {
        let output_path = options.output.as_ref().map(|base| {
            if single_endpoint {
                base.clone()
            } else {
                // Generate unique filename for each endpoint
                let stem = base.file_stem().and_then(|s| s.to_str()).unwrap_or("recording");
                let ext = base.extension().and_then(|s| s.to_str()).unwrap_or("wav");
                base.with_file_name(format!("{}_{}_{}_{}.{}", stem, ep.address, ep.port, Utc::now().format("%Y%m%d_%H%M%S"), ext))
            }
        });
        endpoint_states.insert((ep.address, ep.port), EndpointState::new(ep.address, ep.port, output_path));
    }

    // Output monitoring started
    if options.json {
        for ep in &endpoints {
            output_json(&JsonEvent::MonitoringStarted {
                address: ep.address.to_string(),
                port: ep.port,
                timestamp: Utc::now(),
                endpoint_count: if single_endpoint { None } else { Some(endpoint_count) },
            });
        }
    } else if !options.quiet {
        if single_endpoint {
            let ep = &endpoints[0];
            println!("Monitoring {}:{}...", ep.address, ep.port);
        } else {
            println!("Monitoring {} endpoints:", endpoint_count);
            for ep in &endpoints {
                println!("  {}:{}", ep.address, ep.port);
            }
            println!();
        }
    }

    let start_time = Instant::now();
    let mut last_stats_print = Instant::now();
    let stats_interval = Duration::from_secs(1);
    let idle_timeout = Duration::from_secs(5);
    let mut buf = vec![0u8; 2048];

    loop {
        // Check for overall timeout
        if options.timeout > Duration::ZERO && start_time.elapsed() >= options.timeout {
            if options.json {
                output_json(&JsonEvent::Timeout);
            } else if !options.quiet {
                println!("\nTimeout reached.");
            }
            break;
        }

        // Check for page end on all endpoints
        for state in endpoint_states.values_mut() {
            if state.page_active {
                if let Some(last) = state.last_packet {
                    if last.elapsed() >= idle_timeout {
                        handle_page_end(state, &options)?;
                    }
                }
            }
        }

        // Print periodic stats for active pages
        if last_stats_print.elapsed() >= stats_interval {
            for state in endpoint_states.values_mut() {
                if state.page_active {
                    // Calculate duration based on last received audio to avoid counting idle time
                    state.stats.duration_secs = match (state.page_start, state.last_packet) {
                        (Some(start), Some(last)) => last.duration_since(start).as_secs_f64(),
                        (Some(start), None) => start.elapsed().as_secs_f64(),
                        _ => 0.0,
                    };

                    if options.json {
                        output_json(&JsonEvent::Stats {
                            address: state.address.to_string(),
                            port: state.port,
                            duration_secs: state.stats.duration_secs,
                            packets: state.stats.packets_received,
                            bytes: state.stats.bytes_received,
                            jitter_ms: state.stats.jitter_ms,
                            loss_percent: state.stats.loss_percent(),
                            rms_db: state.current_audio.rms_db,
                            peak_db: state.current_audio.peak_db,
                            dominant_freq_hz: state.current_audio.dominant_freq_hz,
                            glitches: state.audio_stats.total_glitches,
                            clipped: state.audio_stats.total_clipped,
                        });
                    } else if !options.quiet {
                        let prefix = if single_endpoint {
                            String::new()
                        } else {
                            format!("[{}:{}] ", state.address, state.port)
                        };
                        print!(
                            "\r{}Time: {:02}:{:02} | RMS: {} | Peak: {} | Freq: {} | Glitch: {} | Loss: {:.1}%   ",
                            prefix,
                            ((state.stats.duration_secs % 3600.0) / 60.0) as u32,
                            (state.stats.duration_secs % 60.0) as u32,
                            format_db(state.current_audio.rms_db),
                            format_db(state.current_audio.peak_db),
                            format_frequency(state.current_audio.dominant_freq_hz),
                            state.audio_stats.total_glitches,
                            state.stats.loss_percent()
                        );
                        io::stdout().flush().ok();
                    }
                }
            }
            last_stats_print = Instant::now();
        }

        // Receive from all sockets - drain all available packets from each socket
        // to avoid buffered packets causing delayed page-end detection
        let recv_timeout = Duration::from_millis(10);

        for (&port, socket) in &sockets {
            // Drain all available packets from this socket
            loop {
                let recv_result = tokio::time::timeout(recv_timeout, socket.recv_from(&mut buf)).await;

                let (len, src_addr) = match recv_result {
                    Ok(Ok((len, addr))) => (len, addr),
                    Ok(Err(e)) => {
                        if options.json {
                            output_json(&JsonEvent::Error {
                                message: format!("Receive error on port {}: {}", port, e),
                            });
                        }
                        break; // Move to next socket on error
                    }
                    Err(_) => break, // Timeout - no more packets, move to next socket
                };

                // Parse RTP packet
                let Ok(packet) = RtpPacket::parse(&buf[..len], src_addr) else {
                    continue; // Try next packet
                };

                // Find endpoint for this port that either:
                // 1. Has matching SSRC
                // 2. Is not currently active (new page)
                let endpoint_key = endpoint_states.iter()
                    .filter(|((_, p), _)| *p == port)
                    .find(|(_, state)| state.ssrc == Some(packet.header.ssrc) || !state.page_active)
                    .map(|(k, _)| *k);

                if let Some(key) = endpoint_key {
                    if let Some(state) = endpoint_states.get_mut(&key) {
                        handle_packet(state, &packet, &options)?;
                    }
                }
            }
        }
    }

    // Finalize any active recordings
    for state in endpoint_states.values_mut() {
        if state.page_active {
            handle_page_end(state, &options)?;
        }
    }

    Ok(())
}

fn handle_packet(state: &mut EndpointState, packet: &RtpPacket, options: &MonitorRangeOptions) -> Result<(), MonitorError> {
    // Check if this is a new page
    if state.ssrc.is_none() || state.ssrc != Some(packet.header.ssrc) {
        // New page started
        state.ssrc = Some(packet.header.ssrc);
        state.page_start = Some(Instant::now());
        state.page_active = true;
        state.stats = PageStats::default();

        // Determine codec
        let codec_type = options.codec.unwrap_or_else(|| {
            CodecType::from_payload_type(packet.header.payload_type)
                .unwrap_or(CodecType::G711Ulaw)
        });

        let payload_type = PayloadType::from_pt(packet.header.payload_type);

        if options.json {
            output_json(&JsonEvent::PageStarted {
                timestamp: Utc::now(),
                address: state.address.to_string(),
                port: state.port,
                source: packet.source.to_string(),
                codec: codec_type.name().to_string(),
                ssrc: packet.header.ssrc,
            });
        } else if !options.quiet {
            println!("\n[{}:{}] Page started at {}", state.address, state.port, Utc::now().format("%Y-%m-%d %H:%M:%S"));
            println!("  Source: {}", packet.source);
            println!(
                "  Codec: {} ({})",
                payload_type.name(),
                if options.codec.is_some() { "forced" } else { "detected" }
            );
            println!();
        }

        // Create decoder
        state.decoder = Some(create_decoder_for_payload_type(packet.header.payload_type)?);

        // Create audio analyzer with decoder's sample rate
        let sample_rate = state.decoder.as_ref().unwrap().sample_rate();
        state.audio_analyzer = Some(AudioAnalyzer::new(sample_rate));
        state.audio_stats = AudioStats::new();

        // Create recorder if output specified
        if let Some(ref path) = state.output_path {
            let channels = state.decoder.as_ref().unwrap().channels();
            state.recorder = Some(WavRecorder::new(path, sample_rate, channels)?);
        }
    }

    // Update stats
    state.stats.update(packet);
    state.last_packet = Some(Instant::now());

    // Decode, analyze, and record
    if let Some(ref mut dec) = state.decoder {
        if let Ok(samples) = dec.decode(&packet.payload) {
            // Analyze audio
            if let Some(ref mut analyzer) = state.audio_analyzer {
                let analysis = analyzer.analyze(&samples);
                state.audio_stats.update(&analysis, samples.len() as u64);
                state.current_audio = analysis;
            }

            // Record
            if let Some(ref mut rec) = state.recorder {
                rec.write_samples(&samples)?;
            }
        }
    }

    Ok(())
}

fn handle_page_end(state: &mut EndpointState, options: &MonitorRangeOptions) -> Result<(), MonitorError> {
    // Calculate duration based on last received audio, not current time
    // This avoids inflating the duration by the idle timeout period
    let duration = match (state.page_start, state.last_packet) {
        (Some(start), Some(last)) => last.duration_since(start).as_secs_f64(),
        (Some(start), None) => start.elapsed().as_secs_f64(),
        _ => 0.0,
    };

    if options.json {
        output_json(&JsonEvent::PageEnded {
            address: state.address.to_string(),
            port: state.port,
            duration_secs: duration,
            total_packets: state.stats.packets_received,
            total_bytes: state.stats.bytes_received,
            peak_rms_db: state.audio_stats.peak_rms_db,
            avg_rms_db: state.audio_stats.avg_rms_db,
            max_peak_db: state.audio_stats.max_peak_db,
            dominant_freq_hz: state.audio_stats.dominant_freq_hz,
            total_glitches: state.audio_stats.total_glitches,
            total_clipped: state.audio_stats.total_clipped,
            clipping_percent: state.audio_stats.clipping_percent(),
            avg_zero_crossing_rate: state.audio_stats.avg_zero_crossing_rate,
        });
    } else if !options.quiet {
        println!("\n[{}:{}] Page ended. Duration: {:.1}s", state.address, state.port, duration);
        println!("  Network: {} packets, {} bytes, {:.1}% loss, {:.1}ms jitter",
            state.stats.packets_received,
            state.stats.bytes_received,
            state.stats.loss_percent(),
            state.stats.jitter_ms
        );
        println!("  Audio:   Avg RMS: {}, Peak: {}, Dominant Freq: {}",
            format_db(state.audio_stats.avg_rms_db),
            format_db(state.audio_stats.max_peak_db),
            format_frequency(state.audio_stats.dominant_freq_hz)
        );
        if state.audio_stats.total_glitches > 0 || state.audio_stats.total_clipped > 0 {
            println!("  Issues:  {} glitches, {} clipped samples ({:.2}%)",
                state.audio_stats.total_glitches,
                state.audio_stats.total_clipped,
                state.audio_stats.clipping_percent()
            );
        }
    }

    // Save recording if configured
    if let Some(rec) = state.recorder.take() {
        rec.finalize()?;
        if let Some(ref path) = state.output_path {
            if options.json {
                output_json(&JsonEvent::RecordingSaved {
                    address: state.address.to_string(),
                    port: state.port,
                    path: path.to_string_lossy().to_string(),
                });
            } else if !options.quiet {
                println!("  Recording saved to: {}", path.display());
            }
        }
    }

    state.reset_page();
    Ok(())
}

/// Run the monitor command (single endpoint, for backwards compatibility)
#[allow(dead_code)]
pub async fn run_monitor(options: MonitorOptions) -> Result<(), MonitorError> {
    let range_options = MonitorRangeOptions {
        pattern: format!("{}:{}", options.address, options.port),
        default_port: options.port,
        interface: options.interface,
        codec: options.codec,
        output: options.output,
        timeout: options.timeout,
        json: options.json,
        quiet: options.quiet,
    };
    run_monitor_range(range_options).await
}

fn output_json(event: &JsonEvent) {
    if let Ok(json) = serde_json::to_string(event) {
        println!("{}", json);
    }
}

/// Parse an address string into an `Ipv4Addr`
pub fn parse_address(addr: &str) -> Result<Ipv4Addr, MonitorError> {
    addr.parse()
        .map_err(|_| MonitorError::InvalidAddress(addr.to_string()))
}

/// Parse an address pattern (may include ranges) and return endpoints
#[allow(dead_code)]
pub fn parse_address_pattern(pattern: &str, default_port: u16) -> Result<Vec<MulticastEndpoint>, MonitorError> {
    let pattern = if pattern.contains(':') {
        pattern.to_string()
    } else {
        format!("{}:{}", pattern, default_port)
    };
    Ok(parse_range(&pattern)?)
}
