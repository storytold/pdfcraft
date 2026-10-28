$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path "$PSScriptRoot\..").Path
Set-Location $Root

Write-Host "==> Setting up environment"
$llvmBin = "C:\Users\user\AppData\Local\Microsoft\WinGet\Packages\MartinStorsjo.LLVM-MinGW.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe\llvm-mingw-20260908-ucrt-x86_64\bin"
$env:Path = "$llvmBin;" + $env:Path

Write-Host "==> Compiling pdfcraft-evo in release mode"
cargo build --release -p pdfcraft-evo

Write-Host "==> Compiling pdfcraft-cli in release mode"
cargo build --release -p pdfcraft-cli

$binExe = "$Root\target\release\pdfcraft-evo.exe"
$cliExe = "$Root\target\release\pdfcraft-cli.exe"
$ico = "$Root\assets\app-icon\pdfcraft_evo.ico"

# 1. Update portable distribution
$portableDir = "$Root\portable-0.9.1\pdfcraft-evo-windows-x64-portable"
New-Item -ItemType Directory -Force -Path $portableDir | Out-Null
Copy-Item -Force $binExe "$portableDir\pdfcraft-evo.exe"
Copy-Item -Force $cliExe "$portableDir\pdfcraft-cli.exe"
Copy-Item -Force $ico "$portableDir\pdfcraft_evo.ico"

# Include licenses
if (Test-Path "$Root\assets\app-icon\LICENSE-document.txt") {
    Copy-Item -Force "$Root\assets\app-icon\LICENSE-document.txt" "$portableDir\LICENSE-document.txt"
}
if (Test-Path "$Root\LICENSE-MIT") {
    Copy-Item -Force "$Root\LICENSE-MIT" "$portableDir\LICENSE-MIT"
}
if (Test-Path "$Root\LICENSE-APACHE") {
    Copy-Item -Force "$Root\LICENSE-APACHE" "$portableDir\LICENSE-APACHE"
}
if (Test-Path "$Root\NOTICE") {
    Copy-Item -Force "$Root\NOTICE" "$portableDir\NOTICE"
}

if (Test-Path "C:\Program Files\PdfCraft\models") {
    Copy-Item -Recurse -Force "C:\Program Files\PdfCraft\models" "$portableDir\models"
} elseif (Test-Path "$Root\portable-0.9.1\pdfcraft-nightly-windows-x64-portable\models") {
    Copy-Item -Recurse -Force "$Root\portable-0.9.1\pdfcraft-nightly-windows-x64-portable\models" "$portableDir\models"
} else {
    New-Item -ItemType Directory -Force -Path "$portableDir\models" | Out-Null
}

# 2. Build official Windows MSI Installer using WiX
Write-Host "==> Building Windows MSI Installer for PdfCraft EVO"
$distDir = "$Root\dist\release"
New-Item -ItemType Directory -Force -Path $distDir | Out-Null
$version = "0.13.0"
$msiPath = "$distDir\pdfcraft-evo-$version-windows-x64.msi"

wix build packaging\windows\pdfcraft-evo.wxs packaging\windows\installer-ui.wxs -arch x64 -d Version=$version -d BinDir=$portableDir -d ModelsDir="$portableDir\models" -d IconPath=$ico -o $msiPath
Write-Host "==> MSI Build Complete: $msiPath"

# Also place in Downloads directory for user access
$userDownloads = [System.IO.Path]::Combine($env:USERPROFILE, "Downloads")
if (Test-Path $userDownloads) {
    Copy-Item -Force $msiPath "$userDownloads\pdfcraft-evo-$version-windows-x64.msi"
    Write-Host "==> Copied MSI to Downloads: $userDownloads\pdfcraft-evo-$version-windows-x64.msi"
}

# 3. DIRECTLY DEPLOY TO INSTALLED LOCATIONS
Get-Process -Name pdfcraft-nightly, pdfcraft-evo -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

$installTargets = @(
    "$env:LOCALAPPDATA\Programs\PdfCraft EVO",
    "C:\Program Files\PdfCraft EVO",
    "$env:LOCALAPPDATA\Programs\PdfCraft Nightly"
)

foreach ($target in $installTargets) {
    try {
        if (-not (Test-Path $target)) {
            New-Item -ItemType Directory -Force -Path $target | Out-Null
        }
        Write-Host "==> Deploying updated binaries to: $target"
        Copy-Item -Force $binExe "$target\pdfcraft-evo.exe"
        Copy-Item -Force $binExe "$target\pdfcraft-nightly.exe"
        Copy-Item -Force $cliExe "$target\pdfcraft-cli.exe"
        Copy-Item -Force $ico "$target\pdfcraft_evo.ico"
        Copy-Item -Force $ico "$target\pdfcraft_nightly.ico"
        if (Test-Path "$portableDir\models") {
            Copy-Item -Recurse -Force "$portableDir\models" "$target\models"
        }
        $hash = (Get-FileHash "$target\pdfcraft-evo.exe" -Algorithm SHA256).Hash
        Write-Host "    [OK] Deployed pdfcraft-evo.exe to $target (SHA256: $hash)"
    } catch {
        Write-Warning "Could not copy directly to $($target): $_"
    }
}

# 4. Create/update Desktop shortcut for immediate access to newest EVO build
$userDesktop = [Environment]::GetFolderPath('Desktop')
if (Test-Path $userDesktop) {
    $shortcutPath = "$userDesktop\PdfCraft EVO.lnk"
    $wsh = New-Object -ComObject WScript.Shell
    $sc = $wsh.CreateShortcut($shortcutPath)
    $sc.TargetPath = "$env:LOCALAPPDATA\Programs\PdfCraft EVO\pdfcraft-evo.exe"
    $sc.WorkingDirectory = "$env:LOCALAPPDATA\Programs\PdfCraft EVO"
    $sc.IconLocation = "$env:LOCALAPPDATA\Programs\PdfCraft EVO\pdfcraft_evo.ico"
    $sc.Description = "PdfCraft EVO 0.10.2 (High Performance PDF Workbench)"
    $sc.Save()
    Write-Host "==> Desktop shortcut created/updated at: $shortcutPath"
}

Write-Host "==> ALL DEPLOYMENTS COMPLETE!"
Write-Host "MSI Installer: $msiPath"
Write-Host "Portable package updated at: $portableDir"
