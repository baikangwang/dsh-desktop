//! Locate / run DSH via npx (npm exec) against a dedicated cache.
//!
//! The shell NEVER bundles DSH. On startup it resolves the latest version
//! (cached for 6h), gates upgrades on plugin compatibility, then spawns
//!   node <npx-cli> --cache <dedicated> -y @deepseek-ai/dsh@<target> web …
//! npx installs newer versions on demand (long op, streamed to the progress
//! window) and reuses its cache otherwise (fast, no network).

use crate::config;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Minimum dsh version the shell supports (interface contract C1–C3 + routes).
/// This is a compatibility FLOOR: the shell runs the latest registry version
/// but refuses to run anything below this.
pub const MIN_DSH_VERSION: &str = "0.1.0-rc.6";

// ---------------------------------------------------------------------------
// Update channel (user-selectable at startup; persisted)
// ---------------------------------------------------------------------------

/// Which dsh version the shell should run:
/// - `latest`  — follows the official npm `latest` dist-tag (the promoted,
///   stable release; may lag freshly published prereleases).
/// - `preview` — tracks the highest version number on the registry (includes
///   prereleases, e.g. versions published under the `next` dist-tag).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Channel {
    #[default]
    Latest,
    Preview,
}

impl Channel {
    pub fn label(self) -> &'static str {
        match self {
            Channel::Latest => "稳定版 (latest)",
            Channel::Preview => "预览版 (最高版本号)",
        }
    }
}

fn channel_path() -> PathBuf {
    config::data_dir().join("cache").join("dsh-channel.json")
}

/// Persisted channel choice (missing/invalid file → `Latest`).
pub fn read_channel() -> Channel {
    let text = std::fs::read_to_string(channel_path()).ok();
    let json: Option<serde_json::Value> =
        text.and_then(|t| serde_json::from_str(&t).ok());
    let Some(json) = json else {
        return Channel::Latest;
    };
    match json.get("channel").and_then(|c| c.as_str()) {
        Some("preview") => Channel::Preview,
        _ => Channel::Latest,
    }
}

pub fn persist_channel(channel: Channel) {
    let path = channel_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let name = match channel {
        Channel::Latest => "latest",
        Channel::Preview => "preview",
    };
    let json = serde_json::json!({ "channel": name });
    let _ = std::fs::write(path, serde_json::to_string(&json).unwrap_or_default());
}

// ---------------------------------------------------------------------------
// Version helpers (semver-lite with prerelease support)
// ---------------------------------------------------------------------------

/// Version of the dsh the shell last ran successfully (state file).
pub fn current_version() -> Option<String> {
    let text = std::fs::read_to_string(current_version_path()).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("version")?.as_str().map(str::to_owned)
}

/// Record the version we just booted, so the next startup knows the baseline.
pub fn write_current_version(version: &str) {
    let path = current_version_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json = serde_json::json!({ "version": version });
    let _ = std::fs::write(path, serde_json::to_string(&json).unwrap_or_default());
}

fn current_version_path() -> PathBuf {
    config::data_dir().join("cache").join("dsh-current.json")
}

fn split_ver(s: &str) -> (Vec<u64>, Option<&str>) {
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (s, None),
    };
    let nums = core.split('.').map(|n| n.parse().unwrap_or(0)).collect();
    (nums, pre)
}

fn cmp_numeric(a: &[u64], b: &[u64]) -> Ordering {
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            Ordering::Equal => continue,
            o => return o,
        }
    }
    Ordering::Equal
}

fn cmp_prerelease(a: &str, b: &str) -> Ordering {
    let sa: Vec<&str> = a.split('.').collect();
    let sb: Vec<&str> = b.split('.').collect();
    for i in 0..sa.len().max(sb.len()) {
        let x = sa.get(i).copied().unwrap_or("");
        let y = sb.get(i).copied().unwrap_or("");
        if x == y {
            continue;
        }
        if x.is_empty() {
            return Ordering::Less;
        }
        if y.is_empty() {
            return Ordering::Greater;
        }
        let xn = x.parse::<u64>().ok();
        let yn = y.parse::<u64>().ok();
        return match (xn, yn) {
            (Some(a), Some(b)) => a.cmp(&b),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => x.cmp(y),
        };
    }
    Ordering::Equal
}

/// Full semver-lite comparison: numeric core, then prerelease.
pub fn version_cmp(a: &str, b: &str) -> Ordering {
    let (a_core, a_pre) = split_ver(a);
    let (b_core, b_pre) = split_ver(b);
    let core = cmp_numeric(&a_core, &b_core);
    if core != Ordering::Equal {
        return core;
    }
    match (a_pre, b_pre) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => cmp_prerelease(x, y),
    }
}

/// `installed >= minimum`.
pub fn version_at_least(installed: &str, minimum: &str) -> bool {
    version_cmp(installed, minimum) != Ordering::Less
}

fn caret_bounds(ver: &str) -> (String, String) {
    let (nums, _) = split_ver(ver);
    let major = nums.get(0).copied().unwrap_or(0);
    let minor = nums.get(1).copied().unwrap_or(0);
    let patch = nums.get(2).copied().unwrap_or(0);
    let upper = if major > 0 {
        (major + 1, 0u64, 0u64)
    } else if minor > 0 {
        (0, minor + 1, 0)
    } else {
        (0, 0, patch + 1)
    };
    (ver.to_string(), format!("{}.{}.{}-0", upper.0, upper.1, upper.2))
}

fn tilde_bounds(ver: &str) -> (String, String) {
    let (nums, _) = split_ver(ver);
    let major = nums.get(0).copied().unwrap_or(0);
    let minor = nums.get(1).copied().unwrap_or(0);
    (ver.to_string(), format!("{}.{}.0-0", major, minor + 1))
}

/// Whether `version` satisfies an npm-style range string.
pub fn range_satisfied(version: &str, range: &str) -> bool {
    for token in range.split_whitespace() {
        let (op, ver) = if let Some(v) = token.strip_prefix(">=") {
            (">=", v)
        } else if let Some(v) = token.strip_prefix("<=") {
            ("<=", v)
        } else if let Some(v) = token.strip_prefix('>') {
            (">", v)
        } else if let Some(v) = token.strip_prefix('<') {
            ("<", v)
        } else if let Some(v) = token.strip_prefix('^') {
            ("^", v)
        } else if let Some(v) = token.strip_prefix('~') {
            ("~", v)
        } else if let Some(v) = token.strip_prefix('=') {
            ("=", v)
        } else {
            ("=", token)
        };
        let ok = match op {
            ">=" => version_cmp(version, ver) != Ordering::Less,
            "<=" => version_cmp(version, ver) != Ordering::Greater,
            ">" => version_cmp(version, ver) == Ordering::Greater,
            "<" => version_cmp(version, ver) == Ordering::Less,
            "=" => version_cmp(version, ver) == Ordering::Equal,
            "^" => {
                let (lo, hi) = caret_bounds(ver);
                version_cmp(version, &lo) != Ordering::Less && version_cmp(version, &hi) == Ordering::Less
            }
            "~" => {
                let (lo, hi) = tilde_bounds(ver);
                version_cmp(version, &lo) != Ordering::Less && version_cmp(version, &hi) == Ordering::Less
            }
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Registry helpers (npm view via node, no console window)
// ---------------------------------------------------------------------------

fn find_on_path() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "node.exe" } else { "node" };
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(exe))
            .find(|p| p.is_file())
    })
}

fn npm_cli_path() -> Option<PathBuf> {
    let node = find_on_path()?;
    let cli = node
        .parent()?
        .join("node_modules")
        .join("npm")
        .join("bin")
        .join("npm-cli.js");
    cli.is_file().then_some(cli)
}

/// Run `npm view <args> --json` with a timeout. None on failure (offline etc.).
async fn npm_view(args: &[&str]) -> Option<String> {
    let node = find_on_path()?;
    let cli = npm_cli_path()?;
    let mut cmd = Command::new(node);
    #[cfg(windows)]
    {
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(45),
        cmd.arg(&cli)
            .args(["view", "--json", "--no-audit", "--no-fund", "--loglevel", "error"])
            .args(args)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

/// Both channel targets in one registry snapshot (`latest` dist-tag + highest
/// published version), so the startup check is a single cached npm view.
#[derive(Debug, Clone)]
pub struct RemoteVersions {
    pub latest: String,
    pub preview: String,
}

/// TTL for the cached remote-version lookup, so most startups never touch
/// the network.
const REMOTE_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

fn remote_cache_path() -> PathBuf {
    config::data_dir().join("cache").join("dsh-remote.json")
}

/// Fresh cached remote versions (None when stale/missing → caller re-fetches).
pub fn read_remote_cache() -> Option<RemoteVersions> {
    let text = std::fs::read_to_string(remote_cache_path()).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let age = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs()
        .saturating_sub(json.get("ts")?.as_u64()?);
    if age > REMOTE_CACHE_TTL.as_secs() {
        return None;
    }
    Some(RemoteVersions {
        latest: json.get("latest")?.as_str()?.to_owned(),
        preview: json.get("preview")?.as_str()?.to_owned(),
    })
}

fn write_remote_cache(latest: &str, preview: &str) {
    let path = remote_cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let json = serde_json::json!({ "latest": latest, "preview": preview, "ts": ts });
    let _ = std::fs::write(path, serde_json::to_string(&json).unwrap_or_default());
}

/// Whether a fresh remote-version cache exists (startup check is instant).
pub fn remote_cache_fresh() -> bool {
    read_remote_cache().is_some()
}

/// Latest `@deepseek-ai/dsh` registry snapshot (cached; stale fallback).
/// One `npm view <pkg> dist-tags versions` call yields both targets:
/// `latest` = the official `latest` dist-tag (fallback: max version),
/// `preview` = max of all published versions (prereleases included).
pub async fn remote_versions() -> Option<RemoteVersions> {
    if let Some(cached) = read_remote_cache() {
        return Some(cached);
    }
    let json = npm_view(&["@deepseek-ai/dsh", "dist-tags", "versions"]).await?;
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    let tags = v.get("dist-tags").and_then(|t| t.as_object());
    let versions = v.get("versions").and_then(|x| x.as_array());
    let max = versions.and_then(|arr| {
        arr.iter()
            .filter_map(|x| x.as_str())
            .max_by(|a, b| version_cmp(a, b))
            .map(str::to_owned)
    });
    let latest = tags
        .and_then(|t| t.get("latest"))
        .and_then(|s| s.as_str())
        .map(str::to_owned)
        .or_else(|| max.clone())?;
    let preview = max.unwrap_or_else(|| latest.clone());
    write_remote_cache(&latest, &preview);
    Some(RemoteVersions { latest, preview })
}

/// Declared dependency map of `@deepseek-ai/dsh@<version>` (name -> range).
pub async fn dsh_dependencies(version: &str) -> Option<HashMap<String, String>> {
    let arg = format!("@deepseek-ai/dsh@{version}");
    let json = npm_view(&[&arg, "dependencies"]).await?;
    serde_json::from_str::<HashMap<String, String>>(&json).ok()
}

/// Resolve the concrete version npm would install for `pkg@range`.
pub async fn resolve_version(pkg: &str, range: &str) -> Option<String> {
    let arg = format!("{pkg}@{range}");
    let json = npm_view(&[&arg, "version"]).await?;
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    match v {
        serde_json::Value::String(s) => Some(s),
        serde_json::Value::Array(arr) => arr
            .last()
            .and_then(|x| x.as_str().map(str::to_owned)),
        _ => None,
    }
}

/// Plugins in the shared web profile whose `@deepseek-ai/*` peerDependencies
/// would NOT be satisfied by the subpackage versions `dsh@<version>` provides.
pub async fn plugin_incompatibilities(
    home: &Path,
    dsh_deps: &HashMap<String, String>,
) -> Vec<String> {
    let mut out = Vec::new();
    let web_nm = home.join("profiles").join("web").join("node_modules");
    let Ok(entries) = std::fs::read_dir(&web_nm) else {
        return out;
    };
    let mut provided: HashMap<String, String> = HashMap::new();
    for entry in entries.flatten() {
        let pkg_json = entry.path().join("package.json");
        let Ok(text) = std::fs::read_to_string(&pkg_json) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(peers) = json.get("peerDependencies").and_then(|v| v.as_object()) else {
            continue;
        };
        let name = json
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("plugin");
        for (dep, range) in peers {
            if !dep.starts_with("@deepseek-ai/") {
                continue;
            }
            let Some(dsh_range) = dsh_deps.get(dep) else {
                continue;
            };
            let ver = match provided.get(dep) {
                Some(v) => v.clone(),
                None => {
                    let Some(v) = resolve_version(dep, dsh_range).await else {
                        continue;
                    };
                    provided.insert(dep.clone(), v.clone());
                    v
                }
            };
            let range_str = range.as_str().unwrap_or("");
            if !range_satisfied(&ver, range_str) {
                out.push(format!(
                    "{name}: {dep} 需要 {range_str}，但 dsh 提供 {ver}"
                ));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// DshInstall (npx-based execution)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DshInstall {
    /// Node binary used to drive npm/npx.
    pub node: PathBuf,
    /// `node_modules/npm/bin/npx-cli.js` next to node.
    pub npx_cli: PathBuf,
    /// Dedicated npx cache (keeps the dsh install isolated and cleanable).
    pub npx_cache: PathBuf,
}

impl DshInstall {
    pub fn resolve() -> Result<Self> {
        let node = find_on_path()
            .ok_or_else(|| anyhow!("no Node.js runtime found (on PATH)"))?;
        let npx_cli = node
            .parent()
            .ok_or_else(|| anyhow!("cannot locate node directory"))?
            .join("node_modules")
            .join("npm")
            .join("bin")
            .join("npx-cli.js");
        if !npx_cli.is_file() {
            return Err(anyhow!(
                "npx-cli.js not found next to {}",
                node.display()
            ));
        }
        let npx_cache = config::data_dir().join("npx-cache");
        Ok(Self {
            node,
            npx_cli,
            npx_cache,
        })
    }

    /// Build the `dsh web` command via npx:
    /// `node <npx-cli> --cache <cache> -y @deepseek-ai/dsh@<version> web --patch … --port N`
    ///
    /// npx installs `<version>` into its cache on first use (long op, output
    /// streamed) and reuses it afterwards (fast, no network).
    pub fn command(&self, version: &str, port: u16, token: &str, config: &config::Config) -> Command {
        let mut cmd = Command::new(&self.node);
        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        cmd.arg(&self.npx_cli)
            .arg("--cache")
            .arg(&self.npx_cache)
            .arg("-y")
            .arg(format!("@deepseek-ai/dsh@{version}"))
            .arg("web");
        if let Some(patch) = ops_patch_path() {
            cmd.arg("--patch").arg(patch);
        }
        cmd.arg("--port").arg(port.to_string());
        cmd.args(&config.extra_dsh_args);
        cmd.env("DSH_DESKTOP_SHUTDOWN_TOKEN", token);
        if let Some(home) = &config.dsh_home {
            cmd.env("DSH_HOME", home);
        }
        if !config.extra_path.is_empty() {
            let sep = if cfg!(windows) { ";" } else { ":" };
            let extra = config
                .extra_path
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(sep);
            let current = std::env::var("PATH").unwrap_or_default();
            cmd.env("PATH", format!("{extra}{sep}{current}"));
        }
        cmd
    }

    /// `node <npx-cli> --cache … -y @deepseek-ai/dsh@<version> --version` —
    /// installs `<version>` into the npx cache (if absent) and exits once
    /// ready. Used as an install probe: unlike `dsh web`, it has no URL
    /// timeout, so a minutes-long cold install is never killed.
    pub fn probe_command(&self, version: &str) -> Command {
        let mut cmd = Command::new(&self.node);
        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        cmd.arg(&self.npx_cli)
            .arg("--cache")
            .arg(&self.npx_cache)
            .arg("--prefer-offline")
            .arg("-y")
            .arg(format!("@deepseek-ai/dsh@{version}"))
            .arg("--version");
        // npm is silent on a non-TTY stderr during a long download; bump the
        // log level so the progress window shows install activity. The version
        // is pinned exactly, so --prefer-offline (cached packuments/tarballs
        // without revalidation) is safe and keeps re-installs fast.
        cmd.env("npm_config_loglevel", "info");
        cmd
    }

    /// Install `<version>` into the npx cache via the `--version` probe
    /// (streams npm output to `lines`). The later `dsh web` spawn is then a
    /// cache hit — no long URL wait. Err on non-zero exit.
    pub async fn ensure_installed(
        &self,
        version: &str,
        lines: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<()> {
        let mut cmd = self.probe_command(version);
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().context("failed to launch dsh install probe")?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("no stdout pipe"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("no stderr pipe"))?;
        let out = tokio::spawn(drain_lines(stdout, lines.clone()));
        let err = tokio::spawn(drain_lines(stderr, lines));
        let status = child.wait().await.context("dsh install probe wait failed")?;
        let _ = out.await;
        let _ = err.await;
        if !status.success() {
            return Err(anyhow!("dsh {version} 安装失败（npx probe 退出 {status}）"));
        }
        tracing::info!("dsh {version} ready in npx cache");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Plugin installation (local tarball -> web profile)
// ---------------------------------------------------------------------------

/// Ensure a `pnpm.cmd` shim that routes through corepack exists in the
/// shell's private bin dir; returns that dir (prepend to PATH for pnpm users).
pub fn ensure_pnpm_shim() -> Option<PathBuf> {
    let dir = config::data_dir().join("bin");
    std::fs::create_dir_all(&dir).ok()?;
    let shim = dir.join("pnpm.cmd");
    if !shim.exists() {
        let corepack = find_corepack()?;
        let content = format!("@echo off\r\n\"{}\" pnpm %*\r\n", corepack.display());
        std::fs::write(&shim, content).ok()?;
    }
    Some(dir)
}

fn find_corepack() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("corepack.cmd"))
            .find(|p| p.is_file())
    })
}

/// Ensure the web profile's loader mounts `pkg_name` via `cordis.patch.yml`.
pub fn ensure_loader_entry(home: &Path, pkg_name: &str) -> Result<()> {
    let patch = home.join("profiles").join("web").join("cordis.patch.yml");
    let content = std::fs::read_to_string(&patch).unwrap_or_default();
    if content.contains(&format!("name: '{pkg_name}'")) {
        return Ok(());
    }
    let block = format!("- insert:\n    - id: {pkg_name}\n      name: '{pkg_name}'\n");
    let new_content = if content.trim().is_empty() || content.trim() == "[]" {
        format!(
            "# Desktop-managed plugin entries (added by the shell's plugin installer)\n{block}"
        )
    } else {
        format!("{}\n{}", content.trim_end(), block)
    };
    std::fs::write(&patch, new_content).context("write cordis.patch.yml")?;
    tracing::info!(
        "added loader entry for {pkg_name} to {}",
        patch.display()
    );
    Ok(())
}

/// Install a local plugin tarball into the web profile via
/// `dsh plugin --profile web add <tgz>`, then ensure its loader entry.
/// Runs the dsh CLI through npx (pinned to the current version) under a
/// hidden-console powershell so the whole pnpm/cmd chain stays windowless.
pub async fn install_plugin_tarball(
    install: &DshInstall,
    home: &Path,
    tgz: &Path,
    lines: tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<String> {
    let version = current_version().unwrap_or_else(|| "latest".to_string());
    let cmdline = format!(
        "& '{}' '{}' --cache '{}' -y '@deepseek-ai/dsh@{version}' plugin --profile web add '{}'",
        install.node.display(),
        install.npx_cli.display(),
        install.npx_cache.display(),
        tgz.display()
    );
    let mut cmd = Command::new("powershell.exe");
    cmd.arg("-NoProfile")
        .arg("-WindowStyle")
        .arg("Hidden")
        .arg("-Command")
        .arg(cmdline);
    cmd.env("DSH_HOME", home);
    if let Some(bin) = ensure_pnpm_shim() {
        let sep = if cfg!(windows) { ";" } else { ":" };
        let cur = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{}{sep}{cur}", bin.display()));
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().context("failed to launch dsh plugin")?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("no stdout pipe"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("no stderr pipe"))?;
    let out_lines = tokio::spawn(drain_lines(stdout, Some(lines.clone())));
    let err_lines = tokio::spawn(drain_lines(stderr, Some(lines)));
    let status = child.wait().await.context("dsh plugin add wait failed")?;
    let _ = out_lines.await;
    let _ = err_lines.await;
    if !status.success() {
        return Err(anyhow!("dsh plugin add exited with {status}"));
    }
    let pkg_name = profile_package_for_tarball(home, tgz)?;
    ensure_loader_entry(home, &pkg_name)?;
    Ok(pkg_name)
}

fn profile_package_for_tarball(home: &Path, tgz: &Path) -> Result<String> {
    let manifest = home.join("profiles").join("web").join("package.json");
    let text = std::fs::read_to_string(&manifest)?;
    let json: serde_json::Value = serde_json::from_str(&text)?;
    let deps = json
        .get("dependencies")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("profile manifest has no dependencies"))?;
    let file_name = tgz
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or_default()
        .to_string();
    for (name, spec) in deps {
        let s = spec.as_str().unwrap_or("");
        if s.replace('\\', "/").ends_with(&file_name) || s.contains(&file_name) {
            return Ok(name.clone());
        }
    }
    anyhow::bail!(
        "installed package not found in profile manifest for {}",
        tgz.display()
    )
}

async fn drain_lines<R: tokio::io::AsyncRead + Unpin>(
    r: R,
    lines: Option<tokio::sync::mpsc::UnboundedSender<String>>,
) {
    let Some(lines) = lines else {
        return;
    };
    let mut reader = BufReader::new(r).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        if lines.send(line).is_err() {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Ops overlay + paths
// ---------------------------------------------------------------------------

/// Locate the `--patch` overlay that mounts the desktop ops routes.
///
/// npm exec (npx) splits arguments at spaces, and the installed resource path
/// lives under `%LOCALAPPDATA%\DeepSeek Harness\...` (has a space) — so the
/// overlay is staged to a space-free shell data path before being passed.
fn ops_patch_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = config::resource_dir() {
        candidates.push(dir.join("scripts").join("web-surface.patch.yml"));
    }
    let exe = std::env::current_exe().ok()?;
    if let Some(root) = exe.parent().and_then(|p| p.parent()) {
        candidates.push(root.join("scripts").join("web-surface.patch.yml"));
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("dsd-side")
            .join("web-surface.patch.yml"),
    );
    let src = candidates.into_iter().find(|p| p.exists())?;
    let staged = config::data_dir().join("scripts").join("web-surface.patch.yml");
    if let Some(parent) = staged.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::copy(&src, &staged).is_ok() {
        Some(staged)
    } else {
        Some(src)
    }
}

/// Locate the checked-in `@dsh-desktop/dsh-ops` package source.
fn ops_plugin_source() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = config::resource_dir() {
        candidates.push(dir.join("scripts").join("dsd-side"));
    }
    let exe = std::env::current_exe().ok()?;
    if let Some(root) = exe.parent().and_then(|p| p.parent()) {
        candidates.push(root.join("scripts").join("dsd-side"));
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("dsd-side"),
    );
    candidates
        .into_iter()
        .find(|p| p.join("package.json").exists())
}

/// Ensure the desktop ops overlay plugin (`@dsh-desktop/dsh-ops`) is
/// resolvable from the web profile.
pub fn ensure_ops_overlay(home: &Path) -> Result<()> {
    let src = ops_plugin_source().ok_or_else(|| {
        anyhow!("dsd-side plugin source not found (dev: repo dsd-side/; installed: scripts/dsd-side/)")
    })?;
    let dest = home
        .join("profiles")
        .join("node_modules")
        .join("@dsh-desktop")
        .join("dsh-ops");
    std::fs::create_dir_all(&dest).context("create profile node_modules dir")?;
    for file in ["index.js", "package.json"] {
        std::fs::copy(src.join(file), dest.join(file))
            .with_context(|| format!("copy dsd-side {file} into profile node_modules"))?;
    }
    tracing::info!("ops overlay installed at {}", dest.display());
    Ok(())
}

/// Fresh per-boot secret the shell passes to `/api/admin/shutdown`.
pub fn fresh_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
