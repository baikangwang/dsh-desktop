# ensure-dsh.ps1 — install/update the LATEST dsh into the app's private prefix.
#
# The shell never bundles DSH. On every startup it resolves the latest
# @deepseek-ai/dsh from the registry and, when a newer version exists (and the
# shell's plugin-compatibility check passes), reinstalls the prefix. The
# script is a fast no-op when the requested version is already installed.
#
# Output is streamed (stdout) so the shell can surface install progress.

param(
    [string]$RuntimeDir = "$env:LOCALAPPDATA\Programs\DshDesktop\runtime",
    [string]$DshVersion = "latest",            # "latest" or an explicit version
    [string]$Registry   = "https://registry.npmjs.org/"
)
$ErrorActionPreference = "Stop"

Write-Host "[ensure-dsh] runtime dir: $RuntimeDir"
New-Item -ItemType Directory -Force -Path $RuntimeDir | Out-Null

# Resolve a Node runtime: prefer the bundled one, fall back to system node.
$bundledNode = Join-Path $RuntimeDir "node.exe"
$node = if (Test-Path $bundledNode) { $bundledNode }
        else { (Get-Command node.exe -ErrorAction Stop).Source }

# Resolve npm-cli.js next to that node (bundled npm), else system npm.
$npmCli = Join-Path (Split-Path $node -Parent) "node_modules\npm\bin\npm-cli.js"
$npm = if (Test-Path $npmCli) { $npmCli } else { (Get-Command npm -ErrorAction Stop).Source }

# Resolve the concrete version to install ("latest" -> registry lookup).
$target = $DshVersion
if ($DshVersion -eq "latest") {
    $view = & $node $npm view "@deepseek-ai/dsh@latest" version --registry $Registry --no-audit --no-fund 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $view) {
        throw "failed to resolve latest @deepseek-ai/dsh version from $Registry"
    }
    $target = ($view | Select-Object -Last 1).Trim()
    Write-Host "[ensure-dsh] latest on registry: $target"
}

# Fast path: already at the requested version.
$dshPkg = Join-Path $RuntimeDir "node_modules\@deepseek-ai\dsh"
if (Test-Path (Join-Path $dshPkg "package.json")) {
    $current = (Get-Content (Join-Path $dshPkg "package.json") -Raw | ConvertFrom-Json).version
    if ($current -eq $target) {
        Write-Host "[ensure-dsh] already at @deepseek-ai/dsh@$current; nothing to do"
        exit 0
    }
    Write-Host "[ensure-dsh] upgrading @deepseek-ai/dsh $current -> $target"
} else {
    Write-Host "[ensure-dsh] installing @deepseek-ai/dsh@$target"
}

& $node $npm install --prefix $RuntimeDir `
    "@deepseek-ai/dsh@$target" --registry $Registry --no-audit --no-fund
if ($LASTEXITCODE -ne 0) { throw "npm install of @deepseek-ai/dsh@$target failed (exit $LASTEXITCODE)" }

if (-not (Test-Path (Join-Path $dshPkg "package.json"))) {
    throw "dsh install verification failed: $dshPkg"
}
$ver = (Get-Content (Join-Path $dshPkg "package.json") -Raw | ConvertFrom-Json).version
Write-Host "[ensure-dsh] installed @deepseek-ai/dsh@$ver into $RuntimeDir"
