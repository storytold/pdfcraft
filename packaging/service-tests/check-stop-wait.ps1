# Synthetic read-only states: no service, installer or administrator access.
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/wait-stopped.ps1"
foreach ($first in @(
    @{ WorkerStopped = $true; ServiceStopped = $false },
    @{ WorkerStopped = $false; ServiceStopped = $true }
)) {
    $probe = @{ Calls = 0; First = $first }
    Wait-ServiceStopped -TimeoutMilliseconds 2000 -ReadState {
        $probe.Calls++
        if ($probe.Calls -lt 3) { return $probe.First }
        return @{ WorkerStopped = $true; ServiceStopped = $true }
    }
    if ($probe.Calls -ne 3) { throw 'Stop wait returned before both conditions were observed.' }
}
foreach ($pending in @(
    @{ WorkerStopped = $true; ServiceStopped = $false },
    @{ WorkerStopped = $false; ServiceStopped = $true }
)) {
    $rejected = $false
    try { Wait-ServiceStopped -TimeoutMilliseconds 10 -ReadState { $pending } }
    catch { $rejected = $_.Exception.Message -like 'Timed out waiting for graceful service stop*' }
    if (-not $rejected) { throw 'An incomplete stop must time out with a diagnostic.' }
}
$rejected = $false
try { Wait-ServiceStopped -ReadState { throw 'synthetic status query failure' } }
catch { $rejected = $_.Exception.Message -eq 'synthetic status query failure' }
if (-not $rejected) { throw 'Status query failures must propagate.' }
Write-Output 'ok: delayed worker/SCM completion, bounded failure and status-query errors'
