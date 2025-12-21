//! Review mode for examining test results
//!
//! This module provides a command to review test output directories,
//! displaying metrics in a formatted way and optionally playing back audio.

use crate::cli::test::{TestSummary, PageSummary, MetricSnapshot};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}};
use std::time::Duration;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ReviewError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Summary file not found: {0}")]
    SummaryNotFound(PathBuf),

    #[error("Audio error: {0}")]
    Audio(String),

    #[error("WAV error: {0}")]
    Wav(#[from] hound::Error),
}

pub struct ReviewOptions {
    pub directory: PathBuf,
    pub play_audio: bool,
    pub show_metrics: bool,
    pub page_number: Option<u32>,
}

/// Run the review command
pub fn run_review(options: ReviewOptions) -> Result<(), ReviewError> {
    let summary_path = options.directory.join("summary.json");

    if !summary_path.exists() {
        return Err(ReviewError::SummaryNotFound(summary_path));
    }

    // Load summary
    let summary: TestSummary = {
        let file = File::open(&summary_path)?;
        serde_json::from_reader(file)?
    };

    // Display header
    println!();
    println!("╔══════════════════════════════════════════════════════════════════╗");
    println!("║                     TEST RESULTS REVIEW                          ║");
    println!("╚══════════════════════════════════════════════════════════════════╝");
    println!();

    // Display test metadata
    display_metadata(&summary);

    // Display pages
    if let Some(page_num) = options.page_number {
        // Show specific page
        if let Some(page) = summary.pages.iter().find(|p| p.page_number == page_num) {
            display_page_detail(page);

            if options.play_audio {
                let audio_path = options.directory.join(&page.recording_file);
                if audio_path.exists() {
                    play_audio_file(&audio_path)?;
                } else {
                    println!("  ⚠ Audio file not found: {}", page.recording_file);
                }
            }
        } else {
            println!("Page {} not found in results.", page_num);
        }
    } else {
        // Show all pages
        display_pages_summary(&summary.pages);

        // Display endpoint totals
        display_endpoint_totals(&summary);

        // Display errors if any
        if !summary.errors.is_empty() {
            display_errors(&summary.errors);
        }

        // Show metrics summary if requested
        if options.show_metrics {
            display_metrics_summary(&options.directory)?;
        }

        // Play audio if requested
        if options.play_audio && !summary.pages.is_empty() {
            println!();
            println!("┌─────────────────────────────────────────────────────────────────┐");
            println!("│ AUDIO PLAYBACK                                                  │");
            println!("└─────────────────────────────────────────────────────────────────┘");

            for page in &summary.pages {
                let audio_path = options.directory.join(&page.recording_file);
                if audio_path.exists() {
                    println!();
                    println!("  Playing: {} ({:.1}s)", page.recording_file, page.duration_secs);
                    play_audio_file(&audio_path)?;
                } else {
                    println!("  ⚠ Audio file not found: {}", page.recording_file);
                }
            }
        }
    }

    println!();
    Ok(())
}

fn display_metadata(summary: &TestSummary) {
    let meta = &summary.test_metadata;

    println!("┌─────────────────────────────────────────────────────────────────┐");
    println!("│ TEST METADATA                                                   │");
    println!("├─────────────────────────────────────────────────────────────────┤");
    println!("│ Pattern:      {:<50} │", meta.pattern);
    println!("│ Endpoints:    {:<50} │", meta.endpoints_monitored);
    println!("│ Start Time:   {:<50} │", meta.start_time.format("%Y-%m-%d %H:%M:%S UTC"));
    println!("│ End Time:     {:<50} │", meta.end_time.format("%Y-%m-%d %H:%M:%S UTC"));
    println!("│ Duration:     {:<50} │", format!("{:.1}s", meta.duration_secs));
    println!("│ Timeout:      {:<50} │", format!("{}s", meta.timeout_secs));
    println!("│ Metrics Int:  {:<50} │", format!("{}ms", meta.metrics_interval_ms));
    println!("└─────────────────────────────────────────────────────────────────┘");
    println!();
}

fn display_pages_summary(pages: &[PageSummary]) {
    println!("┌─────────────────────────────────────────────────────────────────┐");
    println!("│ PAGES DETECTED: {:<48} │", pages.len());
    println!("├─────────────────────────────────────────────────────────────────┤");

    if pages.is_empty() {
        println!("│ No pages were detected during the test.                        │");
    } else {
        println!("│ {:>4} │ {:^19} │ {:>7} │ {:>6} │ {:>7} │ {:>6} │",
            "Page", "Endpoint", "Duration", "Loss%", "Glitch", "RMS");
        println!("├──────┼─────────────────────┼─────────┼────────┼─────────┼────────┤");

        for page in pages {
            let endpoint_short = if page.endpoint.len() > 19 {
                format!("{}...", &page.endpoint[..16])
            } else {
                page.endpoint.clone()
            };

            let avg_rms_str = page.audio.avg_rms_db
                .map(|v| format!("{:.1}dB", v))
                .unwrap_or_else(|| "-".to_string());
            println!("│ {:>4} │ {:^19} │ {:>6.1}s │ {:>5.1}% │ {:>7} │ {:>7} │",
                page.page_number,
                endpoint_short,
                page.duration_secs,
                page.network.loss_percent,
                page.audio.total_glitches,
                avg_rms_str
            );
        }
    }

    println!("└─────────────────────────────────────────────────────────────────┘");
    println!();
}

fn display_page_detail(page: &PageSummary) {
    println!("┌─────────────────────────────────────────────────────────────────┐");
    println!("│ PAGE {} DETAILS{:>50} │", page.page_number, "");
    println!("├─────────────────────────────────────────────────────────────────┤");
    println!("│ Endpoint:     {:<50} │", page.endpoint);
    println!("│ Start Time:   {:<50} │", page.start_time.format("%Y-%m-%d %H:%M:%S UTC"));
    println!("│ End Time:     {:<50} │", page.end_time.format("%Y-%m-%d %H:%M:%S UTC"));
    println!("│ Duration:     {:<50} │", format!("{:.2}s", page.duration_secs));
    println!("│ Recording:    {:<50} │", page.recording_file);
    println!("├─────────────────────────────────────────────────────────────────┤");
    println!("│ NETWORK STATS                                                   │");
    println!("│   Packets Received: {:<44} │", page.network.packets_received);
    println!("│   Bytes Received:   {:<44} │", page.network.bytes_received);
    println!("│   Packets Lost:     {:<44} │", page.network.packets_lost);
    println!("│   Loss Percent:     {:<44} │", format!("{:.2}%", page.network.loss_percent));
    println!("│   Jitter:           {:<44} │", format!("{:.2}ms", page.network.jitter_ms));
    println!("├─────────────────────────────────────────────────────────────────┤");
    println!("│ AUDIO ANALYSIS                                                  │");
    println!("│   Peak RMS:         {:<44} │", format!("{:.1}dB", page.audio.peak_rms_db));
    let avg_rms_str = page.audio.avg_rms_db
        .map(|v| format!("{:.1}dB", v))
        .unwrap_or_else(|| "N/A (no valid samples)".to_string());
    println!("│   Average RMS:      {:<44} │", avg_rms_str);
    println!("│   Max Peak:         {:<44} │", format!("{:.1}dB", page.audio.max_peak_db));
    println!("│   Dominant Freq:    {:<44} │", format_frequency(page.audio.dominant_freq_hz));
    println!("│   Total Glitches:   {:<44} │", page.audio.total_glitches);
    println!("│   Total Clipped:    {:<44} │", page.audio.total_clipped);
    println!("│   Clipping:         {:<44} │", format!("{:.3}%", page.audio.clipping_percent));
    println!("│   Avg ZCR:          {:<44} │", format!("{:.0}/s", page.audio.avg_zero_crossing_rate));
    println!("└─────────────────────────────────────────────────────────────────┘");
    println!();
}

fn display_endpoint_totals(summary: &TestSummary) {
    if summary.endpoint_totals.is_empty() {
        return;
    }

    println!("┌─────────────────────────────────────────────────────────────────┐");
    println!("│ ENDPOINT TOTALS                                                 │");
    println!("├─────────────────────────────────────────────────────────────────┤");
    println!("│ {:^19} │ {:>5} │ {:>10} │ {:>10} │ {:>10} │",
        "Endpoint", "Pages", "Duration", "Packets", "Bytes");
    println!("├─────────────────────┼───────┼────────────┼────────────┼────────────┤");

    for (endpoint, total) in &summary.endpoint_totals {
        let endpoint_short = if endpoint.len() > 19 {
            format!("{}...", &endpoint[..16])
        } else {
            endpoint.clone()
        };

        println!("│ {:^19} │ {:>5} │ {:>9.1}s │ {:>10} │ {:>10} │",
            endpoint_short,
            total.pages_detected,
            total.total_duration_secs,
            total.total_packets,
            total.total_bytes
        );
    }

    println!("└─────────────────────────────────────────────────────────────────┘");
    println!();
}

fn display_errors(errors: &[String]) {
    println!("┌─────────────────────────────────────────────────────────────────┐");
    println!("│ ⚠ ERRORS ({})                                                   │", errors.len());
    println!("├─────────────────────────────────────────────────────────────────┤");

    for (i, error) in errors.iter().enumerate() {
        let truncated = if error.len() > 62 {
            format!("{}...", &error[..59])
        } else {
            error.clone()
        };
        println!("│ {:>2}. {:<61} │", i + 1, truncated);
    }

    println!("└─────────────────────────────────────────────────────────────────┘");
    println!();
}

fn display_metrics_summary(directory: &Path) -> Result<(), ReviewError> {
    let metrics_path = directory.join("metrics.jsonl");

    if !metrics_path.exists() {
        println!("  Metrics file not found.");
        return Ok(());
    }

    let file = File::open(&metrics_path)?;
    let reader = BufReader::new(file);

    let mut total_samples = 0u64;
    let mut active_samples = 0u64;
    let mut max_rms = f64::NEG_INFINITY;
    let mut min_rms = f64::INFINITY;
    let mut total_glitches = 0u64;

    for line in reader.lines() {
        let line = line?;
        if let Ok(snapshot) = serde_json::from_str::<MetricSnapshot>(&line) {
            total_samples += 1;
            if snapshot.page_active {
                active_samples += 1;
            }
            if snapshot.audio.rms_db > max_rms && snapshot.audio.rms_db.is_finite() {
                max_rms = snapshot.audio.rms_db;
            }
            if snapshot.audio.rms_db < min_rms && snapshot.audio.rms_db.is_finite() {
                min_rms = snapshot.audio.rms_db;
            }
            total_glitches = total_glitches.max(snapshot.audio.glitches);
        }
    }

    println!("┌─────────────────────────────────────────────────────────────────┐");
    println!("│ METRICS SUMMARY                                                 │");
    println!("├─────────────────────────────────────────────────────────────────┤");
    println!("│ Total Samples:    {:<46} │", total_samples);
    println!("│ Active Samples:   {:<46} │", active_samples);
    println!("│ Idle Samples:     {:<46} │", total_samples - active_samples);

    if max_rms.is_finite() {
        println!("│ Max RMS:          {:<46} │", format!("{:.1}dB", max_rms));
    }
    if min_rms.is_finite() {
        println!("│ Min RMS:          {:<46} │", format!("{:.1}dB", min_rms));
    }

    println!("└─────────────────────────────────────────────────────────────────┘");
    println!();

    Ok(())
}

fn format_frequency(freq: f64) -> String {
    if freq <= 0.0 || !freq.is_finite() {
        "-".to_string()
    } else if freq >= 1000.0 {
        format!("{:.1}kHz", freq / 1000.0)
    } else {
        format!("{:.0}Hz", freq)
    }
}

/// Play a WAV file through the default audio output
fn play_audio_file(path: &Path) -> Result<(), ReviewError> {
    // Open WAV file
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();

    println!("    Format: {} channels, {}Hz, {}-bit",
        spec.channels, spec.sample_rate, spec.bits_per_sample);

    // Collect samples
    let samples: Vec<i16> = if spec.bits_per_sample == 16 {
        reader.samples::<i16>().filter_map(|s| s.ok()).collect()
    } else if spec.bits_per_sample == 8 {
        reader.samples::<i8>()
            .filter_map(|s| s.ok())
            .map(|s| (s as i16) << 8)
            .collect()
    } else {
        return Err(ReviewError::Audio(format!(
            "Unsupported bit depth: {}", spec.bits_per_sample
        )));
    };

    if samples.is_empty() {
        println!("    (empty audio file)");
        return Ok(());
    }

    // Set up audio output
    let host = cpal::default_host();
    let device = host.default_output_device()
        .ok_or_else(|| ReviewError::Audio("No output device found".to_string()))?;

    let config = cpal::StreamConfig {
        channels: spec.channels,
        sample_rate: cpal::SampleRate(spec.sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let samples = Arc::new(samples);
    let position = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicBool::new(false));

    let samples_clone = Arc::clone(&samples);
    let position_clone = Arc::clone(&position);
    let finished_clone = Arc::clone(&finished);

    let stream = device.build_output_stream(
        &config,
        move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
            let mut pos = position_clone.load(Ordering::Relaxed);
            for sample in data.iter_mut() {
                if pos < samples_clone.len() {
                    *sample = samples_clone[pos];
                    pos += 1;
                } else {
                    *sample = 0;
                    finished_clone.store(true, Ordering::Relaxed);
                }
            }
            position_clone.store(pos, Ordering::Relaxed);
        },
        |err| eprintln!("Audio stream error: {}", err),
        None,
    ).map_err(|e| ReviewError::Audio(e.to_string()))?;

    stream.play().map_err(|e| ReviewError::Audio(e.to_string()))?;

    // Calculate duration and show progress
    let total_samples = samples.len();
    let duration_secs = total_samples as f64 / (spec.sample_rate as f64 * spec.channels as f64);

    print!("    Playing: [");
    let bar_width = 40;

    while !finished.load(Ordering::Relaxed) {
        let pos = position.load(Ordering::Relaxed);
        let progress = pos as f64 / total_samples as f64;
        let filled = (progress * bar_width as f64) as usize;

        print!("\r    Playing: [");
        for i in 0..bar_width {
            if i < filled {
                print!("█");
            } else {
                print!("░");
            }
        }
        let current_time = pos as f64 / (spec.sample_rate as f64 * spec.channels as f64);
        print!("] {:.1}s / {:.1}s", current_time, duration_secs);

        use std::io::Write;
        std::io::stdout().flush().ok();

        std::thread::sleep(Duration::from_millis(100));
    }

    println!("\r    Playing: [{}] {:.1}s / {:.1}s ✓",
        "█".repeat(bar_width), duration_secs, duration_secs);

    // Small delay to ensure playback completes
    std::thread::sleep(Duration::from_millis(100));

    Ok(())
}
