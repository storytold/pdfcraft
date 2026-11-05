# Internal child of test-native-dialogs.ps1. Authoring preview only: this never
# opens an installation session, executes a sequence or activates any control.
# Windows PowerShell supplies the .NET Framework STA message pump.
param([Parameter(Mandatory)] [string] $Msi,
      [Parameter(Mandatory)] [string] $Dialog,
      [Parameter(Mandatory)] [string] $StopFile)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
  throw 'Native MSI UI previews are restricted to isolated GitHub-hosted runners.'
}
if ([Threading.Thread]::CurrentThread.GetApartmentState() -ne 'STA') { throw 'Preview requires STA' }
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
function Write-PreviewTrace([string] $Stage) {
  # Write and flush directly so a blocked COM call still leaves its last stage
  # in the redirected log. Never log MSI payloads or take native screenshots.
  [Console]::Out.WriteLine('{0:o} pid={1} dialog={2} {3}', [DateTime]::UtcNow, $PID, $Dialog, $Stage)
  [Console]::Out.Flush()
}
Write-PreviewTrace "started; session=$([Diagnostics.Process]::GetCurrentProcess().SessionId); interactive=$([Environment]::UserInteractive); apartment=$([Threading.Thread]::CurrentThread.GetApartmentState()); powershell=$($PSVersionTable.PSVersion)"
Add-Type -AssemblyName System.Windows.Forms
$installer = $null
$database = $null
$preview = $null
try {
  Write-PreviewTrace 'creating WindowsInstaller.Installer'
  $installer = New-Object -ComObject WindowsInstaller.Installer
  Write-PreviewTrace 'opening read-only MSI database'
  $database = $installer.OpenDatabase((Resolve-Path -LiteralPath $Msi).Path, 0)
  Write-PreviewTrace 'enabling UI preview'
  $preview = $database.EnableUIPreview()
  Write-PreviewTrace 'calling ViewDialog'
  [void] $preview.ViewDialog($Dialog)
  Write-PreviewTrace 'ViewDialog returned; pumping authoring window messages'
  $deadline = [DateTime]::UtcNow.AddSeconds(40)
  while (-not (Test-Path -LiteralPath $StopFile)) {
    if ([DateTime]::UtcNow -ge $deadline) { throw 'Native MSI preview exceeded its 40-second deadline' }
    [Windows.Forms.Application]::DoEvents()
    Start-Sleep -Milliseconds 25
  }
  Write-PreviewTrace 'stop file received'
} catch {
  Write-PreviewTrace "failed: $($_.Exception.Message)"
  throw
} finally {
  Write-PreviewTrace 'releasing preview objects'
  # Closing the preview releases only authoring windows; it does not trigger an
  # MSI button event or an installation/custom action.
  foreach ($item in @($preview, $database, $installer)) {
    if ($null -ne $item) { [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($item) }
  }
  Write-PreviewTrace 'finished'
}
