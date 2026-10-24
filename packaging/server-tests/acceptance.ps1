<#
Acceptance of an actual packaged OCR service on a DISPOSABLE elevated CI server.
Installs/uninstalls the MSI, creates a temporary non-admin user and encrypted SMB
share, and deliberately kills only the installed test service. Never run on a
personal/work server. Guards require GitHub Actions, explicit consent, supported
Server build, no existing PdfCraft installation/configuration, and admin identity.
#>
param(
    [Parameter(Mandatory)][string] $InstallerDirectory,
    [Parameter(Mandatory)][ValidatePattern('^[a-f0-9]{40}$')][string] $ExpectedCommit,
    [int] $ExpectedServerBuild = 0,
    [switch] $DisposableHost
)
. "$PSScriptRoot/common.ps1"

Assert-That ($DisposableHost -and $env:GITHUB_ACTIONS -eq 'true') 'This installs a service: explicit disposable GitHub Actions host required.'
Assert-That ($env:GITHUB_EVENT_NAME -eq 'workflow_dispatch') 'The server acceptance suite is manual only.'
Assert-That ($ExpectedCommit -ceq $env:GITHUB_SHA) 'Checked-out workflow and installer commit must match.'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
Assert-That ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) 'An elevated disposable runner is required.'
$os = Get-CimInstance Win32_OperatingSystem
Assert-That ($os.ProductType -ne 1 -and [int] $os.BuildNumber -in @(17763, 20348, 26100)) 'Expected Windows Server 2019, 2022 or 2025.'
if ($ExpectedServerBuild) { Assert-That ([int] $os.BuildNumber -eq $ExpectedServerBuild) 'Self-hosted runner is not the explicitly selected Server version.' }
Assert-That ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'X64') 'This acceptance suite consumes the native x64 installer.'
Assert-That (-not (Get-HotService)) 'Refusing to change an existing PdfCraftHotFolder service.'
$script:DataDirectory = Join-Path $env:ProgramData 'PdfCraft/HotFolder'
Assert-That (-not (Test-Path -LiteralPath $script:DataDirectory)) 'Refusing to use an existing Hot Folder configuration directory.'
Assert-That (-not (Test-Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\pdfcraft.exe')) 'Refusing to change an existing PdfCraft installation.'

$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$script:Results = Join-Path $root 'server-test-results'
Assert-That (-not (Test-Path -LiteralPath $script:Results)) 'Acceptance results already exist; use a fresh checkout.'
[void] (New-Item -ItemType Directory -Path $script:Results)
$token = [guid]::NewGuid().ToString('N')
$script:TestRoot = Join-Path $env:RUNNER_TEMP "pdfcraft-server-$token"
[void] (Assert-SafePath $script:TestRoot $env:RUNNER_TEMP)
[void] (New-Item -ItemType Directory -Path $script:TestRoot)
Set-PrivateDirectory $script:TestRoot @('S-1-5-19')
$appDirectory = Join-Path $script:TestRoot 'app'
$script:Cli = Join-Path $appDirectory 'pdfcraft-cli.exe'
$companion = Join-Path $appDirectory 'pdfcraft-hot-folder.exe'
$script:Config = Join-Path $script:DataDirectory 'tasks.json'
$marker = Join-Path $script:DataDirectory 'CI-OWNER.txt'
$msiExe = Join-Path $env:SystemRoot 'System32/msiexec.exe'
$scExe = Join-Path $env:SystemRoot 'System32/sc.exe'
$secedit = Join-Path $env:SystemRoot 'System32/secedit.exe'
$user = 'pfc' + $token.Substring(0, 12)
$account = "$env:COMPUTERNAME\$user"
$shareName = 'PdfCraftCI-' + $token.Substring(0, 12) + '$'
$shareRoot = Join-Path $script:TestRoot 'smb'
$sharePath = "\\$env:COMPUTERNAME\$shareName"
$password = $null
$accountCreated = $false
$privilegeChanged = $false
$installed = $false
$primaryFailure = $false
$productCodes = [Collections.Generic.List[string]]::new()
$evidence = [ordered]@{
    result = 'running'; os = $os.Caption; build = $os.BuildNumber
    source_commit = $ExpectedCommit; installer_run = $env:INSTALLER_RUN_ID
    tests = [ordered]@{}; limitation = 'Authenticated encrypted loopback SMB, not a specific NAS vendor or domain/gMSA integration.'
}

function Record([string] $Name, $Value) {
    $script:evidence.tests[$Name] = $Value
    Write-Json (Join-Path $script:Results 'report.json') $script:evidence
    Write-Output "passed: $Name"
}

function New-TestFolder([string] $Name) {
    $path = Join-Path $script:TestRoot $Name
    [void] (Assert-SafePath $path $script:TestRoot)
    [void] (New-Item -ItemType Directory -Path $path)
    return $path
}

function Put-Scan([string] $Folder, [string] $Stem, [string] $Source = $script:Scan) {
    Copy-Item -LiteralPath $Source -Destination (Join-Path $Folder "$Stem.pdf")
}

function Wait-TaskChild([string] $TaskId) {
    Wait-For "active OCR child for $TaskId" {
        $children = @(Get-CimInstance Win32_Process -Filter "Name='pdfcraft-hot-folder.exe'" | Where-Object {
            $_.ExecutablePath -eq $companion -and $_.CommandLine -match '--task-child' -and
                $_.CommandLine -match ('--task-id\s+"?' + [regex]::Escape($TaskId) + '(?:"|\s|$)') -and
                ([uint64] $_.KernelModeTime + [uint64] $_.UserModeTime) -gt 1000000
        })
        if ($children.Count) { $script:ObservedChild = $children[0].ProcessId; return $true }
        return $false
    } 80
    return $script:ObservedChild
}

function New-TestShare {
    [void] (New-SmbShare -Name $shareName -Path $shareRoot -FullAccess @($account, $identity.Name) -EncryptData $true -CachingMode None)
}

function Set-ServiceLogonRight([string] $Assignments, [string] $Name) {
    $policy = Join-Path $script:TestRoot "$Name.inf"
    $content = "[Unicode]`r`nUnicode=yes`r`n[Version]`r`nsignature=`"`$CHICAGO`$`"`r`nRevision=1`r`n[Privilege Rights]`r`nSeServiceLogonRight = $Assignments`r`n"
    [IO.File]::WriteAllText($policy, $content, [Text.Encoding]::Unicode)
    [void] (Invoke-Program $secedit @('/configure', '/db', (Join-Path $script:TestRoot "$Name.sdb"), '/cfg', $policy, '/areas', 'USER_RIGHTS', '/quiet') $Name)
}

try {
    $msis = @(Get-ChildItem -LiteralPath $InstallerDirectory -File | Where-Object { $_.Name -match '^pdfcraft-[A-Za-z0-9_.-]+-windows-x64\.msi$' })
    Assert-That ($msis.Count -eq 1) 'Expected exactly one x64 MSI in the verified artifact.'
    $msi = $msis[0].FullName
    $sums = @(Get-ChildItem -LiteralPath $InstallerDirectory -File | Where-Object { $_.Name -eq ($msis[0].Name + '.sha256') -or $_.Name -eq 'SHA256SUMS.txt' })
    Assert-That ($sums.Count -eq 1) 'Expected exactly one installer checksum file.'
    $expectedHash = @((Get-Content -LiteralPath $sums[0].FullName) | Where-Object { $_ -match ('^[a-fA-F0-9]{64}\s+\*?' + [regex]::Escape($msis[0].Name) + '$') })
    Assert-That ($expectedHash.Count -eq 1) 'No unambiguous checksum record for this MSI.'
    $digest = (Get-FileHash -LiteralPath $msi -Algorithm SHA256).Hash.ToLowerInvariant()
    Assert-That ($digest -eq ($expectedHash[0] -split '\s+')[0].ToLowerInvariant()) 'Installer checksum mismatch.'
    $evidence.installer_sha256 = $digest
    $productCodes.Add((Get-ProductCode $msi))
    # Include a partially failed installation in the cleanup attempt too; each
    # product code came from this run's MSI, and pre-existing PdfCraft is refused.
    $installed = $true
    [void] (Invoke-Program $msiExe @('/i', $msi, '/qn', '/norestart', 'ALLUSERS=1', 'INSTALLHOTFOLDER=1', 'INSTALLDESKTOPSHORTCUT=0', "INSTALLFOLDER=$appDirectory", '/L*v', (Join-Path $script:Results 'install-msi.log')) 'install' 180 @(0, 3010))
    Wait-For 'empty service-owned configuration' { Test-Path -LiteralPath $script:Config } 60
    [IO.File]::WriteAllText($marker, $token)
    $first = Get-Content -LiteralPath $script:Config -Raw | ConvertFrom-Json
    Assert-That (@($first.tasks).Count -eq 0) 'Installer unexpectedly enabled processing tasks.'
    $service = Get-HotService
    Assert-That ($service.StartName -eq 'NT AUTHORITY\LocalService') 'Fresh installation must default to LocalService.'
    Assert-That ($service.StartMode -eq 'Auto' -and $service.State -eq 'Running') 'Installed service must be running with automatic startup.'
    $serviceSid = ([Security.Principal.NTAccount]::new('NT SERVICE\PdfCraftHotFolder')).Translate([Security.Principal.SecurityIdentifier]).Value
    Set-PrivateDirectory $script:TestRoot @('S-1-5-19', $serviceSid)
    foreach ($folder in @($script:DataDirectory, (Join-Path $script:DataDirectory 'secrets'))) {
        $acl = Get-Acl -LiteralPath $folder
        Assert-That $acl.AreAccessRulesProtected 'Installed service directory DACL must be protected.'
        $sids = @($acl.Access | ForEach-Object { $_.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value })
        Assert-That ($serviceSid -in $sids) 'Stable service-SID ACE missing from the installed directory.'
        if ($folder.EndsWith('\secrets')) { Assert-That ('S-1-5-32-545' -notin $sids -and 'S-1-1-0' -notin $sids) 'Credentials directory must not grant Users/Everyone access.' }
    }
    $registry = Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Services\PdfCraftHotFolder'
    Assert-That ($registry.ServiceSidType -eq 1 -and $registry.FailureActionsOnNonCrashFailures -eq 1) 'Service SID or non-crash recovery is not enabled.'
    foreach ($query in @('qfailure', 'qfailureflag', 'qsidtype')) { [void] (Invoke-Program $scExe @($query, 'PdfCraftHotFolder') $query) }
    Record 'installed_localservice_acl_recovery' @{ account = $service.StartName; service_sid = $serviceSid; tasks_initially_empty = $true }
    [void] (Stop-HotService)
    $env:PDFCRAFT_SYSTEM_FONTS = '0'
    $env:PDFCRAFT_MODELS = Join-Path $appDirectory 'models'
    $ocr = Invoke-Tool 'ocr-status' 'ocr_status' @{}
    Assert-That $ocr.available 'Packaged modern OCR models unavailable.'
    $text = ('Hot folder searchable output from a synthetic server acceptance scan.' + "`n") * 12
    $script:Scan = Join-Path $script:TestRoot 'scan.pdf'
    $scanImage = Join-Path $script:TestRoot 'scan.png'
    $largeScan = Join-Path $script:TestRoot 'large-scan.pdf'
    [void] (Invoke-CliScript 'create-scans' @(
        @{ tool = 'doc_create'; args = @{ from = 'text'; text = $text } },
        @{ tool = 'page_render'; args = @{ doc = 1; page = 1; dpi = 150 }; out = $scanImage },
        @{ tool = 'doc_create'; args = @{ from = 'images'; paths = @($scanImage); dpi = 150 } },
        @{ tool = 'doc_save'; args = @{ doc = 2; path = $script:Scan } },
        @{ tool = 'doc_create'; args = @{ from = 'images'; paths = @($scanImage) * 24; dpi = 150 } },
        @{ tool = 'doc_save'; args = @{ doc = 3; path = $largeScan } }
    ))
    $scanHash = (Get-FileHash -LiteralPath $script:Scan -Algorithm SHA256).Hash
    $localIn = New-TestFolder 'local-in'; $localOut = New-TestFolder 'local-out'
    $faultIn = New-TestFolder 'fault-in'; $faultOut = New-TestFolder 'fault-out'
    $localTask = New-Task 'local' $localIn $localOut
    $faultTask = New-Task 'fault' $faultIn $faultOut 'delete'
    Save-Tasks @($localTask, $faultTask) 'configure-local'
    Put-Scan $localIn 'baseline'
    Start-HotService
    Wait-Output $localOut 'baseline'
    $search = Invoke-CliScript 'search-saved' @(
        @{ tool = 'doc_open'; args = @{ path = (Join-Path $localOut 'baseline.pdf') } },
        @{ tool = 'text_find'; args = @{ doc = 1; query = 'searchable output' } }
    )
    Assert-That ($search -match '"matches"\s*:\s*\[\s*\{') 'Saved service OCR PDF does not contain searchable text.'
    Assert-That ((Get-FileHash -LiteralPath (Join-Path $localIn 'baseline.pdf')).Hash -eq $scanHash) 'Local source changed.'
    Record 'actual_localservice_ocr' @{ source_unchanged = $true; searchable_pdf = $true; text_export = $true }

    # Write denial is recoverable I/O, and cannot justify deleting the input or
    # blocking an independent task. The denial targets only this test service SID.
    $deny = [Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new($serviceSid), 'Write', 'ContainerInherit,ObjectInherit', 'None', 'Deny')
    $faultAcl = Get-Acl -LiteralPath $faultOut
    $faultAcl.AddAccessRule($deny); Set-Acl -LiteralPath $faultOut -AclObject $faultAcl
    Put-Scan $faultIn 'write-denied'
    Wait-For 'recorded transient output failure' { [bool] (Read-State).tasks.fault.error } 90
    Assert-That ((Get-FileHash -LiteralPath (Join-Path $faultIn 'write-denied.pdf')).Hash -eq $scanHash) 'Failed output discarded or changed source.'
    Put-Scan $localIn 'during-output-failure'
    Wait-Output $localOut 'during-output-failure' 90
    $faultAcl.RemoveAccessRuleSpecific($deny); Set-Acl -LiteralPath $faultOut -AclObject $faultAcl
    Wait-Output $faultOut 'write-denied' 180
    Wait-For 'source deleted only after both outputs' { -not (Test-Path -LiteralPath (Join-Path $faultIn 'write-denied.pdf')) } 30
    Record 'output_failure_isolated_and_auto_retried' @{ independent_task_completed = $true; no_manual_retry = $true; source_kept_until_outputs = $true }

    $brokenPath = Join-Path $localIn 'broken.pdf'
    [IO.File]::WriteAllText($brokenPath, "%PDF-1.7`nnot a valid PDF", [Text.Encoding]::ASCII)
    $brokenHash = (Get-FileHash -LiteralPath $brokenPath -Algorithm SHA256).Hash
    Put-Scan $localIn 'after-broken-document'
    Wait-For 'permanent malformed-document error' { (Read-State).tasks.local.failed -gt 0 } 90
    Wait-Output $localOut 'after-broken-document' 90
    Assert-SourceUnchanged $brokenPath $brokenHash 'Malformed-document handling'
    Record 'malformed_document_does_not_block_next_file' @{ malformed_source_retained = $true; source_unchanged = $true; valid_next_file_completed = $true }

    Put-Scan $localIn 'crash-recovery' $largeScan
    $largeHash = (Get-FileHash -LiteralPath $largeScan).Hash
    $childPid = Wait-TaskChild 'local'
    $oldPid = (Get-HotService).ProcessId
    Assert-That ($oldPid -gt 0 -and (Get-Process -Id $oldPid).Path -eq $companion) 'Refusing to kill an unrelated service process.'
    Stop-Process -Id $oldPid -Force
    Wait-For 'orphaned OCR child terminated by service job' { -not (Get-Process -Id $childPid -ErrorAction SilentlyContinue) } 15
    Wait-For 'automatic SCM recovery after active OCR hard kill' { $s = Get-HotService; $s.State -eq 'Running' -and $s.ProcessId -ne $oldPid } 150
    Wait-Output $localOut 'crash-recovery' 240
    Assert-That ((Get-FileHash -LiteralPath (Join-Path $localIn 'crash-recovery.pdf')).Hash -eq $largeHash) 'Crash recovery changed its source.'
    $hashesBefore = @(Get-ChildItem -LiteralPath $localOut -File | ForEach-Object { "$($_.Name):$((Get-FileHash -LiteralPath $_.FullName).Hash)" })
    Start-Sleep -Seconds 5
    $hashesAfter = @(Get-ChildItem -LiteralPath $localOut -File | ForEach-Object { "$($_.Name):$((Get-FileHash -LiteralPath $_.FullName).Hash)" })
    Assert-That (-not (Compare-Object $hashesBefore $hashesAfter)) 'Recovered queue duplicated or modified completed outputs.'
    Assert-That (@(Get-ChildItem -LiteralPath $localOut -File | Where-Object Name -Like 'crash-recovery*').Count -eq 2) 'Crash recovery emitted duplicate PDF/TXT outputs.'
    Record 'active_ocr_hardkill_recovery' @{ old_service_pid = $oldPid; active_child_pid = $childPid; recovered_service_pid = (Get-HotService).ProcessId; source_unchanged = $true; outputs_idempotent = $true }

    Put-Scan $localIn 'bounded-stop' $largeScan
    $stopChild = Wait-TaskChild 'local'
    $stopSeconds = Stop-HotService 35
    Wait-For 'stopped service child gone' { -not (Get-Process -Id $stopChild -ErrorAction SilentlyContinue) } 10
    Assert-That ($stopSeconds -lt 35) 'Service stop exceeded the acceptance bound.'
    Assert-SourceUnchanged (Join-Path $localIn 'bounded-stop.pdf') $largeHash 'Stopping during OCR'
    Record 'bounded_stop_during_ocr' @{ seconds = $stopSeconds; source_retained = $true; source_unchanged = $true }

    # A filesystem error during worker initialization returns Err rather than a
    # crash. Recovery must include non-crash exits, without a manual second start.
    $workerLock = [IO.Path]::ChangeExtension($script:Config, 'worker.lock')
    $workerBackup = $workerLock + '.ci-backup'
    [void] (Assert-SafePath $workerLock $script:DataDirectory)
    [void] (Assert-SafePath $workerBackup $script:DataDirectory)
    if (Test-Path -LiteralPath $workerLock) { Move-Item -LiteralPath $workerLock -Destination $workerBackup }
    [void] (New-Item -ItemType Directory -Path $workerLock)
    try {
        $request = Invoke-CimMethod -InputObject (Get-HotService) -MethodName StartService
        Assert-That ($request.ReturnValue -in @(0, 7, 8, 9)) "Unexpected controlled-start result: $($request.ReturnValue)"
        Wait-For 'controlled non-crash service failure' { $s = Get-HotService; $s.State -eq 'Stopped' -and $s.ExitCode -eq 1066 -and $s.ServiceSpecificExitCode -eq 1 } 35
    } finally {
        [void] (Assert-SafePath $workerLock $script:DataDirectory)
        Remove-Item -LiteralPath $workerLock -Force
        if (Test-Path -LiteralPath $workerBackup) { Move-Item -LiteralPath $workerBackup -Destination $workerLock }
    }
    Wait-For 'automatic non-crash recovery' { (Get-HotService).State -eq 'Running' } 150
    Wait-Output $localOut 'bounded-stop' 240
    Record 'controlled_err_scm_recovery' @{ lock_fault_removed = $true; recovered_without_manual_restart = $true }
    [void] (Stop-HotService)

    # SMB is authenticated as an ephemeral non-admin Windows account. LocalService
    # is deliberately not given an anonymous or guest share to manufacture success.
    $random = [byte[]]::new(32); [Security.Cryptography.RandomNumberGenerator]::Fill($random)
    $password = [Convert]::ToBase64String($random) + 'aA1!'
    Write-Output "::add-mask::$password"
    [void] (New-LocalUser -Name $user -Password (ConvertTo-SecureString $password -AsPlainText -Force) -PasswordNeverExpires -UserMayNotChangePassword -Description 'Disposable PdfCraft CI service account')
    $accountCreated = $true
    $userSid = (Get-LocalUser -Name $user).SID.Value
    $export = Join-Path $script:TestRoot 'rights-before.inf'
    [void] (Invoke-Program $secedit @('/export', '/cfg', $export, '/areas', 'USER_RIGHTS', '/quiet') 'export-service-rights')
    $line = @(Get-Content -LiteralPath $export | Where-Object { $_ -match '^SeServiceLogonRight\s*=' })
    $originalRight = if ($line.Count) { ($line[0] -split '=', 2)[1].Trim() } else { '' }
    $newRight = (@($originalRight -split ',' | Where-Object { $_ }) + "*$userSid") -join ','
    Set-ServiceLogonRight $newRight 'grant-service-right'
    $privilegeChanged = $true
    Set-PrivateDirectory $script:TestRoot @('S-1-5-19', $serviceSid, $userSid)
    $prepared = Invoke-Tool 'prepare-account' 'hot_folder_service' @{ action = 'prepare-account'; executable = $companion; config = $script:Config }
    Assert-That ($prepared.installed -and $prepared.account -eq 'NT AUTHORITY\LocalService') 'Automation preparation must retain the existing service account.'
    Record 'prepare_account_automation' @{ installed = $prepared.installed; account_unchanged = $true }
    # CIM passes the temporary secret directly to SCM; it is never a command-line
    # argument, task JSON, installer property, source receipt, or log content.
    $changed = Invoke-CimMethod -InputObject (Get-HotService) -MethodName Change -Arguments @{ StartName = $account; StartPassword = $password }
    Assert-That ($changed.ReturnValue -eq 0) "SCM account configuration failed (code $($changed.ReturnValue))."
    $password = $null
    [void] (New-TestFolder 'smb')
    $smbIn = New-TestFolder 'smb/in'; $smbOut = New-TestFolder 'smb/out'
    New-TestShare
    $smbTask = New-Task 'smb' "$sharePath\in" "$sharePath\out"
    Save-Tasks @($localTask, $smbTask) 'configure-smb'
    Put-Scan $smbIn 'authenticated-smb'
    Start-HotService
    Wait-Output $smbOut 'authenticated-smb' 120
    $sessions = @(Get-SmbSession | Where-Object { $_.ClientUserName -ieq $account })
    Assert-That ($sessions.Count -gt 0) 'No SMB session authenticated as the configured service account.'
    Assert-That (Get-SmbShare -Name $shareName).EncryptData 'The test share must require SMB encryption.'
    Assert-SourceUnchanged (Join-Path $smbIn 'authenticated-smb.pdf') $scanHash 'Authenticated SMB OCR'
    Record 'authenticated_smb_ocr' @{ account = $account; encrypted_share = $true; authenticated_sessions = $sessions.Count; source_unchanged = $true }
    Remove-SmbShare -Name $shareName -Force
    Put-Scan $smbIn 'after-share-outage'
    Wait-For 'unavailable UNC recorded for affected task' { [bool] (Read-State).tasks.smb.error } 60
    Put-Scan $localIn 'during-share-outage'
    Wait-Output $localOut 'during-share-outage' 90
    Assert-That ((Get-FileHash -LiteralPath (Join-Path $smbIn 'after-share-outage.pdf')).Hash -eq $scanHash) 'Unavailable share lost or changed the source.'
    New-TestShare
    Wait-Output $smbOut 'after-share-outage' 180
    Record 'smb_outage_isolated_and_auto_retried' @{ independent_local_task_completed = $true; no_manual_retry = $true; source_preserved = $true }

    # The real same-version MajorUpgrade must retain credentials kept by SCM.
    # Changing ProductCode/PackageCode alone preserves the exact shipped payload.
    [void] (Stop-HotService)
    $beforeUpgradeConfig = (Get-FileHash -LiteralPath $script:Config).Hash
    $beforeUpgradeAccount = (Get-HotService).StartName
    $beforeUpgradeBinaries = @(Get-ChildItem -LiteralPath $appDirectory -File -Filter '*.exe' | ForEach-Object { "$($_.Name):$((Get-FileHash -LiteralPath $_.FullName).Hash)" })
    $upgrade = Join-Path $script:TestRoot 'same-version-upgrade-fixture.msi'
    $productCodes.Add((New-UpgradeFixture $msi $upgrade))
    [void] (Invoke-Program $msiExe @('/i', $upgrade, '/qn', '/norestart', 'ALLUSERS=1', 'INSTALLHOTFOLDER=1', 'INSTALLDESKTOPSHORTCUT=0', "INSTALLFOLDER=$appDirectory", '/L*v', (Join-Path $script:Results 'upgrade-msi.log')) 'same-version-upgrade' 180 @(0, 3010))
    Wait-For 'upgraded service running' { (Get-HotService).State -eq 'Running' } 60
    Assert-That ((Get-HotService).StartName -ieq $beforeUpgradeAccount) 'Same-version MSI upgrade reset the service account.'
    Assert-That ((Get-FileHash -LiteralPath $script:Config).Hash -eq $beforeUpgradeConfig) 'Upgrade changed user task settings.'
    $afterUpgradeBinaries = @(Get-ChildItem -LiteralPath $appDirectory -File -Filter '*.exe' | ForEach-Object { "$($_.Name):$((Get-FileHash -LiteralPath $_.FullName).Hash)" })
    Assert-That (-not (Compare-Object $beforeUpgradeBinaries $afterUpgradeBinaries)) 'Upgrade fixture changed the real packaged executables.'
    Put-Scan $smbIn 'after-upgrade'
    Wait-Output $smbOut 'after-upgrade' 120
    Record 'same_version_major_upgrade_preserves_account' @{ same_real_payload = $true; test_only_product_and_package_guids = $true; account_unchanged = $true; tasks_unchanged = $true; authenticated_smb_after_upgrade = $true }
    [void] (Stop-HotService)
    Copy-Item -LiteralPath ([IO.Path]::ChangeExtension($script:Config, 'state.json')) -Destination (Join-Path $script:Results 'final-state.json')
    $evidence.result = 'passed'
} catch {
    $primaryFailure = $true
    $evidence.result = 'failed'
    $evidence.error = $_.Exception.Message
    throw
} finally {
    $password = $null
    $cleanupErrors = [Collections.Generic.List[string]]::new()
    function Cleanup-Step([scriptblock] $Action) {
        try { & $Action } catch { $cleanupErrors.Add($_.Exception.Message) }
    }
    Cleanup-Step {
        $service = Get-HotService
        if ($service) {
            try { [void] (Stop-HotService) } catch {
                if ($service.ProcessId -gt 0 -and (Get-Process -Id $service.ProcessId -ErrorAction SilentlyContinue).Path -eq $companion) {
                    Stop-Process -Id $service.ProcessId -Force
                }
            }
        }
    }
    if ($installed -or (Test-Path -LiteralPath $marker)) {
        foreach ($code in $productCodes) {
            Cleanup-Step {
                Assert-ProductCode $code
                [void] (Invoke-Program $msiExe @('/x', $code, '/qn', '/norestart', '/L*v', (Join-Path $script:Results "uninstall-$($code.Trim('{}')).log")) ('uninstall-' + $code.Trim('{}')) 180 @(0, 1605, 1614, 3010))
            }
        }
    }
    Cleanup-Step {
        if (Get-SmbShare -Name $shareName -ErrorAction SilentlyContinue) { Remove-SmbShare -Name $shareName -Force }
    }
    Cleanup-Step { if ($privilegeChanged) { Set-ServiceLogonRight $originalRight 'restore-service-right' } }
    Cleanup-Step { if ($accountCreated) { Remove-LocalUser -Name $user } }
    Cleanup-Step { Assert-That (-not (Get-HotService)) 'Test service remains after uninstall.' }
    Cleanup-Step {
        if ((Test-Path -LiteralPath $marker) -and -not (Get-HotService)) {
            Assert-That ((Get-Content -LiteralPath $marker -Raw) -ceq $token) 'Refusing to remove a directory without this run owner marker.'
            [void] (Assert-SafePath $script:DataDirectory (Join-Path $env:ProgramData 'PdfCraft'))
            Remove-Item -LiteralPath $script:DataDirectory -Recurse -Force
        }
    }
    $evidence.cleanup = @{ errors = @($cleanupErrors); service_removed = (-not (Get-HotService)); ephemeral_account_removed = (-not (Get-LocalUser -Name $user -ErrorAction SilentlyContinue)); share_removed = (-not (Get-SmbShare -Name $shareName -ErrorAction SilentlyContinue)) }
    if ($cleanupErrors.Count) { $evidence.result = 'failed' }
    Write-Json (Join-Path $script:Results 'report.json') $evidence
    # Keep private working files on failure for the disposable runner's diagnosis;
    # only the explicitly selected result/log folder is uploaded by the workflow.
    if ($evidence.result -eq 'passed') {
        [void] (Assert-SafePath $script:TestRoot $env:RUNNER_TEMP)
        Remove-Item -LiteralPath $script:TestRoot -Recurse -Force
    }
    Assert-CleanupSucceeded @($cleanupErrors) $primaryFailure
}
