//! Runtime configuration and well-known filesystem locations.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Preferred port; `0` lets the OS pick a free one (always parsed from stdout).
    pub port: u16,
    /// Hide to tray on window close instead of quitting.
    pub close_to_tray: bool,
    /// Auto-restart `dsh web` with exponential backoff on unexpected exit.
    pub auto_restart: bool,
    /// Max consecutive restart attempts before entering the Error state.
    pub max_restarts: u32,
    /// Override `DSH_HOME` (default: `%USERPROFILE%\.dsh`).
    pub dsh_home: Option<PathBuf>,
    /// Extra directories prepended to the child `PATH` (so dsh finds git/pwsh).
    pub extra_path: Vec<PathBuf>,
    /// Extra `dsh web` flags (e.g. `--trusted-host`).
    pub extra_dsh_args: Vec<String>,
    /// Timeout for the readiness probe, in seconds.
    pub ready_timeout_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 0,
            close_to_tray: true,
            auto_restart: true,
            max_restarts: 5,
            dsh_home: None,
            extra_path: vec![],
            extra_dsh_args: vec![],
            ready_timeout_secs: 30,
        }
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let path = config_file();
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    #[allow(dead_code)]
    pub fn save(&self) -> anyhow::Result<()> {
        let path = config_file();
        std::fs::create_dir_all(path.parent().unwrap())?;
        Ok(std::fs::write(&path, serde_json::to_string_pretty(self)?)?)
    }
}

fn local_appdata() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Per-user install root (NSIS `installMode: currentUser`).
pub fn install_dir() -> PathBuf {
    local_appdata().join("Programs").join("DshDesktop")
}

/// Private runtime prefix: bundled node + `node_modules/@deepseek-ai/dsh`.
pub fn runtime_dir() -> PathBuf {
    install_dir().join("runtime")
}

/// App data (config, logs, state) — machine-local, non-roaming.
pub fn data_dir() -> PathBuf {
    local_appdata().join("DshDesktop")
}

pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

/// `dsh web` stdout/stderr capture.
pub fn dsh_log_file() -> PathBuf {
    log_dir().join("dsh-web.log")
}

/// The shell's own structured log.
#[allow(dead_code)]
pub fn shell_log_file() -> PathBuf {
    log_dir().join("shell.log")
}

pub fn config_file() -> PathBuf {
    data_dir().join("config.json")
}
