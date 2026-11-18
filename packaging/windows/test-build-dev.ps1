#requires -Version 7.4
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'build-dev.ps1')

function Assert-DevCheck([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw "Developer build regression: $Message" }
    $script:checks++
}
function Assert-DevRejected([scriptblock] $Action, [string] $Message) {
    $rejected = $false
    try { & $Action | Out-Null } catch { $rejected = $true }
    Assert-DevCheck $rejected $Message
}
function Invoke-FixtureGit([string] $Root, [string[]] $Arguments) {
    $value = & git -C $Root @Arguments
    if ($LASTEXITCODE -ne 0) { throw 'Synthetic Git fixture setup failed.' }
    return ($value -join "`n").Trim()
}
function Commit-Fixture([string] $Root) {
    $null = Invoke-FixtureGit -Root $Root -Arguments @('add', '--all')
    $null = Invoke-FixtureGit -Root $Root -Arguments @('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet', '-m', 'Synthetic input')
    return Invoke-FixtureGit -Root $Root -Arguments @('rev-parse', 'HEAD')
}

$script:checks = 0
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('pdfcraft-dev-build-' + [guid]::NewGuid().ToString('N'))
$source = Join-Path $temporary 'working tree'
$i18n = Join-Path $temporary 'translation input'
$fonts = Join-Path $temporary 'font input'
$target = Join-Path $temporary 'output'
$environmentNames = @('CRAFT_I18N_DIR', 'CRAFT_I18N_REQUIRED', 'CRAFT_FONTS_DIR', 'CRAFT_FONTS_REQUIRED', 'PDFCRAFT_REQUIRE_WINRES', 'CARGO_TARGET_DIR', 'PDFCRAFT_BUILD_LABEL')
$originalEnvironment = @{}
foreach ($key in $environmentNames) { $originalEnvironment[$key] = [Environment]::GetEnvironmentVariable($key, 'Process') }
$startLocation = (Get-Location).Path

try {
    New-Item -ItemType Directory -Force -Path (Join-Path $source '.github/workflows'), (Join-Path $source 'packaging/windows'), `
        (Join-Path $i18n 'locales/tr'), (Join-Path $fonts 'fonts/core'), (Join-Path $fonts 'fonts/chinese') | Out-Null
    @('[workspace.package]', 'rust-version = "1.95"') | Set-Content -LiteralPath (Join-Path $source 'Cargo.toml')
    'Synthetic lock fixture' | Set-Content -LiteralPath (Join-Path $source 'Cargo.lock')
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'font-notices.ps1') -Destination (Join-Path $source 'packaging/windows/font-notices.ps1')
    'Uncommitted application edit must survive' | Set-Content -LiteralPath (Join-Path $source 'local-edit.txt')
    "`tSave`tKaydet" | Set-Content -LiteralPath (Join-Path $i18n 'locales/tr/common.tsv')
    "@id`tlanguage.name`tTürkçe" | Set-Content -LiteralPath (Join-Path $i18n 'locales/tr/pdfcraft.tsv')
    'Synthetic font notice' | Set-Content -LiteralPath (Join-Path $fonts 'fonts/core/OFL.txt')
    'Synthetic font notice' | Set-Content -LiteralPath (Join-Path $fonts 'fonts/chinese/OFL.txt')
    'Core | Regular | fonts/core/face.ttf | Latn | OFL-1.1 | fonts/core/OFL.txt' | Set-Content -LiteralPath (Join-Path $fonts 'fonts/manifest.txt')
    'Chinese | Regular | fonts/chinese/face.ttf | Hans | OFL-1.1 | fonts/chinese/OFL.txt' | Set-Content -LiteralPath (Join-Path $fonts 'fonts/simplified-chinese-manifest.txt')
    foreach ($root in $i18n, $fonts) { $null = Invoke-FixtureGit -Root $root -Arguments @('init', '--quiet') }
    $i18nRevision = Commit-Fixture $i18n
    $fontRevision = Commit-Fixture $fonts
    $i18nRevision | Set-Content -LiteralPath (Join-Path $source 'craft-i18n.rev')
    "      craft-fonts-ref: $fontRevision" | Set-Content -LiteralPath (Join-Path $source '.github/workflows/release.yml')

    $configuration = Get-PdfCraftDevConfiguration -SourceRoot $source -I18nDirectory $i18n -FontsDirectory $fonts -TargetDirectory $target
    Assert-DevCheck ($configuration.SourceRoot -eq $source) 'configuration must retain the selected working tree'
    Assert-DevCheck ($configuration.Target -eq 'x86_64-pc-windows-msvc') 'x64 must select the MSVC target'
    Assert-DevCheck ($configuration.I18nRevision -eq $i18nRevision -and $configuration.FontsRevision -eq $fontRevision) 'source pins must match both inputs'

    Assert-DevRejected { Assert-PdfCraftDevCheckout $i18n ('0' * 40) 'fixture' } 'mismatched input revision must fail'
    'untracked' | Set-Content -LiteralPath (Join-Path $i18n 'locales/tr/extra.tsv')
    Assert-DevRejected { Get-PdfCraftDevConfiguration -SourceRoot $source -I18nDirectory $i18n -FontsDirectory $fonts } 'untracked catalog data must fail'
    Remove-Item -LiteralPath (Join-Path $i18n 'locales/tr/extra.tsv')
    "`tSave`tChanged" | Set-Content -LiteralPath (Join-Path $i18n 'locales/tr/common.tsv')
    Assert-DevRejected { Get-PdfCraftDevConfiguration -SourceRoot $source -I18nDirectory $i18n -FontsDirectory $fonts } 'dirty tracked catalog data must fail'
    $null = Invoke-FixtureGit -Root $i18n -Arguments @('restore', '--', 'locales/tr/common.tsv')
    Remove-Item -LiteralPath (Join-Path $i18n 'locales/tr/pdfcraft.tsv')
    $missingCatalogRevision = Commit-Fixture $i18n
    $missingCatalogRevision | Set-Content -LiteralPath (Join-Path $source 'craft-i18n.rev')
    Assert-DevRejected { Get-PdfCraftDevConfiguration -SourceRoot $source -I18nDirectory $i18n -FontsDirectory $fonts } 'a matching input pin without Turkish must fail'
    $null = Invoke-FixtureGit -Root $i18n -Arguments @('checkout', '--quiet', '--detach', $i18nRevision)
    $i18nRevision | Set-Content -LiteralPath (Join-Path $source 'craft-i18n.rev')

    # Substitute only the host/tool checks and command runner. The real build function must
    # still set scoped inputs, use the working tree and --locked, and restore state on failure.
    function Assert-PdfCraftDevPrerequisites($Configuration) { }
    $script:nativeCalls = @()
    $script:failNative = $true
    function Invoke-PdfCraftDevNative([string] $Command, [string[]] $Arguments) {
        Assert-DevCheck ((Get-Location).Path -eq $source) 'Cargo must run inside the modified source tree'
        Assert-DevCheck ($env:CRAFT_I18N_REQUIRED -eq '1' -and $env:CRAFT_FONTS_REQUIRED -eq '1') 'required input guards must be enabled'
        Assert-DevCheck ($env:CRAFT_I18N_DIR -eq $i18n -and $env:CRAFT_FONTS_DIR -eq $fonts) 'Cargo must use validated absolute inputs'
        Assert-DevCheck ('--locked' -in $Arguments) 'Cargo must keep the checked-in dependency lock'
        $script:nativeCalls += ,@($Command, $Arguments)
        if ($script:failNative) { throw 'Simulated Cargo failure' }
        if ($Arguments[0] -eq 'build') {
            $profile = if ('--release' -in $Arguments) { 'release' } else { 'debug' }
            $binaryRoot = Join-Path $target "x86_64-pc-windows-msvc/$profile"
            New-Item -ItemType Directory -Force -Path $binaryRoot | Out-Null
            foreach ($name in 'pdfcraft.exe', 'pdfcraft-cli.exe') {
                'Not executable: synthetic build output' | Set-Content -LiteralPath (Join-Path $binaryRoot $name)
            }
        }
    }
    foreach ($key in $environmentNames) { [Environment]::SetEnvironmentVariable($key, "before-$key", 'Process') }
    Assert-DevRejected { Invoke-PdfCraftDevBuild -Configuration $configuration } 'failed Cargo must stop the build'
    foreach ($key in $environmentNames) {
        Assert-DevCheck ([Environment]::GetEnvironmentVariable($key, 'Process') -eq "before-$key") "failed build must restore $key"
    }
    Assert-DevCheck ((Get-Location).Path -eq $startLocation) 'failed build must restore the caller location'
    Assert-DevCheck ((Get-Content -LiteralPath (Join-Path $source 'local-edit.txt')) -eq 'Uncommitted application edit must survive') 'failed build must preserve source edits'

    $script:failNative = $false
    $script:nativeCalls = @()
    $result = Invoke-PdfCraftDevBuild -Configuration $configuration -Release -RunTests
    Assert-DevCheck ($script:nativeCalls.Count -eq 2) 'test and build commands must both run'
    Assert-DevCheck ($script:nativeCalls[0][1][0] -eq 'test' -and $script:nativeCalls[1][1][0] -eq 'build') 'tests must run before the build'
    Assert-DevCheck ($result -eq (Join-Path $target 'x86_64-pc-windows-msvc/release')) 'release output path must be reported'
    foreach ($key in $environmentNames) {
        Assert-DevCheck ([Environment]::GetEnvironmentVariable($key, 'Process') -eq "before-$key") "successful build must restore $key"
    }
    Assert-DevCheck ((Get-Location).Path -eq $startLocation) 'successful build must restore the caller location'

    $script:prepareCount = 0
    $overridePath = Join-Path $target 'private deps/override.toml'
    function Prepare-PdfCraftDevDxgi($Configuration) {
        $script:prepareCount++
        [pscustomobject]@{ ConfigPath = $overridePath }
    }
    # The mode must select both the reviewed source copies and the matching feature for
    # tests/build, with the config before the subcommand (never forwarded to a test binary).
    $script:nativeCalls = @()
    function Invoke-PdfCraftDevNative([string] $Command, [string[]] $Arguments) {
        Assert-DevCheck ($Arguments[0] -eq '--config' -and $Arguments[1] -eq $overridePath) 'DXGI mode must use the private config as a single argument'
        Assert-DevCheck ('--locked' -in $Arguments) 'DXGI mode must preserve the lockfile'
        Assert-DevCheck ('pdfcraft-print/dxgi-file-output' -in $Arguments -and 'pdfcraft-print' -in $Arguments) 'DXGI mode must select the native feature package'
        $script:nativeCalls += ,@($Command, $Arguments)
        if ($Arguments[2] -eq 'build') {
            $binaryRoot = Join-Path $target 'x86_64-pc-windows-msvc/debug'
            New-Item -ItemType Directory -Force -Path $binaryRoot | Out-Null
            foreach ($name in 'pdfcraft.exe', 'pdfcraft-cli.exe') { 'Synthetic output' | Set-Content -LiteralPath (Join-Path $binaryRoot $name) }
        }
    }
    $null = Invoke-PdfCraftDevBuild -Configuration $configuration -DxgiFileOutput -RunTests
    Assert-DevCheck ($script:prepareCount -eq 1 -and $script:nativeCalls.Count -eq 2) 'DXGI preparation must precede exactly the test and build calls'
    Assert-DevCheck ($script:nativeCalls[0][1][2] -eq 'test' -and $script:nativeCalls[1][1][2] -eq 'build') 'DXGI tests must precede the build'
    foreach ($key in $environmentNames) {
        Assert-DevCheck ([Environment]::GetEnvironmentVariable($key, 'Process') -eq "before-$key") "DXGI build must restore $key"
    }
    Write-Output "$script:checks developer build checks passed (synthetic inputs; Cargo and Windows execution not performed)."
} finally {
    Set-Location -LiteralPath $startLocation
    foreach ($key in $environmentNames) {
        [Environment]::SetEnvironmentVariable($key, $originalEnvironment[$key], 'Process')
    }
    if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary -Recurse -Force }
}
