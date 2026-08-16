# DeepSeek Harness Desktop (DshDesktop)

A professional desktop shell for the [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) Web GUI, built with **Tauri 2 (Rust + WebView2)**.

It is a *thin host wrapper*, not a fork: it spawns the **installed** `dsh web` as a managed child process and loads the served URL in a WebView2 window. It adds what a browser cannot — a real app icon, single instance, tray status, close-to-tray, crash auto-restart, deploy/status observability, and (P2) signed self-update.

## Core principle

> The shell **never bundles DSH**. It locates a pinned `dsh` install in a private prefix and spawns `dsh web`. DSH upgrades are therefore transparent, and every DSH client-plugin feature appears automatically (the WebView loads the same URL served by `dsh web`, which injects `window.__DSH_BOOT__`).

## Repo layout

```
ui/                      minimal static splash + offline pages
scripts/
  ensure-dsh.ps1         first-run: install pinned dsh into the private prefix
  release.ps1            P2: build + sign + generate update manifest
src-tauri/               Tauri 2 Rust backend
  src/
    app.rs               setup wiring (plugins, tray, single-instance, lifecycle)
    process_supervisor.rs  spawn dsh web, parse port from stdout, watch exit
    dsh_manager.rs       locate node/dsh, build env, version gate
    health.rs            readiness / liveness probes
    lifecycle.rs         state machine + restart/backoff strategy
    tray.rs              tray icon + menu + status colors
    commands.rs          WebView<->Rust IPC (status / restart / open logs / quit)
    config.rs            config.json schema + load/save
  tauri.conf.json        window / bundle(NSIS) / plugins
  capabilities/default.json
docs/
  ARCHITECTURE.md        system/architecture/performance/maintainability design
  DEPLOYMENT.md          install + upgrade + directory layout design
  INTERFACE_CONTRACT.md  DSH-side /api/health + /api/admin/shutdown contract
dsd-side/                reference DSH-side plugin package (mounts the two routes)
```

## Prerequisites

- Windows 10 1809+ / Windows 11 (WebView2 Evergreen; the installer bundles the bootstrapper for Win10).
- [Rust](https://rustup.rs) (stable, MSVC toolchain) + Node.js ≥ 20 (for the Tauri CLI only).

## First build (one-time)

```powershell
# 1. install the Tauri CLI
npm install

# 2. generate icons from a 512x512 PNG (required before `tauri build`)
npm run tauri -- icon ./assets/icon.png
#    this creates src-tauri/icons/icon.ico, icon.png, ...

# 3. dev run
npm run tauri -- dev

# 4. production build (NSIS per-user installer)
npm run tauri -- build
```

## How it runs (P1)

1. `main.rs` (with `windows_subsystem="windows"`) launches with no console.
2. `app::setup` loads `config.json`, resolves node + dsh, spawns `dsh web --port 0` hidden, parses the `dsh web: http://127.0.0.1:<port>` stdout line.
3. `health::wait_ready` polls until the server answers, then navigates the WebView to the URL (the splash page shows until then).
4. Tray icon reflects state (yellow starting / green ready / red error); closing the window keeps it in the tray.

See `docs/` for the full design and the DSH-side contract.

## Security note

The `/api/admin/shutdown` route (DSH-side, see `dsd-side/`) is guarded by a per-boot secret passed via the `DSH_DESKTOP_SHUTDOWN_TOKEN` environment variable, so only the shell can gracefully stop its own child.
