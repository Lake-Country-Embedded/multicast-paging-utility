# Multicast Paging Utility

A command-line utility for testing and troubleshooting multicast paging systems. Monitor RTP audio streams, transmit test pages, record audio, and analyze network/audio quality metrics.

## Features

- **Monitor** multicast addresses for RTP audio streams with real-time statistics
- **Transmit** audio files as multicast pages with configurable codecs
- **Record** received pages to WAV files
- **Test mode** for CI/CD integration with structured JSON output
- **Review** test results with formatted display and audio playback
- **Audio analysis** including RMS levels, peak detection, glitch detection, and FFT-based frequency analysis
- **Range syntax** for monitoring multiple addresses/ports simultaneously

## Supported Codecs

| Codec | RTP Payload Type | Description |
|-------|------------------|-------------|
| G.711 μ-law | 0 | Standard telephony codec (North America, Japan) |
| G.711 A-law | 8 | Standard telephony codec (Europe, international) |
| G.722 | 9 | Wideband speech codec |
| L16 | 10/11 | Uncompressed 16-bit PCM |
| Opus | 96+ (dynamic) | Modern low-latency codec |

## Installation

### From Source

```bash
# Clone the repository
git clone https://github.com/yourusername/multicast-paging-utility.git
cd multicast-paging-utility

# Build release binary
cargo build --release

# Install to ~/.cargo/bin
cargo install --path .
```

### Dependencies

**Build Dependencies:**
- Rust 1.70+ (2021 edition)
- Linux: ALSA development libraries (`libasound2-dev`)
- For Opus support: `libopus-dev`

**Runtime Dependencies:**
- **ffmpeg** - Required for G.722 and high-quality G.711 encoding/decoding
  - Ubuntu/Debian: `apt install ffmpeg`
  - Fedora: `dnf install ffmpeg`
  - macOS: `brew install ffmpeg`

The utility will check for ffmpeg availability at startup and display an error if it's not found.

## Usage

### Monitor Mode

Monitor multicast addresses for incoming RTP audio streams:

```bash
# Monitor a single address
multicast-paging-utility monitor --address 224.0.1.1 --port 5004

# Monitor with recording
multicast-paging-utility monitor --address 224.0.1.1 --port 5004 --output recording.wav

# Monitor a range of addresses
multicast-paging-utility monitor --address "224.0.1.{1-10}:5004"

# Monitor multiple ports
multicast-paging-utility monitor --address "224.0.1.1:{5004-5010}"

# Force a specific codec (skip auto-detection)
multicast-paging-utility monitor --address 224.0.1.1 --codec g711ulaw

# JSON output for scripting
multicast-paging-utility monitor --address 224.0.1.1 --timeout 30 --json
```

### Transmit Mode

Transmit audio files as multicast RTP streams:

```bash
# Transmit a WAV file
multicast-paging-utility transmit --file audio.wav --address 224.0.1.1 --port 5004

# Use a specific codec
multicast-paging-utility transmit --file audio.wav --address 224.0.1.1 --codec g711alaw

# Loop continuously
multicast-paging-utility transmit --file audio.wav --address 224.0.1.1 --loop

# Set multicast TTL
multicast-paging-utility transmit --file audio.wav --address 224.0.1.1 --ttl 64
```

### Test Mode (CI/CD)

Run automated tests with structured output for CI/CD pipelines:

```bash
# Run a 60-second test
multicast-paging-utility test --address 224.0.1.1 --output ./test-results --timeout 60

# Custom metrics interval
multicast-paging-utility test --address 224.0.1.1 --output ./test-results --timeout 60 --metrics-interval 100
```

**Output files:**
- `summary.json` - Test summary with page details and statistics
- `metrics.jsonl` - Timestamped metrics (JSON Lines format)
- `page_NNNN_ADDRESS_PORT.wav` - Recorded audio for each page

### Review Mode

Review test results from a previous test run:

```bash
# Display test summary
multicast-paging-utility review --directory ./test-results

# Show detailed metrics
multicast-paging-utility review --directory ./test-results --metrics

# View specific page details
multicast-paging-utility review --directory ./test-results --page 1

# Play back recorded audio
multicast-paging-utility review --directory ./test-results --play
```

### Polycom Paging Mode

Transmit and monitor Polycom PTT/Group Paging traffic. This uses Polycom's proprietary protocol, **not** standard RTP multicast.

```bash
# Transmit a page using G.722 (recommended, 16kHz wideband)
multicast-paging-utility polycom-transmit --file audio.wav --channel 26

# Transmit using G.711 µ-law (8kHz narrowband)
multicast-paging-utility polycom-transmit --file audio.wav --channel 26 --codec g711u

# Set custom caller ID
multicast-paging-utility polycom-transmit --file audio.wav --caller-id "Reception"

# Monitor Polycom pages on a single address
multicast-paging-utility polycom-monitor --address 224.0.1.116 --port 5001

# Monitor with recording
multicast-paging-utility polycom-monitor --output ./recordings

# Monitor specific channels only
multicast-paging-utility polycom-monitor --channels 26-30

# Monitor a range of addresses
multicast-paging-utility polycom-monitor --address "224.0.{1-10}.116:{5001-5010}"
```

**Polycom Channel Reference:**
- Channels 1-25: PTT (Push-to-Talk) mode
  - Channel 24: Priority PTT
  - Channel 25: Emergency PTT
- Channels 26-50: Paging mode (default)
  - Channel 49: Priority Paging
  - Channel 50: Emergency Paging

## Address Range Syntax

The utility supports a flexible range syntax for monitoring multiple endpoints:

| Pattern | Description |
|---------|-------------|
| `224.0.1.1` | Single address (uses --port) |
| `224.0.1.1:5004` | Address with port |
| `224.0.1.{1-10}:5004` | Range in third octet |
| `224.0.{1-5}.{1-5}:5004` | Ranges in multiple octets |
| `224.0.1.1:{5004-5010}` | Range of ports |
| `224.0.{1-2}.1:{5004-5005}` | Combined ranges |

## Output Formats

### Summary JSON

```json
{
  "test_metadata": {
    "start_time": "2024-01-15T10:30:00Z",
    "end_time": "2024-01-15T10:35:00Z",
    "duration_secs": 300.0,
    "pattern": "224.0.1.1:5004",
    "endpoints_monitored": 1
  },
  "pages": [
    {
      "page_number": 1,
      "endpoint": "224.0.1.1:5004",
      "duration_secs": 30.0,
      "recording_file": "page_0001_224_0_1_1_5004.wav",
      "network": {
        "packets_received": 1500,
        "loss_percent": 0.0,
        "jitter_ms": 1.2
      },
      "audio": {
        "peak_rms_db": -12.5,
        "avg_rms_db": -18.3,
        "total_glitches": 0,
        "total_clipped": 0
      }
    }
  ]
}
```

### Metrics JSONL

Each line contains a timestamped snapshot:

```json
{"timestamp":"2024-01-15T10:30:00.500Z","endpoint":"224.0.1.1:5004","page_active":true,"page_number":1,"network":{"packets":260,"bytes":41600,"loss_percent":0.0,"jitter_ms":1.2},"audio":{"rms_db":-18.5,"peak_db":-6.2}}
```

## CI/CD Integration

### GitHub Actions

```yaml
- name: Test Paging System
  run: |
    multicast-paging-utility test \
      --address 224.0.1.1 \
      --output ./test-results \
      --timeout 60

- name: Check Results
  run: |
    jq '.pages | length' ./test-results/summary.json
    jq '.pages[].network.loss_percent < 1' ./test-results/summary.json
```

### GitLab CI

```yaml
test_paging:
  script:
    - multicast-paging-utility test --address 224.0.1.1 --output ./results --timeout 60
  artifacts:
    paths:
      - results/
    reports:
      dotenv: results/summary.json
```

## Audio Analysis Metrics

The utility provides real-time audio analysis:

| Metric | Description |
|--------|-------------|
| RMS (dB) | Root Mean Square level - perceived loudness |
| Peak (dB) | Maximum amplitude |
| Dominant Frequency | FFT-detected primary frequency |
| Glitches | Large sample discontinuities (potential packet loss) |
| Clipping | Samples at maximum amplitude |
| Zero-Crossing Rate | Crossings per second (noise indicator) |
| DC Offset | Average sample offset from zero |

## Network Metrics

| Metric | Description |
|--------|-------------|
| Packets Received | Total RTP packets received |
| Bytes Received | Total payload bytes |
| Packet Loss | Detected gaps in sequence numbers |
| Jitter | Variation in packet arrival times |

## Building & Testing

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Run all tests (unit + integration)
cargo test

# Run only unit tests
cargo test --lib

# Run only integration tests
cargo test --test integration_test

# Run clippy lints
cargo clippy

# Run with verbose output
cargo run -- -v monitor --address 224.0.1.1
```

### Integration Tests

The integration tests validate the full transmit/monitor cycle:
- Generate test audio (sine waves at known frequencies)
- Transmit via multicast
- Monitor and record with the test command
- Verify captured results match expectations (frequency, duration, packet loss)

## Architecture

```
src/
├── main.rs           # Entry point and CLI dispatch
├── cli/
│   ├── mod.rs        # CLI argument definitions (clap)
│   ├── monitor.rs    # Monitor mode implementation
│   ├── transmit.rs   # Transmit mode implementation
│   ├── test.rs       # Test mode for CI/CD
│   ├── review.rs     # Review test results
│   ├── recorder.rs   # WAV file recording
│   ├── audio_analyzer.rs  # Real-time audio analysis
│   ├── polycom_transmit.rs  # Polycom paging transmit
│   └── polycom_monitor.rs   # Polycom paging monitor
├── codec/
│   ├── mod.rs        # Codec factory
│   ├── traits.rs     # Encoder/Decoder traits
│   ├── g711.rs       # G.711 μ-law and A-law
│   ├── g722.rs       # G.722 reference implementation
│   ├── subprocess.rs # FFmpeg-based encoders/decoders
│   ├── opus.rs       # Opus codec
│   └── pcm.rs        # L16 uncompressed PCM
├── network/
│   ├── mod.rs        # Network module exports
│   ├── multicast.rs  # Multicast socket management
│   ├── polycom.rs    # Polycom protocol implementation
│   └── rtp.rs        # RTP packet parsing/building
├── utils/
│   └── range_parser.rs  # Address range syntax parser
└── config.rs         # Configuration management

tests/
└── integration_test.rs  # End-to-end integration tests
```

## License

MIT License - See [LICENSE](LICENSE) for details.

## Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Run tests (`cargo test`)
4. Run clippy (`cargo clippy`)
5. Commit your changes
6. Push to the branch
7. Open a Pull Request
