param(
    [string]$OutFile = "escaneo_directo.png"
)

Write-Host "==============================================" -ForegroundColor Cyan
Write-Host "     PdfCraft - Escaneo Directo (WIA)         " -ForegroundColor Cyan
Write-Host "==============================================" -ForegroundColor Cyan

$fullOutPath = Join-Path (Get-Location) $OutFile
if (Test-Path $fullOutPath) {
    Remove-Item $fullOutPath -Force
}

$mgr = New-Object -ComObject WIA.DeviceManager
$dev = $null
foreach ($d in $mgr.DeviceInfos) {
    if ($d.Type -eq 1) {
        $dev = $d.Connect()
        break
    }
}

if ($dev -eq $null) {
    Write-Host "❌ No se encontró ningún escáner conectado." -ForegroundColor Red
    exit 1
}

$name = $dev.Properties.Item("Name").Value
Write-Host " Conectado a: $name" -ForegroundColor Green
Write-Host "⏱️ Iniciando escaneo en el hardware..." -ForegroundColor Yellow

try {
    $item = $dev.Items.Item(1)
    # Formato BMP nativo de transferencia WIA: {B96B3CAB-0728-11D3-9D7B-0000F81EF32E}
    $image = $item.Transfer("{B96B3CAB-0728-11D3-9D7B-0000F81EF32E}")
    if ($image -eq $null) {
        Write-Host "❌ El escáner no devolvió imagen." -ForegroundColor Red
        exit 1
    }
    
    $tempBmp = [System.IO.Path]::GetTempFileName() + ".bmp"
    $image.SaveFile($tempBmp)
    
    # Convertir a PNG estándar para guardarlo de forma óptima
    Add-Type -AssemblyName System.Drawing
    $bmp = [System.Drawing.Bitmap]::FromFile($tempBmp)
    $bmp.Save($fullOutPath, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Remove-Item $tempBmp -Force

    Write-Host " Escaneo finalizado con éxito!" -ForegroundColor Green
    Write-Host " Imagen guardada en: $fullOutPath" -ForegroundColor Cyan
    
    $fileInfo = Get-Item $fullOutPath
    Write-Host " Tamaño: $($fileInfo.Length) bytes" -ForegroundColor White
} catch {
    Write-Host "❌ Error durante la captura: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}

