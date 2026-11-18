#requires -Version 7.4
[CmdletBinding()]
param(
    [string] $RunnerPath = (Join-Path $PSScriptRoot 'test-turkish-input.ps1'),
    [string] $ReportPath
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. $RunnerPath

$script:results = [Collections.Generic.List[object]]::new()
function Check([string] $Name, [scriptblock] $Action) {
    try {
        & $Action
        $script:results.Add([pscustomobject]@{ name = $Name; status = 'PASS' })
    } catch {
        # Do not print arbitrary exception text, fixture text or private absolute paths.
        $script:results.Add([pscustomobject]@{ name = $Name; status = 'FAIL' })
    }
}
function Assert([bool] $Condition) { if (-not $Condition) { throw 'Assertion failed' } }
function Reject([scriptblock] $Action, [string] $Code) {
    $rejected = $false
    try { & $Action | Out-Null } catch { $rejected = $_.Exception.Message -ceq "PDFCRAFT_INPUT:$Code" }
    Assert $rejected
}
function State([string] $Path, [bool] $Dirty = $false) {
    return [pscustomobject]@{
        documents = @([pscustomobject]@{ active = $true; path = $Path; dirty = $Dirty; doc = 1; pages = 1 })
        pending_opens = @()
        active = [pscustomobject]@{ doc = 1; page = 1; pages_on_screen = @([pscustomobject]@{ page = 1 }) }
    }
}
function Editor([string] $Value, [string] $Id = 'synthetic-editor') {
    return [pscustomobject]@{ id = $Id; role = 'TextInput'; enabled = $true; focused = $true; value = $Value }
}
function Trace([string] $Path, [string] $Text) {
    $phases = @('opened', 'draft', 'committed', 'undone', 'redone', 'saved', 'reopened')
    $dirty = @($false, $false, $true, $false, $true, $false, $false)
    return @(for ($i = 0; $i -lt $phases.Count; $i++) {
        [pscustomobject]@{
            phase = $phases[$i]; state = (State $Path $dirty[$i])
            widgets = @((Editor $Text)); observation = [pscustomobject]@{ process_id = 12345; foreground = $true }
        }
    })
}
function Run-Entry([string[]] $ModeArguments, [hashtable] $Configuration) {
    $arguments = @('-NoProfile', '-File', $RunnerPath) + $ModeArguments
    foreach ($key in $Configuration.Keys) {
        if ($key -eq 'SyntheticFixturesOnly') { $arguments += '-SyntheticFixturesOnly' }
        else { $arguments += @("-$key", [string]$Configuration[$key]) }
    }
    $raw = & (Join-Path $PSHOME 'pwsh.exe') @arguments
    $code = $LASTEXITCODE
    return [pscustomobject]@{ code = $code; raw = ($raw -join "`n"); value = (($raw -join "`n") | ConvertFrom-Json) }
}

$temporaryBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$temporary = Join-Path $temporaryBase ('pdfcraft-input-fixture-' + [guid]::NewGuid().ToString('N'))
$link = Join-Path $temporary 'fixtures/link'
$linksCreated = $false
try {
    New-Item -ItemType Directory -Path (Join-Path $temporary 'bin'), (Join-Path $temporary 'fixtures'), (Join-Path $temporary 'outside') | Out-Null
    $app = Join-Path $temporary 'bin/app.exe'
    $cli = Join-Path $temporary 'bin/cli.exe'
    $fixture = Join-Path $temporary 'fixtures/İş - Şubat.pdf'
    [IO.File]::WriteAllText($app, 'Non-executable synthetic app bytes; never run.')
    [IO.File]::WriteAllText($cli, 'Non-executable synthetic CLI bytes; never run.')
    [IO.File]::WriteAllText($fixture, "%PDF-1.7`n% Synthetic preflight bytes; not opened by a PDF reader.`n")
    $configuration = @{
        AppExecutable = $app; CliExecutable = $cli
        ExpectedAppSha256 = (Get-FileHash -LiteralPath $app).Hash
        ExpectedCliSha256 = (Get-FileHash -LiteralPath $cli).Hash
        FixtureRoot = (Join-Path $temporary 'fixtures'); FixturePdf = $fixture
        ExpectedFixtureSha256 = (Get-FileHash -LiteralPath $fixture).Hash
        RunDirectory = (Join-Path $temporary 'fixtures/fresh-run'); SyntheticFixturesOnly = $true
    }
    $nfd = "C$([char]0x327)ag$([char]0x306)rı I$([char]0x307)pek S$([char]0x327)ahin"
    $fullDraft = ('A' * 255) + "g$([char]0x306)"
    $button = [pscustomobject]@{ role = 'Button'; label = 'Apply'; enabled = $false }
    $initialHashes = @($app, $cli, $fixture | ForEach-Object { (Get-FileHash -LiteralPath $_).Hash })

    Check 'read-only preflight validates exact pinned inputs' {
        $r = Get-PdfCraftInputPreflight @configuration
        Assert ($r.status -ceq 'VALIDATED_INPUTS' -and -not $r.gui_started -and -not $r.clipboard_read -and -not $r.clipboard_written)
        Assert (-not (Test-Path -LiteralPath $configuration.RunDirectory))
    }
    foreach ($case in @(
        @{ key = 'ExpectedAppSha256'; value = ('0' * 64); code = 'APP_HASH_MISMATCH' },
        @{ key = 'ExpectedCliSha256'; value = ('0' * 64); code = 'CLI_HASH_MISMATCH' },
        @{ key = 'ExpectedFixtureSha256'; value = ('0' * 64); code = 'FIXTURE_HASH_MISMATCH' },
        @{ key = 'ExpectedAppSha256'; value = ''; code = 'HASH_PIN_REQUIRED' },
        @{ key = 'SyntheticFixturesOnly'; value = $false; code = 'SYNTHETIC_ATTESTATION_REQUIRED' },
        @{ key = 'FixturePdf'; value = (Join-Path $temporary 'outside/other.pdf'); code = 'OUTSIDE_FIXTURE_ROOT' },
        @{ key = 'RunDirectory'; value = (Join-Path $temporary 'fixtures-sibling/run'); code = 'OUTSIDE_FIXTURE_ROOT' },
        @{ key = 'RunDirectory'; value = $configuration.FixtureRoot; code = 'OUTSIDE_FIXTURE_ROOT' }
    )) {
        Check ("rejects $($case.code) for $($case.key)") {
            $bad = $configuration.Clone(); $bad[$case.key] = $case.value
            Reject { Get-PdfCraftInputPreflight @bad } $case.code
        }
    }
    Check 'existing output refuses reuse' {
        New-Item -ItemType Directory -Path $configuration.RunDirectory | Out-Null
        Reject { Get-PdfCraftInputPreflight @configuration } 'RUN_ALREADY_EXISTS'
        Remove-Item -LiteralPath $configuration.RunDirectory
    }
    Check 'existing isolated profile refuses reuse without reading preferences' {
        $profile = Join-Path $configuration.RunDirectory 'profile/bin/PdfCraftData'
        New-Item -ItemType Directory -Path $profile -Force | Out-Null
        [IO.File]::WriteAllText((Join-Path $profile 'app.ron'), 'Synthetic private placeholder')
        Reject { Get-PdfCraftInputPreflight @configuration } 'RUN_ALREADY_EXISTS'
    }
    # Use another new run name; keep the refused profile intact until final cleanup.
    $configuration.RunDirectory = Join-Path $temporary 'fixtures/another-fresh-run'
    Check 'junction fixture and future output cannot escape root' {
        $null = New-Item -ItemType Junction -Path $link -Target (Join-Path $temporary 'outside')
        $script:linksCreated = $true
        $bad = $configuration.Clone(); $bad.RunDirectory = Join-Path $link 'run'
        Reject { Get-PdfCraftInputPreflight @bad } 'REPARSE_PATH'
        $bad = $configuration.Clone(); $bad.FixturePdf = Join-Path $link 'fixture.pdf'
        Reject { Get-PdfCraftInputPreflight @bad } 'REPARSE_PATH'
    }
    Check 'dot-dot cannot escape fixture root' {
        Reject { Assert-PdfCraftInputChild $configuration.FixtureRoot (Join-Path $configuration.FixtureRoot '../outside/new.pdf') } 'OUTSIDE_FIXTURE_ROOT'
    }
    Check 'Windows canonical local prefix preserves exact NFD spelling' {
        $p = Join-Path $temporary ($nfd + '.pdf')
        Assert (Test-PdfCraftInputSamePath ('\\?\' + $p) $p)
        Assert (-not (Test-PdfCraftInputSamePath $p ($p.Normalize([Text.NormalizationForm]::FormC))))
        Assert ([string]::Equals((ConvertTo-PdfCraftInputPath ('\\?\' + $p)), $p, [StringComparison]::Ordinal))
    }
    Check 'Windows canonical UNC prefix preserves Unicode' {
        Assert (Test-PdfCraftInputSamePath ('\\?\UNC\server\share\' + $nfd + '.pdf') ('\\server\share\' + $nfd + '.pdf'))
    }
    Check 'device namespace and alternate stream refuse' {
        Reject { ConvertTo-PdfCraftInputPath '\\?\GLOBALROOT\Device\HarddiskVolume1\file.pdf' } 'DEVICE_PATH'
        Reject { ConvertTo-PdfCraftInputPath ($fixture + ':stream') } 'DEVICE_OR_STREAM_PATH'
    }
    Check 'exact document and visible page are accepted' { Assert (Test-PdfCraftInputDocument (State $fixture) $fixture) }
    Check 'wrong active path aborts immediately' { Reject { Test-PdfCraftInputDocument (State ($fixture + '.other')) $fixture } 'UNEXPECTED_ACTIVE_PATH' }
    Check 'wrong page refuses' {
        $s = State $fixture; $s.active.page = 2
        Reject { Test-PdfCraftInputDocument $s $fixture } 'UNEXPECTED_PAGE'
    }
    Check 'page state must belong to the exact document identity' {
        $s = State $fixture; $s.active.doc = 2
        Reject { Test-PdfCraftInputDocument $s $fixture } 'UNEXPECTED_DOCUMENT_ID'
    }
    Check 'single-page fixture cannot silently become a different page count' {
        $s = State $fixture; $s.documents[0].pages = 2
        Reject { Test-PdfCraftInputDocument $s $fixture } 'UNEXPECTED_PAGE_COUNT'
    }
    Check 'invisible target page refuses' {
        $s = State $fixture; $s.active.pages_on_screen = @()
        Reject { Test-PdfCraftInputDocument $s $fixture } 'UNEXPECTED_PAGE'
    }
    Check 'mismatched dirty state cannot satisfy save or undo' { Assert (-not (Test-PdfCraftInputDocument (State $fixture $true) $fixture 1 $false)) }
    Check 'string dirty false is not a boolean' {
        $s = State $fixture; $s.documents[0].dirty = 'false'
        Reject { Test-PdfCraftInputDocument $s $fixture } 'DIRTY_STATE_INVALID'
    }
    Check 'asynchronous open waits for pending work and exact clean state' {
        $script:reads = 0
        $script:states = @((State $fixture), (State $fixture $true), (State $fixture))
        $script:states[0].pending_opens = @([pscustomobject]@{ path = $fixture })
        $r = Wait-PdfCraftInputDocument -ExpectedPath $fixture -Attempts 3 -DelayMilliseconds 0 -ReadState {
            $s = $script:states[$script:reads]; $script:reads++; return $s
        }
        Assert ($script:reads -eq 3 -and -not $r.documents[0].dirty)
    }
    Check 'unexpected pending open aborts before another poll' {
        $script:reads = 0
        Reject { Wait-PdfCraftInputDocument -ExpectedPath $fixture -Attempts 3 -DelayMilliseconds 0 -ReadState {
            $script:reads++; $s = State $fixture; $s.pending_opens = @([pscustomobject]@{ path = ($fixture + '.other') }); return $s
        } } 'UNEXPECTED_PENDING_PATH'
        Assert ($script:reads -eq 1)
    }
    Check 'save timeout cannot be reported successful' {
        Reject { Wait-PdfCraftInputDocument -ExpectedPath $fixture -Attempts 2 -DelayMilliseconds 0 -ReadState { State $fixture $true } } 'DOCUMENT_STATE_TIMEOUT'
    }
    Check 'exact editor NFD value is accepted' { Assert-PdfCraftInputEditor @((Editor $nfd)) 'synthetic-editor' $nfd }
    Check 'different editor with same value refuses' { Reject { Assert-PdfCraftInputEditor @((Editor $nfd 'other')) 'synthetic-editor' $nfd } 'EDITOR_IDENTITY_OR_VALUE' }
    Check 'normalized NFC editor value refuses' { Reject { Assert-PdfCraftInputEditor @((Editor ($nfd.Normalize()))) 'synthetic-editor' $nfd } 'EDITOR_IDENTITY_OR_VALUE' }
    Check 'CRLF is not silently normalized to LF' { Reject { Assert-PdfCraftInputEditor @((Editor "one`ntwo")) 'synthetic-editor' "one`r`ntwo" } 'EDITOR_IDENTITY_OR_VALUE' }
    Check 'unfocused editor refuses mutation' {
        $e = Editor $nfd; $e.focused = $false
        Reject { Assert-PdfCraftInputEditor @($e) 'synthetic-editor' $nfd } 'EDITOR_IDENTITY_OR_VALUE'
    }
    Check 'duplicate editor identity refuses' { Reject { Assert-PdfCraftInputEditor @((Editor $nfd), (Editor $nfd)) 'synthetic-editor' $nfd } 'EDITOR_IDENTITY_OR_VALUE' }
    Check 'foreground ownership is exact' {
        Assert-PdfCraftInputForeground ([pscustomobject]@{ process_id = 12345; foreground = $true }) 12345
        Reject { Assert-PdfCraftInputForeground ([pscustomobject]@{ process_id = 12346; foreground = $true }) 12345 } 'FOREGROUND_REQUIRED'
        Reject { Assert-PdfCraftInputForeground ([pscustomobject]@{ process_id = 12345; foreground = $false }) 12345 } 'FOREGROUND_REQUIRED'
    }
    Check 'normal exit requires owned process, completed exit and exact zero code' {
        Assert-PdfCraftInputNormalExit ([pscustomobject]@{ process_id = 12345; has_exited = $true; exit_code = 0 }) 12345
        Reject { Assert-PdfCraftInputNormalExit ([pscustomobject]@{ process_id = 12345; has_exited = $false; exit_code = 0 }) 12345 } 'NORMAL_EXIT_REQUIRED'
        Reject { Assert-PdfCraftInputNormalExit ([pscustomobject]@{ process_id = 12345; has_exited = $true; exit_code = 1 }) 12345 } 'NORMAL_EXIT_REQUIRED'
        Reject { Assert-PdfCraftInputNormalExit ([pscustomobject]@{ process_id = 12346; has_exited = $true; exit_code = 0 }) 12345 } 'NORMAL_EXIT_REQUIRED'
    }
    Check 'scalar count handles NFD and supplementary scalars' {
        Assert ((Get-PdfCraftInputScalarCount $fullDraft) -eq 257)
        Assert ((Get-PdfCraftInputScalarCount ([char]::ConvertFromUtf32(0x1f600))) -eq 1)
        Reject { Get-PdfCraftInputScalarCount ([string][char]0xd800) } 'INVALID_UNICODE'
    }
    Check 'full over-limit NFD draft survives refused Enter with saved value intact' {
        Assert-PdfCraftInputRejectedDraft (State $fixture) $fixture @((Editor $fullDraft)) 'synthetic-editor' $fullDraft 256 $button $nfd $nfd
    }
    Check 'truncated NFD draft refuses' {
        Reject { Assert-PdfCraftInputRejectedDraft (State $fixture) $fixture @((Editor $fullDraft.Substring(0, 256))) 'synthetic-editor' $fullDraft 256 $button $nfd $nfd } 'EDITOR_IDENTITY_OR_VALUE'
    }
    Check 'enabled Apply or overwritten saved value refuses' {
        $enabled = [pscustomobject]@{ role = 'Button'; label = 'Apply'; enabled = $true }
        Reject { Assert-PdfCraftInputRejectedDraft (State $fixture) $fixture @((Editor $fullDraft)) 'synthetic-editor' $fullDraft 256 $enabled $nfd $nfd } 'REJECTED_DRAFT_NOT_ATOMIC'
        Reject { Assert-PdfCraftInputRejectedDraft (State $fixture) $fixture @((Editor $fullDraft)) 'synthetic-editor' $fullDraft 256 $button $nfd 'Changed' } 'REJECTED_DRAFT_NOT_ATOMIC'
    }
    foreach ($channel in 'EventText', 'Paste', 'IME') {
        Check ("$channel mock lifecycle never claims physical or OS clipboard PASS") {
            $r = Test-PdfCraftInputMockLifecycle (Trace $fixture $nfd) $fixture 'synthetic-editor' $nfd $channel
            Assert ($r.scope -ceq 'fixture_only' -and $r.checkpoints -eq 7 -and $r.os_clipboard -ceq 'NOT VERIFIED' -and $r.physical_q -ceq 'NOT VERIFIED' -and $r.native_input -ceq 'NOT VERIFIED')
        }
    }
    Check 'missing reopen, wrong undo, and foreign save cannot pass lifecycle' {
        $trace = Trace $fixture $nfd
        Reject { Test-PdfCraftInputMockLifecycle $trace[0..5] $fixture 'synthetic-editor' $nfd } 'LIFECYCLE_INCOMPLETE'
        $trace[3].state.documents[0].dirty = $true
        Reject { Test-PdfCraftInputMockLifecycle $trace $fixture 'synthetic-editor' $nfd } 'LIFECYCLE_STATE'
        $trace = Trace $fixture $nfd; $trace[5].state.documents[0].path += '.other'
        Reject { Test-PdfCraftInputMockLifecycle $trace $fixture 'synthetic-editor' $nfd } 'UNEXPECTED_ACTIVE_PATH'
    }
    Check 'opaque unknown clipboard metadata always refuses with zero writes' {
        $r = Get-PdfCraftInputClipboardDecision @([pscustomobject]@{ format = 'OpaqueCustom'; type = 'System.IO.MemoryStream'; count = 42; lossless = $true })
        Assert ($r.status -ceq 'REFUSE' -and $r.opaque_or_unknown_count -eq 1 -and -not $r.content_read -and -not $r.clipboard_written)
    }
    Check 'known-only and empty metadata cannot grant clipboard permission' {
        foreach ($r in @((Get-PdfCraftInputClipboardDecision @([pscustomobject]@{ format = 'UnicodeText'; type = 'System.String'; count = 40 })), (Get-PdfCraftInputClipboardDecision))) {
            Assert ($r.status -ceq 'REFUSE' -and -not $r.content_read -and -not $r.clipboard_written)
        }
    }
    Check 'default entry and both preflight spellings are read-only exit zero' {
        foreach ($mode in @('', '-PreflightOnly', '-ValidateInputs')) {
            $modeArgs = if ($mode) { @($mode) } else { @() }
            $r = Run-Entry $modeArgs $configuration
            Assert ($r.code -eq 0 -and $r.value.status -ceq 'VALIDATED_INPUTS')
            Assert (-not $r.raw.Contains($temporary) -and -not (Test-Path -LiteralPath $configuration.RunDirectory))
        }
    }
    Check 'invalid binary hash exit two has only a sanitized reason' {
        $bad = $configuration.Clone(); $bad.ExpectedAppSha256 = '0' * 64
        $r = Run-Entry @('-PreflightOnly') $bad
        Assert ($r.code -eq 2 -and $r.value.reason -ceq 'APP_HASH_MISMATCH' -and -not $r.raw.Contains($temporary))
    }
    Check 'explicit GUI opt-in refuses exit three without launching anything' {
        $r = Run-Entry @('-GuiTest') @{}
        Assert ($r.code -eq 3 -and $r.value.status -ceq 'REFUSE' -and -not $r.value.clipboard_written)
    }
    Check 'runner contains no clipboard, GUI or process mutation API' {
        $tokens = $null; $parseErrors = $null
        $ast = [Management.Automation.Language.Parser]::ParseFile($RunnerPath, [ref]$tokens, [ref]$parseErrors)
        Assert ($parseErrors.Count -eq 0)
        $forbidden = @($ast.FindAll({ param($node)
            $node -is [Management.Automation.Language.CommandAst] -and
            $node.GetCommandName() -in @('Get-Clipboard', 'Set-Clipboard', 'Start-Process', 'Stop-Process', 'Add-Type', 'Set-WinUserLanguageList', 'Set-WinDefaultInputMethodOverride')
        }, $true))
        Assert ($forbidden.Count -eq 0)
    }
    Check 'preflight never changes pinned source files' {
        $finalHashes = @($app, $cli, $fixture | ForEach-Object { (Get-FileHash -LiteralPath $_).Hash })
        Assert (($initialHashes -join '|') -ceq ($finalHashes -join '|'))
    }
} finally {
    # Remove only this test's checked absolute GUID root, using one PowerShell shell.
    # Unlink the junction first; never traverse it during recursive cleanup.
    if ($script:linksCreated) { Remove-Item -LiteralPath $link -Force }
    $checked = [IO.Path]::GetFullPath($temporary)
    if (-not $checked.StartsWith($temporaryBase, [StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetFileName($checked) -notmatch '^pdfcraft-input-fixture-[0-9a-f]{32}$') { throw 'Unsafe fixture cleanup target' }
    if (Test-Path -LiteralPath $checked) { Remove-Item -LiteralPath $checked -Recurse -Force }
}
$failed = @($script:results | Where-Object status -eq 'FAIL').Count
$report = [pscustomobject]@{
    scope = 'synthetic fixtures and mocks only'; passed = $script:results.Count - $failed; failed = $failed
    runner_sha256 = (Get-FileHash -LiteralPath $RunnerPath).Hash.ToLowerInvariant()
    tests_sha256 = (Get-FileHash -LiteralPath $PSCommandPath).Hash.ToLowerInvariant()
    gui_started = $false; clipboard_read = $false; clipboard_written = $false
    physical_q = 'NOT VERIFIED'; physical_f = 'NOT VERIFIED'; os_clipboard = 'NOT VERIFIED'
    checks = $script:results.ToArray()
}
if ($ReportPath) { $report | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $ReportPath -Encoding utf8 }
$report.checks | Format-Table name, status -AutoSize
"Fixture/mock checks: $($report.passed) PASS / $failed FAIL; GUI/OS clipboard/physical input NOT VERIFIED."
if ($failed) { exit 1 }
exit 0
