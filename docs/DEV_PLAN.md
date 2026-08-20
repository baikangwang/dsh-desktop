# 开发计划与交接说明 (DEV_PLAN)

> 交接来源：DSH 会话（原工作区 `D:\working\projects\dsh`），时间 2026-08-17。
> 仓库现址：https://github.com/baikangwang/dsh-desktop.git（原 `deepseek-harness-desktop` 迁移而来）。
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
| DSH 运行时 | **不装不打包**：壳经 npx 自动解析/安装最新 `@deepseek-ai/dsh`（专属缓存 `%LOCALAPPDATA%\DshDesktop\npx-cache`） |

## Phase 0 — 环境搭建（一次性，本机）

```powershell
# 1. Rust（winget 方式，或官网 rustup-init.exe）
winget install --id Rustlang.Rustup -e --source winget
rustup default stable-x86_64-pc-windows-msvc   # 新开终端让 PATH 生效

# 2. VS 2022 Build Tools（提供 link.exe，勾选"使用 C++ 的桌面开发"）+ Windows 11 SDK（rc.exe）
winget install --id Microsoft.VisualStudio.2022.BuildTools -e `
  --override "--add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --passive --wait --norestart"

# 3. Tauri CLI（仓库根目录）
npm install
```

> 现成一键脚本：`.\scripts\local-setup.ps1` 完成 1–3（dsh 运行时由应用首启自动经 npx 安装，无需手动）。

## Phase 1 — 构建验证

```powershell
npm run tauri -- dev    # 验证：spawn dsh web → 解析端口 → 健康就绪 → WebView 加载 → 托盘变绿
```

- 核对 `dsh_manager.rs` 里 `MIN_DSH_VERSION` 门槛与 ensure-dsh.ps1 钉住的版本（0.1.0-rc.6）是否兼容
- 验证优雅关停链路：托盘退出 → `/api/admin/shutdown`（Bearer 秘钥）→ dsh web 干净退出

## Phase 2 — 功能开发

先通读 `src-tauri/src/*.rs` 核对 P1 各模块完成度：`app.rs` / `process_supervisor.rs` / `dsh_manager.rs` / `health.rs` / `lifecycle.rs` / `tray.rs` / `commands.rs` / `config.rs`。

已推进（2026-08-17，提交 fd65bea / 3ee0062）：
- ✅ `MIN_DSH_VERSION` 门禁（`dsh_manager.rs`，0.1.0-rc.6，启动校验）
- ✅ 托盘状态色（黄/绿/橙/红/灰 + tooltip，`tray.rs` + `app.rs` watcher）
- ✅ 优雅关停路由：`dsh web --patch web-surface.patch.yml` + `@dsh-desktop/dsh-ops`
  装入 `<DSH_HOME>/profiles/node_modules`（`ensure_ops_overlay`）；
  已端到端验证：`/api/health` 200、shutdown 无 token 401、带 token 202 且 dsh web 干净退出
- ✅ 日志滚动（10MiB / 3 代，`config::rotate_if_large`）+ `shell.log` 落盘
- ✅ 修复 P1 编译错误（原仓库从未构建过）与日志目录缺失 bug；`Config::load` 容忍 BOM

已推进（2026-08-19，提交 2724521）——按评审后的升级/插件模型重构：

**升级模型（2026-08-20 复审后调整，提交 6ec56d2）**
- Shell：**离线包覆盖升级**（关闭 → 装新包 → 重启），无自动化
- dsh：**npx 执行 + 专属缓存**（`%LOCALAPPDATA%\DshDesktop\npx-cache`）：
  shell 自己决策目标版本（6h 缓存 latest + 插件 peerDeps 兼容门），
  再 `node npx-cli --cache … -y @deepseek-ai/dsh@<目标> web …`——
  npx 负责"装新版/复用缓存/运行"；每次成功启动把版本写入状态文件
  （`cache/dsh-current.json`）；已删除 ensure-dsh.ps1 与私有 runtime 前缀
  - ⚠️ npm exec 会把含空格的参数截断 → `--patch` 暂存到无空格路径
  - 启动清理被强杀残留的孤儿 dsh web 进程
- 插件：**本地离线包（.tgz）按需安装**——托盘"安装插件…"→ 选包 →
  `dsh plugin --profile web add`（corepack pnpm shim）→ 确保 loader entry → 重启 dsh

**已验证（实机）**
- ✅ 干净启动 19s 到就绪（版本判定 2.2s 零网络 + npx 缓存命中 + dsh boot），无弹窗、无崩溃循环
- ✅ dsh rc.6 → rc.7 自动升级（进度窗体）；`/api/health` 200 + `dsh-ide-ui` 加载

已推进（2026-08-20）——**更新通道选择（评审后定稿）**：
- ✅ `Channel{latest,preview}`：持久化 `cache/dsh-channel.json`，默认 `latest`
- ✅ 远端版本一次取齐（`npm view … dist-tags versions` → `cache/dsh-remote.json`，6h TTL）：
  `latest`=官方 latest 标签（rc.8 发布于 `next` 通道，latest 仍为 rc.7），`preview`=最高版本号（rc.8）
- ✅ **确认门启动流程**（用户评审后重做）：splash 先联网取版本目标（选项禁用）→ 版本号呈现到选项
  → 用户选择 → 点「确认启动」（`confirm_channel`）→ 才决策/安装/启动 dsh；确认后选项禁用
- ✅ 冷安装修复：`--version` probe 无 URL 超时（不再 30s 杀循环）+ `--prefer-offline`（精确版本 pin，安全）
- ✅ 双版本共存：npx 按 spec 哈希缓存，latest/preview 互不覆盖；误选重启应用重选
- ⏳ 端到端验证：preview → rc.8 / latest → rc.7（见下）

**待办（P5）**
- Authenticode 代码签名（需证书；release.ps1 留 `AUTHENTICODE_CERT` 钩子）
- 插件"卸载"（v1 只做安装/升级；卸载可手动删 profile 配置）
- 待评估：config 界面 / 状态展示细节是否齐全


## Phase 3 — CI / 发布

- `.github/workflows/release.yml`（唯一 workflow，由 `scripts/dsh-release.mjs` 按模板生成/覆盖）：
  - `v*` tag → 版本断言 → `npm run tauri -- build` → NSIS 上传 GitHub Release（幂等 clobber）
  - 详见 `docs/cicd.md`；发布用 `node scripts/dsh-release.mjs --dry-run` 预览后执行
- 本地推送：`push-to-github.ps1`（需 `$env:GITHUB_TOKEN`，用户已刷新 PAT 并写入本机 GCM；仓库地址 `https://github.com/baikangwang/dsh-desktop`）

## 文档索引

- `docs/ARCHITECTURE.md` — 架构设计（中文）：进程模型、关键机制表、性能/安全边界
- `docs/DEPLOYMENT.md` — 安装/升级/目录布局设计
- `docs/INTERFACE_CONTRACT.md` — DSH 侧 `/api/health` + `/api/admin/shutdown` 契约（`DSH_DESKTOP_SHUTDOWN_TOKEN` 保护）
- `docs/cicd.md` — 打包/发布流程（对齐 DSH 插件生态多项目方案）
- `dsd-side/` — DSH 侧插件包参考实现（挂载上述两个路由），含 `web-surface.patch.yml`
