//! Configuration module — reads `config.toml` from `.config/` by default.
//!
//! Sections:
//!   [miner]   — pool connection & credentials
//!   [machine] — CPU/GPU mode, device selection, resource limits

use serde::Deserialize;
use std::fs;
use std::path::Path;

/// Pool connection and credential settings.
#[derive(Debug, Clone, Deserialize)]
pub struct MinerConfig {
    /// Stratum server address (host:port).
    #[serde(default = "default_server")]
    pub server: String,
    /// Miner username (wallet address . worker name).
    #[serde(default = "default_username")]
    pub username: String,
    /// Miner password (usually "x").
    #[serde(default = "default_password")]
    pub password: String,
}

/// Hardware / resource settings.
#[derive(Debug, Clone, Deserialize)]
pub struct MachineConfig {
    /// Mining mode: "gpu" or "cpu".
    #[serde(default = "default_mode")]
    pub mode: String,

    /// Number of CPU cores to use (0 = auto = all cores).
    /// Only used when mode = "cpu".
    #[serde(default)]
    pub cpu_cores: usize,

    /// Which GPU devices to use by ordinal. Empty = all available.
    /// Example: [0, 1] uses the first two GPUs; [1] uses only the second.
    /// Only used when mode = "gpu".
    #[serde(default)]
    pub gpu_devices: Vec<u32>,

    /// GPU resource usage percentage (1–100). 100 = full throttle.
    /// Lower values insert sleep between batches to reduce GPU utilization.
    /// Only used when mode = "gpu".
    #[serde(default = "default_gpu_usage")]
    pub gpu_usage: u32,
}

/// Top-level application config.
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub miner: MinerConfig,
    #[serde(default)]
    pub machine: MachineConfig,
}

// ─── Default values ────────────────────────────────────────────────────

fn default_server() -> String {
    "pool.btc-classic.org:63101".into()
}

fn default_username() -> String {
    "cc1q6qmx0kgdf94xe8046ee9tnvn6l20hk8nm8naw8.worker1".into()
}

fn default_password() -> String {
    "x".into()
}

fn default_mode() -> String {
    "gpu".into()
}

fn default_gpu_usage() -> u32 {
    100
}

// ─── Loading ───────────────────────────────────────────────────────────

impl AppConfig {
    /// Load config from a specific path.
    pub fn load_from(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(content) => match toml::from_str::<AppConfig>(&content) {
                Ok(cfg) => {
                    eprintln!("[CONFIG] Loaded config from {}", path.display());
                    cfg
                }
                Err(e) => {
                    eprintln!(
                        "[CONFIG] Failed to parse {}: {}. Using defaults.",
                        path.display(),
                        e
                    );
                    AppConfig::default()
                }
            },
            Err(_) => {
                eprintln!(
                    "[CONFIG] {} not found. Using defaults.",
                    path.display()
                );
                AppConfig::default()
            }
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            miner: MinerConfig::default(),
            machine: MachineConfig::default(),
        }
    }
}

impl Default for MinerConfig {
    fn default() -> Self {
        MinerConfig {
            server: default_server(),
            username: default_username(),
            password: default_password(),
        }
    }
}

impl Default for MachineConfig {
    fn default() -> Self {
        MachineConfig {
            mode: default_mode(),
            cpu_cores: 0,
            gpu_devices: vec![],
            gpu_usage: default_gpu_usage(),
        }
    }
}
