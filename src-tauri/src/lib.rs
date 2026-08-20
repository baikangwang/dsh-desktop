pub mod app;
pub mod commands;
pub mod config;
pub mod dsh_manager;
pub mod health;
pub mod lifecycle;
pub mod plugins;
pub mod process_supervisor;
pub mod tray;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Second launch focuses the existing window instead of starting a new one.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        // Close-to-tray: the X button hides the window; the tray "退出" quits.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| app::setup(app.handle()))
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::get_boot_progress,
            commands::confirm_channel,
            commands::restart_dsh,
            commands::open_logs,
            commands::quit
        ])
        .run(tauri::generate_context!())
        .expect("error while running DshDesktop");
}
