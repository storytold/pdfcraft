<#
.SYNOPSIS
  Check the compiled MSI's install scope, publisher, shortcuts, native UI and OCR models, without installing it.
.EXAMPLE
  pwsh packaging/windows/test-msi.ps1 dist/release/pdfcraft-0.2.1-windows-x64.msi
#>
param([Parameter(Mandatory)] [string] $Path)
$ErrorActionPreference = 'Stop'
$Installer = New-Object -ComObject WindowsInstaller.Installer
$Database = $Installer.OpenDatabase((Resolve-Path -LiteralPath $Path).Path, 0)

function Read-Row([string] $Sql, [int] $Columns) {
  $view = $Database.OpenView($Sql)
  try {
    [void] $view.Execute()
    $record = $view.Fetch()
    if (-not $record) { throw "MSI row missing: $Sql" }
    $values = @(for ($i = 1; $i -le $Columns; $i++) { $record.StringData($i) })
    return ,$values
  } finally { [void] $view.Close() }
}

function Assert-Equal($Actual, $Expected, [string] $What) {
  if ($Actual -cne $Expected) { throw "$What`: expected '$Expected', got '$Actual'" }
}

function Assert-NoRow([string] $Sql, [string] $What) {
  $view = $Database.OpenView($Sql)
  try {
    [void] $view.Execute()
    if ($view.Fetch()) { throw "Unexpected MSI row ($What): $Sql" }
  } finally { [void] $view.Close() }
}

# Ensure there is no launch condition forbidding per-user installs. Dual-purpose packages
# support both per-user (without elevation) and per-machine (with elevation) installation.
Assert-NoRow 'SELECT `Condition` FROM `LaunchCondition` WHERE `Description` LIKE ''%per-user installation is not supported%''' 'no per-user reject launch condition'

foreach ($sequence in @('InstallUISequence', 'InstallExecuteSequence')) {
  $launch = Read-Row ('SELECT `Condition`, `Sequence` FROM `' + $sequence + '` WHERE `Action` = ''LaunchConditions''') 2
  Assert-Equal $launch[0] '' "$sequence launch conditions are unconditional"
  $cost = Read-Row ('SELECT `Sequence` FROM `' + $sequence + '` WHERE `Action` = ''CostInitialize''') 1
  if ([int] $launch[1] -le 0 -or [int] $launch[1] -ge [int] $cost[0]) {
    throw "$sequence must evaluate launch conditions before costing"
  }
}
$manufacturer = Read-Row 'SELECT `Value` FROM `Property` WHERE `Property` = ''Manufacturer''' 1
Assert-Equal $manufacturer[0] 'Learning Machines LLC' 'MSI manufacturer'
$status = Read-Row 'SELECT `Text` FROM `Control` WHERE `Dialog_` = ''InstallProgress'' AND `Control` = ''Status''' 1
Assert-Equal $status[0] 'Please wait while setup completes.' 'Persistent progress message'
Assert-NoRow 'SELECT `Event` FROM `EventMapping` WHERE `Dialog_` = ''InstallProgress'' AND `Control_` = ''Status''' 'progress text subscription'

# Plain (non-advertised) shortcuts to pdfcraft.exe, each in its own component; the desktop one is
# gated by INSTALLDESKTOPSHORTCUT, which defaults to 1 and is secure so the UI choice reaches the
# elevated install.
foreach ($entry in @(@('StartMenuShortcut', 'ProgramMenuFolder', 'PdfcraftStartMenuShortcut', ''),
                     @('DesktopShortcut', 'DesktopFolder', 'PdfcraftDesktopShortcut', 'INSTALLDESKTOPSHORTCUT = 1'))) {
  $row = Read-Row ('SELECT `Directory_`, `Target`, `Icon_`, `WkDir`, `Component_` FROM `Shortcut` WHERE `Shortcut` = ''' + $entry[0] + '''') 5
  Assert-Equal $row[0] $entry[1] "$($entry[0]) directory"
  Assert-Equal $row[1] '[#PdfcraftExe]' "$($entry[0]) target"
  Assert-Equal $row[2] 'PdfcraftIcon.ico' "$($entry[0]) icon"
  Assert-Equal $row[3] 'INSTALLFOLDER' "$($entry[0]) working directory"
  Assert-Equal $row[4] $entry[2] "$($entry[0]) component"
  $component = Read-Row ('SELECT `Directory_`, `Condition` FROM `Component` WHERE `Component` = ''' + $entry[2] + '''') 2
  Assert-Equal $component[0] $entry[1] "$($entry[2]) directory"
  Assert-Equal $component[1] $entry[3] "$($entry[2]) condition"
}
$default = Read-Row 'SELECT `Value` FROM `Property` WHERE `Property` = ''INSTALLDESKTOPSHORTCUT''' 1
Assert-Equal $default[0] '1' 'Desktop shortcut default'
$secure = Read-Row 'SELECT `Value` FROM `Property` WHERE `Property` = ''SecureCustomProperties''' 1
if (($secure[0] -split ';') -notcontains 'INSTALLDESKTOPSHORTCUT') { throw "INSTALLDESKTOPSHORTCUT is not secure: '$($secure[0])'" }
if (($secure[0] -split ';') -notcontains 'INSTALLSCOPE') { throw "INSTALLSCOPE is not secure: '$($secure[0])'" }

$scopeDefault = Read-Row 'SELECT `Value` FROM `Property` WHERE `Property` = ''INSTALLSCOPE''' 1
Assert-Equal $scopeDefault[0] 'PerUser' 'INSTALLSCOPE default'

$checkbox = Read-Row 'SELECT `Type`, `Property`, `Text` FROM `Control` WHERE `Dialog_` = ''InstallWelcome'' AND `Control` = ''DesktopShortcut''' 3
Assert-Equal $checkbox[0] 'CheckBox' 'Welcome desktop-shortcut control'
Assert-Equal $checkbox[1] 'INSTALLDESKTOPSHORTCUT' 'Welcome checkbox property'
if ($checkbox[2] -notmatch 'desktop shortcut') { throw "Welcome checkbox label: '$($checkbox[2])'" }
$checked = Read-Row 'SELECT `Value` FROM `CheckBox` WHERE `Property` = ''INSTALLDESKTOPSHORTCUT''' 1
Assert-Equal $checked[0] '1' 'Welcome checkbox value'

$scopeCtrl = Read-Row 'SELECT `Type`, `Property` FROM `Control` WHERE `Dialog_` = ''InstallWelcome'' AND `Control` = ''ScopeRadioGroup''' 2
Assert-Equal $scopeCtrl[0] 'RadioButtonGroup' 'Welcome scope radio group control'
Assert-Equal $scopeCtrl[1] 'INSTALLSCOPE' 'Welcome scope radio group property'
foreach ($val in @('PerUser', 'PerMachine')) {
  $radio = Read-Row ('SELECT `Property`, `Value` FROM `RadioButton` WHERE `Property` = ''INSTALLSCOPE'' AND `Value` = ''' + $val + '''') 2
  Assert-Equal $radio[0] 'INSTALLSCOPE' "RadioButton $val property"
  Assert-Equal $radio[1] $val "RadioButton $val value"
}
$userInstall = Read-Row 'SELECT `Type`, `Text` FROM `Control` WHERE `Dialog_` = ''InstallWelcome'' AND `Control` = ''InstallUser''' 2
Assert-Equal $userInstall[0] 'PushButton' 'Welcome InstallUser control'
Assert-Equal $userInstall[1] '&Install' 'Welcome InstallUser label'
$machInstall = Read-Row 'SELECT `Type`, `Text` FROM `Control` WHERE `Dialog_` = ''InstallWelcome'' AND `Control` = ''InstallMachine''' 2
Assert-Equal $machInstall[0] 'PushButton' 'Welcome InstallMachine control'
Assert-Equal $machInstall[1] '&Install' 'Welcome InstallMachine label'

$app = Read-Row 'SELECT `KeyPath` FROM `Component` WHERE `Component` = ''PdfcraftApp''' 1
Assert-Equal $app[0] 'PdfcraftExe' 'Shortcut executable key path'
$scope = Read-Row 'SELECT `Value` FROM `Property` WHERE `Property` = ''ALLUSERS''' 1
Assert-Equal $scope[0] '2' 'Dual-purpose ALLUSERS scope'
$perUser = Read-Row 'SELECT `Value` FROM `Property` WHERE `Property` = ''MSIINSTALLPERUSER''' 1
Assert-Equal $perUser[0] '1' 'Dual-purpose MSIINSTALLPERUSER default'

# Registry entries use HKMU (-1) so they resolve to HKCU for per-user installs and HKLM for per-machine installs.
foreach ($ext in @('png', 'jpg', 'jpeg', 'tif', 'tiff', 'gif', 'bmp', 'jp2', 'j2k', 'jpx')) {
  $key = 'Software\Classes\SystemFileAssociations\.' + $ext + '\shell\PdfCraft.CreatePdf'
  $menu = Read-Row ('SELECT `Value`, `Component_`, `Root` FROM `Registry` WHERE `Key` = ''' + $key + ''' AND `Name` IS NULL') 3
  Assert-Equal $menu[0] 'Create PDF with PdfCraft…' "$ext context menu label"
  Assert-Equal $menu[1] 'PdfcraftApp' "$ext context menu component"
  Assert-Equal $menu[2] '-1' "$ext context menu HKMU root"
  $command = Read-Row ('SELECT `Value` FROM `Registry` WHERE `Key` = ''' + $key + '\command''') 1
  Assert-Equal $command[0] '"[#PdfcraftExe]" --create-images "%1"' "$ext context menu command"
  $selection = Read-Row ('SELECT `Value` FROM `Registry` WHERE `Key` = ''' + $key + ''' AND `Name` = ''MultiSelectModel''') 1
  Assert-Equal $selection[0] 'Single' "$ext context menu selection"
}
foreach ($sName in @('StartMenu', 'Desktop')) {
  $reg = Read-Row ('SELECT `Root` FROM `Registry` WHERE `Key` = ''Software\PdfCraft\Shortcuts'' AND `Name` = ''' + $sName + '''') 1
  Assert-Equal $reg[0] '-1' "$sName shortcut HKMU root"
}

# Negative sequences are Windows Installer's success/user-exit/failure paths. Only full UI
# shows these dialogs: an unattended /qn or /qb install must never wait for a Finish click.
foreach ($exit in @(@('InstallComplete', '-1'), @('InstallCancelled', '-2'), @('InstallFailed', '-3'))) {
  $row = Read-Row ('SELECT `Condition`, `Sequence` FROM `InstallUISequence` WHERE `Action` = ''' + $exit[0] + '''') 2
  Assert-Equal $row[0] 'UILevel = 5' "$($exit[0]) UI level"
  Assert-Equal $row[1] $exit[1] "$($exit[0]) exit path"
  $finish = Read-Row ('SELECT `Type`, `Text` FROM `Control` WHERE `Dialog_` = ''' + $exit[0] + ''' AND `Control` = ''Finish''') 2
  Assert-Equal $finish[0] 'PushButton' "$($exit[0]) Finish control"
  Assert-Equal $finish[1] '&Finish' "$($exit[0]) Finish label"
  $endDialog = Read-Row ('SELECT `Argument` FROM `ControlEvent` WHERE `Dialog_` = ''' + $exit[0] + ''' AND `Control_` = ''Finish'' AND `Event` = ''EndDialog''') 1
  Assert-Equal $endDialog[0] 'Return' "$($exit[0]) Finish event"
}
$title = Read-Row 'SELECT `Text` FROM `Control` WHERE `Dialog_` = ''InstallComplete'' AND `Control` = ''Title''' 1
if ($title[0] -notmatch 'completed successfully') { throw 'Success dialog does not confirm completion' }
# OCR models (#103): installed into models\ beside pdfcraft.exe, where the app looks for them.
$modelsDir = Read-Row 'SELECT `Directory_Parent`, `DefaultDir` FROM `Directory` WHERE `Directory` = ''ModelsFolder''' 2
Assert-Equal $modelsDir[0] 'INSTALLFOLDER' 'OCR models folder parent'
# DefaultDir is `models`, or `SHORT|models` with a generated 8.3 name.
if ($modelsDir[1] -notmatch '(^|\|)models$') { throw "OCR models folder name: '$($modelsDir[1])'" }
$modelFiles = 0
$view = $Database.OpenView('SELECT `File`.`FileName` FROM `File`, `Component` WHERE `File`.`Component_` = `Component`.`Component` AND `Component`.`Directory_` = ''ModelsFolder''')
try {
  [void] $view.Execute()
  while ($record = $view.Fetch()) { if ($record.StringData(1) -match '\.rten$') { $modelFiles++ } }
} finally { [void] $view.Close() }
if ($modelFiles -lt 2) { throw "expected the OCR models (*.rten) in models\, found $modelFiles" }
$rm = Read-Row 'SELECT `Dialog` FROM `Dialog` WHERE `Dialog` = ''MsiRMFilesInUse''' 1
Assert-Equal $rm[0] 'MsiRMFilesInUse' 'Files-in-use dialog'
[void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($Database)
[void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($Installer)
Write-Output 'ok MSI: dual-purpose install scope (per-user/per-machine), publisher, persistent progress text, Start Menu shortcut, optional desktop shortcut (default on, checkbox), icon/key path, full-UI success/cancel/error and Finish controls, files-in-use dialog, OCR models'
