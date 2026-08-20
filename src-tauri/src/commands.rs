//! WebView <-> Rust IPC (minimal, lifecycle-only; no arbitrary fs/shell).

use crate::app::{AppState, BootProgress, StatusSnapshot};
use crate::dsh_manager::Channel;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub async fn get_status(state: State<'_, AppState>) -> Result<StatusSnapshot, String> {
    Ok(state.snapshot().await)
}

/// Progress for the splash window during dsh install/update or plugin install.
#[tauri::command]
pub async fn get_boot_progress(state: State<'_, AppState>) -> Result<BootProgress, String> {
    Ok(state.progress_snapshot().await)
}

/// Confirm the update channel on the splash: persists the choice and releases
/// the supervisor, which then decides/installs/spawns dsh. The picker is
/// disabled afterwards, so this fires at most once per launch (idempotent).
#[tauri::command]
pub async fn confirm_channel(app: AppHandle, channel: Channel) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.is_confirmed() {
        return Ok(());
    }
    crate::dsh_manager::persist_channel(channel);
    *state.channel.lock().await = channel;
    state
        .confirmed
        .send(true)
        .map_err(|e| format!("confirm channel failed: {e}"))?;
    tracing::info!("update channel confirmed: {channel:?}");
    Ok(())
}

#[tauri::command]
pub async fn restart_dsh(app: AppHandle) -> Result<(), String> {
    crate::lifecycle::restart(&app).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_logs(app: AppHandle) -> Result<(), String> {
    let path = crate::config::dsh_log_file();
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn quit(app: AppHandle) -> Result<(), String> {
    crate::lifecycle::shutdown(&app).await;
    app.exit(0);
    Ok(())
}
