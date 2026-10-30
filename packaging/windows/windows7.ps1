<# Experimental, unsupported Windows 7 SP1 x64 build (docs/windows7.md), ported from VectorCraft's.
   Build on Windows 10/11 with the VS 2022 x64 C++ tools and Windows SDK.
   From the repository root: powershell -File packaging/windows/windows7.ps1 [-Test]
   The resulting portable ZIP targets Windows 7 SP1 x64 with an OpenGL 3.3 driver.
   The compiler runs on the build host, not on Windows 7. -Test also runs the app, CLI and print
   tests on the build host. #>
param([string] $Toolchain = 'nightly-2026-10-01', [switch] $Test)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$Target = 'x86_64-win7-windows-msvc'
function Invoke-Checked([scriptblock] $Command) {
  & $Command
  if ($LASTEXITCODE -ne 0) { throw "Command failed with exit code $LASTEXITCODE" }
}
# Cargo.lock without windows-link's registry source and checksum lines (and with LF line endings),
# so the lock files with and without the patch below compare equal.
function Get-LockWithoutWindowsLinkSource([string] $Text) {
  ($Text -replace "`r`n", "`n") -replace '(?m)^(name = "windows-link"\nversion = "[^"]*"\n)source = "[^"]*"\nchecksum = "[^"]*"\n', '$1'
}
Push-Location $Root
$FlagName = 'CARGO_TARGET_X86_64_WIN7_WINDOWS_MSVC_RUSTFLAGS'
$OldFlags = [Environment]::GetEnvironmentVariable($FlagName)
$OldWinres = $env:PDFCRAFT_REQUIRE_WINRES
$OldAwsLcBuilder = $env:AWS_LC_SYS_CMAKE_BUILDER
$Lock = Join-Path $Root 'Cargo.lock'
$LockBytes = [IO.File]::ReadAllBytes($Lock)
try {
  Invoke-Checked { rustup toolchain install $Toolchain --profile minimal --component rust-src }
  # windows-sys imports CoTaskMemFree from combase.dll, which Windows 7 lacks. Only this build uses
  # the patched copy in vendor/windows-link (see its PDFCRAFT-PATCH.md); every other build keeps
  # the crates.io crate. Cargo records the patch in Cargo.lock, so resolve once, check that nothing
  # but windows-link's source moved, then build --locked. The finally block restores Cargo.lock.
  $Patch = @('--config', "patch.crates-io.windows-link.path='vendor/windows-link'")
  Invoke-Checked { cargo "+$Toolchain" metadata --format-version 1 @Patch | Out-Null }
  $Before = Get-LockWithoutWindowsLinkSource ([Text.Encoding]::UTF8.GetString($LockBytes))
  $After = Get-LockWithoutWindowsLinkSource ([IO.File]::ReadAllText($Lock))
  if ($Before -cne $After) { throw 'Cargo.lock is out of date: applying the windows-link patch changed other entries' }
  # --no-default-features drops wgpu and the AccessKit adapter from the app, and the CLI's default
  # MCP server with them, so the CLI asks for it back.
  $CargoArgs = @('--locked') + $Patch + @('--target', $Target, '--no-default-features', '--features', 'pdfcraft/windows7,pdfcraft-cli/mcp')
  $Tree = cargo "+$Toolchain" tree @CargoArgs -p pdfcraft -p pdfcraft-cli -e normal --prefix none
  if ($LASTEXITCODE -ne 0) { throw 'dependency inspection failed' }
  # craft-print stays for its job and printer types, but its Windows.Data.Pdf spooler is never
  # called on this target (crates/print/src/spool/windows7.rs); the import check below guards that.
  if ($Tree -match '^(wgpu|wgpu-core|wgpu-hal|accesskit_windows) v') { throw 'unsupported Windows backend in compatibility build' }
  if (-not ($Tree -match '^windows-link v[^ ]+ \(.*vendor[\\/]windows-link\)')) { throw 'the patched windows-link is not in use' }
  # The ordinary pc-windows-msvc standard library requires Windows 10. Rebuild std for win7.
  [Environment]::SetEnvironmentVariable($FlagName, '-C target-feature=+crt-static')
  # aws-lc picks its random source at compile time: built for Windows 8+ it looks up ProcessPrng at
  # run time and aborts when it is missing. Its cc builder targets Windows 7 on a win7 triple (and
  # so uses BCryptGenRandom); its CMake builder doesn't, so never let it fall back to CMake.
  $env:AWS_LC_SYS_CMAKE_BUILDER = '0'
  $env:PDFCRAFT_REQUIRE_WINRES = '1'
  if (-not $env:PDFCRAFT_BUILD_SHA) { $env:PDFCRAFT_BUILD_SHA = (git -C $Root rev-parse HEAD 2>$null) }
  if (-not $env:PDFCRAFT_BUILD_DATE) { $env:PDFCRAFT_BUILD_DATE = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd') }
  $BuildArgs = $CargoArgs + @('-p', 'pdfcraft', '-p', 'pdfcraft-cli')
  Invoke-Checked { cargo "+$Toolchain" build -Z build-std=std,panic_unwind @BuildArgs --release }
  $TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
  $Bin = Join-Path $TargetDir "$Target\release"
  # PE subsystem: 2 = Windows GUI (the app opens no console), 3 = console (the CLI's output reaches it).
  foreach ($Check in @(@('pdfcraft.exe', 2), @('pdfcraft-cli.exe', 3))) {
    $Name = $Check[0]
    $Exe = Join-Path $Bin $Name
    $Bytes = [IO.File]::ReadAllBytes($Exe)
    $Pe = [BitConverter]::ToInt32($Bytes, 0x3c)
    if ([BitConverter]::ToUInt16($Bytes, $Pe + 4) -ne 0x8664) { throw "$Name is not x64" }
    $Major = [BitConverter]::ToUInt16($Bytes, $Pe + 0x48)
    $Minor = [BitConverter]::ToUInt16($Bytes, $Pe + 0x4a)
    if ($Major -gt 6 -or ($Major -eq 6 -and $Minor -gt 1)) { throw "$Name requires subsystem $Major.$Minor" }
    $Subsystem = [BitConverter]::ToUInt16($Bytes, $Pe + 0x5c)
    if ($Subsystem -ne $Check[1]) { throw "$Name has PE subsystem $Subsystem, expected $($Check[1])" }
    $Imports = & dumpbin /nologo /imports $Exe
    if ($LASTEXITCODE -ne 0) { throw "Could not inspect $Name imports" }
    # Regression gate for known loader failures; actual Win7 runtime QA remains necessary.
    # combase.dll and api-ms-win-core-winrt-* are the Windows Runtime (Windows 8+), dcomp.dll is
    # DirectComposition (Windows 8+) and d3d12.dll is Direct3D 12 (Windows 10).
    # The DLLs are listed every time, so a run records what the build imports.
    Write-Output "$Name imports:" ($Imports | Where-Object { $_ -match '^\s+\S+\.dll\s*$' } | ForEach-Object { '  ' + $_.Trim() })
    $Bad = @($Imports | Where-Object { $_ -match '(?i)\b(GetSystemTimePreciseAsFileTime|ProcessPrng|WaitOnAddress|WakeByAddressSingle|WakeByAddressAll)\b|combase\.dll|bcryptprimitives\.dll|d3d12\.dll|dcomp\.dll|api-ms-win-core-path-|api-ms-win-core-winrt-' })
    if ($Bad.Count -gt 0) {
      throw "$Name directly imports a known post-Windows-7 API: $(($Bad | ForEach-Object { $_.Trim() }) -join ', ')"
    }
  }
  Invoke-Checked { & (Join-Path $Bin 'pdfcraft-cli.exe') --version }
  $Stage = Join-Path $TargetDir ('windows7-portable-' + [Guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Force $Stage | Out-Null
  Copy-Item (Join-Path $Bin 'pdfcraft.exe'), (Join-Path $Bin 'pdfcraft-cli.exe') $Stage
  # The same notices as the regular portable zip (package.ps1), plus windows-link's licences.
  Copy-Item README.md, LICENSE-MIT, LICENSE-APACHE, NOTICE, assets/app-icon/LICENSE-document.txt $Stage
  Copy-Item vendor/windows-link/license-mit (Join-Path $Stage 'LICENSE-windows-link-MIT')
  Copy-Item vendor/windows-link/license-apache-2.0 (Join-Path $Stage 'LICENSE-windows-link-Apache-2.0')
  Copy-Item docs/windows7.md (Join-Path $Stage 'WINDOWS7.md')
  # OCR models (#103), as in the regular packages: models\ beside pdfcraft.exe.
  $Models = Join-Path $Stage 'models'
  New-Item -ItemType Directory -Force $Models | Out-Null
  # With the patch and --locked too, so the lock file stays as the build and the tests need it.
  # Best effort in this experimental build: when the model host refuses the download, the ZIP
  # ships without models\ (OCR then can't run) rather than not at all.
  cargo "+$Toolchain" @Patch --locked xtask models $Models
  if ($LASTEXITCODE -ne 0) {
    Write-Warning "the OCR models couldn't be fetched (exit code $LASTEXITCODE): this ZIP has no models folder, so OCR won't run"
    Remove-Item -Recurse -Force $Models
  } else {
    Remove-Item -Force (Join-Path $Models '*.part') -ErrorAction SilentlyContinue
  }
  if ($env:CRAFT_FONTS_DIR) {
    . (Join-Path $PSScriptRoot 'font-notices.ps1')
    foreach ($ofl in @(Get-CraftFontNotices $env:CRAFT_FONTS_DIR (Test-Path Env:CRAFT_FONTS_REQUIRED))) {
      Copy-Item -LiteralPath $ofl.FullName -Destination (Join-Path $Stage "OFL-$($ofl.Directory.Name).txt")
    }
  }
  # portable.txt switches on portable mode, as in the regular portable zip (#157).
  Copy-Item (Join-Path $PSScriptRoot 'portable.txt') $Stage
  New-Item -ItemType Directory -Force dist/release | Out-Null
  $Zip = Join-Path $Root 'dist/release/pdfcraft-windows7-x64-portable.zip'
  Compress-Archive -Path (Join-Path $Stage '*') -DestinationPath $Zip -Force
  Remove-Item -Recurse -Force $Stage
  Get-Item $Zip
  if ($Test) {
    # Exercises the Windows code paths on the build host only; it doesn't prove Windows 7 runtime support.
    $TestArgs = $CargoArgs + @('-p', 'pdfcraft', '-p', 'pdfcraft-cli', '-p', 'pdfcraft-print')
    Invoke-Checked { cargo "+$Toolchain" test -Z build-std=std,panic_unwind,test @TestArgs }
  }
} finally {
  [IO.File]::WriteAllBytes($Lock, $LockBytes)
  [Environment]::SetEnvironmentVariable($FlagName, $OldFlags)
  $env:PDFCRAFT_REQUIRE_WINRES = $OldWinres
  $env:AWS_LC_SYS_CMAKE_BUILDER = $OldAwsLcBuilder
  Pop-Location
}
