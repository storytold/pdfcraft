# Validate one non-comment manifest row before adding its family notice.
function Get-CraftFontNoticeRow([string] $rootPath, [string] $manifest, [string] $line) {
  $fields = @($line -split ' \| ' | ForEach-Object { $_.Trim() })
  if ($fields.Count -lt 6 -or $fields[4] -ne 'OFL-1.1') { throw "Missing OFL metadata in ${manifest}: $line" }
  # Manifest notice paths must name a family-local file, never escape the font input.
  $relative = $fields[5].Replace('\', '/')
  if ($relative -notmatch '^fonts/([a-zA-Z0-9_-]+)/OFL\.txt$') { throw "Invalid font notice path: $relative" }
  $family = $Matches[1]
  $fontPath = $fields[2].Replace('\', '/')
  if ($fontPath -notmatch ('^fonts/' + [regex]::Escape($family) + '/[^/]+$')) {
    throw "Font and notice must belong to the same family directory: $fontPath / $relative"
  }
  $notice = Join-Path $rootPath $relative
  if (-not (Test-Path -LiteralPath $notice -PathType Leaf) -or (Get-Item -LiteralPath $notice).Length -eq 0) {
    throw "Missing or empty OFL notice for $($fields[0]): $notice"
  }
  return @{ Family = $family; Notice = Get-Item -LiteralPath $notice }
}

# Validate every declared native font notice before either Windows package is created.
# Checking all declared faces is deliberately conservative: size-capped faces may not be embedded.
function Get-CraftFontNotices([string] $FontRoot, [bool] $RequireChinese = $false) {
  $rootPath = (Resolve-Path -LiteralPath $FontRoot).Path
  $notices = @{}
  foreach ($name in 'manifest.txt', 'simplified-chinese-manifest.txt') {
    $manifest = Join-Path $rootPath "fonts/$name"
    if (-not (Test-Path -LiteralPath $manifest -PathType Leaf)) {
      if ($name -eq 'manifest.txt' -or $RequireChinese) { throw "Missing font manifest: $manifest" }
      continue
    }
    foreach ($line in Get-Content -LiteralPath $manifest) {
      $line = $line.Trim()
      if (-not $line -or $line.StartsWith('#')) { continue }
      $entry = Get-CraftFontNoticeRow $rootPath $manifest $line
      $notices[$entry.Family] = $entry.Notice
    }
  }
  if ($notices.Count -eq 0) { throw "No declared font notices in $rootPath" }
  foreach ($family in @($notices.Keys | Sort-Object)) { $notices[$family] }
}

# MSI FileName may be short|long. Compare the long names with the staged inventory.
function Assert-CraftFontNoticeNames([string[]] $Expected, [string[]] $Actual) {
  $names = @($Actual | ForEach-Object { ($_ -split '\|')[-1] })
  foreach ($name in $Expected) {
    if ($name -notin $names) { throw "The MSI is missing staged font notice: $name" }
  }
}
