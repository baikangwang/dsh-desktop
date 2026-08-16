# dsd-side：DSH 侧最小改动

参考实现 `@dsh-desktop/dsh-ops`（一个 Cordis 插件），提供外壳需要的两个路由：

- `GET /api/health` —— 就绪 + 状态（pid/port/uptime）
- `POST /api/admin/shutdown` —— 优雅关停（Bearer 鉴权）

详见 `../docs/INTERFACE_CONTRACT.md`。

## 挂载方式

### 方式 A：上游化（推荐）

把 `index.js` 的逻辑并入 `@deepseek-ai/dsh-web-app` 的 bundle patch（一个 DSH PR）。
所有 `dsh web` 开箱即得，外壳无需 `--patch`，只需 `MIN_DSH_VERSION` 门禁。

### 方式 B：overlay（不碰上游，今天可用）

1. 把本目录作为 `@dsh-desktop/dsh-ops` 安装到 web profile 的依赖树：
   ```powershell
   dsh plugin --profile web add <本目录的路径或发布后的包名>
   ```
   （等价效果：复制到 `~/.dsh/profiles/node_modules/@dsh-desktop/dsh-ops/`。）
2. 启动时追加 overlay（外壳 `dsh_manager::command` 已支持传入）：
   ```
   dsh web --patch web-surface.patch.yml
   ```

## 环境变量

| 变量 | 说明 |
|---|---|
| `DSH_DESKTOP_SHUTDOWN_TOKEN` | shutdown 的 Bearer 密钥；外壳每进程随机生成。未设置时 `/api/admin/shutdown` 一律 401。 |
