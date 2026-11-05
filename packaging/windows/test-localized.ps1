# Inspect all compiled native dialogs and apply each actual embedded MST to a
# disposable MSI database. No product is installed and no custom action is run.
param([Parameter(Mandatory)] [string] $Msi, [Parameter(Mandatory)] [string] $Work)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'installer-languages.ps1')
$installer = New-Object -ComObject WindowsInstaller.Installer
function Read-Value($Database, [string] $Sql) {
  $view = $Database.OpenView($Sql)
  $record = $null
  try {
    [void] $view.Execute()
    $record = $view.Fetch()
    if (-not $record) { throw "Missing row: $Sql" }
    return $record.StringData(1)
  } finally {
    if ($record) { [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($record) }
    [void] $view.Close()
    [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($view)
  }
}
function Equal($Actual, $Expected, [string] $What) {
  if ($Actual -cne $Expected) { throw "$What`: expected '$Expected', got '$Actual'" }
}
try {
  [xml] $ui = Get-Content (Join-Path $PSScriptRoot 'installer-ui.wxs') -Raw -Encoding utf8
  $productCode = $null
  foreach ($language in $InstallerLanguageKeys) {
    $copy = Join-Path $Work "verify-$language.msi"
    Copy-Item -LiteralPath $Msi -Destination $copy -Force
    $database = $installer.OpenDatabase($copy, 1)
    try {
      if ($language -ne 'en') {
        # This uses the storage in the final MSI, not the loose build-time .mst.
        [void] $database.ApplyTransform(":$language.mst", 32)
      }
      $strings = Get-InstallerStrings $language
      $codepageFile = "codepage-$language.idt"
      [void] $database.Export('_ForceCodepage', [IO.Path]::GetFullPath($Work), $codepageFile)
      $codepage = Get-Content (Join-Path $Work $codepageFile) -Raw
      if ($codepage -notmatch '(?m)^(\d+)\t_ForceCodepage') { throw 'Missing MSI database codepage' }
      Equal $Matches[1] ([string] $InstallerLanguages[$language].codepage) "$language database codepage"
      $summary = $database.SummaryInformation(0)
      Equal (($summary.Property(7) -split ';')[1]) $InstallerLanguageIds 'Multilingual package summary'
      [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($summary)
      Equal (Read-Value $database 'SELECT `Value` FROM `Property` WHERE `Property` = ''ProductLanguage''') ([string] $InstallerLanguages[$language].language) "$language ProductLanguage"
      $code = Read-Value $database 'SELECT `Value` FROM `Property` WHERE `Property` = ''ProductCode'''
      if ($productCode) { Equal $code $productCode "$language ProductCode" } else { $productCode = $code }
      foreach ($dialog in $ui.Wix.Fragment.UI.Dialog) {
        $rtl = [bool] $InstallerLanguages[$language].rtl
        $attributes = [int] (Read-Value $database ('SELECT `Attributes` FROM `Dialog` WHERE `Dialog` = ''' + $dialog.Id + ''''))
        Equal ([bool] ($attributes -band 128)) $rtl "$language $($dialog.Id) dialog RTL"
        $expectedTitle = [regex]::Replace($dialog.Title, '!\(loc\.([A-Za-z]+)\)', { param($m) $strings[$m.Groups[1].Value] })
        Equal (Read-Value $database ('SELECT `Title` FROM `Dialog` WHERE `Dialog` = ''' + $dialog.Id + '''')) $expectedTitle "$language $($dialog.Id) title"
        foreach ($control in $dialog.Control) {
          $where = ' WHERE `Dialog_` = ''' + $dialog.Id + ''' AND `Control` = ''' + $control.Id + ''''
          $attributes = [int] (Read-Value $database ('SELECT `Attributes` FROM `Control`' + $where))
          Equal ([bool] ($attributes -band 32)) $rtl "$language $($dialog.Id).$($control.Id) control RTL"
          $expectedX = if ($rtl) { [int] $dialog.Width - [int] $control.X - [int] $control.Width } else { [int] $control.X }
          Equal ([int] (Read-Value $database ('SELECT `X` FROM `Control`' + $where))) $expectedX "$language $($dialog.Id).$($control.Id) mirrored bounds"
          if (-not $control.Text) { continue }
          $expected = [regex]::Replace($control.Text, '!\(loc\.([A-Za-z]+)\)', { param($m) $strings[$m.Groups[1].Value] })
          $actual = Read-Value $database ('SELECT `Text` FROM `Control` WHERE `Dialog_` = ''' + $dialog.Id + ''' AND `Control` = ''' + $control.Id + '''')
          Equal $actual $expected "$language $($dialog.Id).$($control.Id)"
        }
      }
      Equal (Read-Value $database 'SELECT `Level` FROM `Feature` WHERE `Feature` = ''HotFolder''') '2' "$language Hot Folder remains opt-in"
      Write-Output "ok $language`: embedded transform, ProductCode, language ID, all native dialogs and opt-in feature"
    } finally { [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($database) }
    Remove-Item -LiteralPath $copy
  }
} finally { [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer) }
