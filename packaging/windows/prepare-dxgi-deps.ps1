#Requires -Version 7.2
[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Join-Path $PSScriptRoot '../..'),
    [string]$CargoHome = $(if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path ([Environment]::GetFolderPath('UserProfile')) '.cargo' }),
    [string]$OutputDirectory,
    [switch]$VerifyGraph
)

$ErrorActionPreference = 'Stop'
$repository = (Resolve-Path -LiteralPath $RepositoryRoot).Path
$lockPath = Join-Path $repository 'Cargo.lock'
$lockBefore = (Get-FileHash -LiteralPath $lockPath -Algorithm SHA256).Hash.ToLowerInvariant()
$rootManifest = Join-Path $repository 'Cargo.toml'
$rootBefore = (Get-FileHash -LiteralPath $rootManifest -Algorithm SHA256).Hash.ToLowerInvariant()
$utf8 = [Text.UTF8Encoding]::new($false)

function File-Hash([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Text-Hash([string]$Text) {
    [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($utf8.GetBytes($Text))).ToLowerInvariant()
}

function Find-PinnedCheckout([string]$Prefix, [string]$Revision) {
    $checkouts = Join-Path $CargoHome 'git/checkouts'
    foreach ($group in Get-ChildItem -LiteralPath $checkouts -Directory -Filter "$Prefix-*") {
        foreach ($candidate in Get-ChildItem -LiteralPath $group.FullName -Directory) {
            $actual = & git -C $candidate.FullName rev-parse HEAD 2>$null
            if ($LASTEXITCODE -eq 0 -and $actual -eq $Revision) {
                & git --no-optional-locks -C $candidate.FullName diff --quiet HEAD -- .
                if ($LASTEXITCODE -ne 0) { throw "Pinned checkout is modified: $($candidate.FullName)" }
                return $candidate.FullName
            }
        }
    }
    throw "Missing clean $Prefix checkout at $Revision; fetch the repository's locked dependencies first."
}

function Copy-Tracked([string]$Source, [string]$Destination, [string[]]$Paths) {
    New-Item -ItemType Directory -Path $Destination | Out-Null
    $files = & git -C $Source ls-tree -r --name-only HEAD -- @Paths
    if ($LASTEXITCODE -ne 0) { throw "Cannot list pinned source files: $Source" }
    foreach ($relative in $files) {
        $target = [IO.Path]::GetFullPath((Join-Path $Destination $relative))
        if (-not $target.StartsWith($Destination + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Source filename escaped its dependency directory.'
        }
        $parent = Split-Path -Parent $target
        if (-not (Test-Path -LiteralPath $parent)) { New-Item -ItemType Directory -Path $parent | Out-Null }
        Copy-Item -LiteralPath (Join-Path $Source $relative) -Destination $target
    }
}

# Filled from reviewed patches. All before/after hashes describe UTF-8 with LF newlines.
$patchSpecifications = @'
[
  {
    "library": "craft-print",
    "revision": "e2dc681a3ad8a47038da2f11ddd2c5e1c0454991",
    "checkout_prefix": "craft-libs",
    "copy_directory": "craft-libs",
    "package_path": "crates/print",
    "copy_paths": [
      "Cargo.toml",
      "LICENSE-MIT",
      "LICENSE-APACHE",
      "NOTICE",
      "crates/print"
    ],
    "manifests": [
      "Cargo.toml",
      "crates/print/Cargo.toml"
    ],
    "patch": "craft-print-dxgi-file-output.patch",
    "patch_sha256": "caf8f94a111314d710fb8b96d08b3a89ba61b15a558abcb0cd48b58b2fdbbfa6",
    "files": [
      {
        "path": "crates/print/src/lib.rs",
        "before_sha256": "87d27223af9857cf377cff83d6584915c100671c5a9ec1c7c448f317bf7e20db",
        "after_sha256": "c3086cb21ed45f80b39fdf6c63ab25263f59cf92503eeb2e2fc099a89859d54b"
      },
      {
        "path": "crates/print/src/windows.rs",
        "before_sha256": "482911b8dc2cfa818b34d40bd363447fd73573484791c3b786dfaf0ea5d2dcb2",
        "after_sha256": "cafa91b390f8916689516757d902851e8d55cdf1c574ddb0b2b8bee0b64c817e"
      }
    ]
  },
  {
    "library": "winprint",
    "revision": "72951d7455ebcab569fbd9386c513d3d9f53fb02",
    "checkout_prefix": "winprint.rs",
    "copy_directory": "winprint",
    "package_path": ".",
    "copy_paths": [
      "Cargo.toml",
      "LICENSE.md",
      "README.md",
      "build.rs",
      "src"
    ],
    "manifests": [
      "Cargo.toml"
    ],
    "patch": "winprint-dxgi-file-output.patch",
    "patch_sha256": "245f8e26de363f15f7600e75f4ebf860dbe89fa63df9ccc921cd57067f461ed5",
    "files": [
      {
        "path": "src/printer/dxgi.rs",
        "before_sha256": "cb6fdd38072bebe37e9d920d4a4691bacfb88c3a4d5340515bb4061f9c16784e",
        "after_sha256": "8994a39d782dbeca85aa0110f8c19d47d6dd555f2e8fd510cc0767f5ad76ad15"
      },
      {
        "path": "src/printer/file_target.rs",
        "before_sha256": null,
        "after_sha256": "c6de7d3038cd0658b221fddd7a9bfc1810ec5f80b1aeb9a9ecfd2ed4ec69d011"
      },
      {
        "path": "src/printer/job_init.rs",
        "before_sha256": null,
        "after_sha256": "cd558a8797a6a57cb7f8b081932a23c2924d73af553aa0829d5c9725003df852"
      },
      {
        "path": "src/printer/mod.rs",
        "before_sha256": "89b387c3aa4bc1242abcab1d7d51e7ecbb8d23801839fb903c957206d4a2613d",
        "after_sha256": "6aca390a04e9512eff8312bbabadc7a2430ef2f2c12dcb679048a5908d1d2794"
      },
      {
        "path": "src/printer/win_pdf.rs",
        "before_sha256": "2cf67c1dd1ebe0962981a708e6e2c53a68736bb49de8edd126bc9a2f672eb41e",
        "after_sha256": "2a94db0ba72fea72b6b6e7cadc2c5873e63b551e6959af7ce015b06e60317747"
      }
    ]
  }
]
'@ | ConvertFrom-Json
$combined = ($patchSpecifications | ForEach-Object { $_.patch_sha256 }) -join '|'
$key = (Text-Hash ("source-specific-v4|" + $lockBefore + "|" + $combined)).Substring(0, 16)
$allowedRoot = [IO.Path]::GetFullPath((Join-Path $repository 'target/native-deps'))
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $allowedRoot "dxgi-$key" }
$output = [IO.Path]::GetFullPath($OutputDirectory)
if (-not $output.StartsWith($allowedRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Dependency preparation must stay inside this checkout target/native-deps directory.'
}
$reportPath = Join-Path $output 'preparation.json'
$configPath = Join-Path $output 'override.toml'

foreach ($spec in $patchSpecifications) {
    $patch = Join-Path $PSScriptRoot ("patches/" + $spec.patch)
    if ((File-Hash $patch) -ne $spec.patch_sha256) { throw "Reviewed patch hash mismatch: $patch" }
}

if (Test-Path -LiteralPath $output) {
    if (-not (Test-Path -LiteralPath $reportPath)) { throw "Incomplete dependency preparation exists; preserve/inspect it before selecting a new output directory: $output" }
    $report = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
    if ($report.key -ne $key -or $report.lock_sha256 -ne $lockBefore) { throw 'Prepared dependencies belong to another patch/lock snapshot.' }
    foreach ($file in $report.files) {
        if ((File-Hash (Join-Path $output $file.path)) -ne $file.sha256) { throw "Prepared dependency changed: $($file.path)" }
    }
    if ((File-Hash $configPath) -ne $report.config_sha256) { throw 'Prepared Cargo override configuration changed.' }
} else {
    New-Item -ItemType Directory -Path $output | Out-Null
    foreach ($spec in $patchSpecifications) {
        $source = Find-PinnedCheckout $spec.checkout_prefix $spec.revision
        $copy = Join-Path $output $spec.copy_directory
        if ($spec.library -eq 'winprint') { $copy = Join-Path $copy 'winprint-0.2.1' }
        Copy-Tracked $source $copy @($spec.copy_paths)
        # Manifests are copied verbatim: no dependency, feature, version or source-id changes.
        foreach ($manifest in $spec.manifests) {
            if ((File-Hash (Join-Path $source $manifest)) -ne (File-Hash (Join-Path $copy $manifest))) { throw "Dependency manifest copy differs: $manifest" }
        }
        foreach ($file in $spec.files) {
            $target = Join-Path $copy $file.path
            if ($null -eq $file.before_sha256) {
                if (Test-Path -LiteralPath $target) { throw "New upstream file unexpectedly exists: $($file.path)" }
            } else {
                $text = [IO.File]::ReadAllText($target).Replace("`r`n", "`n")
                if ((Text-Hash $text) -ne $file.before_sha256) { throw "Pinned source hash mismatch: $($file.path)" }
                [IO.File]::WriteAllText($target, $text, $utf8)
            }
        }
        $patch = Join-Path $PSScriptRoot ("patches/" + $spec.patch)
        # Apply working files from the generated copy, without touching any Git index.
        # Before/after file hashes independently verify every patched path.
        Push-Location $copy
        try {
            & git apply --no-index --check $patch
            if ($LASTEXITCODE -ne 0) { throw "Upstream patch does not apply: $($spec.patch)" }
            & git apply --no-index $patch
            if ($LASTEXITCODE -ne 0) { throw "Upstream patch failed: $($spec.patch)" }
        } finally { Pop-Location }
        foreach ($file in $spec.files) {
            if ((File-Hash (Join-Path $copy $file.path)) -ne $file.after_sha256) { throw "Patched source hash mismatch: $($file.path)" }
        }
        foreach ($manifest in $spec.manifests) {
            if ((File-Hash (Join-Path $source $manifest)) -ne (File-Hash (Join-Path $copy $manifest))) { throw "Patch changed a dependency manifest: $manifest" }
        }
        if ($spec.library -eq 'craft-print') {
            # Cargo path overrides inside the app otherwise inherit its workspace version
            # Anchor this private copy to the copied upstream workspace (0.1.0).
            # Dependencies/features/version declarations themselves stay exactly as pinned.
            $packageManifest = Join-Path $copy 'crates/print/Cargo.toml'
            $manifestText = [IO.File]::ReadAllText($packageManifest).Replace("`r`n", "`n")
            if (-not $manifestText.StartsWith("[package]`n")) { throw 'Unexpected craft-print package manifest.' }
            $manifestText = $manifestText.Replace("[package]`n", "[package]`nworkspace = `"../..`"`n")
            [IO.File]::WriteAllText($packageManifest, $manifestText, $utf8)
        }
    }
    $printSpec = $patchSpecifications | Where-Object library -EQ 'craft-print'
    $winprintSpec = $patchSpecifications | Where-Object library -EQ 'winprint'
    $winprintSource = Join-Path $output $winprintSpec.copy_directory
    $package = Join-Path $winprintSource 'winprint-0.2.1'
    $checksums = [ordered]@{}
    foreach ($f in Get-ChildItem -LiteralPath $package -Recurse -File | Sort-Object FullName) {
        $rel = [IO.Path]::GetRelativePath($package, $f.FullName).Replace('\', '/')
        $checksums[$rel] = File-Hash $f.FullName
    }
    $packageChecksum = [ordered]@{ files = $checksums; package = $null }
    [IO.File]::WriteAllText((Join-Path $package '.cargo-checksum.json'), (ConvertTo-Json $packageChecksum -Depth 4), $utf8)

    $printPath = Join-Path (Join-Path $output $printSpec.copy_directory) $printSpec.package_path
    $quotedPrint = ConvertTo-Json -InputObject $printPath.Replace('\', '/') -Compress
    $quotedWinprint = ConvertTo-Json -InputObject $winprintSource.Replace('\', '/') -Compress
    # A name-based winprint override would also replace the independent, older winprint
    # revision used by the app. Scope the source replacement to the print crate's revision.
    $config = @"
paths = [$quotedPrint]

[source.pdfcraft-winprint-dxgi]
git = "https://github.com/storytold/winprint.rs"
rev = "$($winprintSpec.revision)"
replace-with = "pdfcraft-winprint-dxgi-local"

[source.pdfcraft-winprint-dxgi-local]
directory = $quotedWinprint
"@
    [IO.File]::WriteAllText($configPath, $config, $utf8)
    $files = @(Get-ChildItem -LiteralPath $output -Recurse -File | Where-Object FullName -NE $reportPath | ForEach-Object {
        [ordered]@{ path = [IO.Path]::GetRelativePath($output, $_.FullName); sha256 = File-Hash $_.FullName }
    })
    $report = [ordered]@{ key = $key; lock_sha256 = $lockBefore; config_sha256 = File-Hash $configPath; files = $files; graph_verified = $false; build_or_runtime_verified = $false }
    [IO.File]::WriteAllText($reportPath, ($report | ConvertTo-Json -Depth 8), $utf8)
}

if ($VerifyGraph) {
    Push-Location $repository
    try {
        $treeArgs = @('tree', '--locked', '--offline', '--target', 'x86_64-pc-windows-msvc', '-p', 'pdfcraft', '-p', 'pdfcraft-cli', '--edges', 'normal', '--prefix', 'depth', '--format', '{p} [{f}]', '--no-dedupe')
        & cargo @treeArgs 1> (Join-Path $output 'tree-stock.txt') 2> (Join-Path $output 'tree-stock.stderr.txt')
        if ($LASTEXITCODE -ne 0) { throw 'Stock normal dependency graph probe failed; see retained logs.' }
        & cargo @treeArgs --config $configPath 1> (Join-Path $output 'tree-prepared.txt') 2> (Join-Path $output 'tree-prepared.stderr.txt')
        if ($LASTEXITCODE -ne 0) { throw 'Prepared normal dependency graph probe failed; see retained logs.' }
        # Keep source IDs for every package except the private craft-print path override.
        $normalize = { process {
            if ($_ -match '^(\d+)([A-Za-z0-9_-]+) v([^ ]+)(.*?) \[([^\]]*)\]$') {
                $sourceId = if ($Matches[2] -eq 'craft-print') { '' } else { $Matches[4] }
                "$($Matches[1])|$($Matches[2])|$($Matches[3])|$sourceId|$($Matches[5])"
            }
        } }
        $stock = @(Get-Content -LiteralPath (Join-Path $output 'tree-stock.txt') | & $normalize)
        $prepared = @(Get-Content -LiteralPath (Join-Path $output 'tree-prepared.txt') | & $normalize)
        if ($stock.Count -eq 0 -or ($stock -join "`n") -cne ($prepared -join "`n")) { throw 'Prepared normal dependency graph differs from the locked stock graph.' }
        $effective = Get-Content -LiteralPath (Join-Path $output 'tree-prepared.txt') -Raw
        $printSpec = $patchSpecifications | Where-Object library -EQ 'craft-print'
        $expected = Join-Path (Join-Path $output $printSpec.copy_directory) $printSpec.package_path
        if (-not $effective.Contains($expected)) { throw 'Cargo did not select prepared craft-print.' }
    } finally { Pop-Location }
    $report = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
    $report | Add-Member -NotePropertyName normal_graph_verified -NotePropertyValue $true -Force
    $report | Add-Member -NotePropertyName normal_graph_rows -NotePropertyValue $stock.Count -Force
    [IO.File]::WriteAllText($reportPath, ($report | ConvertTo-Json -Depth 8), $utf8)
    Write-Warning 'Normal dependency edges were compared; full build edges, feature-enabled compilation and runtime still require separate verification. Cargo path overrides may emit their own dependency-list warning.'
}
if ((File-Hash $lockPath) -ne $lockBefore -or (File-Hash $rootManifest) -ne $rootBefore) { throw 'Dependency preparation changed the app lockfile or workspace manifest.' }
[pscustomobject]@{ ConfigPath = $configPath; PreparationPath = $reportPath; LockSha256 = $lockBefore; Feature = 'pdfcraft-print/dxgi-file-output'; NormalGraphVerified = [bool]$VerifyGraph; BuildOrRuntimeVerified = $false }
