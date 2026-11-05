# Tiny compile-only payload. Nothing from this fixture is installed or published.
param([ValidateSet('x64', 'x86', 'arm64')] [string] $Arch = 'x64',
      [Parameter(Mandatory)] [string] $Work)
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath $Work) { throw "Fixture directory must be new: $Work" }
$Work = [IO.Path]::GetFullPath($Work)
New-Item -ItemType Directory -Force -Path "$Work/bin/models", "$Work/out", "$Work/build" | Out-Null
foreach ($name in 'pdfcraft.exe', 'pdfcraft-cli.exe', 'pdfcraft-hot-folder.exe', 'LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE', 'LICENSE-document.txt') {
  [IO.File]::WriteAllText("$Work/bin/$name", 'Synthetic localization fixture. Not executable.', [Text.UTF8Encoding]::new($false))
}
foreach ($name in 'synthetic-detection.rten', 'synthetic-recognition.rten') {
  [IO.File]::WriteAllText("$Work/bin/models/$name", 'Synthetic model placeholder.', [Text.UTF8Encoding]::new($false))
}
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
& (Join-Path $PSScriptRoot 'build-localized.ps1') -Arch $Arch -Version 0.6.0 -BundleVersion 0.6.0-languages.1 `
  -BinDir "$Work/bin" -ModelsDir "$Work/bin/models" -IconPath "$root/assets/app-icon/pdfcraft.ico" `
  -Msi "$Work/out/PdfCraft.msi" -Setup "$Work/out/Setup.exe" -Work "$Work/build"
& (Get-Process -Id $PID).Path -NoProfile -File (Join-Path $PSScriptRoot 'test-msi.ps1') "$Work/out/PdfCraft.msi"
if ($LASTEXITCODE -ne 0) { throw 'Native MSI authoring checks failed' }
