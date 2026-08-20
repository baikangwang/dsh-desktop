# dsh-desktop 打包 / 发布流程

> 对齐 DSH 插件生态的多项目发布方案（deepseek-harness-UI `docs/cicd.md`）：
> 「确认后一键发布」——本地脚本自动打 tag，推送 `v*` tag 自动触发本仓库
> workflow（构建 → NSIS 安装包 → GitHub Release）。

## 1. 三层结构

| 层 | 载体 | 职责 |
|---|---|---|
| 操作层 | `skills/release/SKILL.md` | agent 识别项目、读配置、前置检查、**向用户确认**、调执行层、验证 |
| 执行层 | `scripts/dsh-release.mjs`（幂等） | 读 `dsh-release.json`（版本源 `src-tauri/tauri.conf.json`）→ 自动打 tag + push → 维护 `release.yml`（缺失生成/不一致覆盖） |
| CI 层 | `.github/workflows/release.yml`（生成，唯一 workflow） | `v*` tag → 版本断言 → `npm run tauri -- build` → NSIS 上传 GitHub Release |

项目差异收敛在 `dsh-release.json`：

```jsonc
{
  "kind": "tauri",                     // 桌面壳形态（windows + NSIS）
  "packageName": "dsh-desktop",
  "versionFile": "src-tauri/tauri.conf.json",  // 版本来源
  "tagPrefix": "v",
  "build": "npm run tauri -- build"
}
```

**关键语义**：普通 `git push` 不触发任何构建；只有推送 `v*` tag 才触发
`release.yml` 发布。**确认点在打 tag 之前**。注：GitHub 对**已存在的 tag
强制移动不会重新触发**——同版本重发需先删除远程 tag 再推送（或用 `--force`
删除重建，且版本需先 bump 产生新 tag）。

## 2. 使用

```powershell
# 1) bump 版本并提交推送（普通推送，零触发）
#    编辑 src-tauri/tauri.conf.json 的 "version"（与 package.json 保持一致）
git add -A && git commit -m "chore: bump to 0.1.1"
git push origin main

# 2) 确认发布（自动读版本、自动打 tag）
node scripts/dsh-release.mjs            # 交互：显示"将发布 v0.1.1" → 按 Y
#    预演：node scripts/dsh-release.mjs --dry-run（不改任何东西）
#    仅生成 workflow 不发布：node scripts/dsh-release.mjs --emit-workflow .github/workflows/release.yml

# 3) 完成——tag 推送自动触发 release workflow，NSIS 安装包挂到 Release 页面
```

## 3. 幂等行为

| 对象 | 已存在 | 不存在 |
|---|---|---|
| `v<版本>` tag | 跳过（`--force` 强制重建重发） | 自动创建 + 推送 |
| `.github/workflows/release.yml` | 一致 keep；不一致**覆盖** | 自动生成 |
| GitHub Release | workflow 内复用更新（clobber） | workflow 内创建 |

## 4. CI 注意（沿用 DSH 生态教训）

- actions 用 `@v5` / `setup-node@v5`（Node 22）；GitHub 已弃用 Node 20 runner
- secret 放 **job 级 env**（step 的 `if` 条件看不到 step 自身 env / secrets）
- `gh` CLI 在 runner 预装，配 `GH_TOKEN: ${{ github.token }}`；Release 幂等用
  `gh release view` → `upload --clobber`
- workflow 改动后旧运行记录不会更新——排查先核对 commit hash
- 本仓库 `.ps1` 均带 UTF-8 BOM（PS5.1 无 BOM 读中文会乱码）；`dsh-release.mjs`
  用 edit 工具改过后需重新加 BOM（若改动）

## 5. 本地 vs CI

| 步骤 | 本地 | CI |
|---|---|---|
| 依赖 | `npm install` | `npm install` |
| 构建 | `npm run tauri -- build` | 同（windows-latest） |
| 产物 | `src-tauri/target/release/bundle/nsis/*.exe` | 同 → 上传 Release |
| 版本 | 手动 bump `tauri.conf.json` | 校验 tag == tauri.conf.json version |
