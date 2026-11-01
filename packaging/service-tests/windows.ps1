# Disposable CI machine only. Does not require a logged-in desktop for the worker.
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/wait-stopped.ps1"
$probe = (Resolve-Path -LiteralPath 'target/debug/examples/service_probe.exe').Path
$directory = Join-Path $env:RUNNER_TEMP 'Hot Folder service test'
New-Item -ItemType Directory -Path $directory | Out-Null
$config = Join-Path $directory 'tasks.json'
Set-Content -LiteralPath $config -Value '{}'
& icacls.exe $directory /grant '*S-1-5-19:(OI)(CI)F' | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Could not grant LocalService access to the synthetic test directory' }
$command = '"' + $probe + '" --service --config "' + $config + '"'
& sc.exe create PdfCraftHotFolder binPath= $command start= auto obj= 'NT AUTHORITY\LocalService'
if ($LASTEXITCODE -ne 0) { throw 'Service registration failed' }
function Invoke-Probe([string] $Action) {
  $result = & $probe --service-action $Action --config $config
  if ($LASTEXITCODE -ne 0) { throw "Service probe failed: $Action" }
  return $result
}
try {
  Invoke-Probe start
  for ($i = 0; $i -lt 40 -and -not (Test-Path -LiteralPath (Join-Path $directory 'tasks.heartbeat')); $i++) { Start-Sleep -Milliseconds 250 }
  if (-not (Test-Path -LiteralPath (Join-Path $directory 'tasks.heartbeat'))) { throw 'Service never processed its synthetic configuration' }
  if ((Invoke-Probe status) -notmatch 'installed=true enabled=true running=true') { throw 'Wrong running/startup state' }
  $service = Get-CimInstance Win32_Service -Filter "Name='PdfCraftHotFolder'"
  if ($service.StartName -ne 'NT AUTHORITY\LocalService') { throw 'Service is not running as LocalService' }
  if ((Invoke-Probe disable) -notmatch 'enabled=false') { throw 'Disabling boot startup failed' }
  if ((Invoke-Probe enable) -notmatch 'enabled=true') { throw 'Enabling boot startup failed' }
  Invoke-Probe stop
  Wait-ServiceStopped -ReadState {
    $probeState = Invoke-Probe status
    $scmState = Get-CimInstance Win32_Service -Filter "Name='PdfCraftHotFolder'"
    @{
      WorkerStopped = Test-Path -LiteralPath (Join-Path $directory 'tasks.stopped')
      # The public running flag is also false during StopPending; require SCM completion.
      ServiceStopped = $scmState.State -eq 'Stopped' -and $probeState -match 'installed=true enabled=true running=false'
    }
  }
} finally {
  & sc.exe stop PdfCraftHotFolder | Out-Null
  & sc.exe delete PdfCraftHotFolder | Out-Null
}
