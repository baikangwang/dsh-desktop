//! System tray: status icon (color-coded by run state) + menu.

use crate::app::RunState;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager,
};

/// State -> color mapping (黄=starting / 绿=ready / 橙=degraded / 红=error / 灰=idle|stopped).
fn state_color(state: RunState) -> [u8; 3] {
    match state {
        RunState::Starting => [0xF0, 0xB4, 0x2E], // yellow
        RunState::Ready => [0x3F, 0xB9, 0x50],     // green
        RunState::Degraded => [0xF0, 0x8C, 0x3C],  // orange
        RunState::Error => [0xE5, 0x4B, 0x3F],     // red
        RunState::Idle | RunState::Stopped => [0x8B, 0x94, 0x9E], // gray
    }
}

/// Render a 32x32 RGBA tray icon: a filled circle on a transparent background.
fn status_icon(color: [u8; 3]) -> Image<'static> {
    const SIZE: u32 = 32;
    let mut px = vec![0u8; (SIZE * SIZE * 4) as usize];
    let c = (SIZE as f32 - 1.0) / 2.0;
    let r = SIZE as f32 / 2.0 - 1.0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - c;
            let dy = y as f32 - c;
            let i = ((y * SIZE + x) * 4) as usize;
            if dx * dx + dy * dy <= r * r {
                px[i] = color[0];
                px[i + 1] = color[1];
                px[i + 2] = color[2];
                px[i + 3] = 255;
            }
            // else: transparent
        }
    }
    Image::new_owned(px, SIZE, SIZE)
}

/// Push a color-coded icon for `state` onto the tray (no-op before build).
pub fn update_status_color(app: &AppHandle, state: RunState) {
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_icon(Some(status_icon(state_color(state))));
        let label = match state {
            RunState::Starting => "DeepSeek Harness · 启动中",
            RunState::Ready => "DeepSeek Harness · 运行中",
            RunState::Degraded => "DeepSeek Harness · 服务已退出，重启中",
            RunState::Error => "DeepSeek Harness · 错误",
            RunState::Idle => "DeepSeek Harness",
            RunState::Stopped => "DeepSeek Harness · 已停止",
        };
        let _ = tray.set_tooltip(Some(label));
    }
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "打开", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, "restart", "重启服务", true, None::<&str>)?;
    let logs = MenuItem::with_id(app, "logs", "打开日志", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &restart, &logs, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("DeepSeek Harness")
        .on_menu_event(|app, event| {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                match event.id().as_ref() {
                    "open" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "restart" => {
                        let _ = crate::lifecycle::restart(&app).await;
                    }
                    "logs" => {
                        let _ = crate::commands::open_logs(app.clone()).await;
                    }
                    "quit" => {
                        crate::lifecycle::shutdown(&app).await;
                        app.exit(0);
                    }
                    _ => {}
                }
            });
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}
