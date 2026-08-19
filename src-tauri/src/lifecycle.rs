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
    let home = config::resolve_home(&config);

    // --- dsh install / update (long op: shows the progress window) ---
    let installed = dsh_manager::installed_version(&install);
    let mut target: Option<String> = None;
    if !install.is_present() {
        app.state::<AppState>()
            .set_status(RunState::Starting, Some("首次运行：安装 DSH 运行时…".into()))
            .await;
        app.state::<AppState>()
            .set_progress("installing-dsh", "首次运行：安装 DSH 运行时…")
            .await;
        match dsh_manager::latest_dsh_version().await {
            Some(v) => target = Some(v),
            None => {
                return Err(anyhow!(
                    "无法连接 npm registry，且本机没有已安装的 DSH 运行时（首次运行需要网络）"
                ));
            }
        }
    } else if let Some(current) = &installed {
        if let Some(latest) = dsh_manager::latest_dsh_version().await {
            if latest != *current {
                // Plugin-compatibility gate (R1): only upgrade when every
                // profile plugin's @deepseek-ai/* peerDeps stay satisfied.
                let deps = dsh_manager::dsh_dependencies(&latest).await;
                let bad = match &deps {
                    Some(d) => dsh_manager::plugin_incompatibilities(&home, d).await,
                    None => {
                        tracing::warn!("cannot resolve dsh@{latest} deps; skipping upgrade");
                        Vec::new()
                    }
                };
                if bad.is_empty() && deps.is_some() {
                    target = Some(latest.clone());
                    app.state::<AppState>()
                        .set_status(RunState::Starting, Some(format!("正在更新 DSH {current} → {latest}…").into()))
                        .await;
                    app.state::<AppState>()
                        .set_progress("updating-dsh", format!("正在更新 DSH {current} → {latest}…"))
                        .await;
                } else if !bad.is_empty() {
                    tracing::warn!("skip dsh upgrade to {latest}: {}", bad.join("; "));
                    app.state::<AppState>()
                        .set_progress("checking", format!("已跳过 DSH {latest} 升级（插件兼容性）"))
                        .await;
                    for line in &bad {
                        app.state::<AppState>().push_progress_line(line.clone()).await;
                    }
                }
            }
        }
        // registry unreachable: stay on the installed version (offline path)
    }

    if let Some(version) = target {
        // Long install: surface the progress window (splash page) and stream.
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.show();
        }
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(line) = rx.recv().await {
                handle.state::<AppState>().push_progress_line(line).await;
            }
        });
        if let Err(e) = dsh_manager::ensure_dsh(&install, &version, tx).await {
            let msg = format!("DSH 运行时安装失败: {e:#}");
            app.state::<AppState>()
                .set_status(RunState::Error, Some(msg.clone()))
                .await;
            return Err(anyhow!(msg));
        }
        app.state::<AppState>()
            .push_progress_line(format!("DSH 运行时就绪 (@deepseek-ai/dsh@{version})"))
            .await;
        app.state::<AppState>()
            .set_progress("ready", "DSH 运行时就绪")
            .await;
    }

    // Version gate: refuse to drive a dsh older than the contract floor
    // (docs/INTERFACE_CONTRACT.md §6) instead of misparsing its output.
    let installed_ver = dsh_manager::installed_version(&install).unwrap_or_default();
    if !dsh_manager::version_at_least(&installed_ver, dsh_manager::MIN_DSH_VERSION) {
        return Err(anyhow!(
            "dsh 版本过低：需要 ≥ {}，当前 {}（请运行 scripts/ensure-dsh.ps1 升级）",
            dsh_manager::MIN_DSH_VERSION,
            installed_ver
        ));
    }
    tracing::info!("dsh version {installed_ver} (min {})", dsh_manager::MIN_DSH_VERSION);

    // Ensure the ops overlay (`/api/health` + `/api/admin/shutdown`) is
    // resolvable from the web profile before spawning.
    if let Err(e) = dsh_manager::ensure_ops_overlay(&home) {
        tracing::warn!("ops overlay install failed; graceful shutdown will fall back to hard kill: {e:#}");
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
