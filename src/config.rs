//! Configuration management for the multicast paging utility.
//!
//! This module provides persistent configuration storage for the GUI and
//! advanced CLI options. Currently reserved for future GUI implementation.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    ReadError(#[from] std::io::Error),
    #[error("Failed to parse config: {0}")]
    ParseError(#[from] toml::de::Error),
    #[error("Failed to serialize config: {0}")]
    SerializeError(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub audio: AudioConfig,
    pub network: NetworkConfig,
    pub monitor: MonitorConfig,
    pub monitored_ranges: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub output_device: Option<String>,
    pub buffer_size_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkConfig {
    pub default_port: u16,
    pub default_ttl: u8,
    pub default_codec: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorConfig {
    pub idle_timeout_secs: u32,
    pub auto_play_new_pages: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            output_device: None,
            buffer_size_ms: 100,
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            default_port: 5004,
            default_ttl: 32,
            default_codec: "g711ulaw".to_string(),
        }
    }
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            idle_timeout_secs: 5,
            auto_play_new_pages: false,
        }
    }
}

impl Config {
    /// Get the path to the configuration file
    pub fn config_path() -> PathBuf {
        directories::ProjectDirs::from("com", "github", "multicast-paging-utility")
            .map(|dirs| dirs.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
            .join("config.toml")
    }

    /// Load configuration from disk, or return defaults if not found
    pub fn load() -> Self {
        Self::try_load().unwrap_or_default()
    }

    /// Try to load configuration from disk
    pub fn try_load() -> Result<Self, ConfigError> {
        let path = Self::config_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    /// Save configuration to disk
    pub fn save(&self) -> Result<(), ConfigError> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Reset to default values and save
    pub fn reset(&mut self) -> Result<(), ConfigError> {
        *self = Self::default();
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.network.default_port, 5004);
        assert_eq!(config.network.default_ttl, 32);
        assert_eq!(config.audio.buffer_size_ms, 100);
        assert_eq!(config.monitor.idle_timeout_secs, 5);
    }

    #[test]
    fn test_serialize_deserialize() {
        let config = Config::default();
        let serialized = toml::to_string_pretty(&config).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(config.network.default_port, deserialized.network.default_port);
    }
}
