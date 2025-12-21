//! Integration tests for the multicast paging utility.
//!
//! These tests validate the full transmit/monitor cycle by:
//! 1. Generating test audio (sine waves at known frequencies)
//! 2. Transmitting via multicast using the utility
//! 3. Monitoring/recording with the test command
//! 4. Verifying the captured results match expectations

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

/// Get the path to the compiled binary
fn binary_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("target");
    path.push("debug");
    path.push("multicast-paging-utility");
    path
}

/// Generate a WAV file with a sine wave at the specified frequency
fn generate_test_wav(path: &std::path::Path, frequency_hz: u32, duration_secs: f32, sample_rate: u32) {
    let num_samples = (sample_rate as f32 * duration_secs) as usize;
    let mut samples = Vec::with_capacity(num_samples);

    for i in 0..num_samples {
        let t = i as f32 / sample_rate as f32;
        let sample = (0.5 * (2.0 * std::f32::consts::PI * frequency_hz as f32 * t).sin() * 32767.0) as i16;
        samples.push(sample);
    }

    // Write WAV file
    let mut file = fs::File::create(path).expect("Failed to create WAV file");

    // WAV header (44 bytes)
    let data_size = (num_samples * 2) as u32;
    let file_size = data_size + 36;

    // RIFF header
    file.write_all(b"RIFF").unwrap();
    file.write_all(&file_size.to_le_bytes()).unwrap();
    file.write_all(b"WAVE").unwrap();

    // fmt chunk
    file.write_all(b"fmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap(); // chunk size
    file.write_all(&1u16.to_le_bytes()).unwrap(); // audio format (PCM)
    file.write_all(&1u16.to_le_bytes()).unwrap(); // num channels
    file.write_all(&sample_rate.to_le_bytes()).unwrap(); // sample rate
    file.write_all(&(sample_rate * 2).to_le_bytes()).unwrap(); // byte rate
    file.write_all(&2u16.to_le_bytes()).unwrap(); // block align
    file.write_all(&16u16.to_le_bytes()).unwrap(); // bits per sample

    // data chunk
    file.write_all(b"data").unwrap();
    file.write_all(&data_size.to_le_bytes()).unwrap();

    for sample in samples {
        file.write_all(&sample.to_le_bytes()).unwrap();
    }
}

/// Parse the summary.json file and extract key metrics
fn parse_summary(path: &std::path::Path) -> serde_json::Value {
    let content = fs::read_to_string(path).expect("Failed to read summary.json");
    serde_json::from_str(&content).expect("Failed to parse summary.json")
}

#[test]
fn test_transmit_and_monitor_1khz_tone() {
    // Skip if binary doesn't exist (not built yet)
    let binary = binary_path();
    if !binary.exists() {
        eprintln!("Skipping test: binary not found at {:?}", binary);
        return;
    }

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let output_dir = temp_dir.path().join("output");
    fs::create_dir_all(&output_dir).expect("Failed to create output dir");

    // Generate 1kHz test tone (3 seconds)
    let wav_path = temp_dir.path().join("tone_1khz.wav");
    generate_test_wav(&wav_path, 1000, 3.0, 8000);

    // Use a unique multicast address to avoid conflicts with other tests
    let multicast_addr = "224.0.123.1";
    let port = "15004";

    // Start the test monitor in background
    let monitor = Command::new(&binary)
        .args([
            "test",
            "--address", multicast_addr,
            "--port", port,
            "--output", output_dir.to_str().unwrap(),
            "--timeout", "8",
            "--codec", "g711ulaw",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start monitor");

    // Wait for monitor to initialize
    thread::sleep(Duration::from_secs(2));

    // Transmit the test tone
    let transmit_status = Command::new(&binary)
        .args([
            "transmit",
            "--file", wav_path.to_str().unwrap(),
            "--address", multicast_addr,
            "--port", port,
            "--codec", "g711ulaw",
            "--quiet",
        ])
        .status()
        .expect("Failed to run transmit");

    assert!(transmit_status.success(), "Transmit command failed");

    // Wait for monitor to complete
    let monitor_output = monitor.wait_with_output().expect("Failed to wait for monitor");
    assert!(monitor_output.status.success(), "Monitor command failed");

    // Verify output files exist
    let summary_path = output_dir.join("summary.json");
    let metrics_path = output_dir.join("metrics.jsonl");

    assert!(summary_path.exists(), "summary.json not created");
    assert!(metrics_path.exists(), "metrics.jsonl not created");

    // Parse and verify summary
    let summary = parse_summary(&summary_path);

    // Check that we detected exactly 1 page
    let pages = summary["pages"].as_array().expect("pages should be array");
    assert_eq!(pages.len(), 1, "Should detect exactly 1 page");

    let page = &pages[0];

    // Verify duration is approximately 3 seconds (with some tolerance)
    let duration = page["duration_secs"].as_f64().expect("duration should be f64");
    assert!(
        duration >= 2.5 && duration <= 3.5,
        "Duration {} should be approximately 3 seconds",
        duration
    );

    // Verify no packet loss
    let loss_percent = page["network"]["loss_percent"].as_f64().expect("loss_percent should be f64");
    assert!(
        loss_percent < 1.0,
        "Packet loss {} should be less than 1%",
        loss_percent
    );

    // Verify no glitches
    let glitches = page["audio"]["total_glitches"].as_u64().expect("glitches should be u64");
    assert_eq!(glitches, 0, "Should have no glitches");

    // Verify dominant frequency is approximately 1kHz
    let freq = page["audio"]["dominant_freq_hz"].as_f64().expect("freq should be f64");
    assert!(
        freq >= 900.0 && freq <= 1100.0,
        "Dominant frequency {} should be approximately 1000 Hz",
        freq
    );

    // Verify a WAV file was created
    let wav_files: Vec<_> = fs::read_dir(&output_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "wav"))
        .collect();
    assert_eq!(wav_files.len(), 1, "Should have exactly 1 WAV recording");
}

#[test]
fn test_transmit_and_monitor_440hz_tone() {
    let binary = binary_path();
    if !binary.exists() {
        eprintln!("Skipping test: binary not found at {:?}", binary);
        return;
    }

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let output_dir = temp_dir.path().join("output");
    fs::create_dir_all(&output_dir).expect("Failed to create output dir");

    // Generate 440Hz test tone (2 seconds)
    let wav_path = temp_dir.path().join("tone_440hz.wav");
    generate_test_wav(&wav_path, 440, 2.0, 8000);

    // Use different port to avoid conflicts
    let multicast_addr = "224.0.123.2";
    let port = "15005";

    // Start monitor
    let monitor = Command::new(&binary)
        .args([
            "test",
            "--address", multicast_addr,
            "--port", port,
            "--output", output_dir.to_str().unwrap(),
            "--timeout", "6",
            "--codec", "g711ulaw",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start monitor");

    thread::sleep(Duration::from_secs(2));

    // Transmit
    let transmit_status = Command::new(&binary)
        .args([
            "transmit",
            "--file", wav_path.to_str().unwrap(),
            "--address", multicast_addr,
            "--port", port,
            "--codec", "g711ulaw",
            "--quiet",
        ])
        .status()
        .expect("Failed to run transmit");

    assert!(transmit_status.success(), "Transmit command failed");

    let monitor_output = monitor.wait_with_output().expect("Failed to wait for monitor");
    assert!(monitor_output.status.success(), "Monitor command failed");

    // Verify
    let summary_path = output_dir.join("summary.json");
    let summary = parse_summary(&summary_path);
    let pages = summary["pages"].as_array().expect("pages should be array");
    assert_eq!(pages.len(), 1, "Should detect exactly 1 page");

    let page = &pages[0];

    // Verify dominant frequency is approximately 440Hz
    let freq = page["audio"]["dominant_freq_hz"].as_f64().expect("freq should be f64");
    assert!(
        freq >= 400.0 && freq <= 500.0,
        "Dominant frequency {} should be approximately 440 Hz",
        freq
    );

    // Verify zero crossing rate matches 440Hz (should be ~880/s)
    let zcr = page["audio"]["avg_zero_crossing_rate"].as_f64().expect("zcr should be f64");
    assert!(
        zcr >= 800.0 && zcr <= 1000.0,
        "Zero crossing rate {} should be approximately 880/s for 440Hz",
        zcr
    );
}

#[test]
fn test_no_audio_timeout() {
    let binary = binary_path();
    if !binary.exists() {
        eprintln!("Skipping test: binary not found at {:?}", binary);
        return;
    }

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let output_dir = temp_dir.path().join("output");
    fs::create_dir_all(&output_dir).expect("Failed to create output dir");

    // Use different port to avoid conflicts
    let multicast_addr = "224.0.123.3";
    let port = "15006";

    // Start monitor with short timeout, no transmission
    let output = Command::new(&binary)
        .args([
            "test",
            "--address", multicast_addr,
            "--port", port,
            "--output", output_dir.to_str().unwrap(),
            "--timeout", "2",
            "--codec", "g711ulaw",
        ])
        .output()
        .expect("Failed to run monitor");

    assert!(output.status.success(), "Monitor should complete successfully even with no audio");

    // Verify summary shows 0 pages
    let summary_path = output_dir.join("summary.json");
    let summary = parse_summary(&summary_path);
    let pages = summary["pages"].as_array().expect("pages should be array");
    assert_eq!(pages.len(), 0, "Should detect 0 pages when no audio transmitted");
}

#[test]
fn test_review_command() {
    let binary = binary_path();
    if !binary.exists() {
        eprintln!("Skipping test: binary not found at {:?}", binary);
        return;
    }

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let output_dir = temp_dir.path().join("output");
    fs::create_dir_all(&output_dir).expect("Failed to create output dir");

    // Generate test tone
    let wav_path = temp_dir.path().join("tone.wav");
    generate_test_wav(&wav_path, 1000, 2.0, 8000);

    let multicast_addr = "224.0.123.4";
    let port = "15007";

    // Run test to generate results
    let mut monitor = Command::new(&binary)
        .args([
            "test",
            "--address", multicast_addr,
            "--port", port,
            "--output", output_dir.to_str().unwrap(),
            "--timeout", "6",
            "--codec", "g711ulaw",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start monitor");

    thread::sleep(Duration::from_secs(2));

    Command::new(&binary)
        .args([
            "transmit",
            "--file", wav_path.to_str().unwrap(),
            "--address", multicast_addr,
            "--port", port,
            "--codec", "g711ulaw",
            "--quiet",
        ])
        .status()
        .expect("Failed to run transmit");

    monitor.wait().expect("Failed to wait for monitor");

    // Now test the review command
    let review_output = Command::new(&binary)
        .args([
            "review",
            "--directory", output_dir.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run review");

    assert!(review_output.status.success(), "Review command should succeed");

    let stdout = String::from_utf8_lossy(&review_output.stdout);
    assert!(stdout.contains("TEST RESULTS REVIEW"), "Review should show results header");
    assert!(stdout.contains("PAGES DETECTED: 1"), "Review should show 1 page detected");
}
