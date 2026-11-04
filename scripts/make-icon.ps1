# Builds crates\traceview\traceview.ico from the Mac app's icon set, so both
# apps share one icon. Sizes the Mac set has are copied as they are, since
# they're drawn for that size. Windows' in-between sizes are scaled down from
# the next larger one. Each size is stored as a PNG, which Windows 10 and
# later read in icons.
#
#   pwsh -File scripts/make-icon.ps1 C:\Development\TraceView\Resources\AppIcon.iconset
param([Parameter(Mandatory)][string]$IconSet)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$root = Split-Path $PSScriptRoot -Parent
$output = Join-Path $root 'crates\traceview\traceview.ico'

# The Mac set's images by their width. A plain name wins over an @2x copy of
# the same size.
$sources = @{}
Get-ChildItem $IconSet -Filter 'icon_*.png' | Sort-Object { $_.Name -match '@2x' } | ForEach-Object {
    $image = [System.Drawing.Image]::FromFile($_.FullName)
    if (-not $sources.ContainsKey($image.Width)) { $sources[$image.Width] = $_.FullName }
    $image.Dispose()
}

function Get-Png([int]$size) {
    # The commas keep PowerShell from returning the bytes one by one.
    if ($sources.ContainsKey($size)) {
        return , [IO.File]::ReadAllBytes($sources[$size])
    }
    $larger = $sources.Keys | Where-Object { $_ -gt $size } | Sort-Object | Select-Object -First 1
    $source = [System.Drawing.Image]::FromFile($sources[$larger])
    $bitmap = [System.Drawing.Bitmap]::new($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
    $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $edges = [System.Drawing.Imaging.ImageAttributes]::new()
    $edges.SetWrapMode([System.Drawing.Drawing2D.WrapMode]::TileFlipXY)
    $graphics.DrawImage($source, [System.Drawing.Rectangle]::new(0, 0, $size, $size),
        0, 0, $source.Width, $source.Height, [System.Drawing.GraphicsUnit]::Pixel, $edges)
    $stream = [IO.MemoryStream]::new()
    $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
    $graphics.Dispose(); $bitmap.Dispose(); $source.Dispose(); $edges.Dispose()
    return , $stream.ToArray()
}

$sizes = 16, 20, 24, 32, 40, 48, 64, 96, 128, 256
$images = foreach ($size in $sizes) { , (Get-Png $size) }

# An icon file: a header, one 16-byte entry per image, then the images.
$file = [IO.MemoryStream]::new()
$writer = [IO.BinaryWriter]::new($file)
$writer.Write([uint16]0)
$writer.Write([uint16]1)
$writer.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $side = if ($sizes[$i] -ge 256) { 0 } else { $sizes[$i] }
    $writer.Write([byte]$side)
    $writer.Write([byte]$side)
    $writer.Write([byte]0)
    $writer.Write([byte]0)
    $writer.Write([uint16]1)
    $writer.Write([uint16]32)
    $writer.Write([uint32]$images[$i].Length)
    $writer.Write([uint32]$offset)
    $offset += $images[$i].Length
}
foreach ($image in $images) { $writer.Write($image) }
$writer.Flush()
[IO.File]::WriteAllBytes($output, $file.ToArray())
'Wrote {0} ({1:N0} KB, {2} sizes)' -f $output, ($file.Length / 1KB), $sizes.Count
