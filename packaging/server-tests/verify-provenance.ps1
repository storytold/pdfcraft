# Pure GitHub metadata validation. No installer or downloaded code is executed.
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_EVENT_NAME -ne 'workflow_dispatch') { throw 'Server acceptance is manual only.' }
if ($env:INSTALLER_RUN_ID -notmatch '^[0-9]+$') { throw 'A numeric installer run ID is required.' }
if ($env:EXPECTED_SHA -notmatch '^[a-f0-9]{40}$' -or $env:EXPECTED_SHA -cne $env:GITHUB_SHA) {
    throw 'Installer source SHA must exactly match the explicitly selected workflow ref.'
}
if ($env:INCLUDE_2019 -eq 'true' -and $env:CONFIRM_2019 -cne 'DISPOSABLE SERVER 2019') {
    throw 'The self-hosted Server 2019 test needs the exact disposable-host confirmation.'
}
if ($env:ARTIFACT_NAME -notmatch '^[A-Za-z0-9_.-]+$') { throw 'Invalid artifact name.' }
$headers = @{ Authorization = "Bearer $env:GH_TOKEN"; Accept = 'application/vnd.github+json'; 'X-GitHub-Api-Version' = '2022-11-28' }
$api = "$env:GITHUB_API_URL/repos/$env:GITHUB_REPOSITORY/actions"
$run = Invoke-RestMethod -Uri "$api/runs/$env:INSTALLER_RUN_ID" -Headers $headers
if ($run.status -ne 'completed' -or $run.conclusion -ne 'success') { throw 'Installer workflow must have completed successfully.' }
if ($run.head_sha -cne $env:EXPECTED_SHA -or $run.head_repository.full_name -cne $env:GITHUB_REPOSITORY) {
    throw 'Installer artifact is from a different commit or repository.'
}
if ($run.event -notin @('push', 'workflow_dispatch', 'release')) { throw 'PR and other untrusted installer triggers are refused.' }
$workflow = Invoke-RestMethod -Uri "$api/workflows/$($run.workflow_id)" -Headers $headers
if ($workflow.path -notin @('.github/workflows/preview-installers.yml', '.github/workflows/release.yml')) {
    throw 'Only the repository installer workflows are accepted.'
}
$artifacts = Invoke-RestMethod -Uri "$api/runs/$env:INSTALLER_RUN_ID/artifacts?per_page=100" -Headers $headers
if ($artifacts.total_count -gt 100) { throw 'Artifact list exceeds the bounded verifier.' }
$found = @($artifacts.artifacts | Where-Object { $_.name -ceq $env:ARTIFACT_NAME -and -not $_.expired })
if ($found.Count -ne 1) { throw 'Exactly one unexpired matching installer artifact is required.' }
if ($found[0].workflow_run.head_sha -cne $env:EXPECTED_SHA) { throw 'Artifact commit differs from the workflow commit.' }
"artifact_id=$($found[0].id)" | Out-File -LiteralPath $env:GITHUB_OUTPUT -Encoding utf8 -Append
Write-Output "Verified installer run $env:INSTALLER_RUN_ID, SHA $env:EXPECTED_SHA, artifact $($found[0].id)."
