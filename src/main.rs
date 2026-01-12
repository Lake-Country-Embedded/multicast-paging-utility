//! Multicast Paging Utility - A tool for testing and troubleshooting multicast paging systems.
//!
//! This utility supports monitoring multicast addresses for RTP audio streams,
//! transmitting audio files as multicast pages, and recording received pages to files.

// Clippy configuration for code quality
#![warn(clippy::all)]
#![warn(clippy::pedantic)]
// Allow some pedantic lints that are too restrictive for this codebase
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_lossless)] // Explicit casts are clearer in audio/network code
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::uninlined_format_args)] // Explicit format args are often clearer
#![allow(clippy::needless_pass_by_value)] // Options structs are small and passed once
#![allow(clippy::items_after_statements)] // Use statements in scope are fine
#![allow(clippy::redundant_closure_for_method_calls)] // Explicit closures can be clearer
#![allow(clippy::map_unwrap_or)] // map().unwrap_or_else() is more readable than map_or_else()
#![allow(clippy::trivially_copy_pass_by_ref)] // &self is idiomatic for methods
#![allow(clippy::match_same_arms)] // Separate arms can be clearer for documentation
#![allow(clippy::wrong_self_convention)] // const fn methods require &self
#![allow(clippy::struct_field_names)] // Prefixes can clarify intent (e.g., default_)
#![allow(clippy::enum_variant_names)] // Error suffix is conventional for error enums

mod cli;
mod codec;
mod config;
mod network;
mod utils;

use clap::Parser;
use cli::{Cli, Commands};
use std::process::Command;
use std::time::Duration;
use tracing::warn;
use tracing_subscriber::EnvFilter;

/// Check if ffmpeg is available in PATH
fn check_ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Check runtime dependencies and warn if missing
fn check_runtime_dependencies(quiet: bool) {
    if !check_ffmpeg_available() {
        if !quiet {
            eprintln!("Warning: ffmpeg not found in PATH");
            eprintln!("  G.722 encoding/decoding will not be available.");
            eprintln!("  Install ffmpeg: apt install ffmpeg (Debian/Ubuntu)");
            eprintln!();
        }
        warn!("ffmpeg not found - G.722 codec support disabled");
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse CLI arguments
    let args = Cli::parse();

    // Initialize logging
    let filter = if args.verbose {
        EnvFilter::new("debug")
    } else if args.quiet {
        EnvFilter::new("error")
    } else {
        EnvFilter::new("info")
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    // Check runtime dependencies for commands that need them
    if let Some(
        Commands::Transmit { .. }
        | Commands::Monitor { .. }
        | Commands::Test { .. }
        | Commands::PolycomTransmit { .. }
        | Commands::PolycomMonitor { .. },
    ) = &args.command
    {
        check_runtime_dependencies(args.quiet);
    }

    match args.command {
        Some(Commands::Gui) | None => {
            // Launch GUI
            run_gui()?;
        }
        Some(Commands::Monitor {
            address,
            port,
            interface,
            codec,
            output,
            timeout,
            json,
        }) => {
            let codec_type = codec.as_ref().and_then(|c| codec::CodecType::from_str(c));
            let interface_addr = interface
                .as_ref()
                .and_then(|s| s.parse::<std::net::Ipv4Addr>().ok());

            let options = cli::monitor::MonitorRangeOptions {
                pattern: address,
                default_port: port,
                interface: interface_addr,
                codec: codec_type,
                output,
                timeout: if timeout == 0 {
                    Duration::MAX
                } else {
                    Duration::from_secs(timeout)
                },
                json,
                quiet: args.quiet,
            };

            cli::monitor::run_monitor_range(options).await?;
        }
        Some(Commands::Transmit {
            file,
            address,
            port,
            codec,
            ttl,
            r#loop,
        }) => {
            let addr = cli::monitor::parse_address(&address)?;
            let codec_type = codec::CodecType::from_str(&codec)
                .ok_or_else(|| format!("Unknown codec: {}", codec))?;

            let options = cli::transmit::TransmitOptions {
                file,
                address: addr,
                port,
                codec: codec_type,
                ttl,
                loop_audio: r#loop,
                quiet: args.quiet,
            };

            cli::run_transmit(options).await?;
        }
        Some(Commands::Test {
            address,
            port,
            interface,
            codec,
            output,
            timeout,
            metrics_interval,
        }) => {
            let codec_type = codec.as_ref().and_then(|c| codec::CodecType::from_str(c));
            let interface_addr = interface
                .as_ref()
                .and_then(|s| s.parse::<std::net::Ipv4Addr>().ok());

            let options = cli::test::TestOptions {
                pattern: address,
                default_port: port,
                interface: interface_addr,
                codec: codec_type,
                output_dir: output,
                timeout: Duration::from_secs(timeout),
                metrics_interval: Duration::from_millis(metrics_interval),
            };

            cli::run_test(options).await?;
        }
        Some(Commands::Review {
            directory,
            play,
            metrics,
            page,
        }) => {
            let options = cli::review::ReviewOptions {
                directory,
                play_audio: play,
                show_metrics: metrics,
                page_number: page,
            };

            cli::run_review(options)?;
        }
        Some(Commands::PolycomTransmit {
            file,
            address,
            port,
            channel,
            codec,
            caller_id,
            ttl,
            r#loop,
            alert_count,
            end_count,
            control_interval,
            skip_alert,
            skip_end,
            no_redundant,
            no_audio_header,
            little_endian,
            raw,
        }) => {
            let addr = cli::monitor::parse_address(&address)?;

            let options = cli::polycom_transmit::PolycomTransmitOptions {
                file,
                address: addr,
                port,
                channel,
                codec,
                caller_id,
                ttl,
                loop_audio: r#loop,
                quiet: args.quiet,
                alert_count,
                end_count,
                control_interval,
                skip_alert,
                skip_end,
                no_redundant,
                no_audio_header,
                little_endian,
                raw,
            };

            cli::run_polycom_transmit(options).await?;
        }
        Some(Commands::PolycomMonitor {
            address,
            port,
            channels,
            output,
            timeout,
            json,
        }) => {
            let options = cli::polycom_monitor::PolycomMonitorOptions {
                pattern: address,
                default_port: port,
                channels,
                output,
                timeout: if timeout == 0 {
                    Duration::MAX
                } else {
                    Duration::from_secs(timeout)
                },
                json,
                quiet: args.quiet,
            };

            cli::run_polycom_monitor(options).await?;
        }
    }

    Ok(())
}

#[allow(clippy::unnecessary_wraps)] // Will return errors when GUI is implemented
fn run_gui() -> Result<(), Box<dyn std::error::Error>> {
    // For now, print a message that GUI is not yet implemented
    // The full GTK implementation will be added in Phase 7-10
    println!("Multicast Paging Utility v{}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("GUI mode is not yet fully implemented.");
    println!();
    println!("Available CLI commands:");
    println!("  multicast-paging-utility monitor --address 224.0.1.1 --port 5004");
    println!("  multicast-paging-utility transmit --file audio.wav --address 224.0.1.1 --port 5004");
    println!();
    println!("Run with --help for more information.");

    Ok(())
}
