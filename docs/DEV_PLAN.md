# 开发计划与交接说明 (DEV_PLAN)

> 交接来源：DSH 会话（原工作区 `D:\working\projects\dsh`），时间 2026-08-17。
> 仓库已从 https://github.com/baikangwang/deepseek-harness-desktop.git clone 到本目录（master，干净，4 个提交，P1 骨架已完成）。
> 本文件是给新工作区会话的完整交接，先读本文件再动手。

## 项目定位

Tauri 2 (Rust + WebView2) 桌面壳，为 DeepSeek Harness Web GUI 提供 Windows 桌面外壳。

核心原则（红线）：**shell 绝不打包 DSH**。它定位并 spawn 固定版本的 `dsh web`（私有前缀里 npm 安装的 `@deepseek-ai/dsh`），解析 stdout 端口，加载该 URL 到 WebView2。因此 DSH 升级透明，DSH 的 client-plugin 全部自动生效（WebView 加载同一 URL，由 `dsh web` 注入 `window.__DSH_BOOT__`）。

已实现（P1 骨架，commit db67f47 / fb8e198）：
- 进程监督：spawn `dsh web --port 0`，从 stdout 解析端口，退出监视 + 指数退避重启（1→2→4→…→上限）
- 健康探针：TCP connect 轮询就绪后导航 WebView
- 托盘 + 状态颜色（黄=starting / 绿=ready / 红=error），关窗进托盘
- 单实例（tauri-plugin-single-instance）、窗口状态记忆、优雅关停（`/api/admin/shutdown` + Bearer 秘钥 `DSH_DESKTOP_SHUTDOWN_TOKEN`）
- config.json 加载/保存

## 新环境现状（已审计，2026-08-17）

| 依赖 | 状态 |
|---|---|
| Node.js v24.19.0 / npm 10.1.0 | ✅ 已装 |
| WebView2 Evergreen 151 | ✅ 已装 |
| Rust (rustc/cargo) | ❌ **未装，必须先装** |
| MSVC C++ Build Tools (link.exe) | ❌ **未装，必须先装** |
| Tauri CLI | 仓库内 `npm install` 后可用（`npm run tauri`）；图标已提交无需生成 |
| DSH 运行时 | 未装；跑 `scripts/ensure-dsh.ps1` 安装 `@deepseek-ai/dsh@0.1.0-rc.6` 到私有前缀 |

## Phase 0 — 环境搭建（一次性，本机）

```powershell
# 1. Rust（winget 方式，或官网 rustup-init.exe）
winget install --id Rustlang.Rustup -e --source winget
rustup default stable-x86_64-pc-windows-msvc   # 新开终端让 PATH 生效

# 2. VS 2022 Build Tools（提供 link.exe，勾选"使用 C++ 的桌面开发"）
winget install --id Microsoft.VisualStudio.2022.BuildTools -e `
  --override "--add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --passive --wait --norestart"

# 3. Tauri CLI（仓库根目录）
npm install

# 4. 安装固定版 dsh 到应用私有前缀
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\ensure-dsh.ps1
```

> 现成一键脚本：`.\scripts\local-setup.ps1` 依次完成 1–3 并触发 `npm run tauri -- build`。

## Phase 1 — 构建验证

```powershell
npm run tauri -- dev    # 验证：spawn dsh web → 解析端口 → 健康就绪 → WebView 加载 → 托盘变绿
```

- 核对 `dsh_manager.rs` 里 `MIN_DSH_VERSION` 门槛与 ensure-dsh.ps1 钉住的版本（0.1.0-rc.6）是否兼容
- 验证优雅关停链路：托盘退出 → `/api/admin/shutdown`（Bearer 秘钥）→ dsh web 干净退出

## Phase 2 — 功能开发

先通读 `src-tauri/src/*.rs` 核对 P1 各模块完成度：`app.rs` / `process_supervisor.rs` / `dsh_manager.rs` / `health.rs` / `lifecycle.rs` / `tray.rs` / `commands.rs` / `config.rs`。

已知待办（P2）：
- 自更新：启用 `tauri-plugin-updater`（Cargo.toml 注释已预留位置），需要 minisign 公/私钥，配置 `TAURI_SIGNING_PRIVATE_KEY`
- Authenticode 代码签名（NSIS 安装包）
- `scripts/release.ps1`：构建 + 签名 + 生成更新清单 latest.json
- 日志滚动（`dsh-web.log` / `shell.log`，架构文档提到 P2 滚动）
- 待评估：config 界面 / 状态展示细节是否齐全

## Phase 3 — CI / 发布

- `.github/workflows/build-release.yml` 已就绪：
  - push main/master 或 PR → 构建 NSIS + 上传 artifact
  - tag `v*` → tauri-action 构建并发布 GitHub Release（P2 时接 `TAURI_SIGNING_PRIVATE_KEY` secret）
- 本地推送：`scripts/push-to-github.ps1`（需 `$env:GITHUB_TOKEN`，用户已刷新 PAT 并写入本机 GCM）

## 文档索引

- `docs/ARCHITECTURE.md` — 架构设计（中文）：进程模型、关键机制表、性能/安全边界
- `docs/DEPLOYMENT.md` — 安装/升级/目录布局设计
- `docs/INTERFACE_CONTRACT.md` — DSH 侧 `/api/health` + `/api/admin/shutdown` 契约（`DSH_DESKTOP_SHUTDOWN_TOKEN` 保护）
- `dsd-side/` — DSH 侧插件包参考实现（挂载上述两个路由），含 `web-surface.patch.yml`
