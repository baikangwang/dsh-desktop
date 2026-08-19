# release.ps1 — build the offline shell installer and optionally sign it.
#
# The shell upgrade model is "offline package over the old one": distribute the
# NSIS installer, close the app, install over, relaunch. No update manifest.
#
# Requires (optional):
#   AUTHENTICODE_CERT   path to an Authenticode cert (signtool /f)
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\release.ps1

param(
    [string]$Version = (Get-Content (Join-Path $PSScriptRoot "..\src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version
)
$ErrorActionPreference = "Stop"

$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

# 1. Build the installer (the shell never bundles dsh or plugins).
& npm run tauri -- build
if ($LASTEXITCODE -ne 0) { throw "tauri build failed (exit $LASTEXITCODE)" }

$bundle = "$root\src-tauri\target\release\bundle\nsis"
$installer = "DeepSeek Harness_$Version" + "_x64-setup.exe"
$artifact = Join-Path $bundle $installer
if (-not (Test-Path $artifact)) {
    throw "installer not found: $artifact"
}

# 2. Authenticode sign (optional; needs a code-signing cert).
if ($env:AUTHENTICODE_CERT) {
    & signtool sign /f $env:AUTHENTICODE_CERT /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 $artifact
    if ($LASTEXITCODE -ne 0) { throw "signtool failed" }
}

Write-Host "[release] installer ready: $artifact"
Write-Host "[release] distribute this file; users close the app, install over, and relaunch."
