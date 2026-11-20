#requires -Version 7.4
<#
.SYNOPSIS
Read-only input preflight and fixture-only acceptance guards. No GUI or clipboard transport.
.DESCRIPTION
Dot-source to use the guards in synthetic tests. Executing the script always performs a
read-only preflight, including when neither PreflightOnly nor ValidateInputs is supplied.
GuiTest is an explicit reserved opt-in and REFUSES execution in this version. It cannot
start an app, inject keys, change preferences or read/write the Windows clipboard.
#>
[CmdletBinding()]
param(
    [string] $AppExecutable,
    [string] $CliExecutable,
    [string] $ExpectedAppSha256,
    [string] $ExpectedCliSha256,
    [string] $FixtureRoot,
    [string] $FixturePdf,
    [string] $ExpectedFixtureSha256,
    [string] $RunDirectory,
    [switch] $SyntheticFixturesOnly,
    [Alias('ValidateInputs')][switch] $PreflightOnly,
    [switch] $GuiTest
)

function Stop-PdfCraftInput([string] $Code) {
    # Codes only: exceptions must not disclose private paths, text or control tokens.
    throw [InvalidOperationException]::new("PDFCRAFT_INPUT:$Code")
}

function ConvertTo-PdfCraftInputPath([string] $Path) {
    if ([string]::IsNullOrWhiteSpace($Path) -or $Path -match '[\x00-\x1f]') { Stop-PdfCraftInput 'PATH_REQUIRED' }
    # Rust canonicalize uses these prefixes. Remove the namespace spelling only,
    # never Normalize(), Trim() or another Unicode/text transformation.
    if ($Path.StartsWith('\\?\UNC\', [StringComparison]::OrdinalIgnoreCase)) {
        $Path = '\\' + $Path.Substring(8)
    } elseif ($Path.StartsWith('\\?\', [StringComparison]::Ordinal)) {
        $Path = $Path.Substring(4)
        if ($Path -notmatch '^[A-Za-z]:[\\/]') { Stop-PdfCraftInput 'DEVICE_PATH' }
    }
    if ($Path.StartsWith('\\.\', [StringComparison]::Ordinal) -or $Path.Contains('::') -or
        $Path -match '[*?]' -or $Path -match '^[A-Za-z]:[^\\/]' -or
        ($IsWindows -and $Path -match '(?<!^[A-Za-z]):')) { Stop-PdfCraftInput 'DEVICE_OR_STREAM_PATH' }
    try { return [IO.Path]::TrimEndingDirectorySeparator([IO.Path]::GetFullPath($Path)) }
    catch { Stop-PdfCraftInput 'INVALID_PATH' }
}

function Test-PdfCraftInputSamePath([string] $Left, [string] $Right) {
    $comparison = if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
    return [string]::Equals((ConvertTo-PdfCraftInputPath $Left), (ConvertTo-PdfCraftInputPath $Right), $comparison)
}

function Assert-PdfCraftInputChild([string] $Root, [string] $Path) {
    $rootPath = ConvertTo-PdfCraftInputPath $Root
    $childPath = ConvertTo-PdfCraftInputPath $Path
    $comparison = if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
    $prefix = if ([IO.Path]::EndsInDirectorySeparator($rootPath)) { $rootPath } else { $rootPath + [IO.Path]::DirectorySeparatorChar }
    if (-not $childPath.StartsWith($prefix, $comparison)) { Stop-PdfCraftInput 'OUTSIDE_FIXTURE_ROOT' }
}

function Assert-PdfCraftInputNoReparse([string] $Path) {
    $current = ConvertTo-PdfCraftInputPath $Path
    while ($current) {
        # Inspect every existing ancestor, including dangling links. Resolve-Path alone
        # would hide the link that escapes a synthetic root.
        $item = Get-Item -LiteralPath $current -Force -ErrorAction SilentlyContinue
        if ($item -and ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { Stop-PdfCraftInput 'REPARSE_PATH' }
        $parent = [IO.Path]::GetDirectoryName($current)
        if ($parent -eq $current) { break }
        $current = $parent
    }
}

function Assert-PdfCraftInputHash([string] $Path, [string] $Expected, [string] $Code) {
    if ($Expected -notmatch '^[0-9a-fA-F]{64}$') { Stop-PdfCraftInput 'HASH_PIN_REQUIRED' }
    Assert-PdfCraftInputNoReparse $Path
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { Stop-PdfCraftInput 'INPUT_FILE_MISSING' }
    if ((Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash -ine $Expected) { Stop-PdfCraftInput $Code }
}

function Get-PdfCraftInputPreflight {
    [CmdletBinding()]
    param(
        [string] $AppExecutable, [string] $CliExecutable,
        [string] $ExpectedAppSha256, [string] $ExpectedCliSha256,
        [string] $FixtureRoot, [string] $FixturePdf, [string] $ExpectedFixtureSha256,
        [string] $RunDirectory, [switch] $SyntheticFixturesOnly
    )
    if (-not $SyntheticFixturesOnly) { Stop-PdfCraftInput 'SYNTHETIC_ATTESTATION_REQUIRED' }
    $rootPath = ConvertTo-PdfCraftInputPath $FixtureRoot
    $fixturePath = ConvertTo-PdfCraftInputPath $FixturePdf
    $runPath = ConvertTo-PdfCraftInputPath $RunDirectory
    $appPath = ConvertTo-PdfCraftInputPath $AppExecutable
    $cliPath = ConvertTo-PdfCraftInputPath $CliExecutable
    if (-not (Test-Path -LiteralPath $rootPath -PathType Container)) { Stop-PdfCraftInput 'FIXTURE_ROOT_MISSING' }
    foreach ($path in $rootPath, $fixturePath, $runPath, $appPath, $cliPath) { Assert-PdfCraftInputNoReparse $path }
    Assert-PdfCraftInputChild $rootPath $fixturePath
    Assert-PdfCraftInputChild $rootPath $runPath
    if (Test-Path -LiteralPath $runPath) { Stop-PdfCraftInput 'RUN_ALREADY_EXISTS' }
    # The entire run (profile/bin/portable.txt, profile/bin/PdfCraftData, output and
    # control file) must be new. Do not reuse an empty directory or an old profile.
    if ((Test-PdfCraftInputSamePath $appPath $cliPath) -or
        $appPath.StartsWith($runPath + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
        $cliPath.StartsWith($runPath + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        Stop-PdfCraftInput 'BINARY_LAYOUT'
    }
    if ([IO.Path]::GetExtension($fixturePath) -ine '.pdf') { Stop-PdfCraftInput 'PDF_FIXTURE_REQUIRED' }
    Assert-PdfCraftInputHash $appPath $ExpectedAppSha256 'APP_HASH_MISMATCH'
    Assert-PdfCraftInputHash $cliPath $ExpectedCliSha256 'CLI_HASH_MISMATCH'
    Assert-PdfCraftInputHash $fixturePath $ExpectedFixtureSha256 'FIXTURE_HASH_MISMATCH'
    # Hash pins select the coordinator's build; hashes alone do not prove source freshness,
    # executable validity, PDF safety or native acceptance. No binary or PDF is opened.
    return [pscustomobject]@{
        status = 'VALIDATED_INPUTS'; scope = 'read_only_preflight'; exit_code = 0
        app_sha256 = $ExpectedAppSha256.ToLowerInvariant(); cli_sha256 = $ExpectedCliSha256.ToLowerInvariant()
        fixture_sha256 = $ExpectedFixtureSha256.ToLowerInvariant()
        fresh_run_required = $true; portable_profile_created = $false
        gui_started = $false; clipboard_read = $false; clipboard_written = $false
        native_input = 'NOT VERIFIED'; physical_q = 'NOT VERIFIED'; physical_f = 'NOT VERIFIED'
        os_clipboard = 'NOT VERIFIED'; clean_windows = 'NOT VERIFIED'
    }
}

function Get-PdfCraftInputClipboardDecision {
    param([object[]] $Metadata = @())
    # Metadata is caller-supplied format/type/count information only. Do not accept
    # booleans claiming a lossless backup: delayed/opaque/custom data needs a real,
    # separately reviewed round-trip provider. This runner has no such provider.
    $known = @('UnicodeText', 'Text', 'System.String')
    $unknown = @($Metadata | Where-Object { $_.format -cnotin $known -or $_.type -cne 'System.String' })
    return [pscustomobject]@{
        status = 'REFUSE'; operation = 'copy_cut_paste'; format_count = $Metadata.Count
        opaque_or_unknown_count = $unknown.Count; content_read = $false; clipboard_written = $false
        reason = 'NO_VERIFIED_LOSSLESS_BACKUP_RESTORE_PROVIDER'
    }
}

function Assert-PdfCraftInputForeground([object] $Observation, [int] $OwnedProcessId) {
    if ($OwnedProcessId -le 0 -or $Observation.process_id -isnot [int] -or
        $Observation.process_id -ne $OwnedProcessId -or $Observation.foreground -isnot [bool] -or
        -not $Observation.foreground) { Stop-PdfCraftInput 'FOREGROUND_REQUIRED' }
    # A mock observation proves only this guard. A future GUI transport must observe
    # the actual foreground process before every mutation; a ready-file is insufficient.
}

function Assert-PdfCraftInputNormalExit([object] $Observation, [int] $OwnedProcessId) {
    if ($OwnedProcessId -le 0 -or $Observation.process_id -ne $OwnedProcessId -or
        $Observation.has_exited -isnot [bool] -or -not $Observation.has_exited -or
        $Observation.exit_code -isnot [int] -or $Observation.exit_code -ne 0) {
        Stop-PdfCraftInput 'NORMAL_EXIT_REQUIRED'
    }
}

function Test-PdfCraftInputDocument {
    param([object] $State, [string] $ExpectedPath, [int] $ExpectedPage = 1, [bool] $Dirty = $false)
    if ($ExpectedPage -le 0) { Stop-PdfCraftInput 'EXPECTED_PAGE_INVALID' }
    $documents = @($State.documents)
    if ($documents.Count -gt 1) { Stop-PdfCraftInput 'UNEXPECTED_DOCUMENT' }
    foreach ($document in $documents) {
        if (-not (Test-PdfCraftInputSamePath $document.path $ExpectedPath)) { Stop-PdfCraftInput 'UNEXPECTED_ACTIVE_PATH' }
        if ($document.active -isnot [bool] -or -not $document.active) { Stop-PdfCraftInput 'UNEXPECTED_DOCUMENT' }
    }
    foreach ($pending in @($State.pending_opens)) {
        if (-not (Test-PdfCraftInputSamePath $pending.path $ExpectedPath)) { Stop-PdfCraftInput 'UNEXPECTED_PENDING_PATH' }
    }
    if ($documents.Count -eq 0 -or @($State.pending_opens).Count -ne 0) { return $false }
    $active = $State.active
    # Page coordinates must be from this document, not a stale snapshot for another tab.
    if ($active.doc -ne $documents[0].doc -or $documents[0].doc -le 0) { Stop-PdfCraftInput 'UNEXPECTED_DOCUMENT_ID' }
    if ($documents[0].pages -ne 1) { Stop-PdfCraftInput 'UNEXPECTED_PAGE_COUNT' }
    if ($active.page -ne $ExpectedPage -or @($active.pages_on_screen | Where-Object page -eq $ExpectedPage).Count -ne 1) {
        Stop-PdfCraftInput 'UNEXPECTED_PAGE'
    }
    if ($documents[0].dirty -isnot [bool]) { Stop-PdfCraftInput 'DIRTY_STATE_INVALID' }
    return $documents[0].dirty -eq $Dirty
}

function Wait-PdfCraftInputDocument {
    param(
        [scriptblock] $ReadState, [string] $ExpectedPath, [int] $ExpectedPage = 1,
        [bool] $Dirty = $false, [ValidateRange(1, 200)][int] $Attempts = 80,
        [ValidateRange(0, 250)][int] $DelayMilliseconds = 50
    )
    for ($attempt = 0; $attempt -lt $Attempts; $attempt++) {
        $state = & $ReadState
        if (Test-PdfCraftInputDocument $state $ExpectedPath $ExpectedPage $Dirty) { return $state }
        if ($DelayMilliseconds -gt 0) { Start-Sleep -Milliseconds $DelayMilliseconds }
    }
    Stop-PdfCraftInput 'DOCUMENT_STATE_TIMEOUT'
}

function Assert-PdfCraftInputEditor {
    param([object[]] $Widgets, [string] $EditorId, [AllowEmptyString()][string] $ExpectedValue, [switch] $AllowUnfocused)
    if ([string]::IsNullOrEmpty($EditorId)) { Stop-PdfCraftInput 'EDITOR_ID_REQUIRED' }
    $editors = @($Widgets | Where-Object { $_.role -cin @('TextInput', 'MultilineTextInput') })
    $editor = @($editors | Where-Object { $_.id -ceq $EditorId })
    if ($editor.Count -ne 1 -or @($editors | Where-Object focused -eq $true).Count -gt 1 -or
        $editor[0].enabled -isnot [bool] -or -not $editor[0].enabled -or
        (-not $AllowUnfocused -and ($editor[0].focused -isnot [bool] -or -not $editor[0].focused)) -or
        $editor[0].value -isnot [string] -or
        -not [string]::Equals($editor[0].value, $ExpectedValue, [StringComparison]::Ordinal)) {
        Stop-PdfCraftInput 'EDITOR_IDENTITY_OR_VALUE'
    }
}

function Get-PdfCraftInputScalarCount([AllowEmptyString()][string] $Text) {
    $count = 0
    for ($i = 0; $i -lt $Text.Length; $i++) {
        if ([char]::IsHighSurrogate($Text[$i])) {
            if ($i + 1 -ge $Text.Length -or -not [char]::IsLowSurrogate($Text[$i + 1])) { Stop-PdfCraftInput 'INVALID_UNICODE' }
            $i++
        } elseif ([char]::IsLowSurrogate($Text[$i])) { Stop-PdfCraftInput 'INVALID_UNICODE' }
        $count++
    }
    return $count
}

function Assert-PdfCraftInputRejectedDraft {
    param(
        [object] $State, [string] $ExpectedPath, [object[]] $Widgets, [string] $EditorId,
        [string] $FullDraft, [int] $ScalarLimit, [object] $ApplyButton,
        [AllowEmptyString()][string] $SavedBefore, [AllowEmptyString()][string] $SavedAfter
    )
    if ($ScalarLimit -lt 1 -or (Get-PdfCraftInputScalarCount $FullDraft) -le $ScalarLimit) { Stop-PdfCraftInput 'OVER_LIMIT_FIXTURE_REQUIRED' }
    if (-not (Test-PdfCraftInputDocument $State $ExpectedPath 1 $false)) { Stop-PdfCraftInput 'REJECTED_DRAFT_CHANGED_DOCUMENT' }
    Assert-PdfCraftInputEditor $Widgets $EditorId $FullDraft -AllowUnfocused
    if ($ApplyButton.role -cne 'Button' -or $ApplyButton.label -cne 'Apply' -or
        $ApplyButton.enabled -isnot [bool] -or $ApplyButton.enabled -or
        -not [string]::Equals($SavedBefore, $SavedAfter, [StringComparison]::Ordinal)) {
        Stop-PdfCraftInput 'REJECTED_DRAFT_NOT_ATOMIC'
    }
}

function Test-PdfCraftInputMockLifecycle {
    param(
        [object[]] $Checkpoints, [string] $ExpectedPath, [string] $EditorId,
        [string] $ExpectedText,
        [ValidateSet('EventText', 'Paste', 'IME')][string] $Channel = 'EventText'
    )
    # Offline observations only. There is deliberately no executable adapter here:
    # mock success cannot inject input or become native/physical/OS clipboard PASS.
    $phases = @('opened', 'draft', 'committed', 'undone', 'redone', 'saved', 'reopened')
    $dirty = @($false, $false, $true, $false, $true, $false, $false)
    if ($Checkpoints.Count -ne $phases.Count) { Stop-PdfCraftInput 'LIFECYCLE_INCOMPLETE' }
    for ($i = 0; $i -lt $phases.Count; $i++) {
        $checkpoint = $Checkpoints[$i]
        if ($checkpoint.phase -cne $phases[$i]) { Stop-PdfCraftInput 'LIFECYCLE_ORDER' }
        Assert-PdfCraftInputForeground $checkpoint.observation 12345
        if (-not (Test-PdfCraftInputDocument $checkpoint.state $ExpectedPath 1 $dirty[$i])) { Stop-PdfCraftInput 'LIFECYCLE_STATE' }
        if ($phases[$i] -cin @('draft', 'reopened')) {
            Assert-PdfCraftInputEditor $checkpoint.widgets $EditorId $ExpectedText
        }
    }
    return [pscustomobject]@{
        status = 'VALIDATED_MOCK_TRACE'; scope = 'fixture_only'; channel = $Channel
        checkpoints = $phases.Count; native_input = 'NOT VERIFIED'
        physical_q = 'NOT VERIFIED'; physical_f = 'NOT VERIFIED'; os_clipboard = 'NOT VERIFIED'
    }
}

if ($MyInvocation.InvocationName -ne '.') {
    $ErrorActionPreference = 'Stop'
    Set-StrictMode -Version Latest
    if ($GuiTest) {
        [pscustomobject]@{ status = 'REFUSE'; exit_code = 3; reason = 'LIVE_GUI_TRANSPORT_NOT_ENABLED'; clipboard_written = $false } | ConvertTo-Json -Compress
        exit 3
    }
    try {
        $parameters = @{}
        foreach ($name in 'AppExecutable', 'CliExecutable', 'ExpectedAppSha256', 'ExpectedCliSha256',
            'FixtureRoot', 'FixturePdf', 'ExpectedFixtureSha256', 'RunDirectory', 'SyntheticFixturesOnly') {
            $parameters[$name] = Get-Variable -Name $name -ValueOnly
        }
        Get-PdfCraftInputPreflight @parameters | ConvertTo-Json -Compress
        exit 0
    } catch {
        $code = if ($_.Exception.Message -match '^PDFCRAFT_INPUT:([A-Z_]+)$') { $Matches[1] } else { 'VALIDATION_FAILED' }
        [pscustomobject]@{ status = 'INVALID_INPUTS'; exit_code = 2; reason = $code; clipboard_written = $false } | ConvertTo-Json -Compress
        exit 2
    }
}
