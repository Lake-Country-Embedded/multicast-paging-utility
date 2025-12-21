//! Real-time audio analysis for quality monitoring.

#![allow(dead_code)]

use rustfft::{FftPlanner, num_complex::Complex};
use serde::Serialize;
use std::collections::HashMap;

// ============================================================================
// Audio Analysis Constants
// ============================================================================

/// FFT size for frequency analysis. 512 samples provides a good balance between
/// frequency resolution (~15.6 Hz bins at 8kHz) and latency (~64ms at 8kHz).
const FFT_SIZE: usize = 512;

/// Frequency bin width in Hz for grouping similar frequencies.
/// Frequencies within this range are considered the same dominant frequency.
const FREQ_BIN_WIDTH_HZ: f64 = 50.0;

/// Glitch detection threshold as sample-to-sample jump size.
/// ~61% of full scale for 16-bit audio. Real glitches from packet loss or
/// buffer issues typically cause jumps exceeding this threshold, while
/// normal loud audio rarely exceeds 40% sample-to-sample change.
const GLITCH_THRESHOLD: i16 = 20000;

/// Silence detection threshold in dB. Audio below this RMS level is
/// considered silence.
const SILENCE_THRESHOLD_DB: f64 = -50.0;

/// Clipping detection threshold. Samples at or above this absolute value
/// are considered clipped (within ~1% of `i16::MAX`).
const CLIPPING_THRESHOLD: i16 = 32600;

/// Minimum frequency in Hz to consider for dominant frequency detection.
/// Filters out DC and very low frequency noise.
const MIN_FREQUENCY_HZ: f64 = 50.0;

/// Minimum FFT magnitude to consider a frequency significant.
const MIN_FFT_MAGNITUDE: f32 = 0.01;

// ============================================================================
// Data Structures
// ============================================================================

/// Audio analysis results for a frame of audio
#[derive(Debug, Clone, Default, Serialize)]
pub struct AudioAnalysis {
    /// RMS (Root Mean Square) level in dB, representing perceived loudness
    pub rms_db: f64,
    /// Peak amplitude in dB
    pub peak_db: f64,
    /// Dominant frequency in Hz (from FFT)
    pub dominant_freq_hz: f64,
    /// Number of clipped samples (at max/min value)
    pub clipped_samples: u64,
    /// Number of glitches detected (large discontinuities)
    pub glitch_count: u64,
    /// Zero-crossing rate (crossings per second) - high values indicate noise
    pub zero_crossing_rate: f64,
    /// DC offset as percentage of max amplitude
    pub dc_offset_percent: f64,
    /// Number of repeated/stuck samples detected
    pub repeated_samples: u64,
    /// Whether the frame appears to be silence
    pub is_silence: bool,
}

/// Accumulates audio statistics across a page
#[derive(Debug, Clone, Default, Serialize)]
pub struct AudioStats {
    /// Peak RMS level seen
    pub peak_rms_db: f64,
    /// Average RMS level
    pub avg_rms_db: f64,
    /// Maximum peak amplitude
    pub max_peak_db: f64,
    /// Total clipped samples
    pub total_clipped: u64,
    /// Total glitches detected
    pub total_glitches: u64,
    /// Average zero-crossing rate
    pub avg_zero_crossing_rate: f64,
    /// Average DC offset
    pub avg_dc_offset_percent: f64,
    /// Total repeated samples
    pub total_repeated: u64,
    /// Most common dominant frequency
    pub dominant_freq_hz: f64,
    /// Total samples analyzed
    pub total_samples: u64,
    /// Number of frames analyzed
    pub frame_count: u64,
    /// Silent frame count
    pub silent_frames: u64,

    // Internal accumulators
    #[serde(skip)]
    rms_sum: f64,
    #[serde(skip)]
    rms_count: u64,
    #[serde(skip)]
    zcr_sum: f64,
    #[serde(skip)]
    dc_sum: f64,
    /// Frequency bins using `HashMap` for O(1) lookup.
    /// Key is frequency bin index (freq / `FREQ_BIN_WIDTH_HZ` as i32).
    #[serde(skip)]
    freq_bins: HashMap<i32, u32>,
}

impl AudioStats {
    pub fn new() -> Self {
        Self {
            peak_rms_db: f64::NEG_INFINITY,
            max_peak_db: f64::NEG_INFINITY,
            ..Default::default()
        }
    }

    /// Update stats with a new analysis frame
    pub fn update(&mut self, analysis: &AudioAnalysis, sample_count: u64) {
        self.frame_count += 1;
        self.total_samples += sample_count;

        // Update peaks
        if analysis.rms_db > self.peak_rms_db {
            self.peak_rms_db = analysis.rms_db;
        }
        if analysis.peak_db > self.max_peak_db {
            self.max_peak_db = analysis.peak_db;
        }

        // Accumulate for averages (skip infinite values)
        if analysis.rms_db.is_finite() {
            self.rms_sum += analysis.rms_db;
            self.rms_count += 1;
        }
        self.zcr_sum += analysis.zero_crossing_rate;
        self.dc_sum += analysis.dc_offset_percent;

        // Update totals
        self.total_clipped += analysis.clipped_samples;
        self.total_glitches += analysis.glitch_count;
        self.total_repeated += analysis.repeated_samples;

        if analysis.is_silence {
            self.silent_frames += 1;
        }

        // Track dominant frequencies using binned HashMap for O(1) lookup
        if analysis.dominant_freq_hz > 0.0 {
            let bin = (analysis.dominant_freq_hz / FREQ_BIN_WIDTH_HZ) as i32;
            *self.freq_bins.entry(bin).or_insert(0) += 1;
        }

        // Update averages (use rms_count for RMS to avoid NaN from infinite values)
        self.avg_rms_db = if self.rms_count > 0 {
            self.rms_sum / self.rms_count as f64
        } else {
            f64::NEG_INFINITY
        };
        self.avg_zero_crossing_rate = self.zcr_sum / self.frame_count as f64;
        self.avg_dc_offset_percent = self.dc_sum / self.frame_count as f64;

        // Find most common dominant frequency (convert bin back to Hz)
        if let Some((&bin, _)) = self.freq_bins.iter().max_by_key(|(_, &count)| count) {
            self.dominant_freq_hz = (bin as f64 + 0.5) * FREQ_BIN_WIDTH_HZ;
        }
    }

    /// Get clipping percentage
    pub fn clipping_percent(&self) -> f64 {
        if self.total_samples == 0 {
            0.0
        } else {
            100.0 * self.total_clipped as f64 / self.total_samples as f64
        }
    }

    /// Get silence percentage
    pub fn silence_percent(&self) -> f64 {
        if self.frame_count == 0 {
            0.0
        } else {
            100.0 * self.silent_frames as f64 / self.frame_count as f64
        }
    }
}

/// Real-time audio analyzer
pub struct AudioAnalyzer {
    sample_rate: u32,
    fft_size: usize,
    fft_planner: FftPlanner<f32>,
    fft_buffer: Vec<Complex<f32>>,
    fft_scratch: Vec<Complex<f32>>,
    window: Vec<f32>,
    last_sample: Option<i16>,
    /// Threshold for glitch detection (sample jump size)
    glitch_threshold: i16,
    /// Threshold for silence detection (RMS dB)
    silence_threshold_db: f64,
    /// Sample buffer for accumulating samples across RTP packets for FFT analysis.
    /// RTP packets are typically 160 samples (20ms at 8kHz), but FFT needs 512.
    sample_buffer: Vec<i16>,
}

impl AudioAnalyzer {
    /// Create a new audio analyzer for the given sample rate
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        let mut fft_planner = FftPlanner::new();
        let fft = fft_planner.plan_fft_forward(FFT_SIZE);
        let scratch_len = fft.get_inplace_scratch_len();

        // Create Hann window for better frequency resolution
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos())
            })
            .collect();

        Self {
            sample_rate,
            fft_size: FFT_SIZE,
            fft_planner,
            fft_buffer: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            fft_scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            window,
            last_sample: None,
            glitch_threshold: GLITCH_THRESHOLD,
            silence_threshold_db: SILENCE_THRESHOLD_DB,
            sample_buffer: Vec::with_capacity(FFT_SIZE),
        }
    }

    /// Analyze a frame of 16-bit PCM audio samples
    pub fn analyze(&mut self, samples: &[i16]) -> AudioAnalysis {
        if samples.is_empty() {
            return AudioAnalysis::default();
        }

        let mut analysis = AudioAnalysis::default();

        // Calculate RMS and peak
        let mut sum_squares: f64 = 0.0;
        let mut peak: i16 = 0;
        let mut clipped: u64 = 0;
        let mut zero_crossings: u64 = 0;
        let mut glitches: u64 = 0;
        let mut repeated: u64 = 0;
        let mut dc_sum: i64 = 0;
        let mut prev_sample = self.last_sample.unwrap_or(samples[0]);

        for &sample in samples {
            // Use saturating_abs to handle i16::MIN (-32768) without overflow
            let abs_sample = sample.saturating_abs();

            // RMS accumulation
            sum_squares += (sample as f64).powi(2);

            // Peak detection
            if abs_sample > peak {
                peak = abs_sample;
            }

            // Clipping detection (within ~1% of i16::MAX)
            if abs_sample >= CLIPPING_THRESHOLD {
                clipped += 1;
            }

            // Zero-crossing detection
            if (prev_sample >= 0 && sample < 0) || (prev_sample < 0 && sample >= 0) {
                zero_crossings += 1;
            }

            // Glitch detection (large discontinuity)
            let diff = (sample as i32 - prev_sample as i32).abs();
            if diff > self.glitch_threshold as i32 {
                glitches += 1;
            }

            // Repeated sample detection
            if sample == prev_sample {
                repeated += 1;
            }

            // DC offset accumulation
            dc_sum += sample as i64;

            prev_sample = sample;
        }

        self.last_sample = Some(prev_sample);

        let n = samples.len() as f64;

        // Calculate RMS in dB
        let rms = (sum_squares / n).sqrt();
        analysis.rms_db = if rms > 0.0 {
            20.0 * (rms / 32768.0).log10()
        } else {
            f64::NEG_INFINITY
        };

        // Calculate peak in dB
        analysis.peak_db = if peak > 0 {
            20.0 * (peak as f64 / 32768.0).log10()
        } else {
            f64::NEG_INFINITY
        };

        analysis.clipped_samples = clipped;
        analysis.glitch_count = glitches;
        analysis.repeated_samples = repeated;

        // Zero-crossing rate (per second)
        let duration_secs = n / self.sample_rate as f64;
        analysis.zero_crossing_rate = zero_crossings as f64 / duration_secs;

        // DC offset as percentage
        let dc_offset = dc_sum as f64 / n;
        analysis.dc_offset_percent = 100.0 * dc_offset / 32768.0;

        // Silence detection
        analysis.is_silence = analysis.rms_db < self.silence_threshold_db;

        // Accumulate samples for FFT analysis
        // RTP packets are typically 160 samples, but FFT needs 512
        self.sample_buffer.extend_from_slice(samples);

        // FFT for dominant frequency (only if we have enough buffered samples)
        if self.sample_buffer.len() >= self.fft_size {
            // Take the last fft_size samples for analysis
            let start = self.sample_buffer.len() - self.fft_size;
            let fft_samples: Vec<i16> = self.sample_buffer[start..].to_vec();
            analysis.dominant_freq_hz = self.compute_dominant_frequency(&fft_samples);

            // Keep only the last fft_size samples to maintain sliding window
            // and prevent unbounded growth
            if start > 0 {
                self.sample_buffer.drain(0..start);
            }
        }

        analysis
    }

    /// Compute dominant frequency using FFT
    fn compute_dominant_frequency(&mut self, samples: &[i16]) -> f64 {
        // Take the last fft_size samples
        let start = samples.len().saturating_sub(self.fft_size);
        let fft_samples = &samples[start..start + self.fft_size];

        // Apply window and convert to complex (normalize to [-1.0, 1.0])
        for (i, &sample) in fft_samples.iter().enumerate() {
            let windowed = sample as f32 * self.window[i] / i16::MAX as f32;
            self.fft_buffer[i] = Complex::new(windowed, 0.0);
        }

        // Perform FFT
        let fft = self.fft_planner.plan_fft_forward(self.fft_size);
        fft.process_with_scratch(&mut self.fft_buffer, &mut self.fft_scratch);

        // Find peak magnitude (only look at positive frequencies up to Nyquist)
        let nyquist = self.fft_size / 2;
        let mut max_magnitude: f32 = 0.0;
        let mut max_bin = 0;

        // Skip bin 0 (DC) and very low frequencies below MIN_FREQUENCY_HZ
        let min_bin = (MIN_FREQUENCY_HZ * self.fft_size as f64 / self.sample_rate as f64) as usize;

        for i in min_bin..nyquist {
            let magnitude = self.fft_buffer[i].norm();
            if magnitude > max_magnitude {
                max_magnitude = magnitude;
                max_bin = i;
            }
        }

        // Convert bin to frequency
        let freq = max_bin as f64 * self.sample_rate as f64 / self.fft_size as f64;

        // Only return if magnitude is significant
        if max_magnitude > MIN_FFT_MAGNITUDE {
            freq
        } else {
            0.0
        }
    }

    /// Reset state for a new page
    pub fn reset(&mut self) {
        self.last_sample = None;
        self.sample_buffer.clear();
    }
}

/// Format a frequency for display
pub fn format_frequency(freq: f64) -> String {
    if freq <= 0.0 {
        "-".to_string()
    } else if freq >= 1000.0 {
        format!("{:.1}kHz", freq / 1000.0)
    } else {
        format!("{:.0}Hz", freq)
    }
}

/// Format dB level for display
pub fn format_db(db: f64) -> String {
    if db <= -100.0 || db.is_infinite() {
        "-inf".to_string()
    } else {
        format!("{:.1}dB", db)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_silence_detection() {
        let mut analyzer = AudioAnalyzer::new(8000);
        let silence: Vec<i16> = vec![0; 160];
        let analysis = analyzer.analyze(&silence);
        assert!(analysis.is_silence);
    }

    #[test]
    fn test_tone_detection() {
        let mut analyzer = AudioAnalyzer::new(8000);
        // Generate 1kHz tone
        let samples: Vec<i16> = (0..512)
            .map(|i| {
                let t = i as f64 / 8000.0;
                (10000.0 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin()) as i16
            })
            .collect();

        let analysis = analyzer.analyze(&samples);
        // Should detect frequency near 1000 Hz
        assert!((analysis.dominant_freq_hz - 1000.0).abs() < 100.0);
    }

    #[test]
    fn test_clipping_detection() {
        let mut analyzer = AudioAnalyzer::new(8000);
        let mut samples: Vec<i16> = vec![0; 160];
        // Add some clipped samples
        samples[0] = 32767;
        samples[1] = 32767;
        samples[2] = -32768;

        let analysis = analyzer.analyze(&samples);
        assert_eq!(analysis.clipped_samples, 3);
    }

    #[test]
    fn test_glitch_detection() {
        let mut analyzer = AudioAnalyzer::new(8000);
        let mut samples: Vec<i16> = vec![0; 160];
        // Add a glitch (very large jump - like from packet loss/corruption)
        samples[50] = 0;
        samples[51] = 25000; // Huge jump (>61% of full scale)

        let analysis = analyzer.analyze(&samples);
        assert!(analysis.glitch_count >= 1);
    }
}
