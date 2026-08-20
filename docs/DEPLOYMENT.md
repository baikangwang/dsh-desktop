# 安装部署设计

## 1. 目录布局（Windows，每用户免管理员）

```
%LOCALAPPDATA%\DeepSeek Harness\          # 安装根（NSIS currentUser，productName）
├── dsh-desktop.exe
└── scripts\                             # 打包资源：web-surface.patch.yml + dsd-side/（ops overlay）

%LOCALAPPDATA%\DshDesktop\               # 运行数据（data dir，无空格路径）
├── logs\{dsh-web.log, shell.log}
├── config.json
├── cache\{dsh-current.json, dsh-remote.json, dsh-channel.json}   # 版本状态、远端版本缓存、更新通道
├── npx-cache\                           # dsh 专属 npx 缓存（_npx\<hash>\node_modules\@deepseek-ai\dsh）
├── scripts\web-surface.patch.yml        # 无空格暂存（npm exec 会把含空格参数截断）
├── bin\pnpm.cmd                         # corepack pnpm shim（插件安装用）
└── plugins\                             # 本地插件包暂存

%USERPROFILE%\.dsh\                      # DSH_HOME（共享，web/桌面零迁移）
└── profiles\web\                        # web profile：bundle + 用户补丁层 + node_modules（含插件）
```

## 2. 安装流程

1. NSIS per-user 安装器：落盘壳 + resources（scripts/ + dsd-side/）+ 快捷方式/卸载器。
2. 首启（**确认门**）：splash 展示通道选择 → 壳**先联网取版本目标**（`npm view … dist-tags versions`，6h 缓存 `dsh-remote.json`；此时选项禁用）→ **版本号呈现到对应选项**（如 稳定版 rc.7 / 预览版 rc.8；离线则显示"离线/未知"）→ 用户选择（默认上次持久化，首次 `latest`）→ 点**「确认启动」** → 壳才继续（选项即禁用；切换通道=重启应用）。
3. 确认后：版本决策（插件 peerDeps 兼容门）→ 需要则**安装探针**（`node npx-cli --cache … --prefer-offline -y @deepseek-ai/dsh@<目标版本> --version`：无 URL 超时、输出流式到进度窗，冷安装分钟级不被打断；web 的 30s 就绪门不适用于安装期）→ `node npx-cli --cache <DshDesktop\npx-cache> -y @deepseek-ai/dsh@<目标版本> web …`（探针后为缓存命中，秒起）。
4. spawn `dsh web --port 0` → 解析 URL → 健康探测 → 导航 → 显示窗口；成功启动后写 `cache/dsh-current.json`。

## 3. 升级

- **壳升级**：离线包覆盖——关闭应用 → 运行新版 NSIS 安装包 → 重启（无自动化）。
- **dsh 升级**：**重启即更新**——每次启动按所选通道解析目标版本：
  - `latest`（稳定版，默认）：跟随官方 npm `latest` dist-tag；
  - `preview`（预览版）：跟随注册表最高版本号（含预发布，如 `next` 通道的 rc）。
  - 目标版本 ≠ 当前且**插件 peerDeps 兼容**才升级（npx 装新版进缓存，进度窗呈现；不兼容则停在当前版并记录）。
  - **两个通道版本共存**：npx 按 spec 哈希分目录缓存，latest/preview 互不覆盖；误选可重启应用重选，已装版本不丢失、无需重新下载。
- **插件升级**：托盘「安装插件…」选新 `.tgz` → `dsh plugin --profile web add`（覆盖同包名）→ 重启 dsh。

## 4. 回滚与安全

- dsh：旧版本仍在 npx 缓存/`dsh-current.json` 可回溯；升级被兼容门拦住不会破坏插件。
- 优雅关停：`POST /api/admin/shutdown`（每进程 Bearer 密钥）→ dsh 干净退出；`taskkill` 兜底。

## 5. 子进程环境契约

```
PATH     = <DshDesktop\bin> + 用户原 PATH（corepack pnpm shim 供插件安装）
DSH_HOME = %USERPROFILE%\.dsh（config 可覆盖）
DSH_DESKTOP_SHUTDOWN_TOKEN = <每进程随机>          # 关停鉴权
HTTP(S)_PROXY / NO_PROXY = 透传                    # 模型 API 走代理
```

## 6. 卸载

优雅关停 `dsh web` → 移除安装根与 `%LOCALAPPDATA%\DshDesktop\`（含 npx-cache）→ **默认保留 `~\.dsh`**（会话/凭证/设置）。

## 7. 离线/内网变体

默认首启需联网让 npx 安装 dsh；离线场景可将 `DshDesktop\npx-cache` 随安装器预置
（或由管理员提前准备），首启即缓存命中免网络。
