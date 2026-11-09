//! Scanner discovery and image acquisition via platform scanner services:
//! - Windows: Windows Image Acquisition (WIA) via system automation
//! - Linux: SANE (scanimage)

use std::fmt;

/// Scanned image color mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// A discovered physical or network scanner device.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ScannerDevice {
    pub id: String,
    pub name: String,
}

/// Options configured for a scan acquisition session.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ScanOptions {
    pub device_id: Option<String>,
    pub color_mode: ScanColorMode,
    pub resolution: ScanResolution,
    pub use_native_ui: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            device_id: None,
            color_mode: ScanColorMode::Color,
            resolution: ScanResolution::Dpi300,
            use_native_ui: cfg!(target_os = "windows"),
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
pub fn acquire_scan(options: &ScanOptions) -> Result<Vec<u8>, ScanError> {
    let temp_dir = std::env::temp_dir();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let out_file = temp_dir.join(format!("pdfcraft_scan_{}_{}.png", std::process::id(), nonce));
    let out_path_str = out_file.to_string_lossy().to_string();

    let intent_num = match options.color_mode {
        ScanColorMode::Color => 1,
        ScanColorMode::Grayscale => 2,
        ScanColorMode::BlackAndWhite => 4,
    };
    let dpi_num = options.resolution.as_dpi();

    let script = if options.use_native_ui {
        format!(
            r#"
$ErrorActionPreference = 'Stop'
try {{
    $dialog = New-Object -ComObject WIA.CommonDialog
    $image = $dialog.ShowAcquireImage(1, {intent}, 131072, '{{B96B3CAF-0728-11D3-9D7B-0000F81EF32E}}', $false, $true, $false)
    if ($image -eq $null) {{
        Write-Output "CANCELLED"
        exit 0
    }}
    $image.SaveFile('{out_path}')
    Write-Output "OK"
}} catch {{
    Write-Error $_.Exception.Message
    exit 1
}}
"#,
            intent = intent_num,
            out_path = out_path_str.replace('\'', "''"),
        )
    } else {
        let dev_filter = match &options.device_id {
            Some(id) if !id.is_empty() => format!("$targetId = '{}'", id.replace('\'', "''")),
            _ => "$targetId = $null".to_string(),
        };
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
    $item = $dev.Items.Item(1)
    foreach ($p in $item.Properties) {{
        if ($p.Name -eq 'Horizontal Resolution') {{ $p.Value = {dpi} }}
        if ($p.Name -eq 'Vertical Resolution') {{ $p.Value = {dpi} }}
        if ($p.Name -eq 'Current Intent') {{ $p.Value = {intent} }}
    }}
    $image = $item.Transfer('{{B96B3CAB-0728-11D3-9D7B-0000F81EF32E}}')
    if ($image -eq $null) {{
        Write-Output "CANCELLED"
        exit 0
    }}
    $tempBmp = [System.IO.Path]::GetTempFileName() + '.bmp'
    $image.SaveFile($tempBmp)
    Add-Type -AssemblyName System.Drawing
    $bmp = [System.Drawing.Bitmap]::FromFile($tempBmp)
    $bmp.Save('{out_path}', [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Remove-Item $tempBmp -Force
    Write-Output "OK"
}} catch {{
    Write-Error $_.Exception.Message
    exit 1
}}
"#,
            dev_filter = dev_filter,
            dpi = dpi_num,
            intent = intent_num,
            out_path = out_path_str.replace('\'', "''"),
        )
    };

    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|e| ScanError::AcquisitionFailed(format!("Failed to execute scanner process: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if stdout.contains("CANCELLED") {
        let _ = std::fs::remove_file(&out_file);
        return Err(ScanError::Cancelled);
    }

    if !output.status.success() {
        let _ = std::fs::remove_file(&out_file);
        if output.status.code() == Some(2) {
            return Err(ScanError::NoScannerFound);
        }
        return Err(ScanError::AcquisitionFailed(format!(
            "Scanner error: {}",
            stderr.trim()
        )));
    }

    if !out_file.exists() {
        return Err(ScanError::AcquisitionFailed("No scan file was produced".into()));
    }

    let bytes = std::fs::read(&out_file)
        .map_err(|e| ScanError::AcquisitionFailed(format!("Failed to read scan output: {e}")))?;
    let _ = std::fs::remove_file(&out_file);

    Ok(bytes)
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

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn list_scanners() -> Result<Vec<ScannerDevice>, ScanError> {
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
        assert!(opt.device_id.is_none());
    }

    #[test]
    fn test_scan_error_display() {
        let err = ScanError::NoScannerFound;
        assert!(err.to_string().contains("No scanner device"));
        let err_cancel = ScanError::Cancelled;
        assert!(err_cancel.to_string().contains("cancelled"));
    }
}

