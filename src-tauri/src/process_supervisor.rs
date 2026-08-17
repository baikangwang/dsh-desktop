//! Spawn `dsh web` and resolve its port from the printed URL line.

use crate::config;
use crate::dsh_manager::DshInstall;
use crate::config::Config;
use anyhow::{anyhow, Context, Result};
use regex::Regex;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;

pub struct SpawnedServer {
    pub child: Child,
    pub port: u16,
    pub url: String,
}

/// Matches `dsh web: http://127.0.0.1:<port>` — the single stable contract
/// between the shell and DSH (see docs/INTERFACE_CONTRACT.md).
fn url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"dsh web:\s+(http://127\.0\.0\.1:(\d+))").unwrap())
}

pub async fn spawn(install: &DshInstall, config: &Config, token: &str) -> Result<SpawnedServer> {
    let mut cmd = install.command(config.port, token, config);
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().context("failed to spawn dsh web")?;

    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout pipe"))?;
    let stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr pipe"))?;

    // Drain stderr to the log for post-mortem diagnosis (fire-and-forget).
    tokio::spawn(drain_stderr(stderr));

    let port = read_port_from_stdout(stdout, Duration::from_secs(config.ready_timeout_secs)).await?;

    Ok(SpawnedServer {
        url: format!("http://127.0.0.1:{port}"),
        port,
        child,
    })
}

async fn read_port_from_stdout(
    stdout: tokio::process::ChildStdout,
    timeout: Duration,
) -> Result<u16> {
    let mut lines = BufReader::new(stdout).lines();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(anyhow!("timed out waiting for dsh web URL line"));
        }
        let line = tokio::time::timeout(remaining, lines.next_line())
            .await
            .map_err(|_| anyhow!("timed out waiting for dsh web URL line"))?
            .map_err(|e| anyhow!("failed to read dsh web stdout: {e}"))?;
        let Some(line) = line else { continue };
        if let Some(caps) = url_re().captures(&line) {
            return caps
                .get(2)
                .unwrap()
                .as_str()
                .parse::<u16>()
                .context("bad port in URL line");
        }
        tracing::debug!("dsh web: {line}");
    }
}

async fn drain_stderr(stderr: tokio::process::ChildStderr) {
    use std::io::Write;
    let path = config::dsh_log_file();
    // Ensure the logs dir exists (OpenOptions::create only creates the file).
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    config::rotate_if_large(&path);
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!("dsh stderr: {line}");
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| writeln!(f, "{line}"));
    }
}
