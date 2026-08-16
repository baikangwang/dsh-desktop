# DSH ↔ Desktop Shell 接口契约

这是外壳与 `dsh web` 之间唯一需要稳定的契约。原则：**改动极小、加法式、向后兼容**——不 fork、不改 DSH 的组合模型，只在 host 平面加两个 HTTP 路由。

## 0. 稳定的前提契约（外壳依赖的 DSH 行为）

| # | 契约点 | 依据 |
|---|---|---|
| C1 | `dsh web` 可执行、支持 `--port <n>`（含 `--port 0` 让 OS 选空闲端口） | `dsh-web-app/lib/startup.js` |
| C2 | 启动就绪后打印 `dsh web: http://127.0.0.1:<port>` | `dsh-web-app/lib/index.js` `printUrl` |
| C3 | 服务绑定 `127.0.0.1`（默认） | `cordis.patch.yml` `webserver.host` |

外壳据此「`--port 0` + 解析 stdout」从根上消除端口冲突。若 DSH 未来改动 C1–C3，外壳用 `MIN_DSH_VERSION` 门禁拦截。

## 1. 新增端点一：`GET /api/health`（就绪探针 + 状态）

- **方法**：`GET`（`HEAD` 同路径返回 200 空 body）。
- **鉴权**：无（服务仅绑定 loopback，与 DSH 应用同信任边界）。
- **语义**：任一 HTTP 响应即「服务已就绪」。启动期 SPA fallback 未注册前，`/` 会 404；此端点保证**权威就绪信号**并携带状态载荷。

响应 `200 application/json`：

```json
{
  "status": "ok",
  "pid": 12345,
  "uptimeSec": 42,
  "port": 3080,
  "dshHome": "C:\\Users\\you\\.dsh"
}
```

用途：外壳存活探针（每 5s）、托盘状态、`__DSH_DESKTOP__` 状态条（PID/端口/uptime），直接满足「感知部署位置 + 运行状态」的观测需求。

## 2. 新增端点二：`POST /api/admin/shutdown`（优雅关停）

- **方法**：`POST`。
- **鉴权**：`Authorization: Bearer <token>`，token = 每进程随机生成、经环境变量 `DSH_DESKTOP_SHUTDOWN_TOKEN` 传入。**未配置 token 时一律 401**（默认安全）。
- **响应**：`202 { "status": "shutting down" }`，随后 50ms 内触发优雅退出。

**为什么必须走应用内端点而非信号**：`dsh web` 已注册 `SIGTERM`/`SIGINT` 优雅退出（`profile-boot` 的 `createProcessShutdown`，5s grace 后强杀）。但 Windows 上 `child.kill('SIGTERM')` 对 Node 是 `TerminateProcess` 硬杀，**不会触发该 handler**。因此外壳需要一条 in-band 关停通道。

## 3. DSH 内部触点（已核对源码，非猜测）

| 触点 | 事实 | 位置 |
|---|---|---|
| 路由注册 | `ctx.webServer.register({ kind: "exact", path, handler })`，handler 为 `async (req, res)`（node:http） | `dsh-host-webserver/lib/index.js` |
| 优先级 | `match()` **先查 exact 表，再查 prefix**；`/api` 是 `dsh-client-connection` 注册的 **prefix** 路由 | `dsh-host-webserver/lib/index.js#match` + `dsh-client-connection/lib/index.js`(kind:"prefix") |
| 优雅关停 | `appExit` 服务 = 启动器的 `shutdown.shutdown(code)`（dispose → `process.exitCode=code`） | `dsh-cmdline/lib/index.js#provideCmdline` + `profile-boot-DG5t9aNs.js` |

结论：`/api/health`、`/api/admin/shutdown` 作为 **exact** 路由会**先于** `/api` prefix 命中，且不经过 `/api` 的 browser-trust fence——所以 shutdown 必须自带 Bearer 鉴权（已设计）。

## 4. 参考实现（`dsd-side/`）

一个单文件 Cordis 插件 `@dsh-desktop/dsh-ops`，注入 `webServer` 与 `appExit`：

```js
inject = ["webServer", "appExit"];
// ctx.effect(() => ctx.webServer.register({ kind: "exact", path: "/api/health", ... }))
// ctx.effect(() => ctx.webServer.register({ kind: "exact", path: "/api/admin/shutdown", ... }))
// shutdown handler: 校验 Bearer → 202 → setTimeout(() => ctx.appExit(0), 50)
```

### 挂载方式（二选一）

1. **上游化（产品推荐）**：把这两个路由并入 `dsh-web-app` bundle patch（一个 DSH PR），所有 `dsh web` 开箱即得，外壳无需额外挂载，只需 `MIN_DSH_VERSION` 门禁。
2. **overlay（不碰上游、今天可用）**：把 `@dsh-desktop/dsh-ops` 装入 web profile 的 `node_modules`，启动时追加
   `dsh web --patch web-surface.patch.yml`，其中：
   ```yaml
   - insert:
       - id: desktop-surface-ops
         name: '@dsh-desktop/dsh-ops'
   ```

## 5. 关停时序（外壳侧，含兜底）

```
POST /api/admin/shutdown (Bearer) → 202
  → DSH: appExit(0) → fiber.dispose() → process.exitCode=0 → 自然退出
外壳等待 ≤800ms → 若进程仍在（有残留句柄），taskkill /PID <pid> /T /F 兜底
→ 外壳 app.exit(0)
```

## 6. 兼容与安全小结

- 契约总量：**C1–C3 + 两个路由**。路由是加法，缺省（无 token）时 shutdown 关闭、health 无副作用。
- 安全：shutdown 双重约束（loopback 绑定 + 每进程 Bearer）；health 只读、无敏感字段。
- 版本门：外壳声明 `MIN_DSH_VERSION`，低于门则提示升级，避免契约漂移导致误杀进程或误判就绪。
