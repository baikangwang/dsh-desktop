//! Application state and startup wiring.

use crate::config::Config;
use crate::dsh_manager::DshInstall;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

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

pub struct AppState {
    pub config: Config,
    pub install: DshInstall,
    pub shutdown_token: String,
    pub status: Mutex<StatusSnapshot>,
    /// PID of the current `dsh web` child (set by the supervisor; used by
    /// restart/shutdown, which control the child by PID rather than handle).
    pub server_pid: Mutex<Option<u32>>,
    /// Set by `shutdown`; the supervisor stops restarting and winds down.
    pub shutting_down: AtomicBool,
    /// Set by `restart`; forces a restart even when `auto_restart` is off.
    pub force_restart: AtomicBool,
}

impl AppState {
    pub async fn set_status(&self, state: RunState, message: Option<String>) {
        let mut s = self.status.lock().await;
        s.state = state;
        s.message = message;
    }

    pub async fn snapshot(&self) -> StatusSnapshot {
        self.status.lock().await.clone()
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    pub fn take_force_restart(&self) -> bool {
        self.force_restart.swap(false, Ordering::SeqCst)
    }
}

pub fn setup(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .try_init();

    let config = Config::load().unwrap_or_default();
    let install = DshInstall::resolve()?;

    app.manage(AppState {
        config,
        install,
        shutdown_token: crate::dsh_manager::fresh_token(),
        status: Mutex::new(StatusSnapshot::default()),
        server_pid: Mutex::new(None),
        shutting_down: AtomicBool::new(false),
        force_restart: AtomicBool::new(false),
    });

    crate::tray::build(app)?;

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
