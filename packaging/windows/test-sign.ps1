# Run with pwsh. Tests the real signer with dummy material and a fake tool; no network or signing.
param([string] $SignScript = (Join-Path $PSScriptRoot 'sign.ps1'))
$ErrorActionPreference = 'Stop'
$SignScript = (Resolve-Path -LiteralPath $SignScript).Path
foreach ($expectFailure in @($false, $true)) {
  $fixture = Join-Path ([IO.Path]::GetTempPath()) ('craft-sign-test-' + [guid]::NewGuid())
  New-Item -ItemType Directory -Path $fixture | Out-Null
  try {
    $temporaryRoot = Join-Path $fixture 'temp[1]'
    $siblingRoot = Join-Path $fixture 'temp1'
    [IO.Directory]::CreateDirectory($temporaryRoot) | Out-Null
    $artifact = Join-Path $fixture 'artifact[1].exe'
    [IO.File]::WriteAllText($artifact, 'Synthetic artifact; never executed')
    [IO.File]::WriteAllText((Join-Path $fixture 'artifact1.exe'), 'Wrong artifact')
    $calls = Join-Path $fixture 'calls.txt'
    $fakeTool = Join-Path $fixture 'fake-signtool.ps1'
    [IO.File]::WriteAllText($fakeTool, @'
param([Parameter(ValueFromRemainingArguments = $true)][string[]] $Arguments)
if ($Arguments[-1] -cne $env:CRAFT_SIGN_TEST_ARTIFACT) { throw 'Wrong artifact passed to signtool' }
[IO.File]::AppendAllLines($env:CRAFT_SIGN_TEST_CALLS, [string[]]@($Arguments[0]))
if ($Arguments[0] -eq 'sign') {
  $certificate = $Arguments[[Array]::IndexOf($Arguments, '/f') + 1]
  $generated = [IO.Path]::GetDirectoryName($certificate)
  $sibling = Join-Path $env:CRAFT_SIGN_TEST_SIBLING ([IO.Path]::GetFileName($generated))
  [IO.Directory]::CreateDirectory($sibling) | Out-Null
  [IO.File]::WriteAllText((Join-Path $sibling 'preserve.txt'), 'Keep the matching sibling')
}
$global:LASTEXITCODE = [int]$env:CRAFT_SIGN_TEST_EXIT
'@)
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = Join-Path $PSHOME 'pwsh.exe'
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in @('-NoProfile', '-File', $SignScript, $artifact)) { $start.ArgumentList.Add($argument) }
    foreach ($name in @('WINDOWS_CERTIFICATE_PASSWORD', 'AZURE_TENANT_ID', 'AZURE_CLIENT_ID', 'AZURE_CLIENT_SECRET', 'AZURE_SIGNING_ENDPOINT', 'AZURE_SIGNING_ACCOUNT', 'AZURE_CERT_PROFILE')) {
      $start.Environment.Remove($name) | Out-Null
    }
    $start.Environment['WINDOWS_CERTIFICATE'] = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes('Synthetic PFX; not a certificate or secret'))
    $start.Environment['SIGNTOOL'] = $fakeTool
    $start.Environment['TMP'] = $temporaryRoot
    $start.Environment['TEMP'] = $temporaryRoot
    $start.Environment['CRAFT_SIGN_TEST_ARTIFACT'] = $artifact
    $start.Environment['CRAFT_SIGN_TEST_CALLS'] = $calls
    $start.Environment['CRAFT_SIGN_TEST_SIBLING'] = $siblingRoot
    $start.Environment['CRAFT_SIGN_TEST_EXIT'] = if ($expectFailure) { '42' } else { '0' }
    $process = [Diagnostics.Process]::Start($start)
    $output = $process.StandardOutput.ReadToEnd()
    $errorOutput = $process.StandardError.ReadToEnd()
    $process.WaitForExit()
    $code = $process.ExitCode
    $process.Dispose()
    if (($code -eq 0) -eq $expectFailure) { throw "Unexpected exit ${code}: $output $errorOutput" }
    $operations = [IO.File]::ReadAllLines($calls) -join ','
    $expected = if ($expectFailure) { 'sign' } else { 'sign,verify' }
    if ($operations -ne $expected) { throw "Expected fake operations $expected, got $operations" }
    $leftovers = @(Get-ChildItem -LiteralPath $temporaryRoot)
    $markers = @(Get-ChildItem -LiteralPath $siblingRoot -Recurse -Filter preserve.txt -ErrorAction SilentlyContinue)
    if ($leftovers.Count -ne 0 -or $markers.Count -ne 1) {
      throw "Cleanup regression: $($leftovers.Count) temporary entries, $($markers.Count) matching sibling markers"
    }
    Write-Output "PASS exact artifact and cleanup paths (signing failure: $expectFailure)"
  } finally {
    $target = Get-Item -LiteralPath $fixture
    if ($target.FullName -ne [IO.Path]::GetFullPath($fixture) -or ($target.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
      throw 'Unexpected test cleanup target'
    }
    Remove-Item -LiteralPath $target.FullName -Recurse -Force
  }
}
