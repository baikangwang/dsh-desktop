# 安装部署设计

## 1. 目录布局（Windows，每用户免管理员）

```
%LOCALAPPDATA%\Programs\DshDesktop\        # 安装根（NSIS currentUser）
├── DshDesktop.exe
├── resources\  scripts\                   # 资源 + ensure-dsh.ps1 等
├── runtime\
│   ├── node.exe                           # 捆绑 Node LTS（安装器带入）
│   └── node_modules\@deepseek-ai\dsh\     # 私有 prefix（首启 npm 安装，可版本化）
└── WebView2\                              # Evergreen bootstrapper（Win10 缺失时）

%LOCALAPPDATA%\DshDesktop\                 # 运行数据
├── logs\{dsh-web.log, shell.log}
├── config.json
└── updates\                               # 壳更新暂存

%USERPROFILE%\.dsh\                        # DSH_HOME（沿用，零迁移）
```

## 2. 安装流程

1. NSIS per-user 安装器：落盘壳 + 捆绑 Node + 校验 WebView2（缺失则装 bootstrapper）+ 写快捷方式/卸载器。
2. 首启：`ensure-dsh.ps1` 用捆绑 npm 把 `@deepseek-ai/dsh@<pin>` 装入私有 prefix（进度态）。
3. spawn `dsh web --port 0` → 解析 URL → 健康探测 → 导航 → 显示窗口。

## 3. 升级（两条独立通道，互斥）

- **壳升级**（P2，`tauri-plugin-updater`）：查 `latest.json` → 下载到 `updates\` 暂存 → minisign 验签 → **退出时由 NSIS 静默替换并重启**（"install-and-relaunch"）。Windows 运行中 exe 被锁，故「热」= 运行中后台下载 + 退出时秒级切换。
- **dsh 升级**：停子进程 → 装新版本到 `runtime\dsh-<ver>\` → 翻转 junction `runtime\dsh` → 重启。**版本化目录 + 原子切换**保证可回滚、绝不覆盖运行中文件。

## 4. 回滚与安全

- 保留上一版本目录 + `last-known-good`；新版本启动 N 秒内崩溃自动回退。
- 传输 HTTPS + minisign + SHA256；应用走暂存区，绝不在运行时覆盖。
- 壳升级失败不影响 dsh；dsh 升级失败翻回 junction。

## 5. 子进程环境契约

```
PATH     = <install>\runtime + 用户原 PATH        # 让 dsh 找到 git / pwsh / 代理
DSH_HOME = %USERPROFILE%\.dsh（config 可覆盖）
DSH_DESKTOP_SHUTDOWN_TOKEN = <每进程随机>          # 关停鉴权
HTTP(S)_PROXY / NO_PROXY = 透传                    # 模型 API 走代理
CWD      = %USERPROFILE%
```

## 6. 卸载

优雅关停 `dsh web`（`taskkill` 兜底）→ 移除安装根与 `%LOCALAPPDATA%\DshDesktop\` → **默认保留 `~\.dsh`**（会话/凭证/设置）。

## 7. 离线/内网变体

默认首启需联网装 dsh；离线场景改为安装器内置预编译 `dsh node_modules` 压缩包，解压即用。
