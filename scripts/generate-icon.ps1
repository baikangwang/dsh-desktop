# generate-icon.ps1 — produce the app icon set without the Tauri CLI / network.
#
# Renders a simple "DSH" brand mark into:
#   assets/icon.png                512x512 source (regenerate via `tauri icon`)
#   src-tauri/icons/icon.png       512x512 window icon
#   src-tauri/icons/icon.ico       multi-size (16..256) ICO for exe/NSIS
#
# Requires .NET System.Drawing (Windows PowerShell 5.1+ or pwsh on Windows).

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$root = Split-Path $PSScriptRoot -Parent
$assets = Join-Path $root "assets"
$icons = Join-Path $root "src-tauri\icons"
New-Item -ItemType Directory -Force -Path $assets, $icons | Out-Null

function New-BrandBitmap([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    $g.Clear([System.Drawing.Color]::FromArgb(255, 13, 17, 23))            # #0d1117
    $accent = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 88, 166, 255))  # #58a6ff
    $g.FillRectangle($accent, 0, 0, $size, [int]($size * 0.06))
    $font = New-Object System.Drawing.Font(
        "Segoe UI", [single]($size * 0.34),
        [System.Drawing.FontStyle]::Bold,
        [System.Drawing.GraphicsUnit]::Pixel)
    $sf = New-Object System.Drawing.StringFormat
    $sf.Alignment = [System.Drawing.StringAlignment]::Center
    $sf.LineAlignment = [System.Drawing.StringAlignment]::Center
    $rect = New-Object System.Drawing.RectangleF(0, [single]($size * 0.03), [single]$size, [single]$size)
    $g.DrawString("DSH", $font, [System.Drawing.Brushes]::White, $rect, $sf)
    $g.Dispose(); $font.Dispose(); $sf.Dispose(); $accent.Dispose()
    return $bmp
}

function Save-Png([System.Drawing.Bitmap]$bmp, [string]$path) {
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
}

# 512 source + window icon
$src = New-BrandBitmap 512
Save-Png $src (Join-Path $assets "icon.png")
Save-Png $src (Join-Path $icons "icon.png")
$src.Dispose()

# Multi-size ICO (PNG-compressed entries)
$sizes = 16, 24, 32, 48, 64, 128, 256
$pngData = @()
foreach ($s in $sizes) {
    $b = New-BrandBitmap $s
    $ms = New-Object System.IO.MemoryStream
    $b.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $pngData += , $ms.ToArray()
    $ms.Dispose(); $b.Dispose()
}

$count = $sizes.Count
$offset = 6 + 16 * $count
$ms2 = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter($ms2)
$bw.Write([UInt16]0); $bw.Write([UInt16]1); $bw.Write([UInt16]$count)
for ($i = 0; $i -lt $count; $i++) {
    $s = $sizes[$i]
    $w = if ($s -ge 256) { 0 } else { $s }
    $h = if ($s -ge 256) { 0 } else { $s }
    $bw.Write([Byte]$w); $bw.Write([Byte]$h)
    $bw.Write([Byte]0); $bw.Write([Byte]0)
    $bw.Write([UInt16]1); $bw.Write([UInt16]32)
    $bw.Write([UInt32]$pngData[$i].Length)
    $bw.Write([UInt32]$offset)
    $offset += $pngData[$i].Length
}
foreach ($d in $pngData) { $bw.Write($d) }
$bw.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $icons "icon.ico"), $ms2.ToArray())
$bw.Dispose(); $ms2.Dispose()

Write-Host "icons written to $icons and $assets"
