# Safe local checks only. Never run the installer acceptance body here.
param([string] $Cli = '')

$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/common.ps1"
foreach ($file in Get-ChildItem -LiteralPath $PSScriptRoot -Filter '*.ps1') {
    $tokens = $null; $errors = $null
    [void] [Management.Automation.Language.Parser]::ParseFile($file.FullName, [ref] $tokens, [ref] $errors)
    Assert-That (-not $errors) ("PowerShell syntax: " + ($errors -join '; '))
}
$boundary = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
[void] (Assert-SafePath (Join-Path $boundary 'packaging/server-tests') $boundary)
$rejected = $false
try { [void] (Assert-SafePath ($boundary + '-sibling/file') $boundary) } catch { $rejected = $true }
Assert-That $rejected 'Sibling-prefix confinement must reject paths outside the test root.'
$separator = [string] [IO.Path]::DirectorySeparatorChar
[void] (Assert-SafePath $boundary ($boundary + $separator))
[void] (Assert-SafePath (Join-Path $boundary 'packaging/server-tests') ($boundary + $separator))
$filesystemRoot = [IO.Path]::GetPathRoot($boundary)
[void] (Assert-SafePath $filesystemRoot $filesystemRoot)
$rejected = $false
try { [void] (Assert-SafePath (Join-Path $boundary '../outside-test-root') $boundary) } catch { $rejected = $true }
Assert-That $rejected 'Parent traversal must reject paths outside the test root.'
if (-not $IsWindows) {
    $caseRoot = Join-Path $boundary 'case-sensitive-root'
    foreach ($outside in @((Join-Path $boundary 'CASE-SENSITIVE-ROOT'), ($caseRoot + '\sibling/file'))) {
        $rejected = $false
        try { [void] (Assert-SafePath $outside $caseRoot) } catch { $rejected = $true }
        Assert-That $rejected 'Unix confinement must reject different-case and backslash sibling paths.'
    }
}

$guid = '{A9C59D9B-69EB-4AB0-BEA5-419C001B785E}'
Assert-ProductCode $guid
foreach ($invalid in @('', "  $guid", @($null, $null, $guid), "$guid extra", $guid.Trim('{}'))) {
    $rejected = $false
    try { Assert-ProductCode $invalid } catch { $rejected = $true }
    Assert-That $rejected 'Cleanup must reject a malformed or contaminated ProductCode.'
}
Assert-CleanupSucceeded @() $false
try {
    try { throw 'original acceptance failure' }
    finally { Assert-CleanupSucceeded @('secondary cleanup failure') $true 3>$null }
} catch { Assert-That ($_.Exception.Message -ceq 'original acceptance failure') 'Cleanup masked the original failure.' }
$rejected = $false
try { Assert-CleanupSucceeded @('cleanup-only failure') $false } catch {
    $rejected = $_.Exception.Message -ceq 'Acceptance cleanup failed: cleanup-only failure'
}
Assert-That $rejected 'Cleanup-only failure must still fail acceptance.'
if ($IsWindows) { & "$PSScriptRoot/check-msi-database.ps1" }

# The CLI expects an array even for zero or one step. Passing values through the
# PowerShell pipeline silently unwraps a one-element array before serialization.
$script:TestRoot = Join-Path ([IO.Path]::GetTempPath()) ('pdfcraft-server-harness-' + [guid]::NewGuid().ToString('N'))
$script:Results = Join-Path $script:TestRoot 'results'
[void] (New-Item -ItemType Directory -Path $script:Results)
try {
    if ($Cli) {
        $script:Cli = (Get-Item -LiteralPath $Cli -ErrorAction Stop).FullName
        Assert-That ([IO.File]::Exists($script:Cli)) 'Optional CLI must be an existing executable file.'
    }
    $cases = @(
        @{ name = 'empty'; steps = [object[]] @(); count = 0 },
        @{ name = 'single'; steps = [object[]] @(@{ tool = 'ocr_status'; args = @{} }); count = 1 },
        @{ name = 'multiple'; steps = [object[]] @(@{ tool = 'ocr_status'; args = @{} }, @{ tool = 'ocr_status'; args = @{} }); count = 2 }
    )
    foreach ($case in $cases) {
        $path = Join-Path $script:TestRoot ($case.name + '.json')
        Write-Json $path $case.steps
        $json = [IO.File]::ReadAllText($path)
        Assert-That ($json.TrimStart().StartsWith('[')) "$($case.name) CLI script must remain a JSON array."
        $parsed = ConvertFrom-Json -InputObject $json -NoEnumerate
        Assert-That ($parsed.Count -eq $case.count) "$($case.name) CLI script lost steps."
        foreach ($step in $parsed) {
            Assert-That ($step.tool -eq 'ocr_status' -and $null -ne $step.args) 'CLI script changed tool arguments.'
        }
        if ($Cli) {
            # Calls only the read-only status tool: no installer, OCR, task worker
            # or operating-system service is started, even if models are absent.
            $answer = Invoke-CliScript ('cli-' + $case.name) $case.steps
            $answerCount = [regex]::Matches([string] $answer, '"available"\s*:').Count
            Assert-That ($answerCount -eq $case.count) "$($case.name) real CLI did not execute every status call."
        }
    }
    # Reports are objects, so preserving script arrays must not wrap all values.
    $report = Join-Path $script:TestRoot 'report.json'
    Write-Json $report @{ result = 'passed'; tests = @{} }
    Assert-That ([IO.File]::ReadAllText($report).TrimStart().StartsWith('{')) 'Reports must remain JSON objects.'

    # A successful output or stopped process is insufficient if an original was
    # removed or modified. Both failures must reject the whole acceptance result.
    $source = Join-Path $script:TestRoot 'original.bin'
    [IO.File]::WriteAllText($source, 'original synthetic bytes')
    $sourceHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
    Assert-SourceUnchanged $source $sourceHash 'Unchanged fixture'
    [IO.File]::WriteAllText($source, 'changed synthetic bytes')
    $rejected = $false
    try { Assert-SourceUnchanged $source $sourceHash 'Changed fixture' } catch { $rejected = $true }
    Assert-That $rejected 'Changed source bytes must fail acceptance.'
    Remove-Item -LiteralPath $source
    $rejected = $false
    try { Assert-SourceUnchanged $source $sourceHash 'Missing fixture' } catch { $rejected = $true }
    Assert-That $rejected 'A missing source must fail acceptance.'
} finally {
    [void] (Assert-SafePath $script:TestRoot ([IO.Path]::GetTempPath()))
    Remove-Item -LiteralPath $script:TestRoot -Recurse -Force
}

$keys = @('GITHUB_EVENT_NAME', 'INSTALLER_RUN_ID', 'EXPECTED_SHA', 'GITHUB_SHA', 'INCLUDE_2019',
    'CONFIRM_2019', 'ARTIFACT_NAME', 'GITHUB_API_URL', 'GITHUB_REPOSITORY', 'GITHUB_OUTPUT', 'GH_TOKEN')
$saved = @{}
foreach ($key in $keys) { $saved[$key] = [Environment]::GetEnvironmentVariable($key) }
$output = [IO.Path]::GetTempFileName()
$previousCase = Get-Variable -Name PdfcraftServerProvenanceCase -Scope Global -ErrorAction SilentlyContinue
try {
    $env:GITHUB_EVENT_NAME = 'workflow_dispatch'; $env:INSTALLER_RUN_ID = '123'
    $env:EXPECTED_SHA = 'a' * 40; $env:GITHUB_SHA = $env:EXPECTED_SHA
    $env:INCLUDE_2019 = 'false'; $env:CONFIRM_2019 = ''
    $env:ARTIFACT_NAME = 'pdfcraft-preview-windows-x64'
    $env:GITHUB_API_URL = 'https://api.invalid'; $env:GITHUB_REPOSITORY = 'test/pdfcraft'
    $env:GITHUB_OUTPUT = $output; $env:GH_TOKEN = 'synthetic-not-a-token'
    $global:PdfcraftServerProvenanceCase = 'good'
    function Invoke-RestMethod($Uri, $Headers) {
        if ($Uri -like '*/artifacts?*') {
            $sha = if ($global:PdfcraftServerProvenanceCase -eq 'artifact-sha') { 'b' * 40 } else { $env:EXPECTED_SHA }
            return @{ total_count = 1; artifacts = @(@{ name = $env:ARTIFACT_NAME; expired = $false; id = 42; workflow_run = @{ head_sha = $sha } }) }
        }
        if ($Uri -like '*/workflows/*') {
            return @{ path = if ($global:PdfcraftServerProvenanceCase -eq 'workflow') { '.github/workflows/other.yml' } else { '.github/workflows/preview-installers.yml' } }
        }
        return @{
            status = 'completed'; conclusion = if ($global:PdfcraftServerProvenanceCase -eq 'failed') { 'failure' } else { 'success' }
            head_sha = $env:EXPECTED_SHA; workflow_id = 7
            head_repository = @{ full_name = if ($global:PdfcraftServerProvenanceCase -eq 'fork') { 'other/pdfcraft' } else { $env:GITHUB_REPOSITORY } }
            event = if ($global:PdfcraftServerProvenanceCase -eq 'pull-request') { 'pull_request' } else { 'workflow_dispatch' }
        }
    }
    & "$PSScriptRoot/verify-provenance.ps1" | Out-Null
    Assert-That ((Get-Content -LiteralPath $output -Raw) -match 'artifact_id=42') 'Matching trusted artifact should pass.'
    foreach ($invalid in @('failed', 'fork', 'workflow', 'pull-request', 'artifact-sha')) {
        $global:PdfcraftServerProvenanceCase = $invalid; $rejected = $false
        try { & "$PSScriptRoot/verify-provenance.ps1" | Out-Null } catch { $rejected = $true }
        Assert-That $rejected "Unsafe artifact accepted: $invalid"
    }
    $global:PdfcraftServerProvenanceCase = 'good'; $env:INCLUDE_2019 = 'true'; $rejected = $false
    try { & "$PSScriptRoot/verify-provenance.ps1" | Out-Null } catch { $rejected = $true }
    Assert-That $rejected 'Unconfirmed Server 2019 dispatch must fail.'
    $env:INCLUDE_2019 = 'false'; $env:GITHUB_EVENT_NAME = 'pull_request'; $rejected = $false
    try { & "$PSScriptRoot/verify-provenance.ps1" | Out-Null } catch { $rejected = $true }
    Assert-That $rejected 'Automatic parent event must fail.'
} finally {
    Remove-Item -LiteralPath $output -Force
    foreach ($key in $keys) { [Environment]::SetEnvironmentVariable($key, $saved[$key]) }
    Remove-Item Function:Invoke-RestMethod
    if ($previousCase) { $global:PdfcraftServerProvenanceCase = $previousCase.Value }
    else { Remove-Variable -Name PdfcraftServerProvenanceCase -Scope Global }
}
Write-Output 'ok: parser, confinement, strict MSI GUIDs, cleanup error preservation, JSON cardinality, source preservation and eight provenance scenarios; no service or installer executed'
if ($Cli) { Write-Output 'ok: real CLI executed empty, single and multiple read-only status scripts' }
