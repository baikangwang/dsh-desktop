# release.ps1 — P2: build, sign, and produce the Tauri updater manifest.
#
# Prerequisites: minisign installed; MINISIGN_SECRET env points to the secret key;
# UPDATE_BASE_URL is the release host base. This is a documented stub for P2 —
# the P1 build path is simply `npm run tauri -- build`.

param(
    [string]$Version = (Get-Content (Join-Path $PSScriptRoot "..\src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version
)
$ErrorActionPreference = "Stop"

$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$artifact = "$root\src-tauri\target\release\bundle\nsis\DshDesktop-Setup-$Version-x64.exe"

if (-not (Test-Path $artifact)) {
    throw "installer not found; run `npm run tauri -- build` first"
}

# 1. Authenticode sign (if a cert is configured)
if ($env:AUTHENTICODE_CERT) {
    & signtool sign /f $env:AUTHENTICODE_CERT /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 $artifact
}

# 2. minisign signature (the updater verifies this against the embedded pubkey)
$sig = & minisign -S -m $artifact -s $env:MINISIGN_SECRET -x "$artifact.sig" -t "dsh-desktop v$Version" 2>&1
if ($LASTEXITCODE -ne 0) { throw $sig }

# 3. update manifest (upload alongside the artifact)
$manifest = @{
    version   = $Version
    notes     = "Release $Version"
    pub_date  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    platforms = @{
        "windows-x86_64" = @{
            url       = "$env:UPDATE_BASE_URL/$Version/DshDesktop-Setup-$Version-x64.exe"
            signature = (Get-Content "$artifact.sig" -Raw).Trim()
        }
    }
} | ConvertTo-Json -Depth 5
Set-Content -Path "$root\latest.json" -Value $manifest -Encoding utf8

Write-Host "[release] wrote latest.json; upload it and the installer to `$UPDATE_BASE_URL/$Version/"
