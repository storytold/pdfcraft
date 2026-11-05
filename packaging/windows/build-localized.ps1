<# Build the offline localized MSI/Setup pair from an already validated payload.
   Does not execute an installer. Temporary localized MSIs share a CAB cache and
   ProductCode; only the English base plus small embedded transforms is published. #>
param(
  [Parameter(Mandatory)] [ValidateSet('x64', 'x86', 'arm64')] [string] $Arch,
  [Parameter(Mandatory)] [string] $Version,
  [string] $BundleVersion = $Version,
  [Parameter(Mandatory)] [string] $BinDir,
  [Parameter(Mandatory)] [string] $ModelsDir,
  [Parameter(Mandatory)] [string] $IconPath,
  # Staged craft-fonts OFL notices, and 1 when package.ps1 found any (pdfcraft.wxs installs them).
  # The compile-only fixture embeds no craft-fonts and keeps the defaults.
  [string] $DocsDir = '',
  [ValidateSet(0, 1)] [int] $HasCraftFontLicences = 0,
  [Parameter(Mandatory)] [string] $Msi,
  [Parameter(Mandatory)] [string] $Setup,
  [Parameter(Mandatory)] [string] $Work
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'installer-languages.ps1')
function Invoke-Wix([string[]] $Arguments) {
  & wix @Arguments
  if ($LASTEXITCODE -ne 0) { throw "WiX $($Arguments[0]) failed ($LASTEXITCODE)" }
}
$Work = [IO.Path]::GetFullPath($Work)
New-Item -ItemType Directory -Force -Path $Work | Out-Null
$Msi = [IO.Path]::GetFullPath($Msi)
$Setup = [IO.Path]::GetFullPath($Setup)
$productCode = [Guid]::NewGuid().ToString('B').ToUpperInvariant()
Invoke-Wix @('extension', 'add', '-g', 'WixToolset.Util.wixext/5.0.2')
Invoke-Wix @('extension', 'add', '-g', 'WixToolset.BootstrapperApplications.wixext/5.0.2')
$languageFiles = @{}
$rtlUI = Join-Path $Work 'installer-ui-rtl.wxs'
Write-InstallerRtlUI $rtlUI
foreach ($language in $InstallerLanguageKeys) {
  $path = Join-Path $Work "$language.msi"
  $uiPath = if ($InstallerLanguages[$language].rtl) { $rtlUI } else { Join-Path $PSScriptRoot 'installer-ui.wxs' }
  Invoke-Wix @('build', (Join-Path $PSScriptRoot 'pdfcraft.wxs'), $uiPath,
    '-arch', $Arch, '-ext', 'WixToolset.Util.wixext', '-loc', (Join-Path $PSScriptRoot "locales/$language.wxl"),
    '-cc', (Join-Path $Work 'cab-cache'), '-d', "ProductCode=$productCode", '-d', "Version=$Version",
    '-d', "BinDir=$BinDir", '-d', "ModelsDir=$ModelsDir", '-d', "IconPath=$IconPath",
    '-d', "DocsDir=$DocsDir", '-d', "HasCraftFontLicences=$HasCraftFontLicences", '-o', $path)
  $languageFiles[$language] = $path
  if ($language -ne 'en') {
    Invoke-Wix @('msi', 'transform', '-t', 'language', $languageFiles.en, $path, '-o', (Join-Path $Work "$language.mst"))
    # OCR payloads are large. Keep only the base plus the current localization,
    # not one copy per language on a size-limited release runner.
    Remove-Item -LiteralPath $path
  }
  Remove-Item -LiteralPath ([IO.Path]::ChangeExtension($path, '.wixpdb')) -ErrorAction SilentlyContinue
}
Copy-Item -LiteralPath $languageFiles.en -Destination $Msi -Force
# Add transformations before signing: signature covers every language and all CABs.
& (Get-Process -Id $PID).Path -NoProfile -File (Join-Path $PSScriptRoot 'embed-transforms.ps1') -Msi $Msi -Work $Work
if ($LASTEXITCODE -ne 0) { throw 'Embedding MSI translations failed' }
# A child process releases all Windows Installer database handles before signing.
& (Get-Process -Id $PID).Path -NoProfile -File (Join-Path $PSScriptRoot 'test-localized.ps1') -Msi $Msi -Work $Work
if ($LASTEXITCODE -ne 0) { throw 'Localized MSI validation failed' }
# Inspect service, scope, shortcut, model and font-notice authoring after language embedding,
# before signing. In a child process, so its database handle is gone before signtool opens the MSI.
& (Get-Process -Id $PID).Path -NoProfile -File (Join-Path $PSScriptRoot 'test-msi.ps1') $Msi `
  -CraftFontLicences $HasCraftFontLicences -ExpectedFontNoticesDirectory $DocsDir
if ($LASTEXITCODE -ne 0) { throw 'MSI inspection failed' }
# Check the compiled scope, path and registration contract before signing.
& (Join-Path $PSScriptRoot 'check-install-policy.ps1') -MsiPath $Msi
& (Join-Path $PSScriptRoot 'sign.ps1') $Msi

$properties = @('<Include xmlns="http://wixtoolset.org/schemas/v4/wxs">')
foreach ($language in $InstallerTransformKeys) {
  $condition = Get-InstallerLanguageCondition $language
  $selectorId = 'Language_' + $language.Replace('-', '_')
  $properties += "  <SetVariable Id=`"$selectorId`" Variable=`"LanguageTransform`" Type=`"string`" Value=`":$language.mst`" Condition=`"$condition`" />"
}
$properties += '</Include>'
$include = Join-Path $Work 'language-properties.wxi'
[IO.File]::WriteAllLines($include, $properties, [Text.UTF8Encoding]::new($false))
$unsigned = Join-Path $Work 'setup-unsigned.exe'
Invoke-Wix @('build', (Join-Path $PSScriptRoot 'setup.wxs'), '-arch', $Arch,
  '-ext', 'WixToolset.BootstrapperApplications.wixext', '-d', "Version=$BundleVersion", '-d', "MsiPath=$Msi",
  '-d', "IconPath=$IconPath", '-d', "LanguageProperties=$include", '-o', $unsigned)
# Burn caches its engine for maintenance. Sign it as well as the outer container.
$engine = Join-Path $Work 'setup-engine.exe'
Invoke-Wix @('burn', 'detach', $unsigned, '-engine', $engine)
& (Join-Path $PSScriptRoot 'sign.ps1') $engine
Invoke-Wix @('burn', 'reattach', $unsigned, '-engine', $engine, '-o', $Setup)
& (Join-Path $PSScriptRoot 'sign.ps1') $Setup
& (Get-Process -Id $PID).Path -NoProfile -File (Join-Path $PSScriptRoot 'test-setup.ps1') `
  -Setup $Setup -Msi $Msi -Work (Join-Path $Work 'inspect-setup') -Arch $Arch
if ($LASTEXITCODE -ne 0) { throw 'Offline Setup validation failed' }
