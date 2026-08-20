# local-setup.ps1 — one-shot local environment preparation for dsh-desktop.
#
# The shell never bundles DSH: it runs `dsh web` via npx against a dedicated
# cache, so no dsh install step exists here. This script only prepares the
# build environment (Rust + MSVC + Node deps), then optionally builds.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\local-setup.ps1

$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

function Have($cmd) { return $null -ne (Get-Command $cmd -ErrorAction SilentlyContinue) }

Write-Host "== [1/3] Rust toolchain =="
if (-not (Have cargo)) {
    if (Have winget) {
        winget install --id Rustlang.Rustup -e --source winget
    } else {
        $exe = "$env:TEMP\rustup-init.exe"
        Invoke-WebRequest https://win.rustup.rs/x86_64 -OutFile $exe
        & $exe -y --default-toolchain stable --default-host x86_64-pc-windows-msvc
    }
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
}
rustc --version

Write-Host "== [2/3] MSVC Build Tools (link.exe) =="
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
# Windows SDK (rc.exe for the resource compiler)
$sdkRoot = "C:\Program Files (x86)\Windows Kits\10"
if (-not (Test-Path "$sdkRoot\Lib")) {
    Write-Warning "未检测到 Windows SDK（rc.exe / kernel32.lib）。请在 VS Installer 中为 Build Tools 添加“Windows 11 SDK”组件。"
}

Write-Host "== [3/3] npm install (Tauri CLI) =="
npm install

Write-Host ""
Write-Host "环境就绪。"
Write-Host "  开发运行:  npm run tauri -- dev"
Write-Host "  构建安装包: npm run tauri -- build   （NSIS: src-tauri\target\release\bundle\nsis\*.exe）"
Write-Host "  发布:      node scripts\dsh-release.mjs --dry-run  （确认后去掉 --dry-run）"
