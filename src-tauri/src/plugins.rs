//! Local plugin installation: pick an offline plugin tarball (.tgz), install
//! it into the shared web profile via `dsh plugin`, then restart dsh web.
//!
//! Plugins are profile assets (shared with the web version), so upgrading is
//! the same flow: a newer tarball overwrites the old package, then dsh web
//! restarts and the new version is live.

use crate::app::{AppState, RunState};
use crate::{config, dsh_manager, lifecycle};
use std::path::Path;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

/// Tray entry point: native file dialog for a `.tgz`, then install + restart.
pub fn pick_and_install(app: AppHandle) {
    let handle = app.clone();
    app.dialog()
        .file()
        .add_filter("DSH 插件包", &["tgz"])
        .pick_file(move |file| {
            let Some(f) = file else { return };
            let Ok(path) = f.into_path() else { return };
            let handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                install_plugin(&handle, &path).await;
            });
        });
}

async fn install_plugin(app: &AppHandle, tgz: &Path) {
    // 1. Stage a copy under the shell's data dir (the picked file may move).
    let staging = config::data_dir().join("plugins");
    let _ = std::fs::create_dir_all(&staging);
    let file_name = tgz
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("plugin.tgz")
        .to_string();
    let staged = staging.join(&file_name);
    if let Err(e) = std::fs::copy(tgz, &staged) {
        let msg = format!("无法复制插件包: {e}");
        app.state::<AppState>()
            .push_progress_line(msg.clone())
            .await;
        app.state::<AppState>()
            .set_status(RunState::Starting, Some("插件安装失败（无法复制文件）".into()))
            .await;
        return;
    }

    // 2. Surface the progress window.
    app.state::<AppState>()
        .set_progress("plugin-install", format!("正在安装插件 {file_name}…"))
        .await;
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
    }

    // 3. Run `dsh plugin --profile web add <tgz>` with streamed output.
    let install = app.state::<AppState>().install.clone();
    let home = config::resolve_home(&app.state::<AppState>().config);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(line) = rx.recv().await {
            handle.state::<AppState>().push_progress_line(line).await;
        }
    });

    match dsh_manager::install_plugin_tarball(&install, &home, &staged, tx).await {
        Ok(pkg) => {
            app.state::<AppState>()
                .push_progress_line(format!("插件 {pkg} 安装成功，正在重启 DSH…"))
                .await;
            app.state::<AppState>()
                .set_progress("ready", format!("插件 {pkg} 安装成功，正在重启 DSH…"))
                .await;
            let _ = lifecycle::restart(app).await;
        }
        Err(e) => {
            let msg = format!("插件安装失败: {e:#}");
            app.state::<AppState>()
                .push_progress_line(msg.clone())
                .await;
            app.state::<AppState>()
                .set_status(RunState::Starting, Some("插件安装失败，可重试".into()))
                .await;
        }
    }
}
