param([string] $CraftFontsRoot)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'font-notices.ps1')
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('pdfcraft-font-notices-' + [guid]::NewGuid())
function Assert-Rejected([scriptblock] $Action, [string] $Message) {
  $rejected = $false
  try { & $Action | Out-Null } catch { $rejected = $true }
  if (-not $rejected) { throw "Expected rejection: $Message" }
}
try {
  foreach ($family in 'core', 'chinese') {
    New-Item -ItemType Directory -Path (Join-Path $fixture "fonts/$family") -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $fixture "fonts/$family/OFL.txt") -Value 'Synthetic notice fixture'
  }
  $core = 'Core | Regular | fonts/core/face.ttf | Latn | OFL-1.1 | fonts/core/OFL.txt'
  $chinese = 'Chinese | Regular | fonts/chinese/face.ttf | Hans | OFL-1.1 | fonts/chinese/OFL.txt'
  Set-Content (Join-Path $fixture 'fonts/manifest.txt') @('# comment', $core, $core)
  Set-Content (Join-Path $fixture 'fonts/simplified-chinese-manifest.txt') $chinese
  if (@(Get-CraftFontNotices $fixture $true).Count -ne 2) { throw 'Expected both unique family notices' }
  Set-Content (Join-Path $fixture 'fonts/simplified-chinese-manifest.txt') ($chinese.Replace('fonts/chinese/OFL.txt', 'fonts/core/OFL.txt'))
  Assert-Rejected { Get-CraftFontNotices $fixture $true } 'another family cannot supply this notice'
  Set-Content (Join-Path $fixture 'fonts/simplified-chinese-manifest.txt') $chinese
  Remove-Item (Join-Path $fixture 'fonts/chinese/OFL.txt')
  Assert-Rejected { Get-CraftFontNotices $fixture $true } 'one remaining OFL cannot cover the Chinese family'
  Set-Content (Join-Path $fixture 'fonts/chinese/OFL.txt') '' -NoNewline
  Assert-Rejected { Get-CraftFontNotices $fixture $true } 'empty family notice'
  Set-Content (Join-Path $fixture 'fonts/simplified-chinese-manifest.txt') ($chinese.Replace('fonts/chinese/OFL.txt', '../OFL.txt'))
  Assert-Rejected { Get-CraftFontNotices $fixture $true } 'notice path escapes font input'
  Remove-Item (Join-Path $fixture 'fonts/simplified-chinese-manifest.txt')
  Assert-Rejected { Get-CraftFontNotices $fixture $true } 'required Chinese manifest missing'
  if (@(Get-CraftFontNotices $fixture).Count -ne 1) { throw 'Optional Chinese manifest may be absent' }
  Set-Content (Join-Path $fixture 'fonts/manifest.txt') 'malformed'
  Assert-Rejected { Get-CraftFontNotices $fixture } 'malformed metadata'
  Assert-CraftFontNoticeNames @('OFL-core.txt', 'OFL-chinese.txt') @('SHORT.TXT|OFL-core.txt', 'OFL-chinese.txt')
  Assert-Rejected { Assert-CraftFontNoticeNames @('OFL-core.txt', 'OFL-chinese.txt') @('OFL-core.txt') } 'MSI lost one staged notice'
  if ($CraftFontsRoot) {
    $actual = @(Get-CraftFontNotices $CraftFontsRoot $true)
    Write-Output "Verified $($actual.Count) notices from the actual native manifests"
  }
  Write-Output 'Font notice regression checks passed'
} finally {
  Remove-Item -LiteralPath $fixture -Recurse -Force
}
