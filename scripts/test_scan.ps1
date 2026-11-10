# Script de prueba para verificar la funcionalidad de escaneo integrada en PdfCraft
Write-Host "=============================================" -ForegroundColor Cyan
Write-Host "   Prueba de escaneo para PdfCraft (WIA)" -ForegroundColor Cyan
Write-Host "=============================================" -ForegroundColor Cyan

# 1. Detectar escáneres
Write-Host "`n1. Detectando escáneres conectados..." -ForegroundColor Yellow
$mgr = New-Object -ComObject WIA.DeviceManager
$scanners = @()
foreach ($info in $mgr.DeviceInfos) {
    if ($info.Type -eq 1) {
        $name = $info.DeviceID
        foreach ($p in $info.Properties) {
            if ($p.Name -eq 'Name') { $name = $p.Value; break }
        }
        $scanners += [PSCustomObject]@{
            ID = $info.DeviceID
            Name = $name
        }
    }
}

if ($scanners.Count -eq 0) {
    Write-Host "❌ No se encontraron escáneres disponibles." -ForegroundColor Red
    exit 1
}

Write-Host " Escáneres encontrados:" -ForegroundColor Green
for ($i = 0; $i -lt $scanners.Count; $i++) {
    Write-Host "   [$i] $($scanners[$i].Name)" -ForegroundColor White
}

# 2. Iniciar adquisición con diálogo nativo de Windows (WIA.CommonDialog)
Write-Host "`n2. Abriendo interfaz de escaneo..." -ForegroundColor Yellow
$dialog = New-Object -ComObject WIA.CommonDialog
$formatPng = "{B96B3CAF-0728-11D3-9D7B-0000F81EF32E}"
$outPath = Join-Path (Get-Location) "escaneo_prueba.png"

if (Test-Path $outPath) {
    Remove-Item $outPath -Force
}

try {
    # ShowAcquireImage(DeviceType: 1=Scanner, Intent: 0=Unspecified, Bias: 131072=MaximizeQuality, FormatID: PNG, AlwaysSelectDevice: false, UseCommonUI: true, CancelError: false)
    $image = $dialog.ShowAcquireImage(1, 0, 131072, $formatPng, $false, $true, $false)
    if ($image -eq $null) {
        Write-Host "ℹ️ Escaneo cancelado por el usuario." -ForegroundColor Yellow
        exit 0
    }
    $image.SaveFile($outPath)
    Write-Host " Escaneo completado con éxito." -ForegroundColor Green
    Write-Host " Archivo generado: $outPath" -ForegroundColor Cyan
} catch {
    Write-Host "❌ Error durante el escaneo: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}

