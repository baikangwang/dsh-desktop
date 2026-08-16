//! Process supervision: the single owner of the dsh child, plus restart and
//! graceful-shutdown control. External control (restart/quit) addresses the
//! child by PID and the `/api/admin/shutdown` endpoint, never by a shared
//! handle, so the supervisor keeps sole ownership of `tokio::process::Child`.

use crate::app::{AppState, RunState};
use crate::{config, dsh_manager, health, process_supervisor};
use anyhow::{anyhow, Result};
use tauri::{AppHandle, Manager};
use tokio::process::Command;
use std::time::Duration;

/// Run until shutdown. Owns the child for its whole life and restarts per policy.
pub async fn supervise(app: &AppHandle) -> Result<()> {
    let install = app.state::<AppState>().install.clone();
    let config = app.state::<AppState>().config.clone();
    let token = app.state::<AppState>().shutdown_token.clone();

    if !install.is_present() {
        app.state::<AppState>()
            .set_status(RunState::Starting, Some("首次运行：正在安装 DSH 运行时…".into()))
            .await;
        dsh_manager::ensure_installed(&install).await?;
    }

    let mut attempt: u32 = 0;
    loop {
        if app.state::<AppState>().is_shutting_down() {
            break;
        }

        app.state::<AppState>()
            .set_status(RunState::Starting, Some("正在启动 dsh web…".into()))
            .await;

        let server = match process_supervisor::spawn(&install, &config, &token).await {
            Ok(s) => s,
            Err(e) => {
                attempt += 1;
                if attempt > config.max_restarts {
                    return Err(anyhow!("dsh web 启动连续失败: {e:#}"));
                }
                backoff(attempt).await;
                continue;
            }
        };

        // Ready gate + navigate.
        let url = server.url.clone();
        let port = server.port;
        let pid = server.child.id();

        if !health::wait_ready("127.0.0.1", port, Duration::from_secs(config.ready_timeout_secs)).await {
            tracing::warn!("health probe timed out on port {port}; killing");
            let _ = kill_tree(pid).await;
            attempt += 1;
            if attempt > config.max_restarts {
                return Err(anyhow!("dsh web 绑定端口 {port} 但健康探测持续超时"));
            }
            backoff(attempt).await;
            continue;
        }

        if let Some(w) = app.get_webview_window("main") {
            let _ = w.navigate(url.parse::<tauri::Url>()?);
            let _ = w.show();
            let _ = w.set_focus();
        }

        {
            let state = app.state::<AppState>();
            *state.server_pid.lock().await = pid;
            let mut snap = state.status.lock().await;
            snap.state = RunState::Ready;
            snap.port = Some(port);
            snap.url = Some(url);
            snap.pid = pid;
            snap.message = Some(format!("运行中 · 端口 {port}"));
        }
        attempt = 0;

        // Block until the child exits (sole owner of `child`).
        let mut child = server.child;
        let code = child.wait().await.ok();
        tracing::warn!("dsh web exited (code: {code:?})");

        *app.state::<AppState>().server_pid.lock().await = None;

        if app.state::<AppState>().is_shutting_down() {
            break;
        }

        let should_restart = config.auto_restart || app.state::<AppState>().take_force_restart();
        if !should_restart {
            app.state::<AppState>()
                .set_status(RunState::Stopped, Some("服务已停止".into()))
                .await;
            break;
        }

        app.state::<AppState>()
            .set_status(RunState::Degraded, Some("服务已退出，正在重启…".into()))
            .await;
        attempt += 1;
        if attempt > config.max_restarts {
            return Err(anyhow!("连续重启失败，达到上限"));
        }
        backoff(attempt).await;
    }

    app.state::<AppState>()
        .set_status(RunState::Stopped, Some("已退出".into()))
        .await;
    Ok(())
}

/// Ask the supervisor to restart (used by tray/IPC). Kills the current child;
/// the supervisor restarts it because `force_restart` is set.
pub async fn restart(app: &AppHandle) -> Result<()> {
    app.state::<AppState>()
        .force_restart
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let pid = *app.state::<AppState>().server_pid.lock().await;
    if let Some(pid) = pid {
        let _ = kill_tree(Some(pid)).await;
    }
    Ok(())
}

/// Graceful shutdown: POST /api/admin/shutdown, then hard-kill the tree as a
/// fallback. Called before `app.exit(0)` so no orphaned `dsh web` survives.
pub async fn shutdown(app: &AppHandle) {
    let (port, token) = {
        let state = app.state::<AppState>();
        let snap = state.status.lock().await;
        (snap.port, state.shutdown_token.clone())
    };
    app.state::<AppState>()
        .shutting_down
        .store(true, std::sync::atomic::Ordering::SeqCst);

    if let Some(port) = port {
        graceful_shutdown(port, &token);
        // brief grace for the dispose, then ensure the tree is gone
        tokio::time::sleep(Duration::from_millis(800)).await;
    }
    let pid = *app.state::<AppState>().server_pid.lock().await;
    let _ = kill_tree(pid).await;
}

/// Minimal HTTP POST (dependency-free) to the DSH-side shutdown endpoint.
fn graceful_shutdown(port: u16, token: &str) -> bool {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let req = format!(
        "POST /api/admin/shutdown HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Authorization: Bearer {token}\r\n\
         Content-Length: 0\r\n\
         Connection: close\r\n\r\n"
    );
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 512];
    let _ = stream.read(&mut buf);
    buf.starts_with(b"HTTP/1.1 202")
}

/// Kill a process tree by PID (Windows `taskkill /T /F`).
async fn kill_tree(pid: Option<u32>) -> Result<()> {
    let Some(pid) = pid else { return Ok(()) };
    let status = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status()
        .await?;
    tracing::info!("taskkill /PID {pid} -> {status}");
    Ok(())
}

async fn backoff(attempt: u32) {
    let secs = 1u64 << attempt.min(5); // 1,2,4,8,16,32 (cap)
    tokio::time::sleep(Duration::from_secs(secs)).await;
}
