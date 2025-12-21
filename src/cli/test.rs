//! Automated testing mode for CI/CD integration
//!
//! This module provides a test command that monitors multicast addresses,
//! records pages, and outputs structured metrics for automated analysis.

use crate::codec::{create_decoder_for_payload_type, AudioDecoder, CodecType};
use crate::network::{MulticastSocket, RtpPacket, PayloadType};
use crate::cli::audio_analyzer::{AudioAnalyzer, AudioStats, AudioAnalysis};
use crate::cli::recorder::WavRecorder;
use crate::utils::range_parser::parse_range;
use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TestError {
    #[error("Invalid address pattern: {0}")]
    InvalidPattern(#[from] crate::utils::range_parser::RangeParseError),

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

    #[error("Timeout must be greater than 0")]
    InvalidTimeout,
}

/// Options for the test command
pub struct TestOptions {
    pub pattern: String,
    pub default_port: u16,
    pub codec: Option<CodecType>,
    pub output_dir: PathBuf,
    pub timeout: Duration,
    pub metrics_interval: Duration,
}

/// Network metrics for a snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkMetrics {
    pub packets: u64,
    pub bytes: u64,
    pub loss_percent: f64,
    pub jitter_ms: f64,
}

/// Audio metrics for a snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioMetrics {
    pub rms_db: f64,
    pub peak_db: f64,
    pub dominant_freq_hz: f64,
    pub glitches: u64,
    pub clipped: u64,
}

/// A single metrics snapshot written to JSONL
#[derive(Debug, Serialize, Deserialize)]
pub struct MetricSnapshot {
    pub timestamp: DateTime<Utc>,
    pub endpoint: String,
    pub page_active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_number: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
    pub network: NetworkMetrics,
    pub audio: AudioMetrics,
}

/// Network summary for a page
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSummary {
    pub packets_received: u64,
    pub bytes_received: u64,
    pub packets_lost: u64,
    pub loss_percent: f64,
    pub jitter_ms: f64,
}

/// Audio summary for a page
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioSummary {
    pub peak_rms_db: f64,
    /// Average RMS level - None if no valid (non-silence) samples
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_rms_db: Option<f64>,
    pub max_peak_db: f64,
    pub dominant_freq_hz: f64,
    pub total_glitches: u64,
    pub total_clipped: u64,
    pub clipping_percent: f64,
    pub avg_zero_crossing_rate: f64,
}

/// Summary of a single page
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSummary {
    pub page_number: u32,
    pub endpoint: String,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub duration_secs: f64,
    pub recording_file: String,
    pub network: NetworkSummary,
    pub audio: AudioSummary,
}

/// Totals for a single endpoint
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EndpointTotal {
    pub pages_detected: u32,
    pub total_duration_secs: f64,
    pub total_packets: u64,
    pub total_bytes: u64,
}

/// Test metadata
#[derive(Debug, Serialize, Deserialize)]
pub struct TestMetadata {
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub duration_secs: f64,
    pub pattern: String,
    pub endpoints_monitored: usize,
    pub metrics_interval_ms: u64,
    pub timeout_secs: u64,
}

/// Complete test summary
#[derive(Debug, Serialize, Deserialize)]
pub struct TestSummary {
    pub test_metadata: TestMetadata,
    pub pages: Vec<PageSummary>,
    pub endpoint_totals: HashMap<String, EndpointTotal>,
    pub errors: Vec<String>,
}

/// Statistics for a monitored page (reused from monitor)
#[derive(Debug, Clone, Default)]
struct PageStats {
    packets_received: u64,
    bytes_received: u64,
    packets_lost: u64,
    jitter_ms: f64,
    last_sequence: Option<u16>,
    last_timestamp: Option<u32>,
    last_arrival: Option<Instant>,
    jitter_accumulator: f64,
}

impl PageStats {
    fn update(&mut self, packet: &RtpPacket) {
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
            self.jitter_ms = self.jitter_accumulator / 8.0;
        }

        self.last_sequence = Some(packet.header.sequence_number);
        self.last_timestamp = Some(packet.header.timestamp);
        self.last_arrival = Some(packet.received_at);
    }

    fn loss_percent(&self) -> f64 {
        if self.packets_received + self.packets_lost == 0 {
            0.0
        } else {
            100.0 * self.packets_lost as f64 / (self.packets_received + self.packets_lost) as f64
        }
    }
}

/// State for a single monitored endpoint in test mode
struct TestEndpointState {
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
    page_start_utc: Option<DateTime<Utc>>,
    last_packet: Option<Instant>,
    ssrc: Option<u32>,
    // Test-specific
    page_count: u32,
    completed_pages: Vec<PageSummary>,
}

impl TestEndpointState {
    fn new(address: Ipv4Addr, port: u16) -> Self {
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
            page_start_utc: None,
            last_packet: None,
            ssrc: None,
            page_count: 0,
            completed_pages: Vec::new(),
        }
    }

    fn endpoint_string(&self) -> String {
        format!("{}:{}", self.address, self.port)
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
        self.page_start_utc = None;
        self.ssrc = None;
    }
}

/// Handles writing metrics to JSONL file
struct MetricsWriter {
    writer: BufWriter<File>,
    lines_written: u64,
}

impl MetricsWriter {
    fn new(output_dir: &Path) -> io::Result<Self> {
        let path = output_dir.join("metrics.jsonl");
        let file = File::create(path)?;
        Ok(Self {
            writer: BufWriter::new(file),
            lines_written: 0,
        })
    }

    fn write_snapshot(&mut self, snapshot: &MetricSnapshot) -> io::Result<()> {
        let json = serde_json::to_string(snapshot)
            .map_err(io::Error::other)?;
        writeln!(self.writer, "{}", json)?;
        self.lines_written += 1;

        // Flush every 10 lines for crash resilience
        if self.lines_written.is_multiple_of(10) {
            self.writer.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

/// Run the test command
pub async fn run_test(options: TestOptions) -> Result<(), TestError> {
    // Validate timeout
    if options.timeout == Duration::ZERO {
        return Err(TestError::InvalidTimeout);
    }

    // Create output directory
    fs::create_dir_all(&options.output_dir)?;

    // Parse the pattern
    let pattern = if options.pattern.contains(':') {
        options.pattern.clone()
    } else {
        format!("{}:{}", options.pattern, options.default_port)
    };

    let endpoints = parse_range(&pattern)?;
    if endpoints.is_empty() {
        return Err(TestError::NoEndpoints);
    }

    let endpoint_count = endpoints.len();
    let mut errors: Vec<String> = Vec::new();

    // Group endpoints by port
    let mut ports: HashMap<u16, Vec<Ipv4Addr>> = HashMap::new();
    for ep in &endpoints {
        ports.entry(ep.port).or_default().push(ep.address);
    }

    // Create sockets and join multicast groups
    let mut sockets: HashMap<u16, MulticastSocket> = HashMap::new();
    for (&port, addresses) in &ports {
        let mut socket = MulticastSocket::new(port).await?;
        for &addr in addresses {
            socket.join(addr)?;
        }
        sockets.insert(port, socket);
    }

    // Create endpoint states
    let mut endpoint_states: HashMap<(Ipv4Addr, u16), TestEndpointState> = HashMap::new();
    for ep in &endpoints {
        endpoint_states.insert(
            (ep.address, ep.port),
            TestEndpointState::new(ep.address, ep.port),
        );
    }

    // Create metrics writer
    let mut metrics_writer = MetricsWriter::new(&options.output_dir)?;

    // Print start message
    println!("Test mode started");
    println!("  Output directory: {}", options.output_dir.display());
    println!("  Monitoring {} endpoint(s)", endpoint_count);
    println!("  Timeout: {} seconds", options.timeout.as_secs());
    println!("  Metrics interval: {}ms", options.metrics_interval.as_millis());
    println!();

    let test_start_time = Utc::now();
    let start_instant = Instant::now();
    let mut last_metrics_sample = Instant::now();
    let idle_timeout = Duration::from_secs(5);
    let mut buf = vec![0u8; 2048];

    loop {
        // Check for overall timeout
        if start_instant.elapsed() >= options.timeout {
            println!("Timeout reached.");
            break;
        }

        // Check for page end on all endpoints
        for state in endpoint_states.values_mut() {
            if state.page_active {
                if let Some(last) = state.last_packet {
                    if last.elapsed() >= idle_timeout {
                        if let Err(e) = handle_test_page_end(state, &options.output_dir) {
                            errors.push(format!("Error ending page on {}: {}", state.endpoint_string(), e));
                        }
                    }
                }
            }
        }

        // Sample metrics at interval
        if last_metrics_sample.elapsed() >= options.metrics_interval {
            for state in endpoint_states.values() {
                let snapshot = create_metric_snapshot(state);
                if let Err(e) = metrics_writer.write_snapshot(&snapshot) {
                    errors.push(format!("Error writing metrics: {}", e));
                }
            }
            last_metrics_sample = Instant::now();
        }

        // Receive from all sockets
        let recv_timeout = Duration::from_millis(10);

        for (&port, socket) in &sockets {
            loop {
                let recv_result = tokio::time::timeout(recv_timeout, socket.recv_from(&mut buf)).await;

                let (len, src_addr) = match recv_result {
                    Ok(Ok((len, addr))) => (len, addr),
                    Ok(Err(e)) => {
                        errors.push(format!("Receive error on port {}: {}", port, e));
                        break;
                    }
                    Err(_) => break,
                };

                let Ok(packet) = RtpPacket::parse(&buf[..len], src_addr) else {
                    continue;
                };

                let endpoint_key = endpoint_states.iter()
                    .filter(|((_, p), _)| *p == port)
                    .find(|(_, state)| state.ssrc == Some(packet.header.ssrc) || !state.page_active)
                    .map(|(k, _)| *k);

                if let Some(key) = endpoint_key {
                    if let Some(state) = endpoint_states.get_mut(&key) {
                        if let Err(e) = handle_test_packet(state, &packet, &options) {
                            errors.push(format!("Error handling packet on {}: {}", state.endpoint_string(), e));
                        }
                    }
                }
            }
        }
    }

    // Finalize any active recordings
    for state in endpoint_states.values_mut() {
        if state.page_active {
            if let Err(e) = handle_test_page_end(state, &options.output_dir) {
                errors.push(format!("Error finalizing page on {}: {}", state.endpoint_string(), e));
            }
        }
    }

    // Flush metrics
    metrics_writer.flush()?;

    // Generate and write summary
    let test_end_time = Utc::now();
    let summary = generate_summary(
        &options,
        test_start_time,
        test_end_time,
        &endpoint_states,
        errors,
    );
    write_summary(&options.output_dir, &summary)?;

    // Print completion message
    println!();
    println!("Test completed");
    println!("  Duration: {:.1}s", summary.test_metadata.duration_secs);
    println!("  Pages detected: {}", summary.pages.len());
    println!("  Errors: {}", summary.errors.len());
    println!();
    println!("Output files:");
    println!("  {}/metrics.jsonl", options.output_dir.display());
    println!("  {}/summary.json", options.output_dir.display());
    for page in &summary.pages {
        println!("  {}/{}", options.output_dir.display(), page.recording_file);
    }

    Ok(())
}

fn create_metric_snapshot(state: &TestEndpointState) -> MetricSnapshot {
    let duration_secs = if state.page_active {
        state.page_start.map(|s| {
            state.last_packet
                .map(|l| l.duration_since(s).as_secs_f64())
                .unwrap_or_else(|| s.elapsed().as_secs_f64())
        })
    } else {
        None
    };

    MetricSnapshot {
        timestamp: Utc::now(),
        endpoint: state.endpoint_string(),
        page_active: state.page_active,
        page_number: if state.page_active { Some(state.page_count) } else { None },
        duration_secs,
        network: NetworkMetrics {
            packets: state.stats.packets_received,
            bytes: state.stats.bytes_received,
            loss_percent: state.stats.loss_percent(),
            jitter_ms: state.stats.jitter_ms,
        },
        audio: AudioMetrics {
            rms_db: state.current_audio.rms_db,
            peak_db: state.current_audio.peak_db,
            dominant_freq_hz: state.current_audio.dominant_freq_hz,
            glitches: state.audio_stats.total_glitches,
            clipped: state.audio_stats.total_clipped,
        },
    }
}

fn handle_test_packet(
    state: &mut TestEndpointState,
    packet: &RtpPacket,
    options: &TestOptions,
) -> Result<(), TestError> {
    // Check if this is a new page
    if state.ssrc.is_none() || state.ssrc != Some(packet.header.ssrc) {
        state.page_count += 1;
        state.ssrc = Some(packet.header.ssrc);
        state.page_start = Some(Instant::now());
        state.page_start_utc = Some(Utc::now());
        state.page_active = true;
        state.stats = PageStats::default();

        // Codec type for potential future use (logging, metadata)
        let _codec_type = options.codec.unwrap_or_else(|| {
            CodecType::from_payload_type(packet.header.payload_type)
                .unwrap_or(CodecType::G711Ulaw)
        });

        let payload_type = PayloadType::from_pt(packet.header.payload_type);

        println!(
            "[{}] Page {} started (codec: {})",
            state.endpoint_string(),
            state.page_count,
            payload_type.name()
        );

        // Create decoder
        state.decoder = Some(create_decoder_for_payload_type(packet.header.payload_type)?);

        // Create audio analyzer
        let sample_rate = state.decoder.as_ref().unwrap().sample_rate();
        state.audio_analyzer = Some(AudioAnalyzer::new(sample_rate));
        state.audio_stats = AudioStats::new();

        // Create recorder with numbered filename
        let filename = format!(
            "page_{:04}_{}_{}.wav",
            state.page_count,
            state.address.to_string().replace('.', "_"),
            state.port
        );
        let path = options.output_dir.join(&filename);
        let channels = state.decoder.as_ref().unwrap().channels();
        state.recorder = Some(WavRecorder::new(&path, sample_rate, channels)?);
    }

    // Update stats
    state.stats.update(packet);
    state.last_packet = Some(Instant::now());

    // Decode, analyze, and record
    if let Some(ref mut dec) = state.decoder {
        if let Ok(samples) = dec.decode(&packet.payload) {
            if let Some(ref mut analyzer) = state.audio_analyzer {
                let analysis = analyzer.analyze(&samples);
                state.audio_stats.update(&analysis, samples.len() as u64);
                state.current_audio = analysis;
            }

            if let Some(ref mut rec) = state.recorder {
                rec.write_samples(&samples)?;
            }
        }
    }

    Ok(())
}

fn handle_test_page_end(
    state: &mut TestEndpointState,
    _output_dir: &Path,
) -> Result<(), TestError> {
    let duration = match (state.page_start, state.last_packet) {
        (Some(start), Some(last)) => last.duration_since(start).as_secs_f64(),
        (Some(start), None) => start.elapsed().as_secs_f64(),
        _ => 0.0,
    };

    let end_time = Utc::now();
    let start_time = state.page_start_utc.unwrap_or(end_time);

    let filename = format!(
        "page_{:04}_{}_{}.wav",
        state.page_count,
        state.address.to_string().replace('.', "_"),
        state.port
    );

    println!(
        "[{}] Page {} ended (duration: {:.1}s, glitches: {})",
        state.endpoint_string(),
        state.page_count,
        duration,
        state.audio_stats.total_glitches
    );

    // Finalize recording
    if let Some(rec) = state.recorder.take() {
        rec.finalize()?;
    }

    // Create page summary
    let page_summary = PageSummary {
        page_number: state.page_count,
        endpoint: state.endpoint_string(),
        start_time,
        end_time,
        duration_secs: duration,
        recording_file: filename,
        network: NetworkSummary {
            packets_received: state.stats.packets_received,
            bytes_received: state.stats.bytes_received,
            packets_lost: state.stats.packets_lost,
            loss_percent: state.stats.loss_percent(),
            jitter_ms: state.stats.jitter_ms,
        },
        audio: AudioSummary {
            peak_rms_db: state.audio_stats.peak_rms_db,
            avg_rms_db: if state.audio_stats.avg_rms_db.is_finite() {
                Some(state.audio_stats.avg_rms_db)
            } else {
                None
            },
            max_peak_db: state.audio_stats.max_peak_db,
            dominant_freq_hz: state.audio_stats.dominant_freq_hz,
            total_glitches: state.audio_stats.total_glitches,
            total_clipped: state.audio_stats.total_clipped,
            clipping_percent: state.audio_stats.clipping_percent(),
            avg_zero_crossing_rate: state.audio_stats.avg_zero_crossing_rate,
        },
    };

    state.completed_pages.push(page_summary);
    state.reset_page();

    Ok(())
}

fn generate_summary(
    options: &TestOptions,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
    endpoint_states: &HashMap<(Ipv4Addr, u16), TestEndpointState>,
    errors: Vec<String>,
) -> TestSummary {
    let duration_secs = (end_time - start_time).num_milliseconds() as f64 / 1000.0;

    // Collect all pages
    let mut all_pages: Vec<PageSummary> = Vec::new();
    let mut endpoint_totals: HashMap<String, EndpointTotal> = HashMap::new();

    for state in endpoint_states.values() {
        let endpoint_key = state.endpoint_string();

        // Add completed pages
        all_pages.extend(state.completed_pages.clone());

        // Calculate totals
        let total = endpoint_totals.entry(endpoint_key).or_default();
        total.pages_detected = state.completed_pages.len() as u32;
        total.total_duration_secs = state.completed_pages.iter()
            .map(|p| p.duration_secs)
            .sum();
        total.total_packets = state.completed_pages.iter()
            .map(|p| p.network.packets_received)
            .sum();
        total.total_bytes = state.completed_pages.iter()
            .map(|p| p.network.bytes_received)
            .sum();
    }

    // Sort pages by start time
    all_pages.sort_by(|a, b| a.start_time.cmp(&b.start_time));

    TestSummary {
        test_metadata: TestMetadata {
            start_time,
            end_time,
            duration_secs,
            pattern: options.pattern.clone(),
            endpoints_monitored: endpoint_states.len(),
            metrics_interval_ms: options.metrics_interval.as_millis() as u64,
            timeout_secs: options.timeout.as_secs(),
        },
        pages: all_pages,
        endpoint_totals,
        errors,
    }
}

fn write_summary(output_dir: &Path, summary: &TestSummary) -> io::Result<()> {
    let path = output_dir.join("summary.json");
    let file = File::create(path)?;
    serde_json::to_writer_pretty(file, summary)
        .map_err(io::Error::other)
}
