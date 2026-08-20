//! Process supervision: the single owner of the dsh child, plus restart and
//! graceful-shutdown control. External control (restart/quit) addresses the
//! child by PID and the `/api/admin/shutdown` endpoint, never by a shared
//! handle, so the supervisor keeps sole ownership of `tokio::process::Child`.

use crate::app::{AppState, RunState};
use crate::{config, dsh_manager, health, process_supervisor};
use anyhow::{anyhow, Result};
use std::path::Path;
use tauri::{AppHandle, Manager};
use tokio::process::Command;
use std::time::Duration;

/// Run until shutdown. Owns the child for its whole life and restarts per policy.
///
/// Boot gate: the splash first fetches the registry version targets, then
/// WAITS for the user to pick a channel and click 确认启动 (`confirm_channel`)
/// before any version decision / install / spawn happens.
pub async fn supervise(app: &AppHandle) -> Result<()> {
    let install = app.state::<AppState>().install.clone();
    let config = app.state::<AppState>().config.clone();
    let token = app.state::<AppState>().shutdown_token.clone();
    let home = config::resolve_home(&config);

    // Clean up dsh web processes orphaned by a force-killed previous instance
    // (our npx cache path or npm-spec invocation only — never the web version's
    // own npx-installed dsh).
    cleanup_orphan_dsh_web().await;

    // --- Channel picker: fetch version targets, then wait for confirmation.
    // Nothing below starts dsh before the user confirms. ---
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
    }
    app.state::<AppState>()
        .set_progress("choose-channel", "正在获取版本信息…")
        .await;
    let remote = dsh_manager::remote_versions().await;
    {
        let state = app.state::<AppState>();
        *state.versions.lock().await = if remote.is_some() {
            crate::app::VersionState::Ready
        } else {
            crate::app::VersionState::Offline
        };
    }
    match &remote {
        Some(r) => tracing::info!(
            "registry targets: latest={} preview={}",
            r.latest,
            r.preview
        ),
        None => tracing::warn!("registry unreachable; picker shows offline"),
    }
    app.state::<AppState>()
        .set_progress("choose-channel", "请选择 DSH 更新通道后点击「确认启动」")
        .await;
    tracing::info!("waiting for channel confirmation…");

    // Wait for the user's confirmation (confirm_channel releases this).
    {
        let mut confirmed = app.state::<AppState>().confirmed.subscribe();
        while !*confirmed.borrow() {
            if app.state::<AppState>().is_shutting_down() {
                return Ok(());
            }
            confirmed.changed().await.ok();
        }
    }
    let channel = *app.state::<AppState>().channel.lock().await;
    tracing::info!("user confirmed channel: {channel:?}");

    // Decide the dsh version for the confirmed channel (compat gate).
    let target = decide_version(app, &home, channel, remote).await?;
    let version = target
        .clone()
        .unwrap_or_else(|| dsh_manager::current_version().unwrap_or_default());

    // Version gate: refuse to drive a dsh older than the contract floor
    // (docs/INTERFACE_CONTRACT.md §6) instead of misparsing its output.
    if !dsh_manager::version_at_least(&version, dsh_manager::MIN_DSH_VERSION) {
        return Err(anyhow!(
            "dsh 版本过低：需要 ≥ {}，当前 {}（请升级 dsh）",
            dsh_manager::MIN_DSH_VERSION,
            version
        ));
    }
    tracing::info!("dsh version {version} (min {})", dsh_manager::MIN_DSH_VERSION);

    // Ensure the ops overlay (`/api/health` + `/api/admin/shutdown`) is
    // resolvable from the web profile before spawning.
    if let Err(e) = dsh_manager::ensure_ops_overlay(&home) {
        tracing::warn!("ops overlay install failed; graceful shutdown will fall back to hard kill: {e:#}");
    }

    // Progress sink while an install/update is in flight.
    let mut progress_tx = if target.is_some() {
        Some(progress_sink(app))
    } else {
        None
    };

    // Cold install path: the npx `--version` probe has NO URL timeout, so a
    // minutes-long download is never killed by the web spawn's 30s ready gate;
    // the later spawn is a cache hit. Retried with backoff.
    let mut attempt: u32 = 0;
    if let Some(tx) = progress_tx.as_ref() {
        loop {
            if app.state::<AppState>().is_shutting_down() {
                break;
            }
            match install.ensure_installed(&version, Some(tx.clone())).await {
                Ok(()) => break,
                Err(e) => {
                    attempt += 1;
                    if attempt > config.max_restarts {
                        return Err(anyhow!("dsh {version} 安装失败: {e:#}"));
                    }
                    backoff(attempt).await;
                }
            }
        }
        progress_tx = None;
    }

    // Serve loop: spawn dsh web, keep it alive per restart policy. The channel
    // and version are fixed for the whole session (the picker is disabled once
    // confirmed; switching channels means restarting the app).
    loop {
        if app.state::<AppState>().is_shutting_down() {
            break;
        }

        app.state::<AppState>()
            .set_status(RunState::Starting, Some("正在启动 dsh web…".into()))
            .await;

        let server = match process_supervisor::spawn(
            &install,
            &version,
            &config,
            &token,
            progress_tx.clone(),
        )
        .await
        {
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

        // The booted version is the new baseline for the next startup.
        dsh_manager::write_current_version(&version);

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

/// Decide the dsh version to run for the confirmed channel, using the
/// picker's pre-fetched registry snapshot. Returns `None` when staying on
/// the current version (offline, compat-gated upgrade, or already current).
async fn decide_version(
    app: &AppHandle,
    home: &Path,
    channel: dsh_manager::Channel,
    remote: Option<dsh_manager::RemoteVersions>,
) -> Result<Option<String>> {
    let current = dsh_manager::current_version();

    let Some(remote) = remote else {
        // Registry unreachable: stay on the current version; without any
        // recorded version the first run cannot proceed offline.
        if current.is_none() {
            return Err(anyhow!(
                "无法连接 npm registry，且本机没有可用的 DSH 运行时（首次运行需要网络）"
            ));
        }
        app.state::<AppState>()
            .set_progress("checking", "无法连接 npm registry，继续使用当前版本")
            .await;
        return Ok(None);
    };

    let wanted = match channel {
        dsh_manager::Channel::Latest => remote.latest.clone(),
        dsh_manager::Channel::Preview => remote.preview.clone(),
    };
    tracing::info!(
        "channel {channel:?}: latest={} preview={} → wanted={wanted}",
        remote.latest,
        remote.preview
    );

    match &current {
        None => {
            // First run: must install (network required).
            app.state::<AppState>()
                .set_status(RunState::Starting, Some(format!("首次运行：安装 DSH {wanted}…").into()))
                .await;
            app.state::<AppState>()
                .set_progress("installing-dsh", format!("首次运行：安装 DSH {wanted}…"))
                .await;
            Ok(Some(wanted))
        }
        Some(cur) if *cur != wanted => {
            // Plugin-compatibility gate (R1): only upgrade when every profile
            // plugin's @deepseek-ai/* peerDeps stay satisfied.
            let deps = dsh_manager::dsh_dependencies(&wanted).await;
            let bad = match &deps {
                Some(d) => dsh_manager::plugin_incompatibilities(home, d).await,
                None => {
                    tracing::warn!("cannot resolve dsh@{wanted} deps; skipping upgrade");
                    Vec::new()
                }
            };
            if bad.is_empty() && deps.is_some() {
                app.state::<AppState>()
                    .set_status(RunState::Starting, Some(format!("正在更新 DSH {cur} → {wanted}…").into()))
                    .await;
                app.state::<AppState>()
                    .set_progress("updating-dsh", format!("正在更新 DSH {cur} → {wanted}…"))
                    .await;
                Ok(Some(wanted))
            } else if !bad.is_empty() {
                tracing::warn!("skip dsh upgrade to {wanted}: {}", bad.join("; "));
                app.state::<AppState>()
                    .set_progress("checking", format!("已跳过 DSH {wanted} 升级（插件兼容性）"))
                    .await;
                for line in &bad {
                    app.state::<AppState>().push_progress_line(line.clone()).await;
                }
                Ok(None)
            } else {
                Ok(None) // deps unknown → stay on the current version
            }
        }
        Some(_) => Ok(None), // already on the channel target
    }
}

/// Progress sink: surface npx's streamed output on the progress window during
/// an install/update. Dropping the returned sender ends the relay.
fn progress_sink(app: &AppHandle) -> tokio::sync::mpsc::UnboundedSender<String> {
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
    tx
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
/// Kill `node` processes left over from a force-killed previous shell
/// instance: any whose command line references our npx cache dir or the
/// npm-spec invocation `@deepseek-ai/dsh@`. Runs windowless via a
/// hidden-console powershell one-liner (never touches the web version's own
/// npx-installed dsh, which lives under the npm-cache `_npx` path).
async fn cleanup_orphan_dsh_web() {
    let npx_cache = config::data_dir().join("npx-cache");
    let ps = format!(
        "Get-CimInstance Win32_Process -Filter \"Name='node.exe'\" | Where-Object {{ $_.CommandLine -match '{}' -or $_.CommandLine -match '@deepseek-ai/dsh@' }} | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}",
        npx_cache.display()
    );
    let mut cmd = Command::new("powershell.exe");
    cmd.arg("-NoProfile")
        .arg("-WindowStyle")
        .arg("Hidden")
        .arg("-Command")
        .arg(ps);
    let _ = cmd.status().await;
}

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
