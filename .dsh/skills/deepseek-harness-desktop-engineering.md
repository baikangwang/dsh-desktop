---
name: deepseek-harness-desktop-engineering
description: dsh-desktop 项目工程知识：Tauri 2 双层结构（Rust 壳 + WebView2 加载 dsh web）、构建与 NSIS 打包、tag→GitHub Actions 发布流程、Windows 运行期目录与子进程环境契约、.ps1 BOM 与中文提交纪律。
whenToUse: 在本项目做开发/构建/打包/发布/排障，需要项目级工程上下文时（尤其涉及 src-tauri、ui、dsd-side、scripts 四处的改动）。
---

# deepseek-harness-desktop-engineering — 项目级 skill（按需加载）

> 属性：项目级（`.dsh/skills/` 官方发现根）。模型按需 `skill("deepseek-harness-desktop-engineering")` 拉取。
> 用途：低频、参考型的**本项目**工程知识——技术栈做法、目录纪律、构建/打包/发布细节、平台差异、已知坑。
>
> **本文件与 `.dsh/profile.yaml` 的分工**：
> **可结构化的项目事实**（路径、命令、技术栈取值、分支与远端事实）→ `profile.yaml`；
> **知识与纪律**（怎么做、为什么这么做、有哪些坑）→ 本 skill。
> 共享契约只写"如何检查"，不写本项目取值；两者都不重复契约里的通用内容。
>
> 本文件全部事实来自对仓库的真实阅读：`package.json`、`dsh-release.json`、
> `src-tauri/{Cargo.toml,tauri.conf.json,src/*.rs}`、`dsd-side/`、`scripts/`、
> `.github/workflows/release.yml`、`docs/{ARCHITECTURE,DEPLOYMENT,INTERFACE_CONTRACT,cicd}.md`、`README.md`。

## 1. 定位与红线（先说不能做什么）

`dsh-desktop` 是 DeepSeek Harness Web GUI 的**桌面外壳**，**不是 fork、不打包 DSH**：

- 启动时经 `npx @deepseek-ai/dsh@<通道目标版本> web --port 0` **spawn 已安装/已缓存的 `dsh web`**，
  解析它 stdout 里的 `dsh web: http://127.0.0.1:<port>`，再把 WebView2 导航过去。
- **三条红线**（`docs/ARCHITECTURE.md` §1）：绝不打包 DSH、绝不加载自带 dist、绝不起第二个服务器。
  原因是只有 `dsh web` 会注入 `window.__DSH_BOOT__`；壳自己注入或自带前端 = 与浏览器版行为分叉。
- **壳与 dsh 运行时各自独立版本**：壳版本在 `src-tauri/tauri.conf.json`（当前 `0.1.1`），
  dsh 版本由启动流程按更新通道（`latest` / `preview`）解析，两者不同步是设计，不是缺陷。
- 领域 UI（树、git 等）**归 DSH 插件**（走 DSH 权限模型、可移植），**不塞进壳**。壳保持极薄。

## 2. 目录与分层（哪一层归谁改）

| 路径 | 是什么 | 改它的要点 |
|---|---|---|
| `src-tauri/` | Tauri 2 Rust 后端（`src/main.rs` 极薄，逻辑在 `lib.rs` + 8 个模块） | 见下表；`[lib] crate-type = staticlib/cdylib/rlib`，`lib.rs` 装配插件与托盘 |
| `ui/` | 外壳自己的静态页：`index.html`（splash / 安装进度 / 通道选择）、`offline.html` | `tauri.conf.json` 的 `build.frontendDist = "../ui"`，**无打包器**：改完不需要构建，重跑 `dev` 即生效 |
| `dsd-side/` | **DSH 侧**最小加法：`@dsh-desktop/dsh-ops`（两个路由：`GET /api/health`、`POST /api/admin/shutdown`）+ `web-surface.patch.yml`（overlay 挂载） | 改这里等于改 DSH 进程内的行为，必须同时想清楚挂载方式（上游化 or overlay） |
| `scripts/` | `dsh-release.mjs`（发布执行层）、`local-setup.ps1`（一次性环境准备） | 发布逻辑的唯一实现处；`release.yml` 由它生成 |
| `skills/release/SKILL.md` | 面向 agent 的发布操作层（识别项目 → 读配置 → 前置检查 → 向用户确认 → 调执行层 → 验证） | 发布纪律写在这里，不写在契约 |
| `docs/` | `ARCHITECTURE.md` / `DEPLOYMENT.md` / `INTERFACE_CONTRACT.md` / `cicd.md` / `DEV_PLAN.md` | 设计文档产出目录（`paths.docs`） |
| `assets/icon.png`、`src-tauri/icons/` | 图标资源 | 打包走 `bundle.icon` 里列的 `icons/icon.ico` + `icons/icon.png` |
| `.tools/`、`.runtime/`、`.npm-cache/`、`.tmp/` | 本地工具与运行数据 | 均在 `.gitignore` 内，**不是交付件、不要提交** |

`src-tauri/src/` 各模块职责（改代码前先定位到模块，别在 `lib.rs` 里堆逻辑）：

| 模块 | 职责 |
|---|---|
| `process_supervisor.rs` | spawn `dsh web`、从 stdout 解析端口、监视退出（独占 `tokio::process::Child`） |
| `dsh_manager.rs` | npx 执行、版本决策（6h 远端缓存 + peerDeps 兼容门）、npx 专属缓存、插件安装 |
| `health.rs` | 就绪/存活探测（TCP connect，300ms 轮询，30s 超时） |
| `lifecycle.rs` | 状态机 Idle→Starting→Ready→Degraded→Error/Stopped、指数退避自愈、优雅关停 |
| `tray.rs` | 托盘图标/菜单/状态色、关窗到托盘 |
| `plugins.rs` | 本地插件安装流程（选 `.tgz` → `dsh plugin --profile web add` → loader entry → 重启 dsh） |
| `commands.rs` | WebView ↔ Rust 的 IPC |
| `config.rs` | `config.json` 读写与日志滚动（`dsh-web.log` / `shell.log`，10MiB × 3 代） |

## 3. 构建 / 运行（真实命令与前提）

```powershell
npm install                  # 只装 @tauri-apps/cli ^2（devDependencies）；Rust 依赖由 cargo 拉
npm run tauri -- dev         # 开发态运行（等价 npm run dev）
npm run tauri -- build       # 出 NSIS 安装包；与 CI、dsh-release.json 三处一致
```

- 环境前提（`README.md` Requirements）：Windows 10 1809+/11 + WebView2 Evergreen、Rust stable (MSVC)
  + VS 2022 Build Tools（含 Windows SDK）、Node ≥ 20。首次全量编译耗时远大于增量（Rust + NSIS 打包）。
- 产物：`src-tauri/target/release/bundle/nsis/*.exe`（`installMode: currentUser`，每用户免管理员）。
  该目录被 `.gitignore` 的 `/src-tauri/target` 忽略。
- **本项目没有测试框架、没有 lint 入口**（`profile.yaml` 的 `tech_stack.test.runner` / `checks.lint` 均为 `none`）。
  **不要臆造 `npm test` / `cargo test` / `cargo clippy` 作为"已通过的验证"**——它们没有接入；
  本项目的等效验证是"`npm run tauri -- build` 通过 + 手工冒烟（启动 → splash → Ready 后导航到 dsh UI）"。
- 清理 Rust 产物（未接入为声明入口，仅作参考）：`cargo clean --manifest-path src-tauri/Cargo.toml`。

## 4. 打包内容与安装布局（改资源清单时必读）

- `tauri.conf.json` 的 `bundle.resources` 把 **DSH 侧资源映射进安装目录**：
  `dsd-side/index.js`、`dsd-side/package.json` → `<安装根>\scripts\dsd-side\`，
  `dsd-side/web-surface.patch.yml` → `<安装根>\scripts\web-surface.patch.yml`。
  **新增/改名 dsd-side 下的文件必须同步改这张映射**，否则安装包里没有它。
- 安装根：`%LOCALAPPDATA%\DeepSeek Harness\`（`productName`）；运行数据：`%LOCALAPPDATA%\DshDesktop\`。
  **后者刻意用无空格路径**：`npm exec` 会截断含空格的参数（`DEPLOYMENT.md` §1）。
- 运行数据目录内容：`logs/`、`config.json`、`cache/{dsh-current,dsh-remote,dsh-channel}.json`、
  `npx-cache/`（DSH 专属 npx 缓存）、`scripts/web-surface.patch.yml`、`bin/pnpm.cmd`（corepack pnpm shim）、`plugins/`。
- `DSH_HOME = %USERPROFILE%\.dsh` 与浏览器版共用 web profile（`profiles/web/`），**零迁移**；
  卸载默认**保留** `~/.dsh`（会话/凭证/设置）。
- 子进程环境契约（`DEPLOYMENT.md` §5）：`PATH` = `<DshDesktop\bin>` + 用户原 PATH；
  `DSH_DESKTOP_SHUTDOWN_TOKEN` 每进程随机（关停鉴权，未设置时 `/api/admin/shutdown` 一律 401）；
  `HTTP(S)_PROXY` / `NO_PROXY` 透传（模型 API 走代理）。
- 离线/内网变体：可把 `DshDesktop\npx-cache` 随安装器预置，首启即缓存命中免联网。

## 5. 发布流程（tag → CI → Release）

三层（`docs/cicd.md`）：**操作层** `skills/release/SKILL.md` → **执行层** `scripts/dsh-release.mjs` →
**CI 层** `.github/workflows/release.yml`。

```powershell
node scripts/dsh-release.mjs --dry-run     # 预演：只显示计划，不改任何东西
node scripts/dsh-release.mjs               # 交互确认后：维护 workflow + 打 tag + push（触发 CI 出包）
```

- 配置唯一源是根 `dsh-release.json`：`kind: tauri`、`versionFile: src-tauri/tauri.conf.json`、
  `tagPrefix: v`、`build: npm run tauri -- build`。**改发布命令只改这里**（CI 模板由它渲染）。
- 三条容易踩的语义：
  1. **普通 `git push` 不触发任何构建**——只有推送 `v*` tag 才触发 `release.yml`；确认点在打 tag 之前。
  2. **已存在的 tag 强制移动不会重新触发** GitHub workflow；同版本重发要先删远程 tag（脚本 `--force` 会删重建）。
  3. `release.yml` 是**生成物**（首行写着 do not edit manually）：改它要改 `scripts/dsh-release.mjs`
     里的模板，否则下次发布会把手工改动覆盖掉。
- 版本一致性实况：CI 只断言 `tag == tauri.conf.json.version`；
  当前 `package.json` 是 `0.1.0` 而 `tauri.conf.json` / `Cargo.toml` 是 `0.1.1`——**已经漂移**。
  `docs/cicd.md` §2 要求"与 package.json 保持一致"，bump 时三处一起改，别只信 CI 的断言。
- CI 细节：`windows-latest` + Node 22 + `dtolnay/rust-toolchain@stable`（target `x86_64-pc-windows-msvc`）
  + `swatinem/rust-cache@v2`（`workspaces: src-tauri`）；版本断言步骤显式 `shell: bash`
  （windows runner 默认 pwsh，断言是 bash 语法）；Release 幂等用 `gh release view` → `upload --clobber`。

## 6. 平台与运行期差异

- **只有 Windows 一种打包目标**（`bundle.targets: ["nsis"]` + WebView2）。没有 macOS/Linux 目标；
  任何跨平台改动都属新增能力，需要先明确产物形态。
- 进程回收按 **PID**（`taskkill /T /F`）而非共享句柄；优雅关停优先走 `/api/admin/shutdown`（每进程 Bearer），
  硬杀只是兜底。改生命周期代码时不要引入第二套终止路径。
- 冷启动（首次/升级）要等 npx 安装（分钟级，进度窗流式呈现），**web 的 30s 就绪超时不适用于安装期**；
  热启动（版本未变）~10–20s 到就绪。
- WebView 导航锁定 `http://127.0.0.1:<port>`，不开放任意 fs/shell IPC（`ARCHITECTURE.md` §6）。
- **dsh 版本门（改 `dsh_manager.rs` / `lifecycle.rs` 前必看）**：
  `MIN_DSH_VERSION = "0.1.0-rc.6"`（`dsh_manager.rs:22`，由 `lifecycle.rs:81` 在启动时校验）；
  `--no-open` 只在 dsh ≥ `0.1.0-rc.8` 时才传入（`dsh_manager.rs:508-509`）——
  rc.6/rc.7 **没有**该参数，无条件传会启动失败。版本门与参数传递必须成对改。

## 7. 本仓库编码与提交纪律

1. **`.ps1` 必须带 UTF-8 BOM**：本仓库 PowerShell 5.1 无 BOM 读中文会乱码（`docs/cicd.md` §4）。
   用 `edit` / `write` 工具改过 `.ps1` 后要确认 BOM 仍在（丢了要补回来）。
2. **中文提交信息不要经 PowerShell**：用 `write`/`edit` 写消息文件 → `git commit -F <文件>`；
   `Get-Content … | git commit -F -` 会按本机代码页 936 误解码 UTF-8 而乱码。
3. **`.env` 未被 `.gitignore` 忽略**（`git status` 显示 `?? .env`）。
   它是 DSH 的项目根凭证层，**不应提交**；提交时只 `git add` 明确列出的路径（如 `git add .dsh`），
   **绝不 `git add -A` / `git add .`**。
4. **`.dsh/tmp/`、`.dsh/reports/` 是模式的临时产物**（链闭环即删），`.gitignore` 目前也没忽略它们——
   所以按具名路径暂存，别整目录扫。
5. 现有凭证落点（不要复制、不要提交）：DSH 真凭证在 `~/.dsh/.credentials.yaml`，**外壳代码从不读取它**；
   本机 `.tmp/phase1-dsh-home/.credentials.yaml` 是被忽略的 `.tmp/` 下的探测残留。
   `push-to-github.ps1` 用 `$env:GITHUB_TOKEN` 一次性鉴权、不写文件，用后应轮换 token。
6. **分支事实**：本地检出 `master`，`origin/HEAD → origin/master`（远端另有 `origin/main`）。
   这与 `_shared/engineering-rules.md`「分支管理」的「AI 不操作 main/master」冲突；
   事实与冲突已登记在 `profile.yaml` 的 `security.branch_policy.conflict_note`，**动代码前先由用户裁决分支**。
   另注：根 `push-to-github.ps1` 默认推 `main`，与当前默认分支 `master` 不一致，是一次性引导脚本的历史遗留。
7. `docs/cicd.md` §2 的示例命令写了 `git add -A && git commit … && git push origin main`——
   与本模式的"不 `git add -A`、不直接操作 master/main"相悖，属该文档的待修内容，**不要照抄执行**。

## 8. 改动前的自检清单（本项目专属）

- 改 `ui/`：不需要构建；确认 `tauri.conf.json` 的 `frontendDist` 仍指向 `../ui`，页面里没有依赖打包器的 import。
- 改 `src-tauri/`：定位到模块；`npm run tauri -- build` 必须能过（这是本项目唯一的自动验证）；
  涉及子进程/关停路径时同时检查 `lifecycle.rs` 与 `process_supervisor.rs` 两条路径一致。
- 改 `dsd-side/`：同步检查 `tauri.conf.json` 的 `bundle.resources` 映射、`web-surface.patch.yml` 的插件 id、
  以及 `docs/INTERFACE_CONTRACT.md` 里两路由的契约（`/api/health`、`/api/admin/shutdown`）。
- 改发布相关：先 `node scripts/dsh-release.mjs --dry-run`；`dsh-release.json` 是唯一配置源；
  不要手改 `release.yml`（生成物）。
- 任何"已完成/已验证"的声称必须有可复现输出：构建命令的退出码、产物路径、安装包文件名。

## 9. 现状与已知漂移（别把计划当成已完成）

- **未做（P5，见 `docs/DEV_PLAN.md` §100-103）**：Authenticode 代码签名（无证书，安装包未签名，
  首次运行会触发 SmartScreen 告警）、插件"卸载"（v1 只做安装/升级，卸载要手工删 profile 配置）、
  config 界面与状态展示细节仍待评估。**不要声称这些已支持。**
- **文档漂移**：`docs/DEV_PLAN.md` 提到的 `scripts/release.ps1` 与
  `ensure-dsh.ps1` **在仓库里不存在**（后者已随"npx 执行 + 专属缓存"的升级模型重构被删除，
  见同文件 §70-78）；`.gitignore` 的注释仍写着 `scripts/ensure-dsh.ps1`；
  DEV_PLAN 的"新环境现状"表说 Rust 与 MSVC 未装（现已装好且可构建，见 `src-tauri/target` 里的既有产物）。
  **引用这些路径或结论前先核实。**
- **版本漂移**：`package.json` 是 `0.1.0`，而 `src-tauri/tauri.conf.json` 与 `src-tauri/Cargo.toml`
  是 `0.1.1`；CI 只断言 `tag == tauri.conf.json.version`（见 §5）。bump 时三处一起改。
- `push-to-github.ps1` 默认远端写死 `https://github.com/baikangwang/dsh-desktop.git`（与 `origin` 一致），
  但推的是 `main`，而本仓库默认分支是 `master`——一次性引导脚本的历史遗留，日常发布不要用它。

