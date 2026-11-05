# Shared build-time language metadata. Selection uses UI language, never formatting locale.
$InstallerLanguages = Get-Content (Join-Path $PSScriptRoot '../installer-languages.json') -Raw -Encoding utf8 | ConvertFrom-Json -AsHashtable
$InstallerLanguageKeys = @($InstallerLanguages.Keys)
$InstallerTransformKeys = @($InstallerLanguageKeys | Where-Object { $_ -ne 'en' })
$InstallerLanguageIds = ($InstallerLanguageKeys | ForEach-Object { $InstallerLanguages[$_].language }) -join ','

function Get-InstallerLanguage([int] $LanguageId) {
  if ($LanguageId -lt 0 -or $LanguageId -gt 65535) { return 'en' }
  $primary = $LanguageId -band 0x3ff
  $sublanguage = $LanguageId -shr 10
  foreach ($entry in $InstallerLanguages.GetEnumerator()) {
    if ($entry.Value.primary -eq $primary -and
        (-not $entry.Value.ContainsKey('sublanguages') -or $sublanguage -in $entry.Value.sublanguages)) { return $entry.Key }
  }
  return 'en'
}

function Get-InstallerLanguageCondition([string] $Language) {
  # A LANGID has ten primary-language bits and six sublanguage bits. Burn conditions
  # have no arithmetic/bitwise operator, so enumerate those 64 possible LANGIDs.
  # This also covers regional variants not explicitly listed by .NET CultureInfo.
  if (-not $InstallerLanguages.ContainsKey($Language)) { throw "Unknown installer language: $Language" }
  $entry = $InstallerLanguages[$Language]
  $sublanguages = if ($entry.ContainsKey('sublanguages')) { $entry.sublanguages } else { 0..63 }
  return (($sublanguages | ForEach-Object { "UserUILanguageID = $($entry.primary + 1024 * $_)" }) -join ' OR ')
}

function Write-InstallerRtlUI([string] $Destination) {
  # Mirror original authored dialogs at build time, before the language MST is
  # generated. Native controls keep normal tab order and expose RTL text to AT.
  [xml] $ui = Get-Content (Join-Path $PSScriptRoot 'installer-ui.wxs') -Raw -Encoding utf8
  foreach ($dialog in $ui.Wix.Fragment.UI.Dialog) {
    $dialog.SetAttribute('RightToLeft', 'yes')
    $dialog.SetAttribute('RightAligned', 'yes')
    $dialog.SetAttribute('LeftScroll', 'yes')
    foreach ($control in $dialog.Control) {
      $control.SetAttribute('X', [string] ([int] $dialog.Width - [int] $control.X - [int] $control.Width))
      $control.SetAttribute('RightToLeft', 'yes')
      $control.SetAttribute('RightAligned', 'yes')
      $control.SetAttribute('LeftScroll', 'yes')
    }
  }
  $ui.Save($Destination)
}

function Get-InstallerStrings([string] $Language) {
  if (-not $InstallerLanguages.ContainsKey($Language)) { throw "Unknown installer language: $Language" }
  [xml] $xml = Get-Content (Join-Path $PSScriptRoot "locales/$Language.wxl") -Raw -Encoding utf8
  $strings = @{}
  foreach ($item in $xml.WixLocalization.String) { $strings[$item.Id] = $item.Value }
  return $strings
}
