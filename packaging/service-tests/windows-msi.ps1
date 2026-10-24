# Disposable CI machine only: build the real installer definition around a synthetic
# service executable, then test opt-in installation and component removal.
$ErrorActionPreference = 'Stop'
$stage = Join-Path $env:RUNNER_TEMP 'hot-folder-msi'
New-Item -ItemType Directory -Path "$stage/models" -Force | Out-Null
foreach ($name in 'pdfcraft.exe', 'pdfcraft-cli.exe', 'pdfcraft-hot-folder.exe') {
  Copy-Item -LiteralPath 'target/debug/examples/service_probe.exe' -Destination (Join-Path $stage $name)
}
foreach ($name in 'LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE') { Copy-Item -LiteralPath $name -Destination $stage }
Copy-Item -LiteralPath 'assets/app-icon/LICENSE-document.txt' -Destination $stage
Set-Content -LiteralPath "$stage/models/detection.rten" -Value 'synthetic installer fixture; never an OCR model'
Set-Content -LiteralPath "$stage/models/recognition.rten" -Value 'synthetic installer fixture; never an OCR model'
$msi = Join-Path $stage 'service-test.msi'
$upgrade = Join-Path $stage 'service-test-upgrade.msi'
$installedMsi = $msi
& wix extension add -g WixToolset.Util.wixext/5.0.2
if ($LASTEXITCODE -ne 0) { throw 'WiX extension installation failed' }
& wix build packaging/windows/pdfcraft.wxs packaging/windows/installer-ui.wxs -arch x64 -ext WixToolset.Util.wixext -d Version=0.6.0 -d "BinDir=$stage" -d "ModelsDir=$stage/models" -d 'IconPath=assets/app-icon/pdfcraft.ico' -o $msi
if ($LASTEXITCODE -ne 0) { throw 'WiX compilation failed' }
& pwsh -NoProfile -File packaging/windows/test-msi.ps1 $msi
if ($LASTEXITCODE -ne 0) { throw 'Compiled MSI validation failed' }
# A harmless PE overlay makes the upgrade payload observably different, without
# depending on the probe executable having a version resource.
$probeStream = [IO.File]::Open("$stage/pdfcraft-hot-folder.exe", [IO.FileMode]::Append)
try { $probeStream.Write([Text.Encoding]::ASCII.GetBytes('synthetic upgrade payload')) } finally { $probeStream.Dispose() }
$upgradeHash = (Get-FileHash -LiteralPath "$stage/pdfcraft-hot-folder.exe" -Algorithm SHA256).Hash
& wix build packaging/windows/pdfcraft.wxs packaging/windows/installer-ui.wxs -arch x64 -ext WixToolset.Util.wixext -d Version=0.6.1 -d "BinDir=$stage" -d "ModelsDir=$stage/models" -d 'IconPath=assets/app-icon/pdfcraft.ico' -o $upgrade
if ($LASTEXITCODE -ne 0) { throw 'Upgrade MSI compilation failed' }
function Invoke-Msi([string] $Options, [string] $Name) {
  $log = Join-Path $stage "$Name.log"
  $process = Start-Process -FilePath msiexec.exe -ArgumentList "$Options /qn /norestart /l*v `"$log`"" -Wait -PassThru -WindowStyle Hidden
  if ($process.ExitCode -notin @(0,3010)) {
    Select-String -LiteralPath $log -Pattern 'Return value 3|Error [0-9]|failed' -Context 12,4
    throw "MSI $Name failed: $($process.ExitCode)"
  }
}
$data = Join-Path $env:ProgramData 'PdfCraft/HotFolder'
function Wait-NewServiceProcess([uint32] $Previous, [int] $Seconds = 100) {
  $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
  do {
    $current = Get-CimInstance Win32_Service -Filter "Name='PdfCraftHotFolder'"
    if ($current.State -eq 'Running' -and $current.ProcessId -ne 0 -and $current.ProcessId -ne $Previous) { return $current.ProcessId }
    Start-Sleep -Milliseconds 300
  } while ([DateTime]::UtcNow -lt $deadline)
  throw 'SCM did not automatically recover the service within the deadline'
}
try {
  Invoke-Msi "/i `"$msi`"" 'default'
  if (Get-Service PdfCraftHotFolder -ErrorAction SilentlyContinue) { throw 'Default install unexpectedly installed the service' }
  Invoke-Msi "/i `"$msi`" ADDLOCAL=HotFolder INSTALLHOTFOLDER=1" 'opt-in'
  $service = Get-CimInstance Win32_Service -Filter "Name='PdfCraftHotFolder'"
  if ($service.State -ne 'Running' -or $service.StartMode -ne 'Auto' -or $service.StartName -ne 'NT AUTHORITY\LocalService') {
    throw 'Installed service is not running automatically as LocalService'
  }
  for ($i = 0; $i -lt 40 -and -not (Test-Path -LiteralPath "$data/tasks.heartbeat"); $i++) { Start-Sleep -Milliseconds 250 }
  if (-not (Test-Path -LiteralPath "$data/tasks.heartbeat")) { throw 'Installed service cannot write its protected task directory' }
  $registered = Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Services\PdfCraftHotFolder'
  if ($registered.ServiceSidType -ne 1 -or $registered.FailureActionsOnNonCrashFailures -ne 1) {
    throw 'MSI did not prepare a service SID and recovery for controlled errors'
  }
  $identity = [System.Security.Principal.NTAccount]::new('NT SERVICE', 'PdfCraftHotFolder')
  $sid = $identity.Translate([System.Security.Principal.SecurityIdentifier]).Value
  if ($sid -ne 'S-1-5-80-4013549229-3107762393-4025766903-2826249871-1347187068') { throw 'Installer service SID is incorrect' }
  # Recovery must continue after the old two-restart limit, and also after an
  # explicit SERVICE_STOPPED error. All faults stay on this disposable runner.
  $servicePid = [uint32] $service.ProcessId
  1..4 | ForEach-Object {
    Stop-Process -Id $servicePid -Force
    $servicePid = Wait-NewServiceProcess $servicePid
  }
  Set-Content -LiteralPath "$data/tasks.fail" -Value 'synthetic controlled failure'
  $servicePid = Wait-NewServiceProcess $servicePid
  Stop-Service PdfCraftHotFolder
  Start-Sleep -Seconds 65
  if ((Get-Service PdfCraftHotFolder).Status -ne 'Stopped') { throw 'An administrator stop must not trigger recovery' }
  # A different Windows-managed identity requires no test password. The final
  # server suite additionally covers an authenticated SMB account and same-version upgrade.
  & sc.exe config PdfCraftHotFolder obj= 'NT AUTHORITY\NetworkService'
  if ($LASTEXITCODE -ne 0) { throw 'Cannot configure the synthetic alternate service account' }
  Start-Service PdfCraftHotFolder
  $heartbeatBefore = (Get-Item -LiteralPath "$data/tasks.heartbeat").LastWriteTimeUtc
  Start-Sleep -Seconds 2
  if ((Get-Item -LiteralPath "$data/tasks.heartbeat").LastWriteTimeUtc -le $heartbeatBefore) { throw 'Alternate account cannot write through the service SID' }
  Set-Content -LiteralPath "$data/keep-user-tasks.txt" -Value 'synthetic task preservation fixture'
  # Model rows left by the pre-0.8 shared app component, including an obsolete subkey.
  # Menu opt-out on upgrade must remove these while preserving the service account.
  foreach ($ext in @('png', 'jpg', 'jpeg', 'tif', 'tiff', 'gif', 'bmp', 'jp2', 'j2k', 'jpx')) {
    $menuKey = "HKLM:\Software\Classes\SystemFileAssociations\.$ext\shell\PdfCraft.CreatePdf"
    if (-not (Test-Path -LiteralPath "$menuKey\command")) { throw "$ext image menu missing before upgrade" }
    New-Item -Path "$menuKey\LegacyProbe" -Force | Out-Null
  }
  # No INSTALLHOTFOLDER override: a major upgrade must preserve the opt-in choice.
  Invoke-Msi "/i `"$upgrade`" INSTALLIMAGECONTEXTMENU=0" 'upgrade'
  $installedMsi = $upgrade
  foreach ($ext in @('png', 'jpg', 'jpeg', 'tif', 'tiff', 'gif', 'bmp', 'jp2', 'j2k', 'jpx')) {
    if (Test-Path -LiteralPath "HKLM:\Software\Classes\SystemFileAssociations\.$ext\shell\PdfCraft.CreatePdf") {
      throw "$ext image menu survived upgrade opt-out"
    }
  }
  $service = Get-CimInstance Win32_Service -Filter "Name='PdfCraftHotFolder'"
  if ($service.State -ne 'Running' -or $service.StartMode -ne 'Auto' -or $service.StartName -ne 'NT AUTHORITY\NetworkService') {
    throw "Upgrade did not preserve the optional automatic service and its account: state=$($service.State), startup=$($service.StartMode), account=$($service.StartName)"
  }
  if (-not (Test-Path -LiteralPath "$data/keep-user-tasks.txt")) { throw 'Upgrade deleted task data' }
  if ((Get-FileHash -LiteralPath "$env:ProgramFiles/PdfCraft/pdfcraft-hot-folder.exe" -Algorithm SHA256).Hash -ne $upgradeHash) {
    throw 'Upgrade retained the old executable instead of the new payload'
  }
  Invoke-Msi "/i `"$installedMsi`" REMOVE=HotFolder" 'remove-companion'
  if (Get-Service PdfCraftHotFolder -ErrorAction SilentlyContinue) { throw 'Removing the component left its service installed' }
  if (-not (Test-Path -LiteralPath "$data/keep-user-tasks.txt")) { throw 'Removing the component deleted task data' }
} finally {
  Invoke-Msi "/x `"$installedMsi`"" 'uninstall'
}
