use clap::{Parser, Subcommand};
use std::path::PathBuf;

pub mod audio_analyzer;
pub mod monitor;
pub mod polycom_monitor;
pub mod polycom_transmit;
pub mod recorder;
pub mod review;
pub mod test;
pub mod transmit;

// Re-exports for convenient access
pub use polycom_monitor::run_polycom_monitor;
pub use polycom_transmit::run_polycom_transmit;
pub use review::run_review;
pub use test::run_test;
pub use transmit::run_transmit;

#[derive(Parser)]
#[command(name = "multicast-paging-utility")]
#[command(author, version, about = "Multicast paging system testing utility")]
#[derive(Default)]
#[command(long_about = "A utility for testing and troubleshooting multicast paging systems.\n\n\
    Supports monitoring multicast addresses for RTP audio streams, \
    transmitting audio files as multicast pages, and recording received pages to files.")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Enable verbose output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Suppress non-essential output
    #[arg(short, long, global = true)]
    pub quiet: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Launch the GUI application
    Gui,

    /// Monitor a multicast address for pages
    Monitor {
        /// Multicast address pattern to monitor.
        /// Supports range syntax: 224.0.{0-10}.{0-10}:{5000-5010}
        /// Examples:
        ///   224.0.1.1:5004           - single address and port
        ///   224.0.1.1                - single address (uses --port)
        ///   224.0.{1-10}.1:5004      - range of addresses
        ///   224.0.1.1:{5000-5010}    - range of ports
        #[arg(short, long)]
        address: String,

        /// UDP port (used when address doesn't include port)
        #[arg(short, long, default_value = "5004")]
        port: u16,

        /// Force specific codec (auto-detect if not specified)
        /// Options: g711ulaw, g711alaw, g722, opus, l16
        #[arg(short, long)]
        codec: Option<String>,

        /// Output file prefix for recording (WAV format).
        /// For multiple endpoints, files are named: `prefix_224.0.1.1_5004.wav`
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Timeout in seconds (0 = indefinite)
        #[arg(short, long, default_value = "0")]
        timeout: u64,

        /// Output format in JSON (for automated testing)
        #[arg(long)]
        json: bool,
    },

    /// Transmit an audio file as a multicast page
    Transmit {
        /// Audio file to transmit (WAV format)
        #[arg(short, long)]
        file: PathBuf,

        /// Destination multicast address
        #[arg(short, long)]
        address: String,

        /// Destination UDP port
        #[arg(short, long, default_value = "5004")]
        port: u16,

        /// Codec to use for encoding
        /// Options: g711ulaw, g711alaw, opus, l16
        #[arg(short, long, default_value = "g711ulaw")]
        codec: String,

        /// Multicast TTL (Time To Live)
        #[arg(long, default_value = "32")]
        ttl: u8,

        /// Loop the audio file continuously
        #[arg(long)]
        r#loop: bool,
    },

    /// Run automated testing mode for CI/CD integration.
    /// Monitors multicast addresses, records pages, and outputs structured
    /// metrics and summaries for automated analysis.
    Test {
        /// Multicast address pattern to monitor.
        /// Supports range syntax: 224.0.{0-10}.{0-10}:{5000-5010}
        #[arg(short, long)]
        address: String,

        /// UDP port (used when address doesn't include port)
        #[arg(short, long, default_value = "5004")]
        port: u16,

        /// Force specific codec (auto-detect if not specified)
        /// Options: g711ulaw, g711alaw, g722, opus, l16
        #[arg(short, long)]
        codec: Option<String>,

        /// Output directory for test results (required).
        /// Will contain: metrics.jsonl, summary.json, and page recordings
        #[arg(short, long)]
        output: PathBuf,

        /// Test timeout in seconds (required, must be > 0)
        #[arg(short, long)]
        timeout: u64,

        /// Metrics sampling interval in milliseconds
        #[arg(long, default_value = "500")]
        metrics_interval: u64,
    },

    /// Review test results from a previous test run.
    /// Displays formatted metrics and can play back recorded audio.
    Review {
        /// Directory containing test results (with summary.json)
        #[arg(short, long)]
        directory: PathBuf,

        /// Play back recorded audio files
        #[arg(short, long)]
        play: bool,

        /// Show detailed metrics summary
        #[arg(short, long)]
        metrics: bool,

        /// Show details for a specific page number
        #[arg(long)]
        page: Option<u32>,
    },

    /// Transmit audio using Polycom PTT/Group Paging protocol.
    /// This is a proprietary protocol used by Polycom phones,
    /// NOT standard RTP multicast paging.
    PolycomTransmit {
        /// Audio file to transmit (WAV, MP3, FLAC, etc.)
        #[arg(short, long)]
        file: PathBuf,

        /// Destination multicast address
        #[arg(short, long, default_value = "224.0.1.116")]
        address: String,

        /// Destination UDP port
        #[arg(short, long, default_value = "5001")]
        port: u16,

        /// Channel number (1-50).
        /// PTT: 1-25 (24=Priority, 25=Emergency)
        /// Paging: 26-50 (49=Priority, 50=Emergency)
        #[arg(short, long, default_value = "26")]
        channel: u8,

        /// Codec to use: g722 (16kHz, recommended), g711u, g711a (8kHz)
        #[arg(long, default_value = "g722")]
        codec: String,

        /// Caller ID string (displayed on receiving phones)
        #[arg(long, default_value = "MPS-IP")]
        caller_id: String,

        /// Multicast TTL (Time To Live)
        #[arg(long, default_value = "32")]
        ttl: u8,

        /// Loop the audio file continuously
        #[arg(long)]
        r#loop: bool,

        /// Number of Alert packets to send (default: 31)
        #[arg(long, default_value = "31")]
        alert_count: u32,

        /// Number of End packets to send (default: 12)
        #[arg(long, default_value = "12")]
        end_count: u32,

        /// Delay between control packets in ms (default: 30)
        #[arg(long, default_value = "30")]
        control_interval: u64,

        /// Skip Alert packets entirely (for debugging)
        #[arg(long)]
        skip_alert: bool,

        /// Skip End packets entirely (for debugging)
        #[arg(long)]
        skip_end: bool,

        /// Skip redundant audio frames (for debugging)
        #[arg(long)]
        no_redundant: bool,

        /// Skip audio header (send raw audio after Polycom header)
        #[arg(long)]
        no_audio_header: bool,

        /// Use little-endian byte order for sample count (for debugging)
        #[arg(long)]
        little_endian: bool,

        /// File is raw pre-encoded audio (not WAV).
        /// Use with ffmpeg to encode: ffmpeg -i input.wav -ar 16000 -acodec g722 -f g722 output.raw
        #[arg(long)]
        raw: bool,
    },

    /// Monitor for Polycom PTT/Group Paging traffic.
    /// Listens on a multicast address and displays/records
    /// received Polycom pages.
    PolycomMonitor {
        /// Multicast address pattern to monitor.
        /// Supports range syntax: 224.0.{1-10}.116:{5001-5010}
        /// Examples:
        ///   224.0.1.116:5001       - single address and port
        ///   224.0.1.116            - single address (uses --port)
        ///   224.0.{1-10}.116:5001  - range of addresses
        #[arg(short, long, default_value = "224.0.1.116")]
        address: String,

        /// UDP port (used when address doesn't include port)
        #[arg(short, long, default_value = "5001")]
        port: u16,

        /// Channels to monitor: "all", single (e.g. "26"),
        /// range (e.g. "26-50"), or comma-separated (e.g. "26,27,50")
        #[arg(short, long, default_value = "all")]
        channels: String,

        /// Output directory for recordings (WAV format)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Timeout in seconds (0 = indefinite)
        #[arg(short, long, default_value = "0")]
        timeout: u64,

        /// Output format in JSON (for automated testing)
        #[arg(long)]
        json: bool,
    },
}

