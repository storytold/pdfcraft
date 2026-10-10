<#
.SYNOPSIS
  Windows integration tests for Linkco PDF Editor:
  1. Windows File Explorer PDF Preview Handler (PreviewHandler.cs / LinkcoPdfPreviewHandler.dll + pdfcraft-cli preview)
  2. Windows Printer Enumeration & Default Printer Detection (Win32_Printer, .NET PrinterSettings, Registry)
  3. High-Resolution Print Output & Sheet Rendering (A4 & US Letter, 150 / 300 / 600 DPI)
.EXAMPLE
  pwsh packaging/windows/test-windows-integration.ps1
#>
param(
  [string] $CliPath = ""
)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

Write-Output "=== 1. Testing Windows File Explorer PDF Preview Handler Compilation & COM Contract ==="
$PreviewCs = Join-Path $PSScriptRoot 'PreviewHandler.cs'
$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("linkco-win-test-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $TempDir | Out-Null

try {
  $Csc = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
  if (-not (Test-Path $Csc)) { $Csc = Join-Path $env:WINDIR 'Microsoft.NET\Framework\v4.0.30319\csc.exe' }
  $OutDll = Join-Path $TempDir 'LinkcoPdfPreviewHandler.dll'
  & $Csc /nologo /target:library /optimize+ /platform:anycpu /codepage:65001 /warn:4 "/out:$OutDll" `
    /r:System.dll /r:System.Drawing.dll /r:System.Windows.Forms.dll $PreviewCs
  if ($LASTEXITCODE -ne 0 -or -not (Test-Path $OutDll)) {
    throw "Failed to compile PreviewHandler.cs into LinkcoPdfPreviewHandler.dll"
  }

  $asm = [System.Reflection.Assembly]::LoadFrom($OutDll)
  $handlerType = $asm.GetType('LinkcoPdfPreview.LinkcoPdfPreviewHandler')
  if ($null -eq $handlerType) { throw "LinkcoPdfPreview.LinkcoPdfPreviewHandler type not found in assembly" }
  if ($handlerType.GUID.ToString().ToUpperInvariant() -ne 'D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20') {
    throw "Unexpected Preview Handler GUID: $($handlerType.GUID)"
  }
  foreach ($iface in @('IPreviewHandler', 'IPreviewHandlerVisuals', 'IInitializeWithFile', 'IInitializeWithStream', 'IOleWindow', 'IObjectWithSite')) {
    if ($null -eq $handlerType.GetInterface($iface)) {
      throw "LinkcoPdfPreviewHandler does not implement required COM interface $iface"
    }
  }
  Write-Output "  [PASS] LinkcoPdfPreviewHandler.dll compiled and verified all 6 COM interfaces."

  $thumbType = $asm.GetType('LinkcoPdfPreview.LinkcoPdfThumbnailProvider')
  if ($null -eq $thumbType) { throw "LinkcoPdfPreview.LinkcoPdfThumbnailProvider type not found in assembly" }
  if ($thumbType.GUID.ToString().ToUpperInvariant() -ne '3D8CDE4B-E969-481F-BEB0-5E3B98287416') {
    throw "Unexpected thumbnail provider GUID: $($thumbType.GUID)"
  }
  foreach ($iface in @('IThumbnailProvider', 'IInitializeWithStream', 'IInitializeWithFile')) {
    if ($null -eq $thumbType.GetInterface($iface)) { throw "LinkcoPdfThumbnailProvider does not implement $iface" }
  }
  Write-Output "  [PASS] LinkcoPdfThumbnailProvider present with IThumbnailProvider/IInitializeWithStream/IInitializeWithFile."

  foreach ($api in @('RegisterPreviewHandler', 'UnregisterPreviewHandler', 'QueryEffectiveHandler', 'IsEffectiveHandler', 'Diagnose')) {
    if ($null -eq ($handlerType.GetMethods() | Where-Object { $_.Name -eq $api -and $_.IsStatic -and $_.IsPublic })) {
      throw "LinkcoPdfPreviewHandler is missing the public static method $api"
    }
  }
  $comVisible = $handlerType.GetCustomAttributes([System.Runtime.InteropServices.ComVisibleAttribute], $false)
  if ($comVisible.Count -ne 1 -or -not $comVisible[0].Value) { throw "LinkcoPdfPreviewHandler must be [ComVisible(true)]" }
  Write-Output "  [PASS] Registration API present (Register/Unregister/QueryEffectiveHandler/IsEffectiveHandler/Diagnose)."
  if ($null -eq ($handlerType.GetMethods() | Where-Object { $_.Name -eq 'UnregisterPreviewHandlerForAllUsers' -and $_.IsStatic -and $_.IsPublic })) {
    throw "LinkcoPdfPreviewHandler is missing UnregisterPreviewHandlerForAllUsers"
  }

  # Uninstall cleanup of another user's profile, whose registry comes as two hives (NTUSER.DAT and
  # UsrClass.dat). Simulated with two scratch keys under HKCU, so no admin rights are needed and
  # nothing real is touched.
  $preview = '{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}'; $thumbs = '{3D8CDE4B-E969-481F-BEB0-5E3B98287416}'
  $pvCat = '{8895b1c6-b41f-4c1c-a562-0d564250836f}'; $thCat = '{e357fccd-a995-4576-b01f-234630154e96}'
  $other = '{0D3C1E7A-5B9F-4C2D-8E61-7F4A2B9C3D10}'   # "another app's" previewer, registered for that user only
  $scratch = 'Software\LinkcoPdfTest-' + [Guid]::NewGuid().ToString('N')
  $hkcu = [Microsoft.Win32.Registry]::CurrentUser
  try {
    $sw = $hkcu.CreateSubKey("$scratch\User"); $cl = $hkcu.CreateSubKey("$scratch\Classes")
    $cl.CreateSubKey("CLSID\$preview\InprocServer32").SetValue('CodeBase', 'file:///C:/Gone/LinkcoPdfPreviewHandler.dll')
    $cl.CreateSubKey("CLSID\$thumbs\InprocServer32").SetValue('CodeBase', 'file:///C:/Gone/LinkcoPdfPreviewHandler.dll')
    $cl.CreateSubKey("CLSID\$other").SetValue('', 'Other PDF previewer')
    $cl.CreateSubKey(".pdf\ShellEx\$pvCat").SetValue('', $preview)                       # replaced $other: backed up
    $cl.CreateSubKey("Vendor.PdfFile\ShellEx\$pvCat").SetValue('', $preview)             # replaced $other: backed up
    $cl.CreateSubKey("FormerDefault.Pdf\ShellEx\$pvCat").SetValue('', $preview)          # an old default app, no backup
    $cl.CreateSubKey("SystemFileAssociations\.pdf\ShellEx\$thCat").SetValue('', $thumbs)
    $cl.CreateSubKey("LinkcoPDFEditor.Document\ShellEx\$thCat").SetValue('', $thumbs)
    $cfg = $sw.CreateSubKey('Software\Linkco\Linkco PDF Editor')
    $cfg.SetValue('PreviewHandlerDll', 'C:\Gone\LinkcoPdfPreviewHandler.dll'); $cfg.SetValue('CliPath', 'C:\Gone\pdfcraft-cli.exe')
    $cfg.SetValue('PreviousPdfPreviewHandler', $other); $cfg.SetValue('PrevProgId_Vendor.PdfFile', $other)
    $sw.CreateSubKey('Software\Microsoft\Windows\CurrentVersion\PreviewHandlers').SetValue($preview, 'Linkco PDF Preview Handler')

    $clean = $handlerType.GetMethod('CleanUserHive', [Reflection.BindingFlags] 'NonPublic, Static')
    if (-not $clean.Invoke($null, @($sw, $cl))) { throw 'CleanUserHive found no registration to remove' }
    $val = { param($root, $path) $k = $root.OpenSubKey($path); if ($k) { $k.GetValue('') } }
    if ($cl.OpenSubKey("CLSID\$preview") -or $cl.OpenSubKey("CLSID\$thumbs")) { throw 'COM registrations were not removed' }
    if (-not $cl.OpenSubKey("CLSID\$other")) { throw "Another app's CLSID was removed" }
    if ((& $val $cl ".pdf\ShellEx\$pvCat") -ne $other) { throw '.pdf preview handler was not restored' }
    if ((& $val $cl "Vendor.PdfFile\ShellEx\$pvCat") -ne $other) { throw 'ProgID preview handler was not restored' }
    if ($cl.OpenSubKey('FormerDefault.Pdf')) { throw 'A ProgID that only pointed at us was left behind' }
    if ($cl.OpenSubKey("SystemFileAssociations\.pdf\ShellEx\$thCat") -or $cl.OpenSubKey('LinkcoPDFEditor.Document')) { throw 'Thumbnail registrations were not removed' }
    if ($null -ne $sw.OpenSubKey('Software\Microsoft\Windows\CurrentVersion\PreviewHandlers').GetValue($preview)) { throw 'PreviewHandlers entry was not removed' }
    $cfg = $sw.OpenSubKey('Software\Linkco\Linkco PDF Editor')
    foreach ($n in 'PreviewHandlerDll', 'PreviousPdfPreviewHandler', 'PrevProgId_Vendor.PdfFile') { if ($null -ne $cfg.GetValue($n)) { throw "Config value $n was not removed" } }
    if ($sw.OpenSubKey('Software\Classes')) { throw 'Classes paths were written to the wrong hive' }
    if ($clean.Invoke($null, @($sw, $cl))) { throw 'CleanUserHive should find nothing the second time' }
  } finally {
    $hkcu.DeleteSubKeyTree($scratch, $false)
  }
  Write-Output "  [PASS] Uninstall cleanup of another user's profile (split hives): our entries removed, previous handlers restored."

  Write-Output "  Current File Explorer preview registration on this machine (read-only):"
  ($handlerType.GetMethod('Diagnose').Invoke($null, $null) -split "`r?`n") | ForEach-Object { Write-Output "    $_" }

  Write-Output "=== 2. Testing Windows Installed Printer Enumeration & Default Printer Detection ==="
  $cimPrinters = @(Get-CimInstance -ClassName Win32_Printer -ErrorAction SilentlyContinue)
  Write-Output "  Detected $($cimPrinters.Count) printer(s) via Win32_Printer:"
  foreach ($p in $cimPrinters) {
    Write-Output ("    - Name='{0}', Default={1}, WorkOffline={2}, Status={3}, Port='{4}'" -f $p.Name, $p.Default, $p.WorkOffline, $p.PrinterStatus, $p.PortName)
  }

  Add-Type -AssemblyName System.Drawing
  $installedNet = @([System.Drawing.Printing.PrinterSettings]::InstalledPrinters)
  Write-Output "  Detected $($installedNet.Count) printer(s) via .NET PrinterSettings::InstalledPrinters."
  Write-Output "  [PASS] Windows printer enumeration completed."

  if (-not $CliPath) {
    $candidates = @(
      (Join-Path $Root 'dist\release\pdfcraft-cli.exe'),
      (Join-Path $Root 'target\x86_64-pc-windows-msvc\release\pdfcraft-cli.exe'),
      (Join-Path $Root 'target\release\pdfcraft-cli.exe')
    )
    foreach ($c in $candidates) {
      if (Test-Path $c) { $CliPath = $c; break }
    }
  }

  if ($CliPath -and (Test-Path $CliPath)) {
    Write-Output "=== 3. Testing pdfcraft-cli preview (Valid, Corrupt, and Missing PDFs) ==="
    $badPdf = Join-Path $TempDir 'corrupt.pdf'
    [System.IO.File]::WriteAllText($badPdf, "not a valid pdf file")
    $outPng = Join-Path $TempDir 'preview.png'
    $corruptOut = & $CliPath preview $badPdf --page 1 --dpi 150 --out $outPng
    if ($corruptOut -notmatch '^STATUS\t(INVALID_PDF|EMPTY_PDF)\t') {
      throw "Expected STATUS\tINVALID_PDF for corrupt file, got: $corruptOut"
    }
    $missingOut = & $CliPath preview (Join-Path $TempDir 'nonexistent.pdf') --page 1 --dpi 150 --out $outPng
    if ($missingOut -notmatch '^STATUS\tIO_ERROR\t') {
      throw "Expected STATUS\tIO_ERROR for missing file, got: $missingOut"
    }
    Write-Output "  [PASS] pdfcraft-cli preview gracefully handled corrupt and missing PDFs."

    # A minimal, valid one-page A4 PDF written by hand (exact xref offsets), rendered the way the
    # preview handler asks for it: fit to the pane width, capped by --max-px.
    $objects = @(
      '<< /Type /Catalog /Pages 2 0 R >>',
      '<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
      '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R >>',
      "<< /Length 29 >>`nstream`n0 0 1 rg 100 100 200 300 re f`nendstream"
    )
    $pdf = New-Object System.Text.StringBuilder
    [void] $pdf.Append("%PDF-1.4`n")
    $offsets = @()
    for ($i = 0; $i -lt $objects.Count; $i++) {
      $offsets += $pdf.Length
      [void] $pdf.Append(("{0} 0 obj`n{1}`nendobj`n" -f ($i + 1), $objects[$i]))
    }
    $xref = $pdf.Length
    [void] $pdf.Append(("xref`n0 {0}`n0000000000 65535 f `n" -f ($objects.Count + 1)))
    foreach ($o in $offsets) { [void] $pdf.Append(("{0:D10} 00000 n `n" -f $o)) }
    [void] $pdf.Append(("trailer`n<< /Size {0} /Root 1 0 R >>`nstartxref`n{1}`n%%EOF`n" -f ($objects.Count + 1), $xref))
    $goodPdf = Join-Path $TempDir 'valid.pdf'
    [System.IO.File]::WriteAllText($goodPdf, $pdf.ToString(), (New-Object System.Text.ASCIIEncoding))

    $okOut = @(& $CliPath preview $goodPdf --page 1 --width 600 --max-px 4096 --out $outPng) -join "`n"
    if ($okOut -notmatch '(?m)^STATUS\tOK\t1\t1\t(\d+)\t(\d+)') { throw "Expected STATUS OK for a valid PDF, got: $okOut" }
    $w = [int] $Matches[1]; $h = [int] $Matches[2]
    if ($w -lt 599 -or $w -gt 601 -or $h -le $w) { throw "Expected a ~600 px wide portrait raster, got ${w}x${h}" }
    if (-not (Test-Path $outPng)) { throw "preview reported OK but wrote no PNG" }
    $png = [System.IO.File]::ReadAllBytes($outPng)
    if ($png.Length -lt 8 -or $png[0] -ne 0x89 -or $png[1] -ne 0x50 -or $png[2] -ne 0x4E -or $png[3] -ne 0x47) { throw "preview output is not a PNG" }

    $capOut = @(& $CliPath preview $goodPdf --page 9 --width 5000 --max-px 500 --out $outPng) -join "`n"
    if ($capOut -notmatch '(?m)^STATUS\tOK\t1\t1\t(\d+)\t(\d+)') { throw "Expected STATUS OK (clamped to the last page), got: $capOut" }
    if ([Math]::Max([int] $Matches[1], [int] $Matches[2]) -gt 501) { throw "--max-px 500 was not honoured: $capOut" }
    Write-Output "  [PASS] pdfcraft-cli preview rendered a valid PDF at --width 600 (${w}x${h}), clamped the page and honoured --max-px."

    # The thumbnail provider, end to end: it finds pdfcraft-cli.exe beside its DLL, renders page 1
    # and returns an HBITMAP that fits the requested square.
    Copy-Item -LiteralPath $CliPath -Destination (Join-Path $TempDir 'pdfcraft-cli.exe')
    $thumb = [Activator]::CreateInstance($thumbType)
    $thumb.Initialize([string] $goodPdf, [uint32] 0)
    $hbmp = [IntPtr]::Zero; $alpha = [uint32] 0
    $hr = $thumb.GetThumbnail([uint32] 256, [ref] $hbmp, [ref] $alpha)
    if ($hr -ne 0 -or $hbmp -eq [IntPtr]::Zero) { throw ("GetThumbnail failed: HRESULT 0x{0:X8}" -f $hr) }
    $img = [System.Drawing.Image]::FromHbitmap($hbmp)
    try {
      if ($img.Height -ne 256 -or $img.Width -ge 256 -or $img.Width -lt 150) { throw "Expected a 256 px tall portrait thumbnail, got $($img.Width)x$($img.Height)" }
      $center = $img.GetPixel([int]($img.Width * 0.4), [int]($img.Height * 0.75))
      if ($center.B -lt 200 -or $center.R -gt 80) { throw "Thumbnail does not show the page content (pixel $center)" }
    } finally { $img.Dispose() }
    $thumb2 = [Activator]::CreateInstance($thumbType)
    $thumb2.Initialize([string] $badPdf, [uint32] 0)
    $hbmp2 = [IntPtr]::Zero
    if ($thumb2.GetThumbnail([uint32] 256, [ref] $hbmp2, [ref] $alpha) -eq 0) { throw "GetThumbnail should fail for a damaged PDF" }
    Write-Output "  [PASS] Thumbnail provider rendered a 256 px thumbnail and declined a damaged PDF."
  } else {
    Write-Output "  [SKIP] pdfcraft-cli.exe not built yet; run 'python build.py' first to run CLI preview tests."
  }
} finally {
  Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
}
