# Update only a build artifact; a separate process releases all MSI stream handles.
param([Parameter(Mandatory)] [string] $Msi, [Parameter(Mandatory)] [string] $Work)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'installer-languages.ps1')
$installer = New-Object -ComObject WindowsInstaller.Installer
$database = $null
try {
  $database = $installer.OpenDatabase($Msi, 1)
  foreach ($language in $InstallerTransformKeys) {
    $view = $database.OpenView('INSERT INTO `_Storages` (`Name`, `Data`) VALUES (?, ?)')
    try {
      $record = $installer.CreateRecord(2)
      $record.StringData(1) = "$language.mst"
      [void] $record.SetStream(2, (Join-Path $Work "$language.mst"))
      [void] $view.Execute($record)
    } finally { [void] $view.Close() }
  }
  # The multilingual package advertises all embedded languages in its Template
  # Summary; ProductLanguage is still the English base until a transform is applied.
  $summary = $database.SummaryInformation(1)
  $platform = ($summary.Property(7) -split ';')[0]
  $summary.Property(7) = "$platform;$InstallerLanguageIds"
  [void] $summary.Persist()
  [void] $database.Commit()
} finally {
  if ($database) { [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($database) }
  [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer)
}
