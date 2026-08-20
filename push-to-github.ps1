# push-to-github.ps1 — 把本项目推送到你的 GitHub 仓库（本机运行）。
#
# 用法（推荐用环境变量传 token，避免写进历史）：
#   $env:GITHUB_TOKEN = "ghp_..."      # 或执行时被询问
#   .\push-to-github.ps1
#
# 注意：token 只用于本次 git 鉴权，不会写入任何提交文件；用完后请立即在
# GitHub 里轮换该 token（Settings → Developer settings → Personal access tokens）。

param(
    [string]$Remote = "https://github.com/baikangwang/dsh-desktop.git",
    [string]$Token = $env:GITHUB_TOKEN
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

if (-not $Token) {
    $Token = Read-Host "请输入 Personal Access Token（不会回显）"
}
if (-not $Token) { throw "缺少 token" }

# 构造带凭据的远端 URL（仅本次进程内使用）
$uri = [System.Uri]$Remote
$authUrl = "$($uri.Scheme)://x-access-token:$Token@$($uri.Host)$($uri.PathAndQuery)"

if (-not (Test-Path .git)) { git init -b main | Out-Null }
git remote remove origin 2>$null
git remote add origin $authUrl

# 远端若已有初始提交（如 GitHub 自动 README），先合并再推。
git fetch origin main 2>$null
if ($LASTEXITCODE -eq 0) {
    git pull --rebase --allow-unrelated-histories origin main 2>$null
}

git push -u origin main
Write-Host ""
Write-Host "推送完成。请立即轮换该 token。"
