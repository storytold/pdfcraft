<#
.SYNOPSIS
  Build and package PdfCraft Windows x86_64 portable zip with auto-incrementing thai.N version.

.DESCRIPTION
  Finds the highest thai.N in target\pdfcraft-*-thai.*-windows-x86_64-portable.zip,
  increments to thai.(N+1) (starting at thai.1), builds release binaries with static CRT,
  and produces target\pdfcraft-0.10.0-thai.<N>-windows-x86_64-portable.zip.
#>
param(
  [string] $CustomVersion
)

$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
New-Item -ItemType Directory -Force -Path $TargetDir | Out-Null

# Setup PATH for cargo, wix, and rc.exe
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:USERPROFILE\.dotnet\tools;C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64;$env:Path"

# Read base version from Cargo.toml
$BaseVersion = '0.10.1'
$cargoToml = Join-Path $Root 'Cargo.toml'
if (Test-Path $cargoToml) {
  $content = Get-Content $cargoToml -Raw
  if ($content -match 'version\s*=\s*"([^"]+)"') {
    $BaseVersion = $Matches[1]
  }
}

if ($CustomVersion) {
  $Version = $CustomVersion
} else {
  # Scan existing packages in target\ to determine next thai.N for this base version
  $existing = Get-ChildItem -Path $TargetDir -Filter "pdfcraft-$BaseVersion-thai.*-windows-x86_64-portable.zip" -ErrorAction SilentlyContinue
  $maxNum = 0
  foreach ($f in $existing) {
    if ($f.Name -match 'thai\.(\d+)') {
      $num = [int]$Matches[1]
      if ($num -gt $maxNum) { $maxNum = $num }
    }
  }
  $nextNum = $maxNum + 1
  $Version = "$BaseVersion-thai.$nextNum"
}

$ZipName = "pdfcraft-$Version-windows-x86_64-portable.zip"
$FinalZip = Join-Path $TargetDir $ZipName

Write-Output "==> Packaging version: $Version"
Write-Output "==> Target zip: $FinalZip"

# 1. Compile release binaries with static CRT
$Target = 'x86_64-pc-windows-msvc'
$flagVar = 'CARGO_TARGET_' + ($Target.ToUpper() -replace '-', '_') + '_RUSTFLAGS'
[Environment]::SetEnvironmentVariable($flagVar, '-C target-feature=+crt-static')
$env:PDFCRAFT_REQUIRE_WINRES = '1'

Write-Output "==> Building release binaries for $Target..."
try {
  cargo build --release --locked -p pdfcraft -p pdfcraft-cli --target $Target
  if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
} finally {
  [Environment]::SetEnvironmentVariable($flagVar, $null)
}

# Small delay to ensure file handles are released before package script runs
Start-Sleep -Seconds 1

# 2. Package using package.ps1 with -SkipBuild
$prevVersion = $env:PDFCRAFT_VERSION
try {
  $env:PDFCRAFT_VERSION = $Version
  & (Join-Path $PSScriptRoot 'package.ps1') -Arch x64 -SkipBuild
  if ($LASTEXITCODE -ne 0) { throw "package.ps1 failed with exit code $LASTEXITCODE" }

  # 3. Copy the portable zip to target with the x86_64 naming convention
  $Dist = if ($env:DIST) { $env:DIST } else { Join-Path $Root 'dist\release' }
  $BuiltZip = Join-Path $Dist "pdfcraft-$Version-windows-x64-portable.zip"

  if (Test-Path $BuiltZip) {
    Copy-Item -Force $BuiltZip $FinalZip
    Write-Output "==> Successfully created $FinalZip ($(Get-Item $FinalZip).Length bytes)"
  } else {
    throw "Expected built zip not found at $BuiltZip"
  }
} finally {
  $env:PDFCRAFT_VERSION = $prevVersion
}

Get-Item $FinalZip | Format-Table FullName, Length, LastWriteTime
