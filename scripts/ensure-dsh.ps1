# ensure-dsh.ps1 — install a PINNED dsh into the app's private runtime prefix.
#
# Purpose: keep DSH as a separately-versioned npm package (upgrade transparency,
# correct native-dep ABI with the bundled Node) instead of bundling it into the
# installer. Called by the shell on first run (and by dsh_manager.update()).
#
# The shell spawns:
#   <runtime>\node.exe <runtime>\dsh\node_modules\@deepseek-ai\dsh\lib\bin.js web --port 0
# so we only need the package tree, not npm's global bin shims.

param(
    [string]$RuntimeDir = "$env:LOCALAPPDATA\Programs\DshDesktop\runtime",
    [string]$DshVersion = "0.1.0-rc.6",      # pin; bump with the shell's MIN_DSH_VERSION gate
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
if (-not (Test-Path $npmCli)) {
    Write-Host "[ensure-dsh] no bundled npm; using system npm"
    & $node (Get-Command npm -ErrorAction Stop).Source install --prefix $RuntimeDir `
        "@deepseek-ai/dsh@$DshVersion" --registry $Registry --no-audit --no-fund
} else {
    & $node $npmCli install --prefix $RuntimeDir `
        "@deepseek-ai/dsh@$DshVersion" --registry $Registry --no-audit --no-fund
}
if ($LASTEXITCODE -ne 0) { throw "npm install of @deepseek-ai/dsh failed (exit $LASTEXITCODE)" }

$dshPkg = Join-Path $RuntimeDir "node_modules\@deepseek-ai\dsh"
if (-not (Test-Path (Join-Path $dshPkg "package.json"))) {
    throw "dsh install verification failed: $dshPkg"
}
$ver = (Get-Content (Join-Path $dshPkg "package.json") -Raw | ConvertFrom-Json).version
Write-Host "[ensure-dsh] installed @deepseek-ai/dsh@$ver into $RuntimeDir"
