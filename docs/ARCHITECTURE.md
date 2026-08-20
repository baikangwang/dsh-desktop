# 架构设计

## 1. 定位与原则

DshDesktop 是 DeepSeek Harness 的**宿主包装器**，不是 fork：

- 它 spawn **已安装的** `dsh web`，加载其服务的 URL（WebView2 只是又一个 Chromium 宿主）。
- **绝不打包 DSH、绝不加载自带 dist、绝不起第二个服务器**——守住「只有 `dsh web` 注入 `window.__DSH_BOOT__`」这一红线。
- 与 DSH 理念**不冲突**：DSH 源码 `dsh-host-webserver` 已预留桌面消费者（"Electron loads dist over file:// and carries fetch over an IPC bridge"）。本方案走更浅、更稳的 **HTTP 桥**：复用 `dsh web` 原样，外壳零改动 DSH 内部（唯一的 DSH 侧加法是两个路由，见 `INTERFACE_CONTRACT.md`）。

## 2. 进程模型

```
DshDesktop.exe (Tauri, Rust)
 ├─ WebView2 ── http://127.0.0.1:<port>  (DSH Web UI)
 └─ 服务层: process_supervisor / dsh_manager / health / lifecycle / tray / commands / plugins
       └─ node npx-cli --cache <专属缓存> -y @deepseek-ai/dsh@<version> web --port 0
            └─ (npx 内部) dsh web   —— stdout/stderr 捕获, Job 语义回收
```

- 子进程控制**按 PID**（`taskkill /T /F`）而非共享句柄，supervisor 独占 `tokio::process::Child`，规避 Rust 的 `wait()` 借用难题。
- 优雅关停走 `/api/admin/shutdown`（Bearer），硬杀仅作兜底。

## 3. 关键机制

| 机制 | 实现 |
|---|---|
| dsh 运行时 | **npx 执行 + 专属缓存**（`%LOCALAPPDATA%\DshDesktop\npx-cache`）：按**更新通道**决策目标版本（splash 选择，默认 `latest`；6h 缓存 `dsh-remote.json` 一次取 latest 标签 + 最高版本号；插件 peerDeps 兼容门）→ `node npx-cli --cache … -y @deepseek-ai/dsh@<版本> web …`；npx 负责装新版/复用缓存/运行；latest/preview 两个版本按 spec 哈希**共存**于 npx 缓存，互不覆盖 |
| 端口发现 | `--port 0` + 解析 `dsh web: http://…` 行（`process_supervisor::read_port_from_stdout`） |
| 就绪探测 | TCP connect（`health::is_up`），300ms 轮询，30s 超时 |
| 单实例 | `tauri-plugin-single-instance`，二次启动聚焦 |
| 生命周期状态机 | Idle→Starting→Ready→Degraded→Error/Stopped（`lifecycle::supervise`） |
| 自愈 | 指数退避 1→2→4→8→16→32s，超 `max_restarts` 转 Error |
| 关窗到托盘 | `CloseRequested` → `prevent_close` + `hide`；托盘「退出」才真正停 |
| 状态观测 | `AppState::StatusSnapshot`（state/port/url/pid/message）+ 托盘 + 日志 |
| 插件 | 托盘「安装插件…」→ 本地 `.tgz` → `dsh plugin --profile web add`（corepack pnpm shim）→ 确保 `cordis.patch.yml` loader entry → 重启 dsh |
| 安装/升级进度 | splash 页轮询 `get_boot_progress`：阶段 + 流式 npm/pnpm 输出，完成后导航 dsh URL |
| 更新通道 | `Channel{latest,preview}`（`cache/dsh-channel.json` 持久化）；启动流程：**splash 先联网取版本目标（选项禁用）→ 版本号呈现到选项 → 用户选择 → 点「确认启动」（`confirm_channel`）→ 才决策/安装/启动 dsh**；确认后选项禁用，切换通道=重启应用 |

## 4. 性能预算

- 冷启动（首次/升级）：进度窗呈现 npx 安装（分钟级），完成后导航。
- 热启动（版本未变）：版本判定 <1s（缓存命中零网络），npx 缓存命中，整体 ~10–20s 到就绪。
- 内存：Rust 壳 ~10–20MB + WebView2（复用 Evergreen）+ node，与「浏览器 + dsh web」持平。
- 空闲 CPU ~0（探针 5s 一次，退出事件驱动）。

## 5. 可维护性

- 壳/dsh **独立版本** + `MIN_DSH_VERSION` 兼容下限；契约仅 C1–C3 + 两路由。
- 领域 UI 全在 DSH Web（HMR 快循环），壳保持极薄。
- 日志：`dsh-web.log`（子进程）、`shell.log`（tracing）；均滚动（10MiB / 3 代）。
- CI/发布：NSIS per-user；`v*` tag → GitHub Actions 构建 → Release（`docs/cicd.md`）。

## 6. 安全边界

- WebView 导航锁定 `http://127.0.0.1:<port>`；不开放任意 fs/shell IPC。
- 树/git 等域功能**归 DSH 插件**（可移植 + 走 DSH 权限模型），不塞进壳。
- 凭证仍在 `~/.dsh/.credentials.yaml`，外壳从不读取。
