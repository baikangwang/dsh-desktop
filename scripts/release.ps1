# release.ps1 — package a signed release: build (with updater signing), then
# write the Tauri updater manifest (latest.json).
#
# Requires (in the invoking environment):
#   TAURI_SIGNING_PRIVATE_KEY / TAURI_SIGNING_PRIVATE_KEY_PATH
#   TAURI_SIGNING_PRIVATE_KEY_PASSWORD   (if the key has a password)
#   UPDATE_BASE_URL                       (release host base, e.g. https://.../releases)
#   AUTHENTICODE_CERT                     (optional: path to an Authenticode cert)
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\release.ps1

param(
    [string]$Version = (Get-Content (Join-Path $PSScriptRoot "..\src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version
)
$ErrorActionPreference = "Stop"

$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$bundle = "$root\src-tauri\target\release\bundle\nsis"
$installer = "DeepSeek Harness_$Version" + "_x64-setup.exe"
$artifact = Join-Path $bundle $installer

if (-not (Test-Path $artifact)) {
    throw "installer not found ($artifact); run `npm run tauri -- build` first"
}

# 1. Authenticode sign (optional; needs a code-signing cert)
if ($env:AUTHENTICODE_CERT) {
    & signtool sign /f $env:AUTHENTICODE_CERT /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 $artifact
}

# 2. Updater signature: prefer the one tauri produced (createUpdaterArtifacts),
#    otherwise sign with minisign if available.
$sigFile = "$artifact.sig"
if (-not (Test-Path $sigFile)) {
    if (-not $env:MINISIGN_SECRET) {
        throw "no $sigFile and MINISIGN_SECRET not set; rebuild with TAURI_SIGNING_PRIVATE_KEY*"
    }
    & minisign -S -m $artifact -s $env:MINISIGN_SECRET -x $sigFile -t "dsh-desktop v$Version"
    if ($LASTEXITCODE -ne 0) { throw "minisign failed" }
}
if (-not $env:UPDATE_BASE_URL) {
    throw "UPDATE_BASE_URL is required (release host base, e.g. https://example.com/releases)"
}

# 3. Update manifest (upload alongside the artifact)
$manifest = @{
    version   = $Version
    notes     = "Release $Version"
    pub_date  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    platforms = @{
        "windows-x86_64" = @{
            url       = "$env:UPDATE_BASE_URL/$installer"
            signature = (Get-Content $sigFile -Raw).Trim()
        }
    }
} | ConvertTo-Json -Depth 5
$manifestPath = Join-Path $root "latest.json"
Set-Content -Path $manifestPath -Value $manifest -Encoding utf8

Write-Host "[release] wrote $manifestPath; upload it and '$installer' to $env:UPDATE_BASE_URL/"
