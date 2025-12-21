# Architecture Overview

This document describes the architecture and design of the Multicast Paging Utility.

## High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         CLI (clap)                              │
│  ┌─────────┐ ┌──────────┐ ┌────────┐ ┌────────┐ ┌────────────┐  │
│  │ Monitor │ │ Transmit │ │  Test  │ │ Review │ │    GUI     │  │
│  └────┬────┘ └────┬─────┘ └───┬────┘ └───┬────┘ └────────────┘  │
└───────┼──────────┼───────────┼──────────┼───────────────────────┘
        │          │           │          │
┌───────▼──────────▼───────────▼──────────▼───────────────────────┐
│                      Core Services                              │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │
│  │  Audio Analyzer │  │    Recorder     │  │  Page Tracker   │  │
│  └────────┬────────┘  └────────┬────────┘  └────────┬────────┘  │
└───────────┼────────────────────┼────────────────────┼───────────┘
            │                    │                    │
┌───────────▼────────────────────▼────────────────────▼───────────┐
│                         Codec Layer                             │
│  ┌─────────┐  ┌─────────┐  ┌────────┐  ┌────────┐  ┌─────────┐  │
│  │ G.711 μ │  │ G.711 A │  │  G.722 │  │  Opus  │  │   L16   │  │
│  └─────────┘  └─────────┘  └────────┘  └────────┘  └─────────┘  │
└─────────────────────────────┬───────────────────────────────────┘
                              │
┌─────────────────────────────▼───────────────────────────────────┐
│                       Network Layer                             │
│  ┌─────────────────────┐  ┌─────────────────────────────────┐   │
│  │   Multicast Socket  │  │       RTP Packet Parser         │   │
│  └─────────────────────┘  └─────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

## Module Structure

### `src/main.rs`
Entry point that:
- Parses CLI arguments via clap
- Initializes tracing/logging
- Dispatches to appropriate command handlers

### `src/cli/` - Command Implementations

#### `mod.rs`
- Defines CLI structure using clap derive macros
- `Cli` struct with global options (verbose, quiet)
- `Commands` enum for subcommands

#### `monitor.rs`
Real-time multicast stream monitoring:
- `run_monitor_range()` - Main entry point for monitoring
- `EndpointState` - Per-endpoint state tracking
- `PageStats` - Network statistics (packets, bytes, loss, jitter)
- Supports range syntax for multiple endpoints
- Page detection based on RTP traffic gaps (5 second timeout)

#### `transmit.rs`
Audio file transmission as RTP streams:
- Reads WAV files using symphonia
- Resamples audio if needed
- Encodes using selected codec
- Transmits as RTP packets with proper timing

#### `test.rs`
CI/CD test mode:
- `TestOptions` - Test configuration
- `MetricsWriter` - Buffered JSONL output
- `TestSummary`, `PageSummary` - Result structures
- Periodic metrics sampling
- Automatic page recording with numbered filenames

#### `review.rs`
Test result review:
- Parses summary.json and metrics.jsonl
- Formatted table display
- Audio playback via cpal
- Per-page detail view

#### `recorder.rs`
WAV file recording:
- `WavRecorder` - Wrapper around hound
- Thread-safe sample accumulation
- Supports mono and stereo

#### `audio_analyzer.rs`
Real-time audio analysis:
- `AudioAnalyzer` - Per-frame analysis
- `AudioStats` - Accumulated statistics
- FFT-based dominant frequency detection (rustfft)
- Metrics: RMS, peak, glitches, clipping, zero-crossing rate, DC offset

### `src/codec/` - Audio Codec Support

#### `traits.rs`
Common interfaces:
```rust
pub trait AudioDecoder: Send {
    fn decode(&mut self, input: &[u8], output: &mut [i16]) -> Result<usize, CodecError>;
    fn sample_rate(&self) -> u32;
}

pub trait AudioEncoder: Send {
    fn encode(&mut self, input: &[i16], output: &mut [u8]) -> Result<usize, CodecError>;
    fn sample_rate(&self) -> u32;
    fn frame_size(&self) -> usize;
}
```

#### `g711.rs`
G.711 μ-law and A-law implementation:
- Pure Rust implementation (no external dependencies)
- Lookup tables for fast encoding/decoding
- 8kHz sample rate, 8-bit encoding

#### `opus.rs`
Opus codec wrapper:
- Uses audiopus crate
- Supports mono/stereo
- Configurable bitrate
- 48kHz sample rate

#### `pcm.rs`
L16 (Linear PCM) codec:
- Uncompressed 16-bit big-endian
- Direct sample pass-through
- Configurable sample rate

#### `mod.rs`
Codec factory:
- `create_decoder_for_payload_type()` - Create decoder from RTP PT
- `create_encoder()` - Create encoder by codec type
- `CodecType` enum

### `src/network/` - Network Layer

#### `multicast.rs`
Multicast socket management:
- `MulticastSocket` - Async UDP socket wrapper
- Join/leave multicast groups
- Configurable TTL, loopback
- Uses socket2 + tokio

#### `rtp.rs`
RTP packet handling:
- `RtpPacket` - Full packet representation
- `RtpHeader` - Header parsing
- `PayloadType` enum - Standard RTP payload types
- Sequence number tracking for loss detection
- Packet building for transmission

### `src/utils/`

#### `range_parser.rs`
Address range syntax parser:
- `parse_range()` - Parse range patterns
- Supports `{start-end}` syntax in any octet or port
- Returns iterator of `MulticastEndpoint`

### `src/config.rs`
Configuration management:
- TOML-based configuration
- Default endpoint settings
- Persistence to user config directory

## Data Flow

### Monitor Mode

```
┌─────────────┐    UDP     ┌─────────────┐    RTP     ┌─────────────┐
│  Multicast  │ ─────────► │  RTP Parser │ ─────────► │   Decoder   │
│   Socket    │            └─────────────┘            └──────┬──────┘
└─────────────┘                                              │
                                                             │ PCM
       ┌─────────────────────────────────────────────────────┘
       │
       ▼
┌─────────────┐    Stats   ┌─────────────┐   Update   ┌─────────────┐
│   Audio     │ ─────────► │    Page     │ ─────────► │   Display   │
│  Analyzer   │            │   Tracker   │            │   Output    │
└─────────────┘            └──────┬──────┘            └─────────────┘
                                  │
                                  │ Samples
                                  ▼
                           ┌─────────────┐
                           │  Recorder   │
                           │   (WAV)     │
                           └─────────────┘
```

### Transmit Mode

```
┌─────────────┐   Decode   ┌─────────────┐  Resample  ┌─────────────┐
│  WAV File   │ ─────────► │  Symphonia  │ ─────────► │   Encoder   │
│             │            └─────────────┘            └──────┬──────┘
└─────────────┘                                              │
                                                             │ Encoded
       ┌─────────────────────────────────────────────────────┘
       │
       ▼
┌─────────────┐    RTP     ┌─────────────┐    UDP     ┌─────────────┐
│  RTP Packet │ ─────────► │  Multicast  │ ─────────► │  Network    │
│   Builder   │            │   Socket    │            │             │
└─────────────┘            └─────────────┘            └─────────────┘
```

## Concurrency Model

- **Async Runtime**: Tokio multi-threaded runtime
- **Socket I/O**: Async UDP via tokio::net
- **Timer Events**: tokio::time::interval for periodic tasks
- **Channel Communication**: async-channel for component messaging

## Error Handling

- Custom error types per module (thiserror)
- Graceful degradation for non-fatal errors
- Errors captured in test output (not exit codes)
- Tracing for debug/verbose output

## Key Design Decisions

### Page Detection
- 5-second idle timeout after last RTP packet
- Sequence number gaps used for loss calculation
- Page duration based on first/last packet timestamps

### Codec Auto-Detection
- RTP payload type field used to select decoder
- Falls back to specified codec if provided
- Dynamic payload types (96-127) require hints

### Range Syntax
- Flexible multicast address specification
- Expands at runtime to concrete endpoints
- Efficient for monitoring large address spaces

### Test Mode Output
- JSON Lines for streaming metrics (append-friendly)
- Flat directory structure for simplicity
- Summary.json for final aggregated results
- Always exit 0 (errors in output, not exit code)

## Dependencies

| Category | Crate | Purpose |
|----------|-------|---------|
| Async | tokio | Runtime and I/O |
| Network | socket2 | Low-level socket options |
| Audio | cpal | Audio playback |
| Audio | hound | WAV recording |
| Audio | symphonia | Audio file decoding |
| Audio | rustfft | Frequency analysis |
| Codec | audiopus | Opus codec |
| CLI | clap | Argument parsing |
| Data | serde, serde_json | Serialization |
| Logging | tracing | Structured logging |

## Future Enhancements

- **GUI Mode**: GTK4/libadwaita interface (feature-gated)
- **G.722 Decoding**: Currently placeholder
- **RTCP Support**: Sender/receiver reports
- **SDP Parsing**: Session description protocol
- **Multi-channel**: Stereo/surround support
