use hound::{WavSpec, WavWriter};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RecorderError {
    #[error("Failed to create WAV file: {0}")]
    CreateFile(#[from] hound::Error),

    #[error("Failed to write samples: {0}")]
    WriteSamples(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Records audio samples to a WAV file
pub struct WavRecorder {
    writer: WavWriter<BufWriter<File>>,
    samples_written: u64,
}

impl WavRecorder {
    /// Create a new WAV recorder
    pub fn new(path: &Path, sample_rate: u32, channels: u8) -> Result<Self, RecorderError> {
        let spec = WavSpec {
            channels: channels as u16,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let writer = WavWriter::create(path, spec)?;

        Ok(Self {
            writer,
            samples_written: 0,
        })
    }

    /// Write samples to the WAV file
    pub fn write_samples(&mut self, samples: &[i16]) -> Result<(), RecorderError> {
        for &sample in samples {
            self.writer
                .write_sample(sample)
                .map_err(|e| RecorderError::WriteSamples(e.to_string()))?;
            self.samples_written += 1;
        }
        Ok(())
    }

    /// Finalize the WAV file
    pub fn finalize(self) -> Result<u64, RecorderError> {
        let samples = self.samples_written;
        self.writer
            .finalize()
            .map_err(|e| RecorderError::WriteSamples(e.to_string()))?;
        Ok(samples)
    }

    /// Get the number of samples written so far
    #[allow(dead_code)]
    pub fn samples_written(&self) -> u64 {
        self.samples_written
    }

    /// Get the duration in seconds
    #[allow(dead_code)]
    pub fn duration_secs(&self, sample_rate: u32, channels: u8) -> f64 {
        self.samples_written as f64 / sample_rate as f64 / channels as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_wav_recorder() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.wav");

        // Create recorder
        let mut recorder = WavRecorder::new(&path, 8000, 1).unwrap();

        // Write some samples
        let samples: Vec<i16> = (0..1600).map(|i| (i * 20) as i16).collect();
        recorder.write_samples(&samples).unwrap();

        assert_eq!(recorder.samples_written(), 1600);

        // Finalize
        let total = recorder.finalize().unwrap();
        assert_eq!(total, 1600);

        // Verify file exists and is valid
        assert!(path.exists());
        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().sample_rate, 8000);
        assert_eq!(reader.spec().channels, 1);

        // Cleanup
        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_wav_recorder_stereo() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test_stereo.wav");

        let mut recorder = WavRecorder::new(&path, 48000, 2).unwrap();

        let samples: Vec<i16> = (0..960).map(|i| (i * 10) as i16).collect();
        recorder.write_samples(&samples).unwrap();

        let total = recorder.finalize().unwrap();
        assert_eq!(total, 960);

        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().sample_rate, 48000);
        assert_eq!(reader.spec().channels, 2);
    }
}
