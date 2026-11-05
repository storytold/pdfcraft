# Read the compiled Burn manifest and embedded MSI. Never execute Setup.
param([Parameter(Mandatory)] [string] $Setup, [Parameter(Mandatory)] [string] $Msi,
      [Parameter(Mandatory)] [string] $Work, [ValidateSet('x64', 'x86', 'arm64')] [string] $Arch)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'installer-languages.ps1')
if (Test-Path -LiteralPath $Work) { throw "Inspection directory must be new: $Work" }
& wix burn extract $Setup -o (Join-Path $Work 'payload') -oba (Join-Path $Work 'ba')
if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect compiled Setup' }
[xml] $manifest = Get-Content (Join-Path $Work 'ba/manifest.xml') -Raw -Encoding utf8
$root = $manifest.DocumentElement
$packages = @($root.SelectNodes('//*[local-name()="MsiPackage"]'))
if ($packages.Count -ne 1) { throw 'Setup must contain exactly one MSI' }
$properties = @($packages[0].SelectNodes('*[local-name()="MsiProperty" and @Id="TRANSFORMS"]'))
if ($properties.Count -ne 1 -or $properties[0].Id -ne 'TRANSFORMS' -or $properties[0].Value -ne '[LanguageTransform]') { throw 'Missing native MSI language transform property' }
$searches = @($root.SelectNodes('//*[local-name()="SetVariable"]'))
if ($searches.Count -ne $InstallerTransformKeys.Count) { throw 'Missing compiled language selectors' }
$map = @{}
foreach ($search in $searches) {
  if ($search.Variable -ne 'LanguageTransform' -or $search.Type -ne 'string') { throw 'Unexpected language variable' }
  $language = $search.Value.TrimStart(':').Replace('.mst', '')
  $condition = $search.Condition
  if ($condition -cne (Get-InstallerLanguageCondition $language)) { throw "Unexpected compiled selector for $language" }
  foreach ($match in [regex]::Matches($condition, 'UserUILanguageID = (\d+)')) {
    $id = [int] $match.Groups[1].Value
    if ($map.ContainsKey($id)) { throw "Ambiguous language $id" }
    $map[$id] = $language
  }
}
# Exhaustive verification includes every regional sublanguage, English and all
# unsupported primary languages. No OS preference is changed to run this check.
for ($id = 0; $id -le 65535; $id++) {
  $selected = if ($map.ContainsKey($id)) { $map[$id] } else { 'en' }
  if ($selected -cne (Get-InstallerLanguage $id)) { throw "Incorrect fallback for LANGID $id" }
}
foreach ($case in @(
    @('de-AT','de'), @('fr-CA','fr'), @('es-MX','es'), @('nl-BE','nl'), @('it-CH','it'), @('hu-HU','hu'),
    @('en-AU','en'), @('ja-JP','ja'), @('zh-CN','zh-Hans'), @('zh-SG','zh-Hans'),
    @('zh-TW','zh-Hant'), @('zh-HK','zh-Hant'), @('zh-MO','zh-Hant'), @('zh-Hans','zh-Hans'), @('zh-Hant','zh-Hant'),
    @('hi-IN','hi'), @('bn-BD','bn'), @('pt-PT','pt'), @('pt-BR','pt'), @('ar-EG','ar'),
    @('ru-RU','ru'), @('id-ID','id'), @('ur-PK','ur'), @('ko-KR','ko'), @('tr-TR','tr'), @('pl-PL','en'))) {
  $native = [Globalization.CultureInfo]::GetCultureInfo($case[0])
  if ((Get-InstallerLanguage $native.LCID) -ne $case[1]) { throw "Regional locale failure: $($case[0])" }
}
$msiFiles = @(Get-ChildItem (Join-Path $Work 'payload') -Filter *.msi -Recurse)
if ($msiFiles.Count -ne 1 -or (Get-FileHash $msiFiles[0].FullName).Hash -ne (Get-FileHash $Msi).Hash) { throw 'Embedded MSI is not the validated offline payload' }
if ($root.SelectNodes('//*[@DownloadUrl]').Count -ne 0) { throw 'Setup must not download installation payloads' }
$bytes = [IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $Setup).Path)
$pe = [BitConverter]::ToInt32($bytes, 0x3c)
$machine = [BitConverter]::ToUInt16($bytes, $pe + 4)
$expectedMachine = @{ x64 = 0x8664; x86 = 0x14c; arm64 = 0xaa64 }[$Arch]
if ($machine -ne $expectedMachine) { throw "Setup machine $machine does not match $Arch" }
Write-Output 'ok: offline Setup payload, native architecture, 65,536 LANGIDs and regional/fallback selection'
