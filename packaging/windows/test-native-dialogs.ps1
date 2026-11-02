# Run only on an isolated GitHub-hosted Windows desktop. Inspect actual MSI
# preview windows with UI Automation from a separate MTA process. Do not use
# database strings as the observed result, click buttons or install the fixture.
# Coverage is exact Unicode text, unique native identity and visible geometry;
# it does not qualify checkbox state, Invoke/Toggle patterns or user interaction.
# Use Windows PowerShell (-MTA), whose framework includes UIAutomationClient.
param([Parameter(Mandatory)] [string] $Msi, [Parameter(Mandatory)] [string] $Work)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
  throw 'Native MSI UI previews are restricted to isolated GitHub-hosted runners.'
}
if ($PSVersionTable.PSEdition -ne 'Desktop' -or [Threading.Thread]::CurrentThread.GetApartmentState() -ne 'MTA') {
  throw 'Run this probe with Windows PowerShell -MTA (not pwsh).'
}
if (Test-Path -LiteralPath $Work) { throw "Probe directory must be new: $Work" }
$Work = [IO.Path]::GetFullPath($Work)
New-Item -ItemType Directory -Path $Work | Out-Null
$report = [ordered]@{ status = 'incomplete'; scope = 'native MSI preview control text and bounds'; dialogs = @(); error = $null }
$installer = $null
function Resolve-DialogText([string] $Text, $Strings, [string] $ProductName) {
  $text = [regex]::Replace($Text, '!\(loc\.([A-Za-z]+)\)', { param($m) $Strings[$m.Groups[1].Value] })
  $text = [regex]::Replace($text, '\{\\[^}]+\}', '').Replace('[ProductName]', $ProductName)
  # UIA names omit native accelerator markers; doubled ampersands are literal.
  return $text.Replace('&&', [string] [char] 1).Replace('&', '').Replace([string] [char] 1, '&')
}
function Read-ProductName($Database) {
  $view = $Database.OpenView('SELECT `Value` FROM `Property` WHERE `Property` = ''ProductName''')
  $record = $null
  try {
    [void] $view.Execute()
    $record = $view.Fetch()
    if (-not $record) { throw 'Fixture lacks ProductName' }
    return $record.StringData(1)
  } finally {
    if ($record) { [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($record) }
    [void] $view.Close()
    [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($view)
  }
}
function Test-NativeControlIdentity($Current, [string] $ExpectedText, [string] $ExpectedClass, [int] $ExpectedProcessId) {
  # MSI preview exposes HWND-backed Win32 controls, but this runner's UIA
  # provider reports Pane with no patterns even for Static and Button classes.
  # Match observed native identity; do not claim an unreported UIA role/pattern.
  return $Current.Name -ceq $ExpectedText -and $Current.ClassName -ceq $ExpectedClass -and
    $Current.FrameworkId -ceq 'Win32' -and $Current.ProcessId -eq $ExpectedProcessId -and
    $Current.NativeWindowHandle -ne 0 -and -not [string]::IsNullOrWhiteSpace($Current.AutomationId)
}
function Write-PreviewDiagnostics($Child, [string] $Id, [string] $ExpectedTitle) {
  # This runs only after the hosted-runner guard and only on failure. Inspect
  # all top-level candidates to distinguish a title mismatch from a different
  # COM server PID or a hidden/inaccessible desktop; do not relax qualification.
  $details = [ordered]@{
    dialog = $Id; expectedTitle = $ExpectedTitle; expectedProcessId = $Child.Id
    observerProcessId = $PID; observerSessionId = [Diagnostics.Process]::GetCurrentProcess().SessionId
    interactive = [Environment]::UserInteractive; child = $null; windows = @(); errors = @()
  }
  try {
    $Child.Refresh()
    $details.child = [ordered]@{
      exited = $Child.HasExited; sessionId = $Child.SessionId
      mainWindowTitle = $Child.MainWindowTitle; mainWindowHandle = $Child.MainWindowHandle.ToInt64()
      exitCode = $(if ($Child.HasExited) { $Child.ExitCode } else { $null })
    }
  } catch { $details.errors += "Child process inspection: $($_.Exception.Message)" }
  try {
    $candidates = [Windows.Automation.AutomationElement]::RootElement.FindAll(
      [Windows.Automation.TreeScope]::Children, [Windows.Automation.Condition]::TrueCondition)
    $details['topLevelWindowCount'] = $candidates.Count
    foreach ($candidate in @($candidates | Select-Object -First 256)) {
      try {
        $current = $candidate.Current
        $owner = Get-Process -Id $current.ProcessId -ErrorAction SilentlyContinue
        $details.windows += [ordered]@{
          name = $current.Name; processId = $current.ProcessId; processName = $owner.ProcessName
          sessionId = $owner.SessionId; className = $current.ClassName; handle = $current.NativeWindowHandle
          type = $current.ControlType.ProgrammaticName; offscreen = $current.IsOffscreen
          bounds = $current.BoundingRectangle.ToString(); expectedProcess = $current.ProcessId -eq $Child.Id
          expectedTitle = $current.Name -ceq $ExpectedTitle
        }
      } catch { $details.errors += "Window inspection: $($_.Exception.Message)" }
    }
  } catch { $details.errors += "Desktop inspection: $($_.Exception.Message)" }
  $path = Join-Path $Work "$Id.diagnostics.json"
  ConvertTo-Json -InputObject $details -Depth 8 | Set-Content -LiteralPath $path -Encoding utf8
  Write-Output "Native MSI preview diagnostics: $path"
}
try {
  if (-not [Environment]::UserInteractive) {
    throw 'Native MSI UI qualification unavailable: runner has no interactive desktop. This is not a pass.'
  }
  Add-Type -AssemblyName UIAutomationClient
  Add-Type -AssemblyName UIAutomationTypes
  [xml] $ui = Get-Content (Join-Path $PSScriptRoot 'installer-ui.wxs') -Raw -Encoding utf8
  $installer = New-Object -ComObject WindowsInstaller.Installer
  foreach ($language in @('hi', 'bn', 'ur')) {
    [xml] $catalog = Get-Content (Join-Path $PSScriptRoot "locales/$language.wxl") -Raw -Encoding utf8
    $strings = @{}
    foreach ($entry in $catalog.WixLocalization.String) { $strings[$entry.Id] = $entry.Value }
    $copy = Join-Path $Work "preview-$language.msi"
    Copy-Item -LiteralPath $Msi -Destination $copy
    $database = $installer.OpenDatabase($copy, 1)
    try {
      [void] $database.ApplyTransform(":$language.mst", 32)
      $productName = Read-ProductName $database
      [void] $database.Commit()
    } finally { [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($database) }
    # Maintenance's feature conditions and Restart Manager need live install
    # state. Those remain database-tested; this probe never starts an install.
    foreach ($dialog in @($ui.Wix.Fragment.UI.Dialog | Where-Object { $_.Id -notin @('InstallMaintenance', 'MsiRMFilesInUse') })) {
      $id = "$language-$($dialog.Id)"
      $stop = Join-Path $Work "$id.stop"
      $stdout = Join-Path $Work "$id.stdout.log"
      $stderr = Join-Path $Work "$id.stderr.log"
      $childScript = Join-Path $PSScriptRoot 'preview-installer-dialog.ps1'
      # These are Windows filenames, not shell commands; embedded quotes are
      # invalid in Windows paths. Launch without cmd.exe or expression building.
      $arguments = @('-NoProfile', '-NonInteractive', '-STA', '-File', "`"$childScript`"",
        '-Msi', "`"$copy`"", '-Dialog', $dialog.Id, '-StopFile', "`"$stop`"")
      $title = Resolve-DialogText $dialog.Title $strings $productName
      # Keep the existing CI console, but allow the native preview to be shown.
      # WindowStyle Hidden sets STARTUPINFO's first-window state and suppresses
      # the MSI dialog itself, even though ViewDialog returns successfully.
      $child = Start-Process -FilePath (Join-Path $PSHOME 'powershell.exe') -ArgumentList $arguments `
        -NoNewWindow -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
      try {
        $condition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty, $child.Id)
        $deadline = [DateTime]::UtcNow.AddSeconds(15)
        $window = $null
        while (-not $window -and [DateTime]::UtcNow -lt $deadline) {
          if ($child.HasExited) { throw "Preview exited before $id appeared: $(Get-Content -LiteralPath $stderr -Raw)" }
          $windows = [Windows.Automation.AutomationElement]::RootElement.FindAll([Windows.Automation.TreeScope]::Children, $condition)
          foreach ($candidate in $windows) {
            if ($candidate.Current.Name -ceq $title) { $window = $candidate; break }
          }
          if (-not $window) { Start-Sleep -Milliseconds 100 }
        }
        if (-not $window) { throw "Native MSI UI qualification unavailable: $id window was not accessible on the runner desktop (expected '$title')." }
        $windowBounds = $window.Current.BoundingRectangle
        if ($window.Current.IsOffscreen -or $windowBounds.Width -le 0 -or $windowBounds.Height -le 0) { throw "$id preview is not visible" }
        $elements = $window.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.Condition]::TrueCondition)
        $observed = @($elements | ForEach-Object {
          [ordered]@{ name = $_.Current.Name; type = $_.Current.ControlType.ProgrammaticName;
            className = $_.Current.ClassName; automationId = $_.Current.AutomationId;
            framework = $_.Current.FrameworkId; handle = $_.Current.NativeWindowHandle; processId = $_.Current.ProcessId;
            patterns = @($_.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName });
            offscreen = $_.Current.IsOffscreen; bounds = $_.Current.BoundingRectangle.ToString() }
        })
        $observed | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $Work "$id.controls.json") -Encoding utf8
        $checked = 0
        $seenHandles = @{}
        $seenIds = @{}
        foreach ($control in @($dialog.Control | Where-Object { $_.Text })) {
          $expected = Resolve-DialogText $control.Text $strings $productName
          $nativeClass = @{ Text = 'Static'; CheckBox = 'Button'; PushButton = 'Button' }[$control.Type]
          if (-not $nativeClass) { throw "Unaccounted native text control type $($control.Type)" }
          $matching = @($elements | Where-Object { Test-NativeControlIdentity $_.Current $expected $nativeClass $child.Id })
          if ($control.HideCondition) {
            # Scope-dependent twins (Install/InstallUser, Description/DescriptionUser) share a
            # position and sometimes a text; only the one the preview context shows is checked.
            $matching = @($matching | Where-Object { -not $_.Current.IsOffscreen -and -not $seenHandles.ContainsKey($_.Current.NativeWindowHandle) } | Select-Object -First 1)
            if ($matching.Count -eq 0) { continue }
          }
          if ($matching.Count -ne 1) { throw "$id.$($control.Id): expected one native $nativeClass with exact Unicode text '$expected' in the preview process; found $($matching.Count)." }
          $element = $matching[0].Current
          if ($seenHandles.ContainsKey($element.NativeWindowHandle) -or $seenIds.ContainsKey($element.AutomationId)) {
            throw "$id.$($control.Id) reused another control's native identity"
          }
          $seenHandles[$element.NativeWindowHandle] = $true
          $seenIds[$element.AutomationId] = $true
          $bounds = $element.BoundingRectangle
          if ($element.IsOffscreen -or $bounds.Width -le 0 -or $bounds.Height -le 0 -or
              $bounds.Left -lt $windowBounds.Left -or $bounds.Top -lt $windowBounds.Top -or
              $bounds.Right -gt $windowBounds.Right -or $bounds.Bottom -gt $windowBounds.Bottom) {
            throw "$id.$($control.Id) is hidden or outside the native dialog"
          }
          $checked++
        }
        $report.dialogs += [ordered]@{ language = $language; dialog = $dialog.Id; title = $window.Current.Name; controls = $checked }
        Write-Output "ok $id`: $checked actual native Unicode control texts and visible bounds"
      } catch {
        $failure = $_
        try { Write-PreviewDiagnostics $child $id $title }
        catch { Write-Warning "Could not write preview diagnostics: $($_.Exception.Message)" }
        throw $failure
      } finally {
        [IO.File]::WriteAllText($stop, 'stop')
        if (-not $child.WaitForExit(3000)) { $child.Kill(); $child.WaitForExit() }
        $child.Dispose()
      }
    }
  }
  $report.status = 'passed'
} catch {
  $report.status = 'failed'
  $report.error = $_.Exception.Message
  throw
} finally {
  if ($installer) { [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer) }
  $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $Work 'native-dialog-report.json') -Encoding utf8
}
