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
 └─ 服务层: process_supervisor / dsh_manager / health / lifecycle / tray / commands
       └─ spawn dsh web (stdout/stderr 捕获, Job 语义回收)
```

- 子进程控制**按 PID**（`taskkill /T /F`）而非共享句柄，supervisor 独占 `tokio::process::Child`，规避 Rust 的 `wait()` 借用难题。
- 优雅关停走 `/api/admin/shutdown`（Bearer），硬杀仅作兜底。

## 3. 关键机制

| 机制 | 实现 |
|---|---|
| 端口发现 | `--port 0` + 解析 `dsh web: http://…` 行（`process_supervisor::read_port_from_stdout`） |
| 就绪探测 | TCP connect（`health::is_up`），300ms 轮询，30s 超时 |
| 单实例 | `tauri-plugin-single-instance`，二次启动聚焦 |
| 生命周期状态机 | Idle→Starting→Ready→Degraded→Error/Stopped（`lifecycle::supervise`） |
| 自愈 | 指数退避 1→2→4→8→16→32s，超 `max_restarts` 转 Error |
| 关窗到托盘 | `CloseRequested` → `prevent_close` + `hide`；托盘「退出」才真正停 |
| 状态观测 | `AppState::StatusSnapshot`（state/port/url/pid/message）+ 托盘 + 日志 |

## 4. 性能预算

- 冷启动：spawn dsh web 与建窗并行，splash 秒感即开，就绪后导航 → 总 2–4s。
- 热启动：单实例聚焦 <1s。
- 内存：Rust 壳 ~10–20MB + WebView2（复用 Evergreen）+ node，与「浏览器 + dsh web」持平。
- 空闲 CPU ~0（探针 5s 一次，退出事件驱动）。

## 5. 可维护性

- 壳/dsh **独立版本** + `MIN_DSH_VERSION` 兼容门；契约仅 C1–C3 + 两路由。
- 领域 UI 全在 DSH Web（HMR 快循环），壳保持极薄。
- 日志：`dsh-web.log`（子进程）、`shell.log`（tracing）；均滚动（P2）。
- CI/发布：NSIS per-user、Authenticode 签名、minisign 更新签名（P2）。

## 6. 安全边界

- WebView 导航锁定 `http://127.0.0.1:<port>`；不开放任意 fs/shell IPC。
- 树/git 等域功能**归 DSH 插件**（可移植 + 走 DSH 权限模型），不塞进壳。
- 凭证仍在 `~/.dsh/.credentials.yaml`，外壳从不读取。
