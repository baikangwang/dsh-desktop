# local-setup.ps1 — one-shot environment setup + build, run on YOUR machine
# (the coding sandbox has no outbound TLS, so these steps run locally).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\local-setup.ps1
#
# Does: install Rust (MSVC) + VS Build Tools if missing, npm install, and
# `tauri build` (NSIS installer). Icons are already committed.

$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

function Have($cmd) { return $null -ne (Get-Command $cmd -ErrorAction SilentlyContinue) }

Write-Host "== [1/4] Rust toolchain =="
if (-not (Have cargo)) {
    if (Have winget) {
        winget install --id Rustlang.Rustup -e --source winget
    } else {
        $exe = "$env:TEMP\rustup-init.exe"
        Invoke-WebRequest https://win.rustup.rs/x86_64 -OutFile $exe
        & $exe -y --default-toolchain stable --default-host x86_64-pc-windows-msvc
    }
    # refresh PATH for this session
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
}

Write-Host "== [2/4] MSVC Build Tools (link.exe) =="
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$hasVC = (Test-Path $vswhere) -and (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null)
if (-not $hasVC) {
    Write-Warning "未检测到 MSVC C++ 工具链。尝试用 winget 安装（需要管理员）。"
    if (Have winget) {
        winget install --id Microsoft.VisualStudio.2022.BuildTools -e `
          --override "--add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --passive --wait --norestart"
    } else {
        throw "请手动安装 Visual Studio 2022 Build Tools（勾选“使用 C++ 的桌面开发”）。"
    }
}

Write-Host "== [3/4] npm install (Tauri CLI) =="
npm install

Write-Host "== [4/4] tauri build (NSIS installer) =="
npm run tauri -- build
Write-Host ""
Write-Host "安装包: src-tauri\target\release\bundle\nsis\*.exe"
Write-Host ""
Write-Host "推送远程（另开终端，网络可用的机器上）："
Write-Host '  $env:GITHUB_TOKEN = "ghp_..."'
Write-Host "  .\push-to-github.ps1"
