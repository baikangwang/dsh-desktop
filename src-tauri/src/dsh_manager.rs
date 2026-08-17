//! Locate / manage the pinned DSH install in the private runtime prefix.
//!
//! The shell NEVER bundles DSH. It spawns the installed `dsh web` so that DSH
//! upgrades stay transparent and native-dependency ABI matches the bundled Node.

use crate::config;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

/// Minimum dsh version the shell supports (interface contract C1–C3 + routes).
/// Keep in sync with the pin in `scripts/ensure-dsh.ps1` (`$DshVersion`).
pub const MIN_DSH_VERSION: &str = "0.1.0-rc.6";

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

/// Semver-ish gate: `installed >= minimum`. Compares the numeric core
/// (`major.minor.patch`) segment by segment; a prerelease suffix does not
/// lower the core, so `0.1.0-rc.6` satisfies a `0.1.0`-family minimum and
/// `0.1.0-rc.6` satisfies itself.
pub fn version_at_least(installed: &str, minimum: &str) -> bool {
    fn core(s: &str) -> &str {
        s.split('-').next().unwrap_or(s)
    }
    fn seg(s: &str, i: usize) -> u64 {
        core(s)
            .split('.')
            .nth(i)
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    }
    for i in 0..4 {
        let a = seg(installed, i);
        let b = seg(minimum, i);
        if a != b {
            return a > b;
        }
    }
    true
}

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

/// First-run: install the pinned dsh into the private prefix via the bundled
/// `scripts/ensure-dsh.ps1`, streaming its output to the shell log.
pub async fn ensure_installed(install: &DshInstall) -> Result<()> {
    let script = script_path("ensure-dsh.ps1")?;
    let node = bundled_node(&install.prefix).or_else(find_on_path).ok_or_else(|| {
        anyhow!("no Node.js runtime found to run the first-run installer")
    })?;
    let mut child = Command::new("powershell.exe")
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-File")
        .arg(&script)
        .arg("-RuntimeDir")
        .arg(&install.prefix)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to launch ensure-dsh.ps1")?;
    let status = child.wait().await.context("ensure-dsh.ps1 wait failed")?;
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
