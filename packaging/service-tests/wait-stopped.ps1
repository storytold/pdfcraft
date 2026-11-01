# Safe to source on a workstation: this helper only reads the supplied probe.
function Wait-ServiceStopped(
    [scriptblock] $ReadState,
    [ValidateRange(1, 60000)] [int] $TimeoutMilliseconds = 10000
) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    do {
        $state = & $ReadState
        # The worker marker precedes the host's monitor join and SCM Stopped report.
        # Either observation alone is insufficient to establish a graceful stop.
        if ($state.WorkerStopped -and $state.ServiceStopped) { return }
        Start-Sleep -Milliseconds ([Math]::Min(250, $TimeoutMilliseconds))
    } while ($timer.ElapsedMilliseconds -lt $TimeoutMilliseconds)
    throw "Timed out waiting for graceful service stop: workerStopped=$($state.WorkerStopped), serviceStopped=$($state.ServiceStopped)"
}
