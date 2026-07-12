//! Configuration structs mirroring `config.toml`. Loaded at startup; the
//! store, api, and tui all read from a shared [`Config`].

use serde::{Deserialize, Serialize};

use crate::units::BgUnit;

/// Root configuration, mirroring the TOML table layout one-for-one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub qr: QrConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub backup: BackupConfig,
    #[serde(default)]
    pub log: LogConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerConfig {
    pub bind: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            bind: "0.0.0.0".to_string(),
            port: 8443,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Root directory holding the db, models/, photos/, backups/.
    pub data_dir: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        StorageConfig {
            data_dir: "./data".to_string(),
        }
    }
}

impl StorageConfig {
    pub fn db_path(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.data_dir).join("t1dm.db")
    }
    pub fn models_dir(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.data_dir).join("models")
    }
    pub fn photos_dir(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.data_dir).join("photos")
    }
    pub fn backups_dir(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.data_dir).join("backups")
    }
}

/// Advertised connection details embedded in the login QR.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QrConfig {
    pub advertise_addr: String,
    pub advertise_port: u16,
}

impl Default for QrConfig {
    fn default() -> Self {
        QrConfig {
            advertise_addr: "100.64.0.1".to_string(),
            advertise_port: 8443,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiConfig {
    pub theme: String,
    pub fps: u16,
    pub show_boot: bool,
    pub bg_unit: BgUnit,
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig {
            theme: "tron".to_string(),
            fps: 60,
            show_boot: true,
            bg_unit: BgUnit::Mgdl,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupConfig {
    pub enabled: bool,
    pub path: String,
    pub interval_hours: u32,
}

impl Default for BackupConfig {
    fn default() -> Self {
        BackupConfig {
            enabled: true,
            path: "./data/backups".to_string(),
            interval_hours: 24,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogConfig {
    pub level: String,
}

impl Default for LogConfig {
    fn default() -> Self {
        LogConfig {
            level: "info".to_string(),
        }
    }
}
