# DSH｜dsh-desktop｜Tauri 2 桌面壳：托管 `dsh web`，让 DeepSeek Harness 以原生应用形态运行

> **非官方项目**，由社区成员独立开发和维护，与 DeepSeek 官方无关联。
>
> **Unofficial project**, independently developed and maintained by a community
> member. Not affiliated with or endorsed by DeepSeek.

**Project URL / 项目地址**: <https://github.com/baikangwang/dsh-desktop>

---

## 简介 / Introduction

**中文**：`dsh-desktop` 是 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)
Web 界面的桌面壳（Tauri 2 + Rust + WebView2）。它是**极薄的宿主包装器，不是 fork**：
不打包 DSH，启动时通过 npx 解析并安装最新 `dsh web`，以受管子进程运行它，再把
WebView 导航到其服务的 URL。补上了浏览器给不了的体验：独立应用图标、单实例、
系统托盘、关窗驻留、崩溃自愈、优雅关停、本地插件安装。

**English**: `dsh-desktop` is a thin desktop shell for the
[DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) Web GUI,
built with Tauri 2 (Rust + WebView2). It never bundles DSH: on startup it
resolves the latest `dsh web` via npx, manages it as a child process, and loads
its URL in a WebView2 window — adding what a browser cannot: an app icon,
single instance, system tray, close-to-tray, crash auto-restart, graceful
shutdown, and local plugin installation.

### Screenshots / 截图

（占位：启动进度窗、主界面、托盘菜单 —— 后续补充）
*(Placeholder: splash/progress window, main UI, tray menu — to be added.)*

---

## 与 DSH 的集成方式 / How it integrates with DSH

| 集成点 | 机制 |
|---|---|
| 运行 DSH | 启动时经 `npx @deepseek-ai/dsh@<latest>`（专属缓存）运行 `dsh web --port 0`，解析 stdout 端口 |
| WebView 加载 | 导航到 `dsh web` 服务的 URL（`window.__DSH_BOOT__` 由 dsh web 注入——本壳不注入、不打包任何前端） |
| 会话 / 工作区 / 插件 | 与浏览器版共享 `DSH_HOME`（`~/.dsh`）：零迁移，web 与桌面双端一致 |
| 健康 / 优雅关停 | DSH 侧 ops overlay（`dsd-side/`）挂载 `/api/health` + `/api/admin/shutdown`（Bearer 每进程密钥） |
| 插件 | 托盘「安装插件…」：选择本地 `.tgz` → `dsh plugin --profile web add` → 重启 dsh 生效（与 web 版同 profile） |

### Requirements / 环境要求

- Windows 10 1809+ / Windows 11（WebView2 Evergreen）
- [Rust](https://rustup.rs) stable (MSVC) + VS 2022 Build Tools（含 Windows SDK）+ Node.js ≥ 20（构建期）

### Build & Run / 构建与运行

```powershell
npm install
npm run tauri -- dev       # 开发运行
npm run tauri -- build     # 构建 NSIS 安装包（per-user）
```

### Release / 发布

见 [`docs/cicd.md`](docs/cicd.md)：`node scripts/dsh-release.mjs --dry-run` 预览，
确认后执行（自动打 `v*` tag → GitHub Actions 构建 → NSIS 产物挂到 GitHub Release）。

---

## 仓库结构 / Repo layout

```
ui/                        splash / 安装进度 / 离线页
scripts/
  dsh-release.mjs          发布执行层（幂等：读版本 → tag → 生成 release workflow）
  local-setup.ps1          一次性构建环境准备（Rust + MSVC + npm install）
src-tauri/                 Tauri 2 Rust 后端
  src/
    app.rs                 setup 装配（插件、托盘、单实例、生命周期）
    process_supervisor.rs  spawn dsh web、解析端口、监视退出
    dsh_manager.rs         npx 执行、版本决策/缓存、插件兼容门、插件安装
    health.rs              就绪/存活探针
    lifecycle.rs           状态机 + 重启/退避 + 优雅关停
    tray.rs                托盘图标/菜单/状态色
    plugins.rs             本地插件安装流程（选包 → dsh plugin → loader entry → 重启）
    commands.rs            WebView<->Rust IPC
    config.rs              config.json 与日志滚动
docs/
  ARCHITECTURE.md          架构设计
  DEPLOYMENT.md            安装/升级/目录布局
  INTERFACE_CONTRACT.md    DSH 侧 /api/health + /api/admin/shutdown 契约
  cicd.md                  打包/发布流程（对齐 DSH 插件生态多项目方案）
dsd-side/                  DSH 侧 ops overlay 插件（挂载两个路由）
skills/release/            DSH release skill（agent 发布操作层）
```

## 文档 / Docs

- [ARCHITECTURE.md](docs/ARCHITECTURE.md) — 架构设计（进程模型、关键机制、性能/安全边界）
- [DEPLOYMENT.md](docs/DEPLOYMENT.md) — 安装 / 升级 / 目录布局
- [INTERFACE_CONTRACT.md](docs/INTERFACE_CONTRACT.md) — DSH 侧契约
- [cicd.md](docs/cicd.md) — 打包 / 发布流程

## License

[Apache-2.0](LICENSE)（与 DSH 生态一致；dsh-desktop 本身非官方、与官方无关联）。
