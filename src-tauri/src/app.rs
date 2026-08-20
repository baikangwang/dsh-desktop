//! Application state and startup wiring.

use crate::config::Config;
use crate::dsh_manager::{Channel, DshInstall};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;
use tracing_subscriber::prelude::*;
use tracing_subscriber::Layer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunState {
    Idle,
    Starting,
    Ready,
    Degraded,
    Error,
    Stopped,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusSnapshot {
    pub state: RunState,
    pub port: Option<u16>,
    pub url: Option<String>,
    pub pid: Option<u32>,
    pub message: Option<String>,
}

impl Default for StatusSnapshot {
    fn default() -> Self {
        Self { state: RunState::Idle, port: None, url: None, pid: None, message: None }
    }
}

/// Whether the picker's version lookup finished (drives option enablement).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum VersionState {
    /// The shell is fetching the registry targets (options disabled).
    #[default]
    Fetching,
    /// Targets known (`latest_version`/`preview_version` populated).
    Ready,
    /// Registry unreachable; proceed with the locally installed version.
    Offline,
}

/// Long-running boot / plugin-install progress surfaced to the splash window.
#[derive(Debug, Clone, Serialize, Default)]
pub struct BootProgress {
    /// One of: "choose-channel" | "checking" | "installing-dsh" | "updating-dsh" | "plugin-install" | "ready".
    pub phase: String,
    pub message: String,
    /// Last N streamed lines (npm/pnpm output).
    pub lines: Vec<String>,
    /// Update channel currently in effect (drives the splash picker).
    pub channel: Channel,
    /// Version the shell last ran successfully (if any).
    pub current_version: Option<String>,
    /// Channel targets from the remote lookup, for the picker labels.
    pub latest_version: Option<String>,
    pub preview_version: Option<String>,
    /// Registry lookup state (options enabled only when Ready/Offline).
    pub versions: VersionState,
}

pub struct AppState {
    pub config: Config,
    pub install: DshInstall,
    pub shutdown_token: String,
    pub status: Mutex<StatusSnapshot>,
    pub progress: Mutex<BootProgress>,
    /// Update channel the user confirmed at startup (`confirm_channel`).
    pub channel: Mutex<Channel>,
    /// Set true by `confirm_channel`; the supervisor waits for it before
    /// deciding/installing/spawning dsh.
    pub confirmed: tokio::sync::watch::Sender<bool>,
    /// Registry lookup state for the picker (fetching → ready/offline).
    pub versions: Mutex<VersionState>,
    /// PID of the current `dsh web` child (set by the supervisor; used by
    /// restart/shutdown, which control the child by PID rather than handle).
    pub server_pid: Mutex<Option<u32>>,
    /// Set by `shutdown`; the supervisor stops restarting and winds down.
    pub shutting_down: AtomicBool,
    /// Set by `restart`; forces a restart even when `auto_restart` is off.
    pub force_restart: AtomicBool,
}

const PROGRESS_MAX_LINES: usize = 200;

impl AppState {
    pub async fn set_status(&self, state: RunState, message: Option<String>) {
        let mut s = self.status.lock().await;
        s.state = state;
        s.message = message;
    }

    pub async fn snapshot(&self) -> StatusSnapshot {
        self.status.lock().await.clone()
    }

    pub async fn set_progress(&self, phase: &str, message: impl Into<String>) {
        let mut p = self.progress.lock().await;
        p.phase = phase.to_string();
        p.message = message.into();
    }

    pub async fn push_progress_line(&self, line: String) {
        let mut p = self.progress.lock().await;
        let over = p.lines.len().saturating_sub(PROGRESS_MAX_LINES - 1);
        if over > 0 {
            p.lines.drain(0..over);
        }
        p.lines.push(line);
    }

    pub async fn progress_snapshot(&self) -> BootProgress {
        let mut p = self.progress.lock().await.clone();
        p.channel = *self.channel.lock().await;
        p.versions = *self.versions.lock().await;
        p.current_version = crate::dsh_manager::current_version();
        if let Some(r) = crate::dsh_manager::read_remote_cache() {
            p.latest_version = Some(r.latest);
            p.preview_version = Some(r.preview);
        }
        p
    }

    pub fn is_confirmed(&self) -> bool {
        *self.confirmed.borrow()
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    pub fn take_force_restart(&self) -> bool {
        self.force_restart.swap(false, Ordering::SeqCst)
    }
}

pub fn setup(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info".into());
    let stdout_layer = tracing_subscriber::fmt::layer().with_filter(filter.clone());
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(LogFileWriter)
        .with_ansi(false)
        .with_filter(filter);
    let _ = tracing_subscriber::registry()
        .with(stdout_layer)
        .with(file_layer)
        .try_init();

    if let Ok(dir) = app.path().resource_dir() {
        crate::config::set_resource_dir(dir);
    }

    let config = Config::load().unwrap_or_default();
    let install = DshInstall::resolve()?;
    let (confirmed, _) = tokio::sync::watch::channel(false);

    app.manage(AppState {
        config,
        install,
        shutdown_token: crate::dsh_manager::fresh_token(),
        status: Mutex::new(StatusSnapshot::default()),
        progress: Mutex::new(BootProgress::default()),
        channel: Mutex::new(crate::dsh_manager::read_channel()),
        confirmed,
        versions: Mutex::new(VersionState::Fetching),
        server_pid: Mutex::new(None),
        shutting_down: AtomicBool::new(false),
        force_restart: AtomicBool::new(false),
    });

    crate::tray::build(app)?;

    // Tray status color watcher: reflect RunState changes on the tray icon;
    // once Ready, also refresh the menu's dsh version line (post-upgrade).
    {
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut last: Option<RunState> = None;
            loop {
                let state = handle.state::<AppState>().status.lock().await.state;
                if last != Some(state) {
                    last = Some(state);
                    crate::tray::update_status_color(&handle, state);
                    if state == RunState::Ready {
                        crate::tray::refresh_versions(&handle);
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        });
    }

    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = crate::lifecycle::supervise(&handle).await {
            tracing::error!("supervisor stopped: {e:#}");
            let _ = handle
                .state::<AppState>()
                .set_status(RunState::Error, Some(e.to_string()))
                .await;
            show_offline(&handle);
        }
    });

    Ok(())
}

/// Point the window at the bundled offline page (same origin as the splash).
pub fn show_offline(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if let Ok(current) = w.url() {
            if let Ok(offline) = current.join("offline.html") {
                let _ = w.navigate(offline);
            }
        }
        let _ = w.show();
    }
}

/// Tracing writer for the shell's own log (`%LOCALAPPDATA%\DshDesktop\logs\shell.log`),
/// with size-based rotation (see `config::rotate_if_large`).
struct LogFileWriter;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogFileWriter {
    type Writer = std::fs::File;
    fn make_writer(&'a self) -> Self::Writer {
        let path = crate::config::shell_log_file();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        crate::config::rotate_if_large(&path);
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap_or_else(|_| {
                // Extremely unlikely after create_dir_all; fall back to a
                // throwaway file so logging never panics the app.
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(std::env::temp_dir().join("dsh-shell.log"))
                    .expect("temp dir writable")
            })
    }
}
