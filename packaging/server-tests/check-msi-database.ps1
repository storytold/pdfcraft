# Windows-only database regression. Creates a tiny synthetic MSI database, never
# opens an installation session and never runs msiexec, a service or native UI.
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/common.ps1"
$work = Join-Path ([IO.Path]::GetTempPath()) ('pdfcraft-msi-database-' + [guid]::NewGuid().ToString('N'))
[void] (New-Item -ItemType Directory -Path $work)
$original = Join-Path $work 'original.msi'
$copy = Join-Path $work 'upgrade.msi'
$initialCode = '{A9C59D9B-69EB-4AB0-BEA5-419C001B785E}'
try {
    $installer = $database = $summary = $null
    try {
        $installer = New-Object -ComObject WindowsInstaller.Installer
        $database = $installer.OpenDatabase($original, 3)
        foreach ($sql in @(
            'CREATE TABLE `Property` (`Property` CHAR(72) NOT NULL, `Value` CHAR(0) LOCALIZABLE PRIMARY KEY `Property`)',
            ('INSERT INTO `Property` (`Property`, `Value`) VALUES (''ProductCode'', ''' + $initialCode + ''')')
        )) {
            $view = $database.OpenView($sql)
            try { [void] $view.Execute() }
            finally {
                [void] $view.Close()
                [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($view)
            }
        }
        $summary = $database.SummaryInformation(20)
        $summary.Property(9) = '{1F634BD3-2DCA-45FC-9A8F-0160A1A9D1BD}'
        [void] $summary.Persist()
        [void] $database.Commit()
    } finally {
        foreach ($item in @($summary, $database, $installer)) {
            if ($null -ne $item) { [void] [Runtime.InteropServices.Marshal]::FinalReleaseComObject($item) }
        }
    }
    $originalHash = (Get-FileHash -LiteralPath $original).Hash
    $read = Get-ProductCode $original
    Assert-That ($read -is [string] -and $read -ceq $initialCode) 'ProductCode reader must emit one GUID string only.'
    $code = New-UpgradeFixture $original $copy
    Assert-That ($code -is [string]) 'Upgrade helper emitted COM results alongside ProductCode.'
    Assert-ProductCode $code
    Assert-That ($code -cne $initialCode) 'Upgrade fixture did not change ProductCode.'
    # No GC workaround: the helper must release all COM owners before returning.
    # This reproduces the storage-sharing error which made real msiexec return 1619.
    $stream = [IO.File]::Open($copy, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    $stream.Dispose()
    Assert-That ((Get-ProductCode $copy) -ceq $code) 'Persisted upgrade ProductCode differs from the returned GUID.'
    foreach ($path in @($original, $copy)) {
        $stream = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
        $stream.Dispose()
    }
    Assert-That ((Get-FileHash -LiteralPath $original).Hash -ceq $originalHash) 'Upgrade helper changed its source MSI.'
    Write-Output 'ok: synthetic MSI identity, scalar return, committed changes and released storage locks; no installation'
} finally {
    [void] (Assert-SafePath $work ([IO.Path]::GetTempPath()))
    Remove-Item -LiteralPath $work -Recurse -Force
}
