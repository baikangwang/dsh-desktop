//! WebView <-> Rust IPC (minimal, lifecycle-only; no arbitrary fs/shell).

use crate::app::{AppState, StatusSnapshot};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub async fn get_status(state: State<'_, AppState>) -> Result<StatusSnapshot, String> {
    Ok(state.snapshot().await)
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
