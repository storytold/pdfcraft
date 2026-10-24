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

function Read-Rows([string] $Sql, [int] $Columns) {
  $view = $Database.OpenView($Sql)
  try {
    [void] $view.Execute()
    $rows = @(while ($record = $view.Fetch()) { , @(for ($i = 1; $i -le $Columns; $i++) { $record.StringData($i) }) })
    return , $rows
  } finally { [void] $view.Close() }
}

# Install scope. The authored context stays per-machine, so msiexec asks for elevation as before when
# it starts without properties (the launcher's /passive update); per user is ALLUSERS=2
# MSIINSTALLPERUSER=1, or the full UI's "Only me".
$allUsers = Read-Row 'SELECT `Value` FROM `Property` WHERE `Property` = ''ALLUSERS''' 1
Assert-Equal $allUsers[0] '1' 'Authored per-machine context'
Assert-NoRow 'SELECT `Value` FROM `Property` WHERE `Property` = ''MSIINSTALLPERUSER''' 'authored MSIINSTALLPERUSER'
# The full UI picks the welcome dialog's default scope with type 51 actions between AppSearch and
# LaunchConditions; the execute sequence never changes the context.
$search = Read-Row 'SELECT `Sequence` FROM `InstallUISequence` WHERE `Action` = ''AppSearch''' 1
$scopeActions = @((Read-Rows 'SELECT `CustomAction`.`Source`, `CustomAction`.`Target`, `InstallUISequence`.`Condition`, `InstallUISequence`.`Sequence`, `CustomAction`.`Type` FROM `CustomAction`, `InstallUISequence` WHERE `CustomAction`.`Action` = `InstallUISequence`.`Action`' 5) |
  Where-Object { (([int] $_[4]) -band 0x3F) -eq 51 -and $_[0] -in @('INSTALLSCOPE', 'ALLUSERS', 'MSIINSTALLPERUSER', 'PERMACHINEFOLDER') } |
  Sort-Object { [int] $_[3] })
if ($scopeActions.Count -lt 5) { throw "InstallUISequence has $($scopeActions.Count) install-scope actions" }
foreach ($sequence in @('InstallUISequence', 'InstallExecuteSequence')) {
  $launch = Read-Row ('SELECT `Condition`, `Sequence` FROM `' + $sequence + '` WHERE `Action` = ''LaunchConditions''') 2
  Assert-Equal $launch[0] '' "$sequence launch conditions are unconditional"
  $cost = Read-Row ('SELECT `Sequence` FROM `' + $sequence + '` WHERE `Action` = ''CostInitialize''') 1
  if ([int] $launch[1] -ge [int] $cost[0]) { throw "$sequence must check the install scope before costing" }
  if ($sequence -eq 'InstallUISequence') {
    foreach ($action in $scopeActions) {
      if ([int] $action[3] -le [int] $search[0] -or [int] $action[3] -ge [int] $launch[1]) {
        throw "InstallUISequence must set $($action[0]) after AppSearch and before LaunchConditions"
      }
    }
  }
}
foreach ($row in (Read-Rows 'SELECT `CustomAction`.`Source` FROM `CustomAction`, `InstallExecuteSequence` WHERE `CustomAction`.`Action` = `InstallExecuteSequence`.`Action`' 1)) {
  if ($row[0] -in @('INSTALLSCOPE', 'ALLUSERS', 'MSIINSTALLPERUSER')) { throw "the execute sequence changes $($row[0])" }
}
# The machine folder is read before the context changes: Program Files in the package's own view.
$machineFolder = @($scopeActions | Where-Object { $_[0] -eq 'PERMACHINEFOLDER' })
$view64 = $Database.SummaryInformation(0).Property(7) -notmatch '^Intel;'
Assert-Equal $machineFolder[0][1] $(if ($view64) { '[ProgramFiles64Folder]PdfCraft\' } else { '[ProgramFilesFolder]PdfCraft\' }) 'Per-machine install folder'
if ([int] $machineFolder[0][3] -ne [int] $scopeActions[0][3]) { throw 'the per-machine folder must be read before the context changes' }
# The UI's upgrade search runs in the chosen context: after the last scope action and before
# LaunchConditions and MigrateFeatureStates. The execute sequence's keeps its standard place, before
# AppSearch and RemoveExistingProducts.
$uiFind = Read-Row 'SELECT `Sequence` FROM `InstallUISequence` WHERE `Action` = ''FindRelatedProducts''' 1
$uiLaunch = Read-Row 'SELECT `Sequence` FROM `InstallUISequence` WHERE `Action` = ''LaunchConditions''' 1
$uiMigrate = Read-Row 'SELECT `Sequence` FROM `InstallUISequence` WHERE `Action` = ''MigrateFeatureStates''' 1
if ([int] $uiFind[0] -le [int] $scopeActions[-1][3] -or [int] $uiFind[0] -ge [int] $uiLaunch[0] -or [int] $uiFind[0] -ge [int] $uiMigrate[0]) {
  throw "InstallUISequence must run FindRelatedProducts ($($uiFind[0])) after the scope actions and before LaunchConditions and MigrateFeatureStates"
}
$execFind = Read-Row 'SELECT `Sequence` FROM `InstallExecuteSequence` WHERE `Action` = ''FindRelatedProducts''' 1
$execSearch = Read-Row 'SELECT `Sequence` FROM `InstallExecuteSequence` WHERE `Action` = ''AppSearch''' 1
$execRemove = Read-Row 'SELECT `Sequence` FROM `InstallExecuteSequence` WHERE `Action` = ''RemoveExistingProducts''' 1
if ([int] $execFind[0] -ge [int] $execSearch[0] -or [int] $execFind[0] -ge [int] $execRemove[0]) {
  throw "InstallExecuteSequence must run FindRelatedProducts ($($execFind[0])) before AppSearch and RemoveExistingProducts"
}

$launchConditions = Read-Rows 'SELECT `Condition`, `Description` FROM `LaunchCondition`' 2
$Installer.UILevel = 2
$session = $null
try {
  $session = $Installer.OpenPackage((Resolve-Path -LiteralPath $Path).Path, 1)
  $exe = 'C:\PdfCraft\pdfcraft.exe'
  # The welcome dialog's default, replayed with Windows Installer's evaluator in a restricted session
  # that cannot change machine state. Name, App Paths found per machine and per user, then the
  # resulting INSTALLSCOPE, ALLUSERS and MSIINSTALLPERUSER.
  foreach ($case in @(
      @('first install', '', '', 'perUser', '2', '1'),
      @('update of a per-machine install', $exe, '', 'perMachine', '1', ''),
      @('update of a per-user install', '', $exe, 'perUser', '2', '1'),
      @('installed in both scopes', $exe, $exe, 'perMachine', '1', ''))) {
    $session.Property('INSTALLSCOPE') = ''
    $session.Property('ALLUSERS') = '1'
    $session.Property('MSIINSTALLPERUSER') = ''
    $session.Property('Installed') = ''
    $session.Property('EXISTINGMACHINEINSTALL') = $case[1]
    $session.Property('EXISTINGUSERINSTALL') = $case[2]
    foreach ($action in $scopeActions) {
      if ($session.EvaluateCondition($action[2]) -eq 1) { $session.Property($action[0]) = $action[1] }
    }
    Assert-Equal $session.Property('INSTALLSCOPE') $case[3] "Full UI, $($case[0]): INSTALLSCOPE"
    Assert-Equal $session.Property('ALLUSERS') $case[4] "Full UI, $($case[0]): ALLUSERS"
    Assert-Equal $session.Property('MSIINSTALLPERUSER') $case[5] "Full UI, $($case[0]): MSIINSTALLPERUSER"
    foreach ($condition in $launchConditions) {
      if ($session.EvaluateCondition($condition[0]) -ne 1) { throw "Full UI, $($case[0]): stopped by '$($condition[1])'" }
    }
  }
  # Launch conditions: no second copy in the other scope. ALLUSERS and MSIINSTALLPERUSER are given
  # as Windows Installer leaves them (ALLUSERS=2 is resolved to 1 or empty) and as passed. Name,
  # ALLUSERS, MSIINSTALLPERUSER, Installed, App Paths found per machine and per user, setup goes on (1)
  # or stops (0).
  foreach ($case in @(
      @('first install per machine', '1', '', '', '', '', 1),
      @('first install per user', '', '1', '', '', '', 1),
      @('first install per user, as passed', '2', '1', '', '', '', 1),
      @('first install, ALLUSERS=2 MSIINSTALLPERUSER=""', '2', '', '', '', '', 1),
      @('per-machine update', '1', '', '', $exe, '', 1),
      @('per-user update', '', '1', '', '', $exe, 1),
      @('per-user copy beside a per-machine install', '', '1', '', $exe, '', 0),
      @('per-user copy beside a per-machine install, as passed', '2', '1', '', $exe, '', 0),
      @('per-machine copy beside a per-user install', '1', '', '', '', $exe, 0),
      @('per-user update, installed in both scopes', '', '1', '', $exe, $exe, 1),
      @('per-machine update, installed in both scopes', '1', '', '', $exe, $exe, 1),
      @('per-machine repair', '1', '', '1', $exe, '', 1),
      @('per-user uninstall', '', '1', '1', '', $exe, 1),
      @('legacy per-user uninstall', '', '', '1', $exe, '', 1))) {
    $session.Property('ALLUSERS') = $case[1]
    $session.Property('MSIINSTALLPERUSER') = $case[2]
    $session.Property('Installed') = $case[3]
    $session.Property('EXISTINGMACHINEINSTALL') = $case[4]
    $session.Property('EXISTINGUSERINSTALL') = $case[5]
    $passes = 1
    foreach ($condition in $launchConditions) { if ($session.EvaluateCondition($condition[0]) -ne 1) { $passes = 0 } }
    Assert-Equal $passes $case[6] "$($case[0]): launch conditions"
  }
} finally {
  if ($session) { [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($session) }
}
foreach ($installSearch in @(@('EXISTINGMACHINEINSTALL', '2'), @('EXISTINGUSERINSTALL', '1'))) {
  $locator = Read-Row ('SELECT `Root`, `Key`, `Name` FROM `RegLocator`, `AppSearch` WHERE `AppSearch`.`Signature_` = `RegLocator`.`Signature_` AND `AppSearch`.`Property` = ''' + $installSearch[0] + '''') 3
  Assert-Equal $locator[0] $installSearch[1] "$($installSearch[0]) search root"
  Assert-Equal $locator[1] 'Software\Microsoft\Windows\CurrentVersion\App Paths\pdfcraft.exe' "$($installSearch[0]) search key"
  Assert-Equal $locator[2] '' "$($installSearch[0]) search reads the default value"
}
$secure = Read-Row 'SELECT `Value` FROM `Property` WHERE `Property` = ''SecureCustomProperties''' 1
if (($secure[0] -split ';') -notcontains 'INSTALLFOLDER') { throw "INSTALLFOLDER is not secure: '$($secure[0])'" }
# Every registration follows the install scope: HKMU (-1) is HKLM per machine and HKCU per user.
foreach ($row in (Read-Rows 'SELECT `Registry`, `Root` FROM `Registry`' 2)) {
  Assert-Equal $row[1] '-1' "Registry row $($row[0]) root"
}
# The welcome dialog offers the scope only for a first install. Each Install button sets the context
# the way Windows Installer's single package authoring sample does, and the install folder, and only
# the per-machine one carries the UAC shield.
$radio = Read-Row 'SELECT `Type`, `Property` FROM `Control` WHERE `Dialog_` = ''InstallWelcome'' AND `Control` = ''Scope''' 2
Assert-Equal $radio[0] 'RadioButtonGroup' 'Welcome scope control'
Assert-Equal $radio[1] 'INSTALLSCOPE' 'Welcome scope property'
$choices = @((Read-Rows 'SELECT `Value`, `Order` FROM `RadioButton` WHERE `Property` = ''INSTALLSCOPE''' 2) | Sort-Object { [int] $_[1] } | ForEach-Object { $_[0] })
Assert-Equal ($choices -join ',') 'perUser,perMachine' 'Welcome scope choices'
$locked = Read-Row 'SELECT `Condition` FROM `ControlCondition` WHERE `Dialog_` = ''InstallWelcome'' AND `Control_` = ''Scope'' AND `Action` = ''Disable''' 1
Assert-Equal $locked[0] 'EXISTINGMACHINEINSTALL OR EXISTINGUSERINSTALL' 'Welcome scope locked on update'
foreach ($button in @(
    @('InstallPerUser', 0, 'perUser', 'ALLUSERS=2;MSIINSTALLPERUSER=1;INSTALLFOLDER=[LocalAppDataFolder]Programs\PdfCraft\'),
    @('InstallPerMachine', 1, 'perMachine', 'ALLUSERS=1;INSTALLFOLDER=[PERMACHINEFOLDER]'))) {
  $control = Read-Row ('SELECT `Attributes` FROM `Control` WHERE `Dialog_` = ''InstallWelcome'' AND `Control` = ''' + $button[0] + '''') 1
  # msidbControlAttributesElevationShield (8388608).
  $shield = if (([int] $control[0] -band 8388608) -ne 0) { 1 } else { 0 }
  Assert-Equal $shield $button[1] "$($button[0]) UAC shield"
  $show = Read-Row ('SELECT `Condition` FROM `ControlCondition` WHERE `Dialog_` = ''InstallWelcome'' AND `Control_` = ''' + $button[0] + ''' AND `Action` = ''Show''') 1
  if ($show[0] -notmatch $button[2]) { throw "$($button[0]) is shown for '$($show[0])'" }
  $events = Read-Rows ('SELECT `Event`, `Argument`, `Ordering` FROM `ControlEvent` WHERE `Dialog_` = ''InstallWelcome'' AND `Control_` = ''' + $button[0] + '''') 3
  $sets = @($events | Where-Object { $_[0] -match '^\[.+\]$' } | Sort-Object { [int] $_[2] } | ForEach-Object { $_[0].Trim('[', ']') + '=' + $_[1] })
  Assert-Equal ($sets -join ';') $button[3] "$($button[0]) installation context"
  $end = @($events | Where-Object { $_[0] -eq 'EndDialog' })
  if ($end.Count -ne 1 -or $end[0][1] -ne 'Return' -or [int] $end[0][2] -le $sets.Count) { throw "$($button[0]) must set the context before it returns" }
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
# Defaults an update keeps (#1058, #305). No static default: one action, in both sequences but run
# only in the first (msidbCustomActionTypeFirstSequence), turns it on for a first install and, for an
# update from a release that offered it ($Release set), only while what it installed is still there
# in the install scope's place (found per machine or per user, from AppSearch). In the full UI it runs
# after the scope and the upgrade search, which follow the dialog's choice, so the checkboxes start
# from it; otherwise it runs in the execute sequence after AppSearch. Each sequence has its own
# searches: the desktop shortcut's execute-sequence search is DesktopFolder, which follows the context
# setup starts in, while the full UI starts per machine and looks for "Only me" by path. A
# command-line value is left alone. Windows Installer's evaluator runs the sequence's condition and
# then $Component's own condition.
function Assert-KeptOnUpdate([string] $Property, [string] $Release, [string] $UiMachine, [string] $UiUser, [string] $ExecMachine, [string] $ExecUser, [string] $FoundValue, [string] $Component, [string] $What) {
  Assert-NoRow ('SELECT `Value` FROM `Property` WHERE `Property` = ''' + $Property + '''') "$What static default"
  $action = Read-Row ('SELECT `Action`, `Target`, `Type` FROM `CustomAction` WHERE `Source` = ''' + $Property + '''') 3
  Assert-Equal $action[1] '1' "$What default value"
  # Type 51 (set a property) with msidbCustomActionTypeFirstSequence (256).
  Assert-Equal ([int] $action[2] -band 0x13F) 307 "$What default action type"
  $conditions = @{}
  foreach ($sequence in @('InstallUISequence', 'InstallExecuteSequence')) {
    $search = Read-Row ('SELECT `Sequence` FROM `' + $sequence + '` WHERE `Action` = ''AppSearch''') 1
    $find = Read-Row ('SELECT `Sequence` FROM `' + $sequence + '` WHERE `Action` = ''FindRelatedProducts''') 1
    $set = Read-Row ('SELECT `Sequence`, `Condition` FROM `' + $sequence + '` WHERE `Action` = ''' + $action[0] + '''') 2
    if ([int] $set[0] -le [int] $search[0] -or [int] $set[0] -le [int] $find[0]) {
      throw "$sequence must set the $What default after AppSearch and FindRelatedProducts"
    }
    $conditions[$sequence] = $set[1]
  }
  $installed = Read-Row ('SELECT `Condition` FROM `Component` WHERE `Component` = ''' + $Component + '''') 1
  Assert-Equal $installed[0] "$Property = 1" "$Component condition"
  $old = '{00000000-0000-0000-0000-000000000000}'
  # Name, command-line value, WIX_UPGRADE_DETECTED, $Release, found for this scope, found only for the
  # other scope, installed (1) or not (0).
  $cases = @(
    @('first install', '', '', '', '', '', 1),
    @("first install, $Property=0", '0', '', '', '', '', 0),
    @('update, still there', '', $old, $old, $FoundValue, '', 1),
    @('update, removed', '', $old, $old, '', '', 0),
    @("update, $Property=0", '0', $old, $old, $FoundValue, '', 0),
    @("update, removed, $Property=1", '1', $old, $old, '', '', 1))
  # An update from a release from before it existed (WIX_UPGRADE_DETECTED alone) is a first install.
  if ($Release -ne 'WIX_UPGRADE_DETECTED') { $cases += , @('update from a release without it', '', $old, '', '', '', 1) }
  $session = $null
  try {
    $session = $Installer.OpenPackage((Resolve-Path -LiteralPath $Path).Path, 1)
    # The context as the full UI's scope actions leave it (ALLUSERS=1 per machine, ALLUSERS=2 and
    # MSIINSTALLPERUSER=1 per user) and as Windows Installer resolves it for the execute sequence
    # (ALLUSERS=1, or empty per user); the searches this sequence reads for that scope.
    foreach ($scope in @(
        @('full UI, per machine', 'InstallUISequence', '1', '', $UiMachine, $UiUser),
        @('full UI, per user', 'InstallUISequence', '2', '1', $UiUser, $UiMachine),
        @('unattended, per machine', 'InstallExecuteSequence', '1', '', $ExecMachine, $ExecUser),
        @('unattended, per user', 'InstallExecuteSequence', '', '1', $ExecUser, $ExecMachine))) {
      $scopeCases = $cases
      if ($scope[4] -ne $scope[5]) { $scopeCases += , @('update, removed, still there for the other scope', '', $old, $old, '', $FoundValue, 0) }
      foreach ($case in $scopeCases) {
        foreach ($found in @($UiMachine, $UiUser, $ExecMachine, $ExecUser)) { $session.Property($found) = '' }
        $session.Property('ALLUSERS') = $scope[2]
        $session.Property('MSIINSTALLPERUSER') = $scope[3]
        $session.Property($Property) = $case[1]
        $session.Property('WIX_UPGRADE_DETECTED') = $case[2]
        $session.Property($Release) = $case[3]
        if ($scope[4] -ne $scope[5]) { $session.Property($scope[5]) = $case[5] }
        $session.Property($scope[4]) = $case[4]
        if ($session.EvaluateCondition($conditions[$scope[1]]) -eq 1) { $session.Property($Property) = $action[1] }
        Assert-Equal ($session.EvaluateCondition($installed[0])) $case[6] "$What, $($scope[0])`: $($case[0])"
      }
    }
  } finally {
    if ($session) { [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($session) }
  }
}
Assert-KeptOnUpdate 'INSTALLDESKTOPSHORTCUT' 'WIX_UPGRADE_DETECTED' 'EXISTINGDESKTOPSHORTCUT' 'EXISTINGUSERDESKTOPSHORTCUT' 'EXISTINGDESKTOPSHORTCUT' 'EXISTINGDESKTOPSHORTCUT' 'C:\Users\Public\Desktop\PdfCraft.lnk' 'PdfcraftDesktopShortcut' 'Desktop shortcut'
if (($secure[0] -split ';') -notcontains 'INSTALLDESKTOPSHORTCUT') { throw "INSTALLDESKTOPSHORTCUT is not secure: '$($secure[0])'" }
$checkbox = Read-Row 'SELECT `Type`, `Property`, `Text` FROM `Control` WHERE `Dialog_` = ''InstallWelcome'' AND `Control` = ''DesktopShortcut''' 3
Assert-Equal $checkbox[0] 'CheckBox' 'Welcome desktop-shortcut control'
Assert-Equal $checkbox[1] 'INSTALLDESKTOPSHORTCUT' 'Welcome checkbox property'
if ($checkbox[2] -notmatch 'desktop shortcut') { throw "Welcome checkbox label: '$($checkbox[2])'" }
$checked = Read-Row 'SELECT `Value` FROM `CheckBox` WHERE `Property` = ''INSTALLDESKTOPSHORTCUT''' 1
Assert-Equal $checked[0] '1' 'Welcome checkbox value'
$app = Read-Row 'SELECT `KeyPath` FROM `Component` WHERE `Component` = ''PdfcraftApp''' 1
Assert-Equal $app[0] 'PdfcraftExe' 'Shortcut executable key path'

# Each File Explorer menu remembers whether it was installed by finding its key path in HKLM (per
# machine) or HKCU (per user), in the package's own registry view (64-bit for x64 and arm64). Only a
# release that offered the menu can leave it out, so the OnlyDetect Upgrade rows start at the first
# release with that menu.
$upgradeCode = Read-Row 'SELECT `Value` FROM `Property` WHERE `Property` = ''UpgradeCode''' 1
function Assert-MenuSearch([string] $Found, [string] $Root, [string] $Key, [string] $Release, [string] $Since, [string] $What) {
  $search = Read-Row ('SELECT `Root`, `Key`, `Name`, `Type` FROM `RegLocator`, `AppSearch` WHERE `AppSearch`.`Signature_` = `RegLocator`.`Signature_` AND `AppSearch`.`Property` = ''' + $Found + '''') 4
  Assert-Equal $search[0] $Root "$What search root"
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
  Assert-Equal $menu[2] '-1' "$ext context menu HKMU root"
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
Assert-KeptOnUpdate 'INSTALLIMAGECONTEXTMENU' 'IMAGECONTEXTMENURELEASE' 'EXISTINGIMAGECONTEXTMENU' 'EXISTINGUSERIMAGECONTEXTMENU' 'EXISTINGIMAGECONTEXTMENU' 'EXISTINGUSERIMAGECONTEXTMENU' 'Create PDF with PdfCraft…' 'PdfcraftImageContextMenu' 'Image context menu'
Assert-MenuSearch 'EXISTINGIMAGECONTEXTMENU' '2' $imageKey 'IMAGECONTEXTMENURELEASE' '0.4.0' 'Image context menu (per machine)'
Assert-MenuSearch 'EXISTINGUSERIMAGECONTEXTMENU' '1' $imageKey 'IMAGECONTEXTMENURELEASE' '0.4.0' 'Image context menu (per user)'
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
Assert-Equal $pdfMenu[3] '-1' 'PDF context menu HKMU root'
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
Assert-KeptOnUpdate 'INSTALLPDFCONTEXTMENU' 'PDFCONTEXTMENURELEASE' 'EXISTINGPDFCONTEXTMENU' 'EXISTINGUSERPDFCONTEXTMENU' 'EXISTINGPDFCONTEXTMENU' 'EXISTINGUSERPDFCONTEXTMENU' 'Edit with PdfCraft' 'PdfcraftPdfContextMenu' 'PDF context menu'
Assert-MenuSearch 'EXISTINGPDFCONTEXTMENU' '2' $pdfKey 'PDFCONTEXTMENURELEASE' '0.7.1' 'PDF context menu (per machine)'
Assert-MenuSearch 'EXISTINGUSERPDFCONTEXTMENU' '1' $pdfKey 'PDFCONTEXTMENURELEASE' '0.7.1' 'PDF context menu (per user)'
if (($secure[0] -split ';') -notcontains 'INSTALLPDFCONTEXTMENU') { throw "INSTALLPDFCONTEXTMENU is not secure: '$($secure[0])'" }
$pdfCheckbox = Read-Row 'SELECT `Type`, `Property`, `Text` FROM `Control` WHERE `Dialog_` = ''InstallWelcome'' AND `Control` = ''PdfContextMenu''' 3
Assert-Equal $pdfCheckbox[0] 'CheckBox' 'Welcome PDF context-menu control'
Assert-Equal $pdfCheckbox[1] 'INSTALLPDFCONTEXTMENU' 'Welcome PDF context-menu property'
if ($pdfCheckbox[2] -notmatch 'Edit with PdfCraft') { throw "Welcome PDF context-menu label: '$($pdfCheckbox[2])'" }
$pdfChecked = Read-Row 'SELECT `Value` FROM `CheckBox` WHERE `Property` = ''INSTALLPDFCONTEXTMENU''' 1
Assert-Equal $pdfChecked[0] '1' 'Welcome PDF context-menu value'
Assert-NoRow 'SELECT `Registry` FROM `Registry` WHERE `Key` = ''Software\Classes\.pdf'' AND `Name` IS NULL' '.pdf default app'

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
Write-Output 'ok MSI: per-machine or per-user scope (authored per machine, full-UI choice, no second copy in the other scope), HKMU registrations, publisher, persistent progress text, Start Menu shortcut, optional desktop shortcut and PDF and image context menus (on for a first install, kept on update only while present; checkboxes for the first two), icon/key path, full-UI success/cancel/error and Finish controls, files-in-use dialog, OCR models'
