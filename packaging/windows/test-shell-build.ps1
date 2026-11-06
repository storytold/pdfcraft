<# Compile/inspect installer metadata using tiny synthetic payloads. Never install or execute it. #>
param([Parameter(Mandatory)] [string] $OutDir)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$stage = Join-Path $OutDir ('shell-fixture-' + [Guid]::NewGuid().ToString('N'))
$models = Join-Path $stage 'models'
New-Item -ItemType Directory -Path $models -Force | Out-Null
foreach ($file in @('pdfcraft.exe', 'pdfcraft-cli.exe')) {
  Set-Content -LiteralPath (Join-Path $stage $file) -Value 'Synthetic metadata fixture; do not execute.' -Encoding utf8
}
foreach ($file in @('detector.rten', 'recognizer.rten')) {
  Set-Content -LiteralPath (Join-Path $models $file) -Value 'Synthetic model placeholder; do not load.' -Encoding utf8
}
foreach ($file in @('LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE')) { Copy-Item -LiteralPath (Join-Path $root $file) -Destination $stage }
Copy-Item -LiteralPath (Join-Path $root 'assets/app-icon/LICENSE-document.txt') -Destination $stage
$msi = Join-Path $stage 'metadata-only-do-not-install.msi'
& wix build (Join-Path $PSScriptRoot 'pdfcraft.wxs') (Join-Path $PSScriptRoot 'installer-ui.wxs') -arch x64 `
  -d Version=0.6.0 -d "BinDir=$stage" -d "ModelsDir=$models" -d "IconPath=$root/assets/app-icon/pdfcraft.ico" -o $msi
if ($LASTEXITCODE -ne 0) { throw 'WiX failed to compile the Explorer metadata fixture' }
& (Join-Path $PSScriptRoot 'test-msi.ps1') $msi
