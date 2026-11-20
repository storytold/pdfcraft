#requires -Version 7.4
<#
.SYNOPSIS
Build the current PdfCraft working tree for Windows without packaging or replacing sources.
.DESCRIPTION
Use Developer PowerShell for Visual Studio with Rust MSVC, the C++ tools and Windows SDK.
Translation and font inputs must be clean Git checkouts at the revisions this source pins.
CheckInputsOnly validates those inputs on any platform without compiling or downloading.
.EXAMPLE
./packaging/windows/build-dev.ps1 -I18nDirectory ../craft-i18n -FontsDirectory ../craft-fonts -RunTests
.EXAMPLE
./packaging/windows/build-dev.ps1 -Release -CheckInputsOnly
#>
[CmdletBinding()]
param(
    [string] $SourceRoot = (Join-Path $PSScriptRoot '../..'),
    [string] $I18nDirectory = $env:CRAFT_I18N_DIR,
    [string] $FontsDirectory = $env:CRAFT_FONTS_DIR,
    [ValidateSet('x64', 'x86', 'arm64')][string] $Architecture = 'x64',
    [string] $TargetDirectory = $env:CARGO_TARGET_DIR,
    [switch] $Release,
    [switch] $RunTests,
    [switch] $DxgiFileOutput,
    [switch] $CheckInputsOnly
)

function Get-PdfCraftDevGitText([string] $Directory, [string[]] $Arguments) {
    $output = & git -C $Directory @Arguments
    if ($LASTEXITCODE -ne 0) { throw "Git input validation failed in $Directory (exit $LASTEXITCODE)." }
    return ($output -join "`n").Trim()
}

function Assert-PdfCraftDevCheckout([string] $Directory, [string] $Revision, [string] $Name) {
    if (-not $Directory -or -not (Test-Path -LiteralPath $Directory -PathType Container)) {
        throw "$Name is missing. Supply its checkout explicitly or set the corresponding CRAFT_*_DIR variable."
    }
    $root = (Resolve-Path -LiteralPath $Directory).ProviderPath
    $gitRoot = Get-PdfCraftDevGitText -Directory $root -Arguments @('rev-parse', '--show-toplevel')
    $comparison = if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
    $canonicalGitRoot = [IO.Path]::TrimEndingDirectorySeparator([IO.Path]::GetFullPath($gitRoot))
    $canonicalInputRoot = [IO.Path]::TrimEndingDirectorySeparator([IO.Path]::GetFullPath($root))
    if (-not $canonicalGitRoot.Equals($canonicalInputRoot, $comparison)) {
        throw "$Name must name the checkout root, not a directory inside another repository."
    }
    $actual = Get-PdfCraftDevGitText -Directory $root -Arguments @('rev-parse', '--verify', 'HEAD')
    if ($actual -ne $Revision) { throw "$Name must be at $Revision; its HEAD is $actual. No checkout was changed." }
    # Ignored/untracked locale files can also be embedded by build.rs. Refuse them instead of
    # claiming that HEAD alone describes the actual build input.
    $changes = Get-PdfCraftDevGitText -Directory $root -Arguments @('status', '--porcelain=v1', '--untracked-files=all', '--ignored=matching')
    if ($changes) { throw "$Name has modified, untracked or ignored files. Use a clean pinned checkout; no files were changed." }
    return $root
}

function Get-PdfCraftDevConfiguration {
    param(
        [Parameter(Mandatory)][string] $SourceRoot,
        [string] $I18nDirectory,
        [string] $FontsDirectory,
        [ValidateSet('x64', 'x86', 'arm64')][string] $Architecture = 'x64',
        [string] $TargetDirectory
    )
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) { throw 'Git is required to verify pinned build inputs.' }
    $root = (Resolve-Path -LiteralPath $SourceRoot).ProviderPath
    foreach ($file in 'Cargo.toml', 'Cargo.lock', 'craft-i18n.rev', '.github/workflows/release.yml') {
        if (-not (Test-Path -LiteralPath (Join-Path $root $file) -PathType Leaf)) {
            throw "Not a PdfCraft source tree: missing $file."
        }
    }
    $i18nRevision = (Get-Content -LiteralPath (Join-Path $root 'craft-i18n.rev') -Raw).Trim()
    if ($i18nRevision -notmatch '^[0-9a-f]{40}$') { throw 'craft-i18n.rev is not a full pinned commit.' }
    $workflow = Get-Content -LiteralPath (Join-Path $root '.github/workflows/release.yml') -Raw
    $fontPins = [regex]::Matches($workflow, '(?m)^\s*craft-fonts-ref:\s*([0-9a-f]{40})\s*$')
    if ($fontPins.Count -ne 1) { throw 'Expected one full craft-fonts-ref commit in the source release workflow.' }
    $fontRevision = $fontPins[0].Groups[1].Value
    $minimumRust = $null
    $inPackage = $false
    foreach ($line in Get-Content -LiteralPath (Join-Path $root 'Cargo.toml')) {
        if ($line -match '^\s*\[') { $inPackage = ($line.Trim() -eq '[workspace.package]'); continue }
        if ($inPackage -and $line -match '^\s*rust-version\s*=\s*"([0-9]+\.[0-9]+(?:\.[0-9]+)?)"') {
            $minimumRust = [version]$Matches[1]
            break
        }
    }
    if (-not $minimumRust) { throw 'Could not read the source workspace rust-version.' }
    $i18n = Assert-PdfCraftDevCheckout $I18nDirectory $i18nRevision 'CRAFT_I18N_DIR'
    $fonts = Assert-PdfCraftDevCheckout $FontsDirectory $fontRevision 'CRAFT_FONTS_DIR'
    foreach ($file in 'locales/tr/common.tsv', 'locales/tr/pdfcraft.tsv') {
        $path = Join-Path $i18n $file
        if (-not (Test-Path -LiteralPath $path -PathType Leaf) -or (Get-Item -LiteralPath $path).Length -eq 0) {
            throw "The pinned translation checkout has no nonempty Turkish catalog: $file"
        }
    }
    # Reuse the same native-manifest/OFL validation the Windows packaging path uses.
    . (Join-Path $root 'packaging/windows/font-notices.ps1')
    $null = @(Get-CraftFontNotices $fonts $true)
    $target = switch ($Architecture) {
        'x64' { 'x86_64-pc-windows-msvc' }
        'x86' { 'i686-pc-windows-msvc' }
        'arm64' { 'aarch64-pc-windows-msvc' }
    }
    $targetRoot = if ($TargetDirectory) { [IO.Path]::GetFullPath($TargetDirectory) } else { Join-Path $root 'target' }
    return [pscustomobject]@{
        SourceRoot = $root
        I18nDirectory = $i18n
        I18nRevision = $i18nRevision
        FontsDirectory = $fonts
        FontsRevision = $fontRevision
        Target = $target
        TargetDirectory = $targetRoot
        MinimumRust = $minimumRust
    }
}

function Assert-PdfCraftDevPrerequisites($Configuration) {
    if (-not $IsWindows) { throw 'The development build requires Windows MSVC. Use -CheckInputsOnly for input validation on this platform.' }
    foreach ($command in 'cargo', 'rustc', 'rustup', 'cl.exe', 'rc.exe') {
        if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
            throw "Missing $command. Use Developer PowerShell with Rust MSVC, the C++ desktop tools and Windows SDK installed."
        }
    }
    $versionText = & rustc --version
    if ($LASTEXITCODE -ne 0 -or $versionText -notmatch '^rustc (\d+\.\d+\.\d+)') { throw 'Could not read the Rust compiler version.' }
    if ([version]$Matches[1] -lt $Configuration.MinimumRust) { throw "Rust $($Configuration.MinimumRust) or later is required by this source." }
    $details = & rustc -vV
    if ($LASTEXITCODE -ne 0 -or -not ($details -match '^host: .+-pc-windows-msvc$')) {
        throw 'Select a Windows MSVC Rust toolchain, for example rustup default stable-msvc.'
    }
    $targets = @(& rustup target list --installed)
    if ($LASTEXITCODE -ne 0 -or $Configuration.Target -notin $targets) {
        throw "Install the requested Rust target first: rustup target add $($Configuration.Target)"
    }
}

function Invoke-PdfCraftDevNative([string] $Command, [string[]] $Arguments) {
    & $Command @Arguments | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "$Command failed with exit code $LASTEXITCODE." }
}

function Prepare-PdfCraftDevDxgi($Configuration) {
    # Prepare hash-verified private copies; never patch Cargo's shared checkout or global config.
    & (Join-Path $Configuration.SourceRoot 'packaging/windows/prepare-dxgi-deps.ps1') `
        -RepositoryRoot $Configuration.SourceRoot
}

function Invoke-PdfCraftDevBuild {
    param(
        [Parameter(Mandatory)] $Configuration,
        [switch] $Release,
        [switch] $RunTests,
        [switch] $DxgiFileOutput
    )
    Assert-PdfCraftDevPrerequisites $Configuration
    $settings = @{
        CRAFT_I18N_DIR = $Configuration.I18nDirectory
        CRAFT_I18N_REQUIRED = '1'
        CRAFT_FONTS_DIR = $Configuration.FontsDirectory
        CRAFT_FONTS_REQUIRED = '1'
        PDFCRAFT_REQUIRE_WINRES = '1'
        CARGO_TARGET_DIR = $Configuration.TargetDirectory
        PDFCRAFT_BUILD_LABEL = $(if ($env:PDFCRAFT_BUILD_LABEL) { $env:PDFCRAFT_BUILD_LABEL } else { 'windows.dev' })
    }
    $previous = @{}
    foreach ($key in $settings.Keys) {
        $previous[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
        [Environment]::SetEnvironmentVariable($key, $settings[$key], 'Process')
    }
    $pushed = $false
    try {
        Push-Location -LiteralPath $Configuration.SourceRoot
        $pushed = $true
        $cargoPrefix = @()
        $nativeFeature = @()
        if ($DxgiFileOutput) {
            $prepared = Prepare-PdfCraftDevDxgi $Configuration
            $cargoPrefix = @('--config', $prepared.ConfigPath)
            $nativeFeature = @('-p', 'pdfcraft-print', '--features', 'pdfcraft-print/dxgi-file-output')
        }
        if ($RunTests) {
            # These are host tests, including when compiling a different Windows architecture.
            $testArguments = $cargoPrefix + @('test', '--locked', '-p', 'pdfcraft-edit', '-p', 'pdfcraft-ocr', '-p', 'pdfcraft-automation') + $nativeFeature
            Invoke-PdfCraftDevNative -Command cargo -Arguments $testArguments
        }
        $buildArguments = $cargoPrefix + @('build', '--locked', '-p', 'pdfcraft', '-p', 'pdfcraft-cli', '--target', $Configuration.Target) + $nativeFeature
        if ($Release) { $buildArguments += '--release' }
        Invoke-PdfCraftDevNative -Command cargo -Arguments $buildArguments
        $profile = if ($Release) { 'release' } else { 'debug' }
        $binaryRoot = Join-Path $Configuration.TargetDirectory "$($Configuration.Target)/$profile"
        foreach ($name in 'pdfcraft.exe', 'pdfcraft-cli.exe') {
            if (-not (Test-Path -LiteralPath (Join-Path $binaryRoot $name) -PathType Leaf)) {
                throw "Cargo returned success but the expected executable is missing: $name"
            }
        }
        Write-Host "Built the current working tree: $($Configuration.SourceRoot)"
        Write-Host "Unsigned development binaries: $binaryRoot"
        return $binaryRoot
    } finally {
        if ($pushed) { Pop-Location }
        foreach ($key in $previous.Keys) {
            [Environment]::SetEnvironmentVariable($key, $previous[$key], 'Process')
        }
    }
}

# Dot-sourcing exposes the helpers for regression tests without starting a build.
if ($MyInvocation.InvocationName -ne '.') {
    Set-StrictMode -Version Latest
    $ErrorActionPreference = 'Stop'
    if (-not $CheckInputsOnly -and -not $IsWindows) {
        throw 'The development build requires Windows MSVC. Use -CheckInputsOnly for input validation on this platform.'
    }
    $configuration = Get-PdfCraftDevConfiguration -SourceRoot $SourceRoot -I18nDirectory $I18nDirectory `
        -FontsDirectory $FontsDirectory -Architecture $Architecture -TargetDirectory $TargetDirectory
    if ($CheckInputsOnly) { $configuration } else {
        Invoke-PdfCraftDevBuild -Configuration $configuration -Release:$Release -RunTests:$RunTests -DxgiFileOutput:$DxgiFileOutput
    }
}
