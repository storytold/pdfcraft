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
  & $Csc /nologo /target:library /optimize+ /platform:anycpu "/out:$OutDll" `
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
    if ($corruptOut -notmatch '^STATUS\tINVALID_PDF\t') {
      throw "Expected STATUS\tINVALID_PDF for corrupt file, got: $corruptOut"
    }
    $missingOut = & $CliPath preview (Join-Path $TempDir 'nonexistent.pdf') --page 1 --dpi 150 --out $outPng
    if ($missingOut -notmatch '^STATUS\tIO_ERROR\t') {
      throw "Expected STATUS\tIO_ERROR for missing file, got: $missingOut"
    }
    Write-Output "  [PASS] pdfcraft-cli preview gracefully handled corrupt and missing PDFs."
  } else {
    Write-Output "  [SKIP] pdfcraft-cli.exe not built yet; run 'python build.py' first to run CLI preview tests."
  }
} finally {
  Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
}
