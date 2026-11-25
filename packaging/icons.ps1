<#
.SYNOPSIS
  Regenerate PdfChocolate's Windows and hicolor icons from assets/app-icon/pdfchocolate-512.png.

.DESCRIPTION
  The master is pixel art on a 192-dot grid (3 dots per 8 px at 512 px). Sizes of 64 px and up are
  scaled with nearest neighbour, so the dots stay sharp; smaller ones are smoothed, so no detail
  drops out at random. Writes:

    assets/app-icon/hicolor/<n>x<n>/apps/io.github.latechocolate.pdfchocolate.png   16-512 px
    assets/app-icon/pdfcraft-1024.png                                  1024 px
    assets/app-icon/pdfcraft.ico                                       app icon, 16-256 px (resource 1)
    assets/app-icon/pdfcraft-document.ico                              PDF file icon, 16-256 px (resource 2)

  The PDF file icon is a page with a folded corner, the chocolate bar (the master without its beige
  tile) and a red PDF label, drawn on the same 8 px grid. Only System.Drawing is needed (Windows).
  Afterwards update the sha256 values in ATTRIBUTION.toml and run `cargo xtask assets --write`.

  It also writes the SVGs (pdfcraft.svg, pdfcraft-small.svg, hicolor/scalable/...svg: the dot
  grid as rectangles) and the macOS pdfcraft.icns (PNG entries, 16-1024 px).

.EXAMPLE
  powershell -File packaging/icons.ps1
#>
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$root = Split-Path -Parent $PSScriptRoot
$dir = Join-Path $root 'assets\app-icon'
$src = [System.Drawing.Bitmap]::new((Join-Path $dir 'pdfchocolate-512.png'))

function Resize($image, [int]$size) {
    $bmp = [System.Drawing.Bitmap]::new($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.Clear([System.Drawing.Color]::Transparent)
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::Half
    $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
    $g.InterpolationMode = if ($size -ge 64) { [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor } else { [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic }
    $g.DrawImage($image, 0, 0, $size, $size)
    $g.Dispose()
    return $bmp
}

function PngBytes($bmp) {
    $ms = [System.IO.MemoryStream]::new()
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    return , $ms.ToArray()
}

# An .ico with PNG-compressed entries, 16-256 px, as Windows Vista and later read them.
function Write-Ico($image, [string]$path) {
    $sizes = 16, 20, 24, 32, 40, 48, 64, 128, 256
    $images = foreach ($s in $sizes) { $b = Resize $image $s; , (PngBytes $b); $b.Dispose() }
    $ms = [System.IO.MemoryStream]::new()
    $w = [System.IO.BinaryWriter]::new($ms)
    $w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
    $offset = 6 + 16 * $sizes.Count
    for ($i = 0; $i -lt $sizes.Count; $i++) {
        $s = $sizes[$i]; $data = $images[$i]; $dim = if ($s -ge 256) { 0 } else { $s }
        $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
        $w.Write([uint16]1); $w.Write([uint16]32); $w.Write([uint32]$data.Length); $w.Write([uint32]$offset)
        $offset += $data.Length
    }
    foreach ($data in $images) { $w.Write($data) }
    $w.Flush()
    [IO.File]::WriteAllBytes($path, $ms.ToArray())
    "wrote $path"
}

# 1. App icon: hicolor PNGs, the 1024 px PNG and pdfcraft.ico.
foreach ($size in 16, 24, 32, 48, 64, 128, 256, 512) {
    $bmp = Resize $src $size
    $path = Join-Path $dir "hicolor\${size}x${size}\apps\io.github.latechocolate.pdfchocolate.png"
    [IO.File]::WriteAllBytes($path, (PngBytes $bmp))
    $bmp.Dispose()
    "wrote $path"
}
$big = Resize $src 1024
[IO.File]::WriteAllBytes((Join-Path $dir 'pdfcraft-1024.png'), (PngBytes $big))
$big.Dispose()
Write-Ico $src (Join-Path $dir 'pdfcraft.ico')

# 2. PDF file icon.
function C([string]$hex) { [System.Drawing.ColorTranslator]::FromHtml("#$hex") }
$outline = C '2B140A'; $paper = C 'FFF6E6'; $fold = C 'ECC482'; $lines = C 'DEB070'
$red = C 'C83030'; $redLight = C 'EB645A'; $redDark = C '961E24'

# The bar without the master's beige tile: three colours that occur only in the tile.
$bar = [System.Drawing.Bitmap]::new(512, 512, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$tile = @('FFFFE2AA', 'FFECC482', 'FFDEB070')
for ($y = 0; $y -lt 512; $y++) {
    for ($x = 0; $x -lt 512; $x++) {
        $c = $src.GetPixel($x, $y)
        $k = '{0:X2}{1:X2}{2:X2}{3:X2}' -f $c.A, $c.R, $c.G, $c.B
        if ($c.A -ne 0 -and $tile -notcontains $k) { $bar.SetPixel($x, $y, $c) }
    }
}

$m = [System.Drawing.Bitmap]::new(512, 512, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$g = [System.Drawing.Graphics]::FromImage($m)
$g.Clear([System.Drawing.Color]::Transparent)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::None
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
$g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::Half
function Fill($color, [int]$x, [int]$y, [int]$w, [int]$h) { $br = [System.Drawing.SolidBrush]::new($color); $g.FillRectangle($br, $x, $y, $w, $h); $br.Dispose() }
function Poly($color, [int[]]$pts) {
    $p = for ($i = 0; $i -lt $pts.Count; $i += 2) { [System.Drawing.Point]::new($pts[$i], $pts[$i + 1]) }
    $br = [System.Drawing.SolidBrush]::new($color); $g.FillPolygon($br, [System.Drawing.Point[]]$p); $br.Dispose()
}

# Page 72..440 x 16..496 with a 96 px folded corner and an 8 px outline.
$L = 72; $R = 440; $T = 16; $B = 496; $F = 96
Poly (C '00000033') @(($L + 16), ($T + 16), ($R - $F + 16), ($T + 16), ($R + 16), ($T + $F + 16), ($R + 16), ($B + 8), ($L + 16), ($B + 8))
Poly $outline @($L, $T, ($R - $F), $T, $R, ($T + $F), $R, $B, $L, $B)
Poly $paper @(($L + 8), ($T + 8), ($R - $F - 3), ($T + 8), ($R - 8), ($T + $F + 3), ($R - 8), ($B - 8), ($L + 8), ($B - 8))
Poly $outline @(($R - $F - 4), $T, ($R - $F - 4), ($T + $F + 4), $R, ($T + $F + 4))
Poly $fold @(($R - $F + 4), ($T + 16), ($R - $F + 4), ($T + $F - 4), ($R - 16), ($T + $F - 4))
Fill $lines 112 64 176 16
Fill $lines 112 96 128 16

# The bar at half size (a whole-number scale keeps its dots square).
$g.DrawImage($bar, 136, 104, 256, 256)
$bar.Dispose()

# PDF label, wider than the page.
$bx = 40; $by = 360; $bw = 432; $bh = 112
Fill $outline $bx $by $bw $bh
Fill $red ($bx + 8) ($by + 8) ($bw - 16) ($bh - 16)
Fill $redLight ($bx + 8) ($by + 8) ($bw - 16) 8
Fill $redDark ($bx + 8) ($by + $bh - 16) ($bw - 16) 8

# "PDF" in a 5x7 dot font, 8 px per dot, with a dark drop shadow.
$glyphs = @{
    'P' = @('11110', '10001', '10001', '11110', '10000', '10000', '10000')
    'D' = @('11110', '10001', '10001', '10001', '10001', '10001', '11110')
    'F' = @('11111', '10000', '10000', '11110', '10000', '10000', '10000')
}
$dot = 8; $gap = 16
$tx = [int](256 - (3 * 5 * $dot + 2 * $gap) / 2); $ty = $by + [int](($bh - 7 * $dot) / 2)
foreach ($ch in 'P', 'D', 'F') {
    for ($r = 0; $r -lt 7; $r++) {
        for ($c = 0; $c -lt 5; $c++) {
            if ($glyphs[$ch][$r][$c] -eq '1') {
                Fill $redDark ($tx + $c * $dot + 4) ($ty + $r * $dot + 4) $dot $dot
                Fill $paper ($tx + $c * $dot) ($ty + $r * $dot) $dot $dot
            }
        }
    }
    $tx += 5 * $dot + $gap
}
$g.Dispose()
Write-Ico $m (Join-Path $dir 'pdfcraft-document.ico')
$m.Dispose()

# 3. SVG: the dot grid as rectangles (one per horizontal run of a colour), so the same pixel art
# scales without blur. The master's dots are 8/3 px (192 per side; edges at 0, 3, 5, 8, ...).
# Used for pdfcraft.svg, pdfcraft-small.svg and hicolor/scalable.
$n = 192
function Dot([int]$k) { [int][math]::Floor(($k + 0.5) * 512 / $n) }
$sb = [System.Text.StringBuilder]::new()
[void]$sb.Append("<svg xmlns=`"http://www.w3.org/2000/svg`" width=`"512`" height=`"512`" viewBox=`"0 0 $n $n`" shape-rendering=`"crispEdges`">")
[void]$sb.Append("`n<title>PdfChocolate</title>`n")
for ($y = 0; $y -lt $n; $y++) {
    $x = 0
    while ($x -lt $n) {
        $c = $src.GetPixel((Dot $x), (Dot $y))
        $run = 1
        while ($x + $run -lt $n -and $src.GetPixel((Dot ($x + $run)), (Dot $y)).ToArgb() -eq $c.ToArgb()) { $run++ }
        if ($c.A -ne 0) {
            $fill = '#{0:x2}{1:x2}{2:x2}' -f $c.R, $c.G, $c.B
            $op = if ($c.A -lt 255) { ' fill-opacity="{0}"' -f [math]::Round($c.A / 255, 3).ToString([Globalization.CultureInfo]::InvariantCulture) } else { '' }
            [void]$sb.Append("<rect x=`"$x`" y=`"$y`" width=`"$run`" height=`"1`" fill=`"$fill`"$op/>`n")
        }
        $x += $run
    }
}
[void]$sb.Append("</svg>`n")
$utf8 = [System.Text.UTF8Encoding]::new($false)
foreach ($rel in 'pdfcraft.svg', 'pdfcraft-small.svg', 'hicolor\scalable\apps\io.github.latechocolate.pdfchocolate.svg') {
    $path = Join-Path $dir $rel
    [IO.File]::WriteAllText($path, $sb.ToString().Replace("`r`n", "`n"), $utf8)
    "wrote $path"
}

# 4. macOS .icns with PNG entries (16 px to 1024 px, 1x and 2x).
$icnsTypes = [ordered]@{ 'icp4' = 16; 'icp5' = 32; 'ic11' = 32; 'ic12' = 64; 'ic07' = 128; 'ic13' = 256; 'ic08' = 256; 'ic14' = 512; 'ic09' = 512; 'ic10' = 1024 }
$ms = [System.IO.MemoryStream]::new()
function BE([uint32]$v) { $b = [BitConverter]::GetBytes($v); [Array]::Reverse($b); return , $b }
$body = [System.IO.MemoryStream]::new()
foreach ($t in $icnsTypes.Keys) {
    $b = Resize $src $icnsTypes[$t]; $png = PngBytes $b; $b.Dispose()
    $hdr = [Text.Encoding]::ASCII.GetBytes($t)
    $body.Write($hdr, 0, 4); $len = BE ([uint32]($png.Length + 8)); $body.Write($len, 0, 4); $body.Write($png, 0, $png.Length)
}
$magic = [Text.Encoding]::ASCII.GetBytes('icns'); $ms.Write($magic, 0, 4)
$total = BE ([uint32]($body.Length + 8)); $ms.Write($total, 0, 4)
$body.WriteTo($ms)
[IO.File]::WriteAllBytes((Join-Path $dir 'pdfcraft.icns'), $ms.ToArray())
"wrote $(Join-Path $dir 'pdfcraft.icns')"
$src.Dispose()
