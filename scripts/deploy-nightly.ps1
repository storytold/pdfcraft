$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path "$PSScriptRoot\..").Path
Set-Location $Root

Write-Host "==> Setting up environment"
$llvmBin = "C:\Users\user\AppData\Local\Microsoft\WinGet\Packages\MartinStorsjo.LLVM-MinGW.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe\llvm-mingw-20260908-ucrt-x86_64\bin"
$env:Path = "$llvmBin;" + $env:Path

Write-Host "==> Compiling pdfcraft-nightly in release mode"
cargo build --release -p pdfcraft-nightly

Write-Host "==> Compiling pdfcraft-cli in release mode"
cargo build --release -p pdfcraft-cli

$binExe = "$Root\target\release\pdfcraft-nightly.exe"
$cliExe = "$Root\target\release\pdfcraft-cli.exe"
$ico = "$Root\Pdf nightly assets\pdfcraft_nightly.ico"

# 1. Update portable distribution
$portableDir = "$Root\portable-0.9.1\pdfcraft-nightly-windows-x64-portable"
New-Item -ItemType Directory -Force -Path $portableDir | Out-Null
Copy-Item -Force $binExe "$portableDir\pdfcraft-nightly.exe"
Copy-Item -Force $cliExe "$portableDir\pdfcraft-cli.exe"
Copy-Item -Force $ico "$portableDir\pdfcraft_nightly.ico"

if (Test-Path "C:\Program Files\PdfCraft\models") {
    Copy-Item -Recurse -Force "C:\Program Files\PdfCraft\models" "$portableDir\models"
}

# 2. Build official Windows MSI Installer using WiX
Write-Host "==> Building Windows MSI Installer for PdfCraft Nightly"
$distDir = "$Root\dist\release"
New-Item -ItemType Directory -Force -Path $distDir | Out-Null
$version = "0.10.2"
$msiPath = "$distDir\pdfcraft-nightly-$version-windows-x64.msi"

wix build packaging\windows\pdfcraft-nightly.wxs packaging\windows\installer-ui.wxs -arch x64 -d Version=$version -d BinDir=$portableDir -d ModelsDir="$portableDir\models" -d IconPath=$ico -o $msiPath
Write-Host "==> MSI Build Complete: $msiPath"

# Also place in Downloads directory for convenience
$userDownloads = [System.IO.Path]::Combine($env:USERPROFILE, "Downloads")
if (Test-Path $userDownloads) {
    Copy-Item -Force $msiPath "$userDownloads\pdfcraft-nightly-$version-windows-x64.msi"
    Write-Host "==> Copied MSI to Downloads: $userDownloads\pdfcraft-nightly-$version-windows-x64.msi"
}

# 3. DIRECTLY DEPLOY TO INSTALLED LOCATIONS
Get-Process -Name pdfcraft-nightly -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

$installTargets = @(
    "$env:LOCALAPPDATA\Programs\PdfCraft Nightly",
    "C:\Program Files\PdfCraft Nightly"
)

foreach ($target in $installTargets) {
    try {
        if (-not (Test-Path $target)) {
            New-Item -ItemType Directory -Force -Path $target | Out-Null
        }
        Write-Host "==> Deploying updated binaries to: $target"
        Copy-Item -Force $binExe "$target\pdfcraft-nightly.exe"
        Copy-Item -Force $cliExe "$target\pdfcraft-cli.exe"
        Copy-Item -Force $ico "$target\pdfcraft_nightly.ico"
        if (Test-Path "$portableDir\models") {
            Copy-Item -Recurse -Force "$portableDir\models" "$target\models"
        }
        $hash = (Get-FileHash "$target\pdfcraft-nightly.exe" -Algorithm SHA256).Hash
        Write-Host "    [OK] Deployed pdfcraft-nightly.exe to $target (SHA256: $hash)"
    } catch {
        Write-Warning "Could not copy directly to $target (requires elevation or installer): $_"
    }
}

# 4. Create/update Desktop shortcut for immediate access to newest Nightly build
$userDesktop = [Environment]::GetFolderPath('Desktop')
if (Test-Path $userDesktop) {
    $shortcutPath = "$userDesktop\PdfCraft Nightly.lnk"
    $wsh = New-Object -ComObject WScript.Shell
    $sc = $wsh.CreateShortcut($shortcutPath)
    $sc.TargetPath = "$env:LOCALAPPDATA\Programs\PdfCraft Nightly\pdfcraft-nightly.exe"
    $sc.WorkingDirectory = "$env:LOCALAPPDATA\Programs\PdfCraft Nightly"
    $sc.IconLocation = "$env:LOCALAPPDATA\Programs\PdfCraft Nightly\pdfcraft_nightly.ico"
    $sc.Description = "PdfCraft Nightly 0.10.2 (with Page Color Filters & Dark Mode Fixes)"
    $sc.Save()
    Write-Host "==> Desktop shortcut created/updated at: $shortcutPath"
}

Write-Host "==> ALL DEPLOYMENTS COMPLETE!"
Write-Host "MSI Installer: $msiPath"
Write-Host "Portable package updated at: $portableDir"
