<#
.SYNOPSIS
  Record the facts behind a scanner detection of PdfCraft Windows files: SHA-256, Authenticode
  status and, with -YaraExe and -YaraRule, the matched strings and offsets of an external YARA rule.

.DESCRIPTION
  Read-only. It never changes, runs or unpacks the files it inspects, makes no network connection
  itself and needs no administrator rights. Neither YARA nor any rule is bundled or downloaded: pass
  your own copies. See docs/releasing.md ("Scanner detections").

  A YARA match only means that a byte pattern occurred in the file. It is not proof of malware.
  Report the rule file's SHA-256 (printed here), the matched identifiers and offsets, and the
  condition branch that fired.

.EXAMPLE
  pwsh packaging/windows/security-audit.ps1 -Path dist\release\pdfcraft-0.5.0-windows-x64-portable\pdfcraft.exe

.EXAMPLE
  pwsh packaging/windows/security-audit.ps1 -Path .\pdfcraft.exe, .\pdfcraft-cli.exe -YaraExe C:\tools\yara64.exe -YaraRule C:\rules\CobaltStrikeBeacon.yar
#>
param(
  [Parameter(Mandatory)] [string[]] $Path,
  [string] $YaraExe,
  [string] $YaraRule
)
$ErrorActionPreference = 'Stop'

function Invoke-Yara([string[]] $Arguments) {
  # Native stderr would end the script under $ErrorActionPreference = 'Stop' (Windows PowerShell 5.1),
  # so capture it as text and judge the result by the exit code.
  $ErrorActionPreference = 'Continue'
  $lines = @(& $YaraExe @Arguments 2>&1 | ForEach-Object { "$_" })
  return [pscustomobject] @{ Code = $LASTEXITCODE; Lines = $lines }
}

if (([bool] $YaraExe) -ne ([bool] $YaraRule)) { throw 'Pass -YaraExe and -YaraRule together.' }
Write-Output "Scanned (UTC): $((Get-Date).ToUniversalTime().ToString('yyyy-MM-dd HH:mm:ss'))"
if ($YaraExe) {
  foreach ($tool in $YaraExe, $YaraRule) {
    if (-not (Test-Path -LiteralPath $tool -PathType Leaf)) { throw "not found: $tool" }
  }
  $YaraExe = (Resolve-Path -LiteralPath $YaraExe).ProviderPath
  $YaraRule = (Resolve-Path -LiteralPath $YaraRule).ProviderPath
  Write-Output "YARA:         $((Invoke-Yara @('--version')).Lines | Select-Object -First 1) ($YaraExe)"
  Write-Output "Rule:         $YaraRule"
  Write-Output "Rule SHA-256: $((Get-FileHash -LiteralPath $YaraRule -Algorithm SHA256).Hash)"
} else {
  Write-Output 'YARA:         not run (pass -YaraExe and -YaraRule to scan with an external rule)'
}

$failed = $false
foreach ($item in $Path) {
  if (-not (Test-Path -LiteralPath $item -PathType Leaf)) {
    Write-Output "error: not a file: $item"
    $failed = $true
    continue
  }
  $file = (Resolve-Path -LiteralPath $item).ProviderPath
  $sig = Get-AuthenticodeSignature -LiteralPath $file
  $signer = if ($sig.SignerCertificate) { $sig.SignerCertificate.Subject } else { '(none)' }
  $stamper = if ($sig.TimeStamperCertificate) { $sig.TimeStamperCertificate.Subject } else { '(none)' }

  Write-Output ''
  Write-Output "== $file"
  Write-Output "Size:         $((Get-Item -LiteralPath $file).Length) bytes"
  Write-Output "SHA-256:      $((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash)"
  Write-Output "Authenticode: $($sig.Status) ($($sig.StatusMessage))"
  Write-Output "Signer:       $signer"
  Write-Output "Timestamper:  $stamper"

  if ($YaraExe) {
    $scan = Invoke-Yara @('-s', '-m', $YaraRule, $file)
    if ($scan.Code -ne 0) {
      Write-Output "YARA:         failed with exit code $($scan.Code)"
      $scan.Lines | ForEach-Object { Write-Output "  $_" }
      $failed = $true
      continue
    }
    $ruleLines = @($scan.Lines | Where-Object { $_ -and $_ -notmatch '^0x' })
    $stringLines = @($scan.Lines | Where-Object { $_ -match '^0x' })
    Write-Output "YARA:         $($ruleLines.Count) rule match(es), $($stringLines.Count) string match(es)"
    $scan.Lines | Where-Object { $_ } | ForEach-Object { Write-Output "  $_" }
  }
}

if ($YaraExe) {
  Write-Output ''
  Write-Output 'A YARA match is a pattern match, not a verdict. Keep the rule revision, identifiers, offsets and branch with any report.'
}
if ($failed) { exit 1 }
