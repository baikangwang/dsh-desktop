//! Locate / manage the DSH install in the private runtime prefix.
//!
//! The shell NEVER bundles DSH. On every startup it resolves the latest
//! `@deepseek-ai/dsh` from the registry, checks plugin compatibility, and
//! reinstalls the prefix when a newer compatible version exists.

use crate::config;
use anyhow::{anyhow, Context, Result};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Minimum dsh version the shell supports (interface contract C1–C3 + routes).
/// This is a compatibility FLOOR, not a pin: the shell installs the latest
/// registry version but refuses to run anything below this.
pub const MIN_DSH_VERSION: &str = "0.1.0-rc.6";

// ---------------------------------------------------------------------------
// Version helpers (semver-lite with prerelease support)
// ---------------------------------------------------------------------------

/// Read the installed `@deepseek-ai/dsh` version from its package.json.
pub fn installed_version(install: &DshInstall) -> Option<String> {
    let pkg = install
        .prefix
        .join("node_modules")
        .join("@deepseek-ai")
        .join("dsh")
        .join("package.json");
    let text = std::fs::read_to_string(pkg).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("version")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
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

/// Full semver-lite comparison: numeric core, then prerelease
/// (release > any prerelease; numeric identifiers < alphanumeric).
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

/// Whether `version` satisfies an npm-style range string
/// (space-separated `op version` tokens; ops `>= > <= < = ^ ~`).
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
// npm registry helpers
// ---------------------------------------------------------------------------

/// Locate `npm-cli.js` next to the system Node runtime (used instead of
/// `npm.cmd` so the shell can spawn node directly with no console window).
fn npm_cli_path() -> Option<PathBuf> {
    let node = find_on_path()?;
    let cli = node.parent()?.join("node_modules").join("npm").join("bin").join("npm-cli.js");
    cli.is_file().then_some(cli)
}

/// Run `npm view <args> --json` with a timeout. None on failure (offline etc.).
/// Uses `node <npm-cli.js>` with CREATE_NO_WINDOW so no console window pops.
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

/// TTL for the cached "latest dsh version" lookup, so most startups do not
/// touch the network at all.
const LATEST_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

fn latest_cache_path() -> PathBuf {
    config::data_dir().join("cache").join("dsh-latest.json")
}

fn read_latest_cache() -> Option<String> {
    let path = latest_cache_path();
    let text = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let age = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs()
        .saturating_sub(json.get("ts")?.as_u64()?);
    if age > LATEST_CACHE_TTL.as_secs() {
        return None;
    }
    json.get("version")?.as_str().map(str::to_owned)
}

fn write_latest_cache(version: &str) {
    let path = latest_cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let json = serde_json::json!({ "version": version, "ts": ts });
    let _ = std::fs::write(path, serde_json::to_string(&json).unwrap_or_default());
}

/// Whether a fresh "latest version" cache exists (so the startup check will
/// be instant and touch no network).
pub fn latest_cache_fresh() -> bool {
    read_latest_cache().is_some()
}

/// Latest `@deepseek-ai/dsh` version on the registry (cached for
/// `LATEST_CACHE_TTL`); falls back to the stale cache when offline.
pub async fn latest_dsh_version() -> Option<String> {
    if let Some(cached) = read_latest_cache() {
        return Some(cached);
    }
    let json = npm_view(&["@deepseek-ai/dsh@latest", "version"]).await?;
    let v = serde_json::from_str::<serde_json::Value>(&json)
        .ok()?
        .as_str()
        .map(str::to_owned)?;
    write_latest_cache(&v);
    Some(v)
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
/// Returns human-readable incompatibility descriptions.
pub async fn plugin_incompatibilities(
    home: &Path,
    dsh_deps: &HashMap<String, String>,
) -> Vec<String> {
    let mut out = Vec::new();
    let web_nm = home.join("profiles").join("web").join("node_modules");
    let Ok(entries) = std::fs::read_dir(&web_nm) else {
        return out;
    };
    // Cache the resolved subpackage version per dependency name.
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
                        continue; // registry unreachable: cannot judge -> skip
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
// DshInstall
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DshInstall {
    /// Node binary used to run dsh (bundled runtime node, else system node).
    pub node: PathBuf,
    /// `.../node_modules/@deepseek-ai/dsh/lib/bin.js`.
    pub dsh_bin: PathBuf,
    /// Private prefix that owns `node_modules`.
    pub prefix: PathBuf,
}

impl DshInstall {
    pub fn resolve() -> Result<Self> {
        let prefix = config::runtime_dir();
        let node = bundled_node(&prefix).or_else(find_on_path).ok_or_else(|| {
            anyhow!("no Node.js runtime found (bundled or on PATH)")
        })?;
        let dsh_bin = prefix
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh")
            .join("lib")
            .join("bin.js");
        Ok(Self { node, dsh_bin, prefix })
    }

    pub fn is_present(&self) -> bool {
        self.dsh_bin.exists()
    }

    /// Build the `dsh web ...` command with the environment contract.
    ///
    /// `--patch` must precede the web app's passthrough flags (`--port` etc.):
    /// the `web` subcommand stops parsing options at the first positional /
    /// passthrough argument, so a `--patch` after `--port` would be handed to
    /// the web app's own parser and rejected.
    pub fn command(&self, port: u16, token: &str, config: &config::Config) -> Command {
        let mut cmd = Command::new(&self.node);
        #[cfg(windows)]
        {
            // The shell is a GUI app (no console in release); without this the
            // console-subsystem node child would pop its own console window.
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        cmd.arg(&self.dsh_bin).arg("web");
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
}

// ---------------------------------------------------------------------------
// Install / update
// ---------------------------------------------------------------------------

/// Install (or update) `@deepseek-ai/dsh@<version>` into the private prefix by
/// running the bundled `scripts/ensure-dsh.ps1`. Streams the installer's
/// stdout/stderr line by line to `lines`.
pub async fn ensure_dsh(
    install: &DshInstall,
    version: &str,
    lines: tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<()> {
    let script = script_path("ensure-dsh.ps1")?;
    let node = bundled_node(&install.prefix)
        .or_else(find_on_path)
        .ok_or_else(|| anyhow!("no Node.js runtime found to run the installer"))?;
    let mut cmd = Command::new("powershell.exe");
    // -WindowStyle Hidden gives powershell a HIDDEN console (no CREATE_NO_WINDOW:
    // with no console at all, every console-subsystem child — npm, node, script
    // powershells — would create its own visible window). Children inherit the
    // hidden console, so the whole install tree stays silent.
    let mut child = cmd
        .arg("-NoProfile")
        .arg("-WindowStyle")
        .arg("Hidden")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-File")
        .arg(&script)
        .arg("-RuntimeDir")
        .arg(&install.prefix)
        .arg("-DshVersion")
        .arg(version)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to launch ensure-dsh.ps1")?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("no stdout pipe"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("no stderr pipe"))?;
    let out_lines = tokio::spawn(drain_lines(stdout, lines.clone()));
    let err_lines = tokio::spawn(drain_lines(stderr, lines));
    let status = child.wait().await.context("ensure-dsh.ps1 wait failed")?;
    let _ = out_lines.await;
    let _ = err_lines.await;
    if !status.success() {
        return Err(anyhow!("ensure-dsh.ps1 exited with {status}"));
    }
    if !install.is_present() {
        return Err(anyhow!(
            "dsh still missing after install; check {}",
            install.dsh_bin.display()
        ));
    }
    let _ = node; // reserved for future flags
    Ok(())
}

async fn drain_lines<R: tokio::io::AsyncRead + Unpin>(
    r: R,
    lines: tokio::sync::mpsc::UnboundedSender<String>,
) {
    let mut reader = BufReader::new(r).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        if lines.send(line).is_err() {
            break;
        }
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

/// Ensure the web profile's loader mounts `pkg_name` via `cordis.patch.yml`
/// (an `insert` entry). Idempotent: skips when the name is already present.
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
/// Returns the installed package name. Streams CLI output to `lines`.
pub async fn install_plugin_tarball(
    install: &DshInstall,
    home: &Path,
    tgz: &Path,
    lines: tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<String> {
    // Run through a hidden-console powershell so `dsh plugin`'s internal
    // `pnpm` (pnpm.cmd -> cmd.exe) inherits the hidden console instead of
    // popping its own window.
    let cmdline = format!(
        "& '{}' '{}' plugin --profile web add '{}'",
        install.node.display(),
        install.dsh_bin.display(),
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
    let out_lines = tokio::spawn(drain_lines(stdout, lines.clone()));
    let err_lines = tokio::spawn(drain_lines(stderr, lines));
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

// ---------------------------------------------------------------------------
// Ops overlay + paths
// ---------------------------------------------------------------------------

/// Locate the `--patch` overlay that mounts the desktop ops routes
/// (`@dsh-desktop/dsh-ops`: `/api/health` + `/api/admin/shutdown`).
/// Installed layout: `<resource-dir>/scripts/web-surface.patch.yml`;
/// dev layout: `<repo>/dsd-side/web-surface.patch.yml`.
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
    candidates.into_iter().find(|p| p.exists())
}

/// Locate the checked-in `@dsh-desktop/dsh-ops` package source.
/// Installed layout: `<resource-dir>/scripts/dsd-side/`;
/// dev layout: `<repo>/dsd-side/`.
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
/// resolvable from the web profile: the Cordis loader resolves inserted
/// plugins from the profile directory (`<DSH_HOME>/profiles/web/…`, walking
/// up to `<DSH_HOME>/profiles/node_modules`), NOT from the runtime prefix.
/// Idempotent copy of the checked-in package.
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

fn bundled_node(prefix: &Path) -> Option<PathBuf> {
    let p = prefix.join("node.exe");
    p.exists().then_some(p)
}

fn find_on_path() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "node.exe" } else { "node" };
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(exe))
            .find(|p| p.is_file())
    })
}

fn script_path(name: &str) -> Result<PathBuf> {
    // Production layout: bundled resources at <resource-dir>/scripts/<name>.
    if let Some(dir) = config::resource_dir() {
        let candidate = dir.join("scripts").join(name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    let exe = std::env::current_exe()?;
    let root = exe
        .parent()
        .and_then(|p| p.parent())
        .ok_or_else(|| anyhow!("cannot locate install root"))?;
    let candidate = root.join("scripts").join(name);
    if candidate.exists() {
        Ok(candidate)
    } else {
        // Dev layout: repo root/scripts/<name>
        Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("scripts")
            .join(name))
    }
}
