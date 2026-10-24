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

# Test the compiled condition with Windows Installer's evaluator, in a restricted session that
# cannot change machine state. Normal installs/repairs work, per-user overrides fail, and removal
# of an older incorrectly scoped installation remains possible (#305).
$scopeMessage = 'PdfCraft must be installed for all users. Run setup with administrator privileges and ALLUSERS=1; per-user installation is not supported.'
$scopeCondition = Read-Row ('SELECT `Condition` FROM `LaunchCondition` WHERE `Description` = ''' + $scopeMessage + '''') 1
$Installer.UILevel = 2
$session = $null
try {
  $session = $Installer.OpenPackage((Resolve-Path -LiteralPath $Path).Path, 1)
  foreach ($case in @(
      @('machine install', '1', '', '', '', 1),
      @('machine repair', '1', '', '1', '', 1),
      @('forced per-user install', '2', '1', '', '', 0),
      @('empty ALLUSERS', '', '', '', '', 0),
      @('resolved per-user install', '', '1', '', '', 0),
      @('forced per-user repair', '2', '1', '1', '', 0),
      @('machine uninstall', '1', '', '1', 'ALL', 1),
      @('legacy per-user uninstall', '', '1', '1', 'ALL', 1))) {
    $session.Property('ALLUSERS') = $case[1]
    $session.Property('MSIINSTALLPERUSER') = $case[2]
    $session.Property('Installed') = $case[3]
    $session.Property('REMOVE') = $case[4]
    Assert-Equal ($session.EvaluateCondition($scopeCondition[0])) $case[5] $case[0]
  }
} finally {
  if ($session) { [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($session) }
}
foreach ($sequence in @('InstallUISequence', 'InstallExecuteSequence')) {
  $launch = Read-Row ('SELECT `Condition`, `Sequence` FROM `' + $sequence + '` WHERE `Action` = ''LaunchConditions''') 2
  Assert-Equal $launch[0] '' "$sequence launch conditions are unconditional"
  $cost = Read-Row ('SELECT `Sequence` FROM `' + $sequence + '` WHERE `Action` = ''CostInitialize''') 1
  if ([int] $launch[1] -le 0 -or [int] $launch[1] -ge [int] $cost[0]) {
    throw "$sequence must reject per-user overrides before costing"
  }
}
$manufacturer = Read-Row 'SELECT `Value` FROM `Property` WHERE `Property` = ''Manufacturer''' 1
Assert-Equal $manufacturer[0] 'Learning Machines LLC' 'MSI manufacturer'
$status = Read-Row 'SELECT `Text` FROM `Control` WHERE `Dialog_` = ''InstallProgress'' AND `Control` = ''Status''' 1
Assert-Equal $status[0] 'Please wait while setup completes.' 'Persistent progress message'
Assert-NoRow 'SELECT `Event` FROM `EventMapping` WHERE `Dialog_` = ''InstallProgress'' AND `Control_` = ''Status''' 'progress text subscription'

# Plain (non-advertised) shortcuts to pdfcraft.exe, each in its own component; the desktop one is
# gated by INSTALLDESKTOPSHORTCUT, which is secure so the UI choice reaches the
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
# Defaults an update keeps (#1058, #305). No static default: a SetProperty after AppSearch turns it on
# for a first install and, for an update from a release that offered it ($Release set), only while what
# it installed is still there ($Found, from AppSearch). A command-line value is left alone. Windows
# Installer's evaluator runs the SetProperty condition and then $Component's own condition.
function Assert-KeptOnUpdate([string] $Property, [string] $Release, [string] $Found, [string] $FoundValue, [string] $Component, [string] $What) {
  Assert-NoRow ('SELECT `Value` FROM `Property` WHERE `Property` = ''' + $Property + '''') "$What static default"
  $action = Read-Row ('SELECT `Action`, `Target` FROM `CustomAction` WHERE `Source` = ''' + $Property + '''') 2
  Assert-Equal $action[1] '1' "$What default value"
  $condition = $null
  foreach ($sequence in @('InstallUISequence', 'InstallExecuteSequence')) {
    $search = Read-Row ('SELECT `Sequence` FROM `' + $sequence + '` WHERE `Action` = ''AppSearch''') 1
    $set = Read-Row ('SELECT `Sequence`, `Condition` FROM `' + $sequence + '` WHERE `Action` = ''' + $action[0] + '''') 2
    if ([int] $set[0] -le [int] $search[0]) { throw "$sequence must set the $What default after AppSearch" }
    $condition = $set[1]
  }
  $installed = Read-Row ('SELECT `Condition` FROM `Component` WHERE `Component` = ''' + $Component + '''') 1
  Assert-Equal $installed[0] "$Property = 1" "$Component condition"
  $old = '{00000000-0000-0000-0000-000000000000}'
  # Name, command-line value, WIX_UPGRADE_DETECTED, $Release, $Found, installed (1) or not (0).
  $cases = @(
    @('first install', '', '', '', '', 1),
    @("first install, $Property=0", '0', '', '', '', 0),
    @('update, still there', '', $old, $old, $FoundValue, 1),
    @('update, removed', '', $old, $old, '', 0),
    @("update, $Property=0", '0', $old, $old, $FoundValue, 0),
    @("update, removed, $Property=1", '1', $old, $old, '', 1))
  # An update from a release from before it existed (WIX_UPGRADE_DETECTED alone) is a first install.
  if ($Release -ne 'WIX_UPGRADE_DETECTED') { $cases += , @('update from a release without it', '', $old, '', '', 1) }
  $session = $null
  try {
    $session = $Installer.OpenPackage((Resolve-Path -LiteralPath $Path).Path, 1)
    foreach ($case in $cases) {
      $session.Property($Property) = $case[1]
      $session.Property('WIX_UPGRADE_DETECTED') = $case[2]
      $session.Property($Release) = $case[3]
      $session.Property($Found) = $case[4]
      if ($session.EvaluateCondition($condition) -eq 1) { $session.Property($Property) = $action[1] }
      Assert-Equal ($session.EvaluateCondition($installed[0])) $case[5] "$What`: $($case[0])"
    }
  } finally {
    if ($session) { [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($session) }
  }
}
Assert-KeptOnUpdate 'INSTALLDESKTOPSHORTCUT' 'WIX_UPGRADE_DETECTED' 'EXISTINGDESKTOPSHORTCUT' 'C:\Users\Public\Desktop\PdfCraft.lnk' 'PdfcraftDesktopShortcut' 'Desktop shortcut'
$secure = Read-Row 'SELECT `Value` FROM `Property` WHERE `Property` = ''SecureCustomProperties''' 1
if (($secure[0] -split ';') -notcontains 'INSTALLDESKTOPSHORTCUT') { throw "INSTALLDESKTOPSHORTCUT is not secure: '$($secure[0])'" }
$checkbox = Read-Row 'SELECT `Type`, `Property`, `Text` FROM `Control` WHERE `Dialog_` = ''InstallWelcome'' AND `Control` = ''DesktopShortcut''' 3
Assert-Equal $checkbox[0] 'CheckBox' 'Welcome desktop-shortcut control'
Assert-Equal $checkbox[1] 'INSTALLDESKTOPSHORTCUT' 'Welcome checkbox property'
if ($checkbox[2] -notmatch 'desktop shortcut') { throw "Welcome checkbox label: '$($checkbox[2])'" }
$checked = Read-Row 'SELECT `Value` FROM `CheckBox` WHERE `Property` = ''INSTALLDESKTOPSHORTCUT''' 1
Assert-Equal $checked[0] '1' 'Welcome checkbox value'
$app = Read-Row 'SELECT `KeyPath` FROM `Component` WHERE `Component` = ''PdfcraftApp''' 1
Assert-Equal $app[0] 'PdfcraftExe' 'Shortcut executable key path'
$scope = Read-Row 'SELECT `Value` FROM `Property` WHERE `Property` = ''ALLUSERS''' 1
Assert-Equal $scope[0] '1' 'Per-machine shortcut scope'

# Each File Explorer menu remembers whether it was installed by finding its key path in HKLM, in the
# package's own registry view (64-bit for x64 and arm64). Only a release that offered the menu can
# leave it out, so the OnlyDetect Upgrade rows start at the first release with that menu.
$upgradeCode = Read-Row 'SELECT `Value` FROM `Property` WHERE `Property` = ''UpgradeCode''' 1
$view64 = $Database.SummaryInformation(0).Property(7) -notmatch '^Intel;'
function Assert-MenuSearch([string] $Found, [string] $Key, [string] $Release, [string] $Since, [string] $What) {
  $search = Read-Row ('SELECT `Root`, `Key`, `Name`, `Type` FROM `RegLocator`, `AppSearch` WHERE `AppSearch`.`Signature_` = `RegLocator`.`Signature_` AND `AppSearch`.`Property` = ''' + $Found + '''') 4
  Assert-Equal $search[0] '2' "$What search HKLM root"
  Assert-Equal $search[1] $Key "$What search key"
  Assert-Equal $search[2] '' "$What search reads the default value"
  # msidbLocatorTypeRawValue (2), plus msidbLocatorType64bit (16) in a 64-bit package.
  Assert-Equal ([int] $search[3]) $(if ($view64) { 18 } else { 2 }) "$What search type"
  $upgrade = Read-Row ('SELECT `UpgradeCode`, `VersionMin`, `VersionMax`, `Attributes` FROM `Upgrade` WHERE `ActionProperty` = ''' + $Release + '''') 4
  Assert-Equal $upgrade[0] $upgradeCode[0] "$What release detection upgrade code"
  Assert-Equal $upgrade[1] $Since "$What release detection minimum"
  Assert-Equal $upgrade[2] '' "$What release detection maximum"
  # msidbUpgradeAttributesOnlyDetect (2) + VersionMinInclusive (256): never removes anything itself.
  Assert-Equal ([int] $upgrade[3]) 258 "$What release detection attributes"
  if (($secure[0] -split ';') -notcontains $Release) { throw "$Release is not secure: '$($secure[0])'" }
}
# The old version is removed before the new one is installed, so rows that moved between components
# (the image menu left PdfcraftApp after 0.7.1) are removed with it rather than orphaned.
$validate = Read-Row 'SELECT `Sequence` FROM `InstallExecuteSequence` WHERE `Action` = ''InstallValidate''' 1
$remove = Read-Row 'SELECT `Sequence` FROM `InstallExecuteSequence` WHERE `Action` = ''RemoveExistingProducts''' 1
$initialize = Read-Row 'SELECT `Sequence` FROM `InstallExecuteSequence` WHERE `Action` = ''InstallInitialize''' 1
if ([int] $remove[0] -le [int] $validate[0] -or [int] $remove[0] -ge [int] $initialize[0]) {
  throw "RemoveExistingProducts ($($remove[0])) must run between InstallValidate and InstallInitialize"
}

# Image context menu (#190) opens the DPI chooser. Its own component, gated by INSTALLIMAGECONTEXTMENU
# (on for a first install, kept on update only while present, secure), with the .png label as key
# path so uninstall removes every row. No image default association is changed.
$imageKey = 'Software\Classes\SystemFileAssociations\.png\shell\PdfCraft.CreatePdf'
foreach ($ext in @('png', 'jpg', 'jpeg', 'tif', 'tiff', 'gif', 'bmp', 'jp2', 'j2k', 'jpx')) {
  $key = 'Software\Classes\SystemFileAssociations\.' + $ext + '\shell\PdfCraft.CreatePdf'
  $menu = Read-Row ('SELECT `Value`, `Component_`, `Root` FROM `Registry` WHERE `Key` = ''' + $key + ''' AND `Name` IS NULL') 3
  Assert-Equal $menu[0] 'Create PDF with PdfCraft…' "$ext context menu label"
  Assert-Equal $menu[1] 'PdfcraftImageContextMenu' "$ext context menu component"
  Assert-Equal $menu[2] '2' "$ext context menu HKLM root"
  $command = Read-Row ('SELECT `Value`, `Component_` FROM `Registry` WHERE `Key` = ''' + $key + '\command''') 2
  Assert-Equal $command[0] '"[#PdfcraftExe]" --create-images "%1"' "$ext context menu command"
  Assert-Equal $command[1] 'PdfcraftImageContextMenu' "$ext context menu command component"
  $selection = Read-Row ('SELECT `Value`, `Component_` FROM `Registry` WHERE `Key` = ''' + $key + ''' AND `Name` = ''MultiSelectModel''') 2
  Assert-Equal $selection[0] 'Single' "$ext context menu selection"
  Assert-Equal $selection[1] 'PdfcraftImageContextMenu' "$ext context menu selection component"
}
$view = $Database.OpenView('SELECT `Key` FROM `Registry` WHERE `Component_` = ''PdfcraftApp''')
try {
  [void] $view.Execute()
  while ($record = $view.Fetch()) {
    if ($record.StringData(1) -match 'PdfCraft\.CreatePdf') { throw "image menu row left in PdfcraftApp: $($record.StringData(1))" }
  }
} finally { [void] $view.Close() }
$imageKeyPath = Read-Row ('SELECT `Registry` FROM `Registry` WHERE `Key` = ''' + $imageKey + ''' AND `Name` IS NULL') 1
$imageComponent = Read-Row 'SELECT `KeyPath`, `Attributes` FROM `Component` WHERE `Component` = ''PdfcraftImageContextMenu''' 2
Assert-Equal $imageComponent[0] $imageKeyPath[0] 'Image context menu key path'
# msidbComponentAttributesRegistryKeyPath (4).
if (([int] $imageComponent[1] -band 4) -eq 0) { throw "Image context menu key path is not a registry value: attributes $($imageComponent[1])" }
Assert-KeptOnUpdate 'INSTALLIMAGECONTEXTMENU' 'IMAGECONTEXTMENURELEASE' 'EXISTINGIMAGECONTEXTMENU' 'Create PDF with PdfCraft…' 'PdfcraftImageContextMenu' 'Image context menu'
Assert-MenuSearch 'EXISTINGIMAGECONTEXTMENU' $imageKey 'IMAGECONTEXTMENURELEASE' '0.4.0' 'Image context menu'
if (($secure[0] -split ';') -notcontains 'INSTALLIMAGECONTEXTMENU') { throw "INSTALLIMAGECONTEXTMENU is not secure: '$($secure[0])'" }

# PDF context menu (#682): "Edit with PdfCraft" opens the file in PdfCraft. Its own component, gated
# by INSTALLPDFCONTEXTMENU (on for a first install, kept on update only while present, secure, welcome
# checkbox), with its menu value as key path so uninstall removes it. Several selected PDFs each start pdfcraft.exe "%1" (Document), which the
# single-instance hand-off opens as tabs. The .pdf default is never set, so the user's default app
# stays.
$pdfKey = 'Software\Classes\SystemFileAssociations\.pdf\shell\PdfCraft.Edit'
$pdfMenu = Read-Row ('SELECT `Registry`, `Value`, `Component_`, `Root` FROM `Registry` WHERE `Key` = ''' + $pdfKey + ''' AND `Name` IS NULL') 4
Assert-Equal $pdfMenu[1] 'Edit with PdfCraft' 'PDF context menu label'
Assert-Equal $pdfMenu[2] 'PdfcraftPdfContextMenu' 'PDF context menu component'
Assert-Equal $pdfMenu[3] '2' 'PDF context menu HKLM root'
foreach ($value in @(@('Icon', '"[#PdfcraftExe]",0'), @('MultiSelectModel', 'Document'))) {
  $row = Read-Row ('SELECT `Value`, `Component_` FROM `Registry` WHERE `Key` = ''' + $pdfKey + ''' AND `Name` = ''' + $value[0] + '''') 2
  Assert-Equal $row[0] $value[1] "PDF context menu $($value[0])"
  Assert-Equal $row[1] 'PdfcraftPdfContextMenu' "PDF context menu $($value[0]) component"
}
$pdfCommand = Read-Row ('SELECT `Value`, `Component_` FROM `Registry` WHERE `Key` = ''' + $pdfKey + '\command''') 2
Assert-Equal $pdfCommand[0] '"[#PdfcraftExe]" "%1"' 'PDF context menu command'
Assert-Equal $pdfCommand[1] 'PdfcraftPdfContextMenu' 'PDF context menu command component'
$pdfComponent = Read-Row 'SELECT `Condition`, `KeyPath` FROM `Component` WHERE `Component` = ''PdfcraftPdfContextMenu''' 2
Assert-Equal $pdfComponent[0] 'INSTALLPDFCONTEXTMENU = 1' 'PDF context menu condition'
Assert-Equal $pdfComponent[1] $pdfMenu[0] 'PDF context menu key path'
Assert-KeptOnUpdate 'INSTALLPDFCONTEXTMENU' 'PDFCONTEXTMENURELEASE' 'EXISTINGPDFCONTEXTMENU' 'Edit with PdfCraft' 'PdfcraftPdfContextMenu' 'PDF context menu'
Assert-MenuSearch 'EXISTINGPDFCONTEXTMENU' $pdfKey 'PDFCONTEXTMENURELEASE' '0.7.1' 'PDF context menu'
if (($secure[0] -split ';') -notcontains 'INSTALLPDFCONTEXTMENU') { throw "INSTALLPDFCONTEXTMENU is not secure: '$($secure[0])'" }
$pdfCheckbox = Read-Row 'SELECT `Type`, `Property`, `Text` FROM `Control` WHERE `Dialog_` = ''InstallWelcome'' AND `Control` = ''PdfContextMenu''' 3
Assert-Equal $pdfCheckbox[0] 'CheckBox' 'Welcome PDF context-menu control'
Assert-Equal $pdfCheckbox[1] 'INSTALLPDFCONTEXTMENU' 'Welcome PDF context-menu property'
if ($pdfCheckbox[2] -notmatch '&Edit' -or $pdfCheckbox[2] -notmatch 'Combine files') { throw "Welcome PDF context-menu label: '$($pdfCheckbox[2])'" }
$pdfChecked = Read-Row 'SELECT `Value` FROM `CheckBox` WHERE `Property` = ''INSTALLPDFCONTEXTMENU''' 1
Assert-Equal $pdfChecked[0] '1' 'Welcome PDF context-menu value'
Assert-NoRow 'SELECT `Registry` FROM `Registry` WHERE `Key` = ''Software\Classes\.pdf'' AND `Name` IS NULL' '.pdf default app'
# "Combine files in PdfCraft" (#585): in the PDF menu's component, so the one switch covers both
# verbs. Each selected file starts `pdfcraft --combine "<file>"` (Document), and the running
# PdfCraft collects them into its Combine files tab.
$combineKey = 'Software\Classes\SystemFileAssociations\.pdf\shell\PdfCraft.Combine'
$combineMenu = Read-Row ('SELECT `Value`, `Component_`, `Root` FROM `Registry` WHERE `Key` = ''' + $combineKey + ''' AND `Name` IS NULL') 3
Assert-Equal $combineMenu[0] 'Combine files in PdfCraft' 'Combine verb label'
Assert-Equal $combineMenu[1] 'PdfcraftPdfContextMenu' 'Combine verb component'
Assert-Equal $combineMenu[2] '2' 'Combine verb HKLM root'
foreach ($value in @(@('Icon', '"[#PdfcraftExe]",0'), @('MultiSelectModel', 'Document'))) {
  $row = Read-Row ('SELECT `Value`, `Component_` FROM `Registry` WHERE `Key` = ''' + $combineKey + ''' AND `Name` = ''' + $value[0] + '''') 2
  Assert-Equal $row[0] $value[1] "Combine verb $($value[0])"
  Assert-Equal $row[1] 'PdfcraftPdfContextMenu' "Combine verb $($value[0]) component"
}
$combineCommand = Read-Row ('SELECT `Value`, `Component_` FROM `Registry` WHERE `Key` = ''' + $combineKey + '\command''') 2
Assert-Equal $combineCommand[0] '"[#PdfcraftExe]" --combine "%1"' 'Combine verb command'
Assert-Equal $combineCommand[1] 'PdfcraftPdfContextMenu' 'Combine verb command component'

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
Write-Output 'ok MSI: per-machine scope guard, publisher, persistent progress text, Start Menu shortcut, optional desktop shortcut and PDF (Edit, Combine files) and image context menus (on for a first install, kept on update only while present; checkboxes for the first two), icon/key path, full-UI success/cancel/error and Finish controls, files-in-use dialog, OCR models'
