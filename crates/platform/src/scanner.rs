//! Scanner discovery and image acquisition via platform scanner services:
//! - Windows: Windows Image Acquisition (WIA) via system automation
//! - Linux: SANE (scanimage)

use std::fmt;

/// Scanned image color mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScanColorMode {
    #[default]
    Color,
    Grayscale,
    BlackAndWhite,
}

impl ScanColorMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Color => "Color",
            Self::Grayscale => "Grayscale",
            Self::BlackAndWhite => "Black & White",
        }
    }
}

/// Scanning DPI resolution presets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScanResolution {
    Dpi75,
    Dpi150,
    #[default]
    Dpi300,
    Dpi600,
}

impl ScanResolution {
    pub fn as_dpi(self) -> u32 {
        match self {
            Self::Dpi75 => 75,
            Self::Dpi150 => 150,
            Self::Dpi300 => 300,
            Self::Dpi600 => 600,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Dpi75 => "75 DPI (Draft)",
            Self::Dpi150 => "150 DPI (Fast)",
            Self::Dpi300 => "300 DPI (Standard)",
            Self::Dpi600 => "600 DPI (High Quality)",
        }
    }
}

/// Paper source for scanning.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScanSource {
    #[default]
    Auto,
    Flatbed,
    Feeder,
}

impl ScanSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Flatbed => "Flatbed",
            Self::Feeder => "Feeder (ADF)",
        }
    }
}

/// Sides to scan (Simplex vs Duplex).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScanDuplex {
    #[default]
    Simplex,
    Duplex,
}

impl ScanDuplex {
    pub fn label(self) -> &'static str {
        match self {
            Self::Simplex => "1-sided",
            Self::Duplex => "2-sided (Duplex)",
        }
    }
}

/// Paper size preset for scanner acquisition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScanPageSize {
    #[default]
    A4,
    Letter,
    Legal,
    A5,
    Auto,
}

impl ScanPageSize {
    pub fn label(self) -> &'static str {
        match self {
            Self::A4 => "A4 (210 × 297 mm)",
            Self::Letter => "Letter (8.5 × 11 in)",
            Self::Legal => "Legal (8.5 × 14 in)",
            Self::A5 => "A5 (148 × 210 mm)",
            Self::Auto => "Auto / Max",
        }
    }

    /// Extents in pixels (width, height) at the given DPI.
    /// Returns None for Auto (scanner default / unconstrained).
    pub fn extents_px(self, dpi: u32) -> Option<(u32, u32)> {
        let dpi_f = dpi as f64;
        match self {
            Self::A4 => {
                let w = ((210.0 / 25.4) * dpi_f).round() as u32;
                let h = ((297.0 / 25.4) * dpi_f).round() as u32;
                Some((w, h))
            }
            Self::Letter => {
                let w = (8.5 * dpi_f).round() as u32;
                let h = (11.0 * dpi_f).round() as u32;
                Some((w, h))
            }
            Self::Legal => {
                let w = (8.5 * dpi_f).round() as u32;
                let h = (14.0 * dpi_f).round() as u32;
                Some((w, h))
            }
            Self::A5 => {
                let w = ((148.0 / 25.4) * dpi_f).round() as u32;
                let h = ((210.0 / 25.4) * dpi_f).round() as u32;
                Some((w, h))
            }
            Self::Auto => None,
        }
    }

    /// Millimeter dimensions (width, height) for SANE.
    /// Returns None for Auto.
    pub fn dimensions_mm(self) -> Option<(f64, f64)> {
        match self {
            Self::A4 => Some((210.0, 297.0)),
            Self::Letter => Some((215.9, 279.4)),
            Self::Legal => Some((215.9, 355.6)),
            Self::A5 => Some((148.0, 210.0)),
            Self::Auto => None,
        }
    }
}

/// A discovered physical or network scanner device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScannerDevice {
    pub id: String,
    pub name: String,
}

/// Options configured for a scan acquisition session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanOptions {
    pub device_id: Option<String>,
    pub color_mode: ScanColorMode,
    pub resolution: ScanResolution,
    pub page_size: ScanPageSize,
    pub source: ScanSource,
    pub duplex: ScanDuplex,
    pub all_pages: bool,
    pub use_native_ui: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            device_id: None,
            color_mode: ScanColorMode::Color,
            resolution: ScanResolution::Dpi300,
            page_size: ScanPageSize::A4,
            source: ScanSource::Auto,
            duplex: ScanDuplex::Simplex,
            all_pages: true,
            use_native_ui: false,
        }
    }
}

/// Errors that can occur during scanner discovery or acquisition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanError {
    NoScannerFound,
    Cancelled,
    DeviceBusy(String),
    AcquisitionFailed(String),
    UnsupportedPlatform,
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoScannerFound => write!(f, "No scanner device was found on this system."),
            Self::Cancelled => write!(f, "The scan operation was cancelled."),
            Self::DeviceBusy(msg) => write!(f, "The scanner is busy or not responding: {msg}"),
            Self::AcquisitionFailed(msg) => write!(f, "Scanner acquisition failed: {msg}"),
            Self::UnsupportedPlatform => write!(f, "Scanner hardware acquisition is not supported on this platform."),
        }
    }
}

impl std::error::Error for ScanError {}

#[cfg(target_os = "windows")]
pub fn list_scanners() -> Result<Vec<ScannerDevice>, ScanError> {
    let script = r#"
$ErrorActionPreference = 'Stop'
try {
    $mgr = New-Object -ComObject WIA.DeviceManager
    $devices = @()
    foreach ($info in $mgr.DeviceInfos) {
        if ($info.Type -eq 1) {
            $name = $info.DeviceID
            foreach ($p in $info.Properties) {
                if ($p.Name -eq 'Name') { $name = $p.Value; break }
            }
            $devices += "$($info.DeviceID)|$name"
        }
    }
    $devices -join "`n"
} catch {
    exit 1
}
"#;
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .map_err(|e| ScanError::AcquisitionFailed(format!("Failed to execute PowerShell: {e}")))?;

    if !output.status.success() {
        return Err(ScanError::AcquisitionFailed("Failed to query WIA DeviceManager".into()));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut scanners = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((id, name)) = trimmed.split_once('|') {
            scanners.push(ScannerDevice {
                id: id.trim().to_string(),
                name: name.trim().to_string(),
            });
        }
    }

    Ok(scanners)
}

#[cfg(target_os = "windows")]
pub fn acquire_scan_pages(options: &ScanOptions) -> Result<Vec<Vec<u8>>, ScanError> {
    let temp_dir = std::env::temp_dir();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let out_dir = temp_dir.join(format!("pdfcraft_scan_batch_{}_{}", std::process::id(), nonce));
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| ScanError::AcquisitionFailed(format!("Failed to create temp batch directory: {e}")))?;
    let out_dir_str = out_dir.to_string_lossy().to_string();

    let intent_num = match options.color_mode {
        ScanColorMode::Color => 1,
        ScanColorMode::Grayscale => 2,
        ScanColorMode::BlackAndWhite => 4,
    };
    let dpi_num = options.resolution.as_dpi();
    let (x_extent, y_extent) = options
        .page_size
        .extents_px(dpi_num)
        .unwrap_or((0, 0));

    let script = if options.use_native_ui {
        format!(
            r#"
$ErrorActionPreference = 'Stop'
try {{
    $dialog = New-Object -ComObject WIA.CommonDialog
    $image = $dialog.ShowAcquireImage(1, {intent}, 131072, '{{B96B3CAB-0728-11D3-9D7B-0000F81EF32E}}', $false, $true, $false)
    if ($image -eq $null) {{
        Write-Output "CANCELLED"
        exit 0
    }}
    $tempBmp = [System.IO.Path]::GetTempFileName() + '.bmp'
    $image.SaveFile($tempBmp)
    Add-Type -AssemblyName System.Drawing
    $bmp = [System.Drawing.Bitmap]::FromFile($tempBmp)
    $outPng = Join-Path '{out_dir}' "page_0001.png"
    $bmp.Save($outPng, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Remove-Item $tempBmp -Force
    Write-Output "OK:1"
}} catch {{
    Write-Error $_.Exception.Message
    exit 1
}}
"#,
            intent = intent_num,
            out_dir = out_dir_str.replace('\'', "''"),
        )
    } else {
        let dev_filter = match &options.device_id {
            Some(id) if !id.is_empty() => format!("$targetId = '{}'", id.replace('\'', "''")),
            _ => "$targetId = $null".to_string(),
        };

        let handling_select = match (options.source, options.duplex) {
            (_, ScanDuplex::Duplex) => 5,
            (ScanSource::Feeder, _) => 1,
            (ScanSource::Flatbed, _) => 2,
            (ScanSource::Auto, _) => 0,
        };

        let max_pages = if options.duplex == ScanDuplex::Duplex { 2 } else { 1 };
        let all_pages_val = if options.all_pages { "$true" } else { "$false" };

        format!(
            r#"
$ErrorActionPreference = 'Stop'
try {{
    {dev_filter}
    $mgr = New-Object -ComObject WIA.DeviceManager
    $dev = $null
    if ($targetId) {{
        foreach ($d in $mgr.DeviceInfos) {{
            if ($d.DeviceID -eq $targetId) {{
                $dev = $d.Connect()
                break
            }}
        }}
    }}
    if ($dev -eq $null) {{
        foreach ($d in $mgr.DeviceInfos) {{
            if ($d.Type -eq 1) {{
                $dev = $d.Connect()
                break
            }}
        }}
    }}
    if ($dev -eq $null) {{
        Write-Error "No scanner device found"
        exit 2
    }}

    $handlingSelect = {handling_select}
    if ($handlingSelect -gt 0) {{
        foreach ($p in $dev.Properties) {{
            if ($p.PropertyID -eq 3088) {{
                try {{ $p.Value = $handlingSelect }} catch {{
                    try {{ $p.Value = 5 }} catch {{}}
                }}
            }}
            if ($p.PropertyID -eq 3096) {{
                try {{ $p.Value = 1 }} catch {{}}
            }}
        }}
    }}

    $item = $dev.Items.Item(1)
    foreach ($p in $item.Properties) {{
        if ($p.Name -eq 'Horizontal Resolution') {{ try {{ $p.Value = {dpi} }} catch {{}} }}
        if ($p.Name -eq 'Vertical Resolution') {{ try {{ $p.Value = {dpi} }} catch {{}} }}
        if ($p.Name -eq 'Current Intent') {{ try {{ $p.Value = {intent} }} catch {{}} }}
    }}

    $xExtent = {x_extent}
    $yExtent = {y_extent}
    if ($xExtent -gt 0 -and $yExtent -gt 0) {{
        foreach ($p in $item.Properties) {{
            if ($p.PropertyID -eq 6149) {{ try {{ $p.Value = 0 }} catch {{}} }}
            if ($p.PropertyID -eq 6150) {{ try {{ $p.Value = 0 }} catch {{}} }}
            if ($p.PropertyID -eq 6151) {{ try {{ $p.Value = $xExtent }} catch {{}} }}
            if ($p.PropertyID -eq 6152) {{ try {{ $p.Value = $yExtent }} catch {{}} }}
        }}
    }}

    Add-Type -AssemblyName System.Drawing
    $pageIdx = 0
    $maxPages = {max_pages}
    $allPages = {all_pages_val}

    while ($true) {{
        $image = $null
        try {{
            $image = $item.Transfer('{{B96B3CAB-0728-11D3-9D7B-0000F81EF32E}}')
        }} catch {{
            if ($pageIdx -gt 0) {{
                break
            }}
            try {{
                $image = $item.Transfer()
            }} catch {{
                if ($pageIdx -gt 0) {{
                    break
                }}
                Write-Error $_.Exception.Message
                exit 1
            }}
        }}

        if ($image -eq $null) {{
            if ($pageIdx -eq 0) {{
                Write-Output "CANCELLED"
                exit 0
            }}
            break
        }}

        $pageIdx++
        $tempBmp = [System.IO.Path]::GetTempFileName() + '.bmp'
        $image.SaveFile($tempBmp)
        $outPng = Join-Path '{out_dir}' ("page_" + $pageIdx.ToString('0000') + ".png")
        $bmp = [System.Drawing.Bitmap]::FromFile($tempBmp)
        $bmp.Save($outPng, [System.Drawing.Imaging.ImageFormat]::Png)
        $bmp.Dispose()
        Remove-Item $tempBmp -Force

        if (-not $allPages -and ($pageIdx -ge $maxPages)) {{
            break
        }}
        if ($handlingSelect -eq 2) {{
            break
        }}
    }}
    Write-Output "OK:$pageIdx"
}} catch {{
    Write-Error $_.Exception.Message
    exit 1
}}
"#,
            dev_filter = dev_filter,
            handling_select = handling_select,
            dpi = dpi_num,
            intent = intent_num,
            x_extent = x_extent,
            y_extent = y_extent,
            max_pages = max_pages,
            all_pages_val = all_pages_val,
            out_dir = out_dir_str.replace('\'', "''"),
        )
    };

    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|e| ScanError::AcquisitionFailed(format!("Failed to execute scanner process: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if stdout.contains("CANCELLED") {
        let _ = std::fs::remove_dir_all(&out_dir);
        return Err(ScanError::Cancelled);
    }

    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&out_dir);
        if output.status.code() == Some(2) {
            return Err(ScanError::NoScannerFound);
        }
        return Err(ScanError::AcquisitionFailed(format!(
            "Scanner error: {}",
            stderr.trim()
        )));
    }

    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&out_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("png") {
                files.push(path);
            }
        }
    }
    files.sort();

    if files.is_empty() {
        let _ = std::fs::remove_dir_all(&out_dir);
        return Err(ScanError::AcquisitionFailed("No scan pages were produced".into()));
    }

    let mut pages = Vec::new();
    for f in &files {
        match std::fs::read(f) {
            Ok(bytes) => pages.push(bytes),
            Err(e) => {
                let _ = std::fs::remove_dir_all(&out_dir);
                return Err(ScanError::AcquisitionFailed(format!("Failed to read scan output {}: {e}", f.display())));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&out_dir);

    Ok(pages)
}

#[cfg(target_os = "windows")]
pub fn acquire_scan(options: &ScanOptions) -> Result<Vec<u8>, ScanError> {
    let pages = acquire_scan_pages(options)?;
    pages.into_iter().next().ok_or_else(|| ScanError::AcquisitionFailed("No scan pages were produced".into()))
}

#[cfg(target_os = "linux")]
pub fn list_scanners() -> Result<Vec<ScannerDevice>, ScanError> {
    let output = std::process::Command::new("scanimage")
        .arg("-L")
        .output()
        .map_err(|_| ScanError::NoScannerFound)?;

    if !output.status.success() {
        return Err(ScanError::NoScannerFound);
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut scanners = Vec::new();
    for line in text.lines() {
        if let Some(start) = line.find('`')
            && let Some(end) = line[start + 1..].find('\'')
        {
            let id = &line[start + 1..start + 1 + end];
            let name = if let Some(is_pos) = line.find("is a ") {
                line[is_pos + 5..].trim()
            } else {
                id
            };
            scanners.push(ScannerDevice {
                id: id.to_string(),
                name: name.to_string(),
            });
        }
    }
    Ok(scanners)
}

#[cfg(target_os = "linux")]
pub fn acquire_scan(options: &ScanOptions) -> Result<Vec<u8>, ScanError> {
    let temp_dir = std::env::temp_dir();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let out_file = temp_dir.join(format!("pdfcraft_scan_{}_{}.png", std::process::id(), nonce));

    let mut cmd = std::process::Command::new("scanimage");
    cmd.arg("--format=png");
    cmd.arg(format!("--resolution={}", options.resolution.as_dpi()));

    let mode_str = match options.color_mode {
        ScanColorMode::Color => "Color",
        ScanColorMode::Grayscale => "Gray",
        ScanColorMode::BlackAndWhite => "Lineart",
    };
    cmd.arg(format!("--mode={mode_str}"));

    if let Some(dev) = &options.device_id {
        cmd.arg(format!("--device={dev}"));
    }

    if let Some((w, h)) = options.page_size.dimensions_mm() {
        cmd.arg(format!("-x={w}"));
        cmd.arg(format!("-y={h}"));
    }

    let file = std::fs::File::create(&out_file)
        .map_err(|e| ScanError::AcquisitionFailed(format!("Failed to create temp file: {e}")))?;
    cmd.stdout(file);

    let status = cmd
        .status()
        .map_err(|e| ScanError::AcquisitionFailed(format!("Failed to run scanimage: {e}")))?;

    if !status.success() {
        let _ = std::fs::remove_file(&out_file);
        return Err(ScanError::AcquisitionFailed(format!("scanimage failed with status {status}")));
    }

    let bytes = std::fs::read(&out_file)
        .map_err(|e| ScanError::AcquisitionFailed(format!("Failed to read scan output: {e}")))?;
    let _ = std::fs::remove_file(&out_file);

    Ok(bytes)
}

#[cfg(target_os = "linux")]
pub fn acquire_scan_pages(options: &ScanOptions) -> Result<Vec<Vec<u8>>, ScanError> {
    let single = acquire_scan(options)?;
    Ok(vec![single])
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn list_scanners() -> Result<Vec<ScannerDevice>, ScanError> {
    Err(ScanError::UnsupportedPlatform)
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn acquire_scan_pages(_options: &ScanOptions) -> Result<Vec<Vec<u8>>, ScanError> {
    Err(ScanError::UnsupportedPlatform)
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn acquire_scan(_options: &ScanOptions) -> Result<Vec<u8>, ScanError> {
    Err(ScanError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_resolution_values() {
        assert_eq!(ScanResolution::Dpi75.as_dpi(), 75);
        assert_eq!(ScanResolution::Dpi150.as_dpi(), 150);
        assert_eq!(ScanResolution::Dpi300.as_dpi(), 300);
        assert_eq!(ScanResolution::Dpi600.as_dpi(), 600);
    }

    #[test]
    fn test_scan_options_default() {
        let opt = ScanOptions::default();
        assert_eq!(opt.resolution, ScanResolution::Dpi300);
        assert_eq!(opt.color_mode, ScanColorMode::Color);
        assert_eq!(opt.page_size, ScanPageSize::A4);
        assert!(opt.device_id.is_none());
    }

    #[test]
    fn test_scan_page_size_extents() {
        assert_eq!(ScanPageSize::Auto.extents_px(300), None);
        // A4 at 300 DPI: 2480 x 3508
        let (a4_w, a4_h) = ScanPageSize::A4.extents_px(300).expect("A4 extents");
        assert_eq!((a4_w, a4_h), (2480, 3508));
        // Letter at 300 DPI: 2550 x 3300
        let (let_w, let_h) = ScanPageSize::Letter.extents_px(300).expect("Letter extents");
        assert_eq!((let_w, let_h), (2550, 3300));
        // Legal at 300 DPI: 2550 x 4200
        let (leg_w, leg_h) = ScanPageSize::Legal.extents_px(300).expect("Legal extents");
        assert_eq!((leg_w, leg_h), (2550, 4200));
        // A5 at 300 DPI: 1748 x 2480
        let (a5_w, a5_h) = ScanPageSize::A5.extents_px(300).expect("A5 extents");
        assert_eq!((a5_w, a5_h), (1748, 2480));
    }

    #[test]
    fn test_scan_error_display() {
        let err = ScanError::NoScannerFound;
        assert!(err.to_string().contains("No scanner device"));
        let err_cancel = ScanError::Cancelled;
        assert!(err_cancel.to_string().contains("cancelled"));
    }
}

