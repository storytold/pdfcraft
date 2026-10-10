//! The system print spooler.
//!
//! - On **macOS and Linux**, printers are enumerated via CUPS (`lpstat -p -d`) and print jobs are
//!   submitted via `lp` with job options (copies, collation, duplex, grayscale, resolution, and
//!   `fit-to-page=false` because sheets are already imposed at their target paper size).
//! - On **Windows**, printers are enumerated from the Windows Print Spooler (`Win32_Printer` CIM/WMI
//!   with automatic fallback to `.NET` `System.Drawing.Printing.PrinterSettings::InstalledPrinters`
//!   and the Windows Registry `HKCU\Software\Microsoft\Windows NT\CurrentVersion\Devices`),
//!   detecting local, network, and virtual printers along with default-printer and offline/paused
//!   status. Print jobs are submitted to the Windows Print Spooler via `.NET`
//!   `System.Drawing.Printing.PrintDocument` using high-resolution (300–600 DPI) rendered sheets
//!   with exact physical point dimensions, orientation, copies, collation, duplex, and grayscale.

use crate::PrintError;

/// Default print rendering resolution in dots per inch (300 DPI).
pub const DEFAULT_PRINT_DPI: u32 = 300;
/// Minimum allowed print rendering resolution (72 DPI).
pub const MIN_PRINT_DPI: u32 = 72;
/// Maximum allowed print rendering resolution (600 DPI).
pub const MAX_PRINT_DPI: u32 = 600;

/// Operational status of an installed printer reported by the OS print spooler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PrinterStatus {
    #[default]
    Ready,
    Printing,
    Paused,
    Offline,
    Error,
}

impl PrinterStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Printing => "printing",
            Self::Paused => "paused",
            Self::Offline => "offline",
            Self::Error => "error",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Printing => "Printing",
            Self::Paused => "Paused",
            Self::Offline => "Offline",
            Self::Error => "Error",
        }
    }

    pub fn is_offline(self) -> bool {
        self == Self::Offline
    }

    pub fn is_available(self) -> bool {
        matches!(self, Self::Ready | Self::Printing)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Printer {
    pub name: String,
    pub default: bool,
}

/// Extended printer metadata including availability/offline status, port, and driver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrinterDetail {
    pub name: String,
    pub default: bool,
    pub status: PrinterStatus,
    pub port: String,
    pub driver: String,
    pub network: bool,
}

impl PrinterDetail {
    pub fn to_printer(&self) -> Printer {
        Printer { name: self.name.clone(), default: self.default }
    }
}

/// One print-ready sheet rasterized at high DPI for OS print drivers (such as Windows GDI/XPS).
#[derive(Clone, Debug, PartialEq)]
pub struct RenderedSheet {
    /// Sheet width in PDF points (1/72 inch).
    pub width_pt: f64,
    /// Sheet height in PDF points (1/72 inch).
    pub height_pt: f64,
    /// Rendered image width in pixels.
    pub width_px: u32,
    /// Rendered image height in pixels.
    pub height_px: u32,
    /// Dots per inch used to rasterize this sheet.
    pub dpi: u32,
    /// Lossless PNG bytes of the rendered sheet.
    pub png: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Duplex {
    #[default]
    Off,
    LongEdge,
    ShortEdge,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    /// `None` = the system default printer.
    pub printer: Option<String>,
    pub copies: u32,
    pub collate: bool,
    pub duplex: Duplex,
    pub grayscale: bool,
    pub title: String,
}

impl Default for Job {
    fn default() -> Self {
        Job { printer: None, copies: 1, collate: true, duplex: Duplex::Off, grayscale: false, title: "Linkco PDF Editor".into() }
    }
}

/// Convert premultiplied RGBA8 pixels in-place to perceptual grayscale (ITU-R BT.709).
pub fn apply_grayscale_rgba(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let r = u32::from(px[0]);
        let g = u32::from(px[1]);
        let b = u32::from(px[2]);
        let y = ((54 * r + 183 * g + 19 * b + 128) >> 8).min(255) as u8;
        px[0] = y;
        px[1] = y;
        px[2] = y;
    }
}

/// Parse `lpstat -p -d` output into detailed printer entries.
pub fn parse_lpstat_detailed(out: &str) -> Vec<PrinterDetail> {
    let default = out.lines().find_map(|l| l.strip_prefix("system default destination:")).map(|s| s.trim().to_string());
    out.lines()
        .filter_map(|l| l.strip_prefix("printer "))
        .filter_map(|l| {
            let name = l.split_whitespace().next()?;
            let lower = l.to_ascii_lowercase();
            let status = if lower.contains("offline") {
                PrinterStatus::Offline
            } else if lower.contains("disabled") || lower.contains("paused") {
                PrinterStatus::Paused
            } else if lower.contains("now printing") || lower.contains("printing ") {
                PrinterStatus::Printing
            } else {
                PrinterStatus::Ready
            };
            Some(PrinterDetail {
                name: name.to_string(),
                default: default.as_deref() == Some(name),
                status,
                port: String::new(),
                driver: String::new(),
                network: false,
            })
        })
        .collect()
}

/// Parse `lpstat -p -d` output.
pub fn parse_lpstat(out: &str) -> Vec<Printer> {
    parse_lpstat_detailed(out).into_iter().map(|p| p.to_printer()).collect()
}

/// Parse tab-separated Windows printer lines produced by PowerShell (`Win32_Printer` or `.NET`
/// `PrinterSettings::InstalledPrinters`):
/// `Name\tDefault\tWorkOffline\tPrinterStatus\tExtendedPrinterStatus\tPortName\tDriverName`
pub fn parse_windows_printers_tsv(out: &str) -> Vec<PrinterDetail> {
    let mut list: Vec<PrinterDetail> = Vec::new();
    for raw_line in out.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        let name = cols.first().copied().unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }
        if list.iter().any(|p| p.name.eq_ignore_ascii_case(name)) {
            continue;
        }
        let is_default = cols.get(1).is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes"));
        let work_offline = cols.get(2).is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes"));
        let p_status: u32 = cols.get(3).and_then(|v| v.trim().parse().ok()).unwrap_or(3);
        let ext_status: u32 = cols.get(4).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
        let port = cols.get(5).copied().unwrap_or("").trim().to_string();
        let driver = cols.get(6).copied().unwrap_or("").trim().to_string();

        // Win32_Printer status codes:
        // PrinterStatus: 1=Other, 2=Unknown, 3=Idle, 4=Printing, 5=Warmup, 6=Stopped Printing, 7=Offline
        // ExtendedPrinterStatus: 7=Offline, 8=Paused, 9=Error, 10=Busy, 11=Not Available
        let status = if work_offline || p_status == 7 || ext_status == 7 {
            PrinterStatus::Offline
        } else if p_status == 6 || ext_status == 8 {
            PrinterStatus::Paused
        } else if matches!(ext_status, 9 | 11) {
            PrinterStatus::Error
        } else if p_status == 4 {
            PrinterStatus::Printing
        } else {
            PrinterStatus::Ready
        };
        let network = name.starts_with(r"\\") || port.to_ascii_uppercase().starts_with("IP_") || port.to_ascii_uppercase().starts_with("WSD");
        list.push(PrinterDetail { name: name.to_string(), default: is_default, status, port, driver, network });
    }
    list
}

/// Parse `reg.exe query` output for `HKCU\Software\Microsoft\Windows NT\CurrentVersion\Devices`
/// and `HKCU\Software\Microsoft\Windows NT\CurrentVersion\Windows /v Device`.
pub fn parse_windows_reg_printers(devices_out: &str, default_out: &str) -> Vec<PrinterDetail> {
    let default_name = default_out
        .lines()
        .find_map(|line| {
            let idx = line.find("REG_SZ")?;
            let val = line[idx + "REG_SZ".len()..].trim();
            let name = val.split(',').next()?.trim();
            (!name.is_empty()).then(|| name.to_string())
        });

    let mut list = Vec::new();
    for line in devices_out.lines() {
        let Some(idx) = line.find("REG_SZ") else { continue };
        let name = line[..idx].trim();
        if name.is_empty() || name.eq_ignore_ascii_case("(Default)") {
            continue;
        }
        let val = line[idx + "REG_SZ".len()..].trim();
        let mut parts = val.split(',');
        let driver = parts.next().unwrap_or("").trim().to_string();
        let port = parts.next().unwrap_or("").trim().to_string();
        let is_default = default_name.as_deref().is_some_and(|d| d.eq_ignore_ascii_case(name));
        let network = name.starts_with(r"\\") || port.to_ascii_uppercase().starts_with("IP_");
        list.push(PrinterDetail {
            name: name.to_string(),
            default: is_default,
            status: PrinterStatus::Ready,
            port,
            driver,
            network,
        });
    }
    list
}

/// The `lp` arguments for a job printing `file`.
pub fn lp_args(job: &Job, file: &str) -> Vec<String> {
    let mut a = Vec::new();
    if let Some(p) = &job.printer {
        a.extend(["-d".to_string(), p.clone()]);
    }
    a.extend(["-n".to_string(), job.copies.clamp(1, 999).to_string()]);
    a.extend(["-t".to_string(), job.title.clone()]);
    let mut opt = |o: &str| a.extend(["-o".to_string(), o.to_string()]);
    opt(if job.collate { "collate=true" } else { "collate=false" });
    opt(match job.duplex {
        Duplex::Off => "sides=one-sided",
        Duplex::LongEdge => "sides=two-sided-long-edge",
        Duplex::ShortEdge => "sides=two-sided-short-edge",
    });
    if job.grayscale {
        opt("print-color-mode=monochrome");
    }
    // The sheets are already laid out at their final size.
    opt("fit-to-page=false");
    a.push("--".into());
    a.push(file.to_string());
    a
}

/// `lpstat -p -d`, forced to print untranslated messages so [`parse_lpstat`] can read them.
///
/// `LC_ALL`/`LANG=C` is enough on Linux. macOS CUPS ignores them and follows the user's
/// interface language (`AppleLanguages`) unless `SOFTWARE` is set, in which case it uses `LANG`.
pub fn lpstat_command() -> std::process::Command {
    let mut c = std::process::Command::new("lpstat");
    c.args(["-p", "-d"]).env("LC_ALL", "C").env("LANG", "C").env("SOFTWARE", "Linkco PDF Editor");
    c
}

#[cfg(target_os = "windows")]
fn configure_hidden_command(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.stdin(std::process::Stdio::null()).creation_flags(CREATE_NO_WINDOW);
}

#[cfg(target_os = "windows")]
fn query_windows_printers() -> Vec<PrinterDetail> {
    let ps_script = r#"
$ErrorActionPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$found = $false
try {
    $printers = Get-CimInstance -ClassName Win32_Printer -ErrorAction Stop
    foreach ($p in $printers) {
        $found = $true
        $n = [string]$p.Name
        $d = [bool]$p.Default
        $w = [bool]$p.WorkOffline
        $s = [int]$p.PrinterStatus
        $e = [int]$p.ExtendedPrinterStatus
        $port = [string]$p.PortName
        $drv = [string]$p.DriverName
        Write-Output "$n`t$d`t$w`t$s`t$e`t$port`t$drv"
    }
} catch {}
if (-not $found) {
    try {
        Add-Type -AssemblyName System.Drawing -ErrorAction Stop
        $def = (New-Object System.Drawing.Printing.PrinterSettings).PrinterName
        foreach ($n in [System.Drawing.Printing.PrinterSettings]::InstalledPrinters) {
            $isDef = ($n -eq $def)
            Write-Output "$n`t$isDef`tFalse`t3`t0`t`t"
        }
    } catch {}
}
"#;
    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", ps_script]);
    configure_hidden_command(&mut cmd);
    if let Ok(out) = cmd.output() {
        let parsed = parse_windows_printers_tsv(&String::from_utf8_lossy(&out.stdout));
        if !parsed.is_empty() {
            return parsed;
        }
    }

    // Fallback to Windows Registry query via reg.exe if PowerShell is unavailable or restricted.
    let mut dev_cmd = std::process::Command::new("reg.exe");
    dev_cmd.args(["query", r"HKCU\Software\Microsoft\Windows NT\CurrentVersion\Devices"]);
    configure_hidden_command(&mut dev_cmd);

    let mut def_cmd = std::process::Command::new("reg.exe");
    def_cmd.args(["query", r"HKCU\Software\Microsoft\Windows NT\CurrentVersion\Windows", "/v", "Device"]);
    configure_hidden_command(&mut def_cmd);

    let dev_out = dev_cmd.output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let def_out = def_cmd.output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    parse_windows_reg_printers(&dev_out, &def_out)
}

/// Detailed list of installed printers from the OS print spooler (including status and port).
pub fn printers_detailed() -> Vec<PrinterDetail> {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        match lpstat_command().output() {
            Ok(o) => parse_lpstat_detailed(&String::from_utf8_lossy(&o.stdout)),
            Err(_) => Vec::new(),
        }
    }
    #[cfg(target_os = "windows")]
    {
        query_windows_printers()
    }
    #[cfg(not(any(all(unix, not(target_arch = "wasm32")), target_os = "windows")))]
    {
        Vec::new()
    }
}

/// The printers the system knows (empty when there are none or no spooler).
pub fn printers() -> Vec<Printer> {
    printers_detailed().into_iter().map(|p| p.to_printer()).collect()
}

/// Send pre-rendered high-DPI sheets (or vector PDF on CUPS) to the system print spooler.
pub fn submit_sheets(pdf: &[u8], sheets: &[RenderedSheet], job: &Job) -> Result<String, PrintError> {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        let _ = sheets;
        submit_unix(pdf, job)
    }
    #[cfg(target_os = "windows")]
    {
        submit_windows(pdf, sheets, job)
    }
    #[cfg(not(any(all(unix, not(target_arch = "wasm32")), target_os = "windows")))]
    {
        let _ = (pdf, sheets, job);
        Err(PrintError::Spool("printing to a printer isn't available on this platform yet; save the print-ready PDF instead".into()))
    }
}

/// Send a print-ready PDF to the spooler. Returns the spooler's message (the job id).
pub fn submit(pdf: &[u8], job: &Job) -> Result<String, PrintError> {
    submit_sheets(pdf, &[], job)
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
fn submit_unix(pdf: &[u8], job: &Job) -> Result<String, PrintError> {
    if let Some(target) = &job.printer {
        let installed = printers_detailed();
        if !installed.is_empty() {
            if let Some(found) = installed.iter().find(|p| p.name == *target) {
                if found.status == PrinterStatus::Offline {
                    return Err(PrintError::Spool(format!("printer \"{target}\" is currently offline")));
                }
            } else {
                return Err(PrintError::Spool(format!("printer \"{target}\" is not installed")));
            }
        }
    }
    let dir = std::env::temp_dir().join(format!("pdfcraft-print-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| PrintError::Spool(e.to_string()))?;
    let file = dir.join(format!("job-{}.pdf", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())));
    std::fs::write(&file, pdf).map_err(|e| PrintError::Spool(e.to_string()))?;
    let out = std::process::Command::new("lp").args(lp_args(job, &file.to_string_lossy())).output();
    let _ = std::fs::remove_file(&file);
    let out = out.map_err(|e| PrintError::Spool(format!("the print spooler is not available: {e}")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(PrintError::Spool(if err.is_empty() { "the print job was refused".into() } else { err }))
    }
}

#[cfg(target_os = "windows")]
fn submit_windows(pdf: &[u8], sheets: &[RenderedSheet], job: &Job) -> Result<String, PrintError> {
    if pdf.is_empty() && sheets.is_empty() {
        return Err(PrintError::NoPages);
    }

    // Check printer availability and offline status before submitting.
    let installed = printers_detailed();
    let target_printer = match &job.printer {
        Some(name) => {
            if !installed.is_empty() {
                match installed.iter().find(|p| p.name.eq_ignore_ascii_case(name)) {
                    Some(p) if p.status == PrinterStatus::Offline => {
                        return Err(PrintError::Spool(format!("printer \"{}\" is offline; check its connection or power and try again", p.name)));
                    }
                    Some(p) if p.status == PrinterStatus::Error => {
                        return Err(PrintError::Spool(format!("printer \"{}\" reported a hardware error; check the device", p.name)));
                    }
                    Some(p) => p.name.clone(),
                    None => return Err(PrintError::Spool(format!("printer \"{name}\" is not installed on this system"))),
                }
            } else {
                name.clone()
            }
        }
        None => {
            if let Some(def) = installed.iter().find(|p| p.default).or_else(|| installed.first()) {
                if def.status == PrinterStatus::Offline {
                    return Err(PrintError::Spool(format!("default printer \"{}\" is offline", def.name)));
                }
                def.name.clone()
            } else {
                return Err(PrintError::Spool("no printers are installed on this Windows system".into()));
            }
        }
    };

    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("pdfcraft-winprint-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| PrintError::Spool(format!("could not create temporary print folder: {e}")))?;

    let result = submit_windows_in_dir(&dir, pdf, sheets, job, &target_printer);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

#[cfg(target_os = "windows")]
fn submit_windows_in_dir(
    dir: &std::path::Path,
    pdf: &[u8],
    sheets: &[RenderedSheet],
    job: &Job,
    target_printer: &str,
) -> Result<String, PrintError> {
    let manifest_path = dir.join("manifest.tsv");
    let mut manifest = String::new();

    if !sheets.is_empty() {
        for (idx, s) in sheets.iter().enumerate() {
            let png_path = dir.join(format!("sheet-{:04}.png", idx + 1));
            std::fs::write(&png_path, &s.png).map_err(|e| PrintError::Spool(format!("could not write print sheet {}: {e}", idx + 1)))?;
            manifest.push_str(&format!("{}\t{:.4}\t{:.4}\t{}\n", png_path.display(), s.width_pt, s.height_pt, s.dpi));
        }
    } else {
        // Fallback when called via `spool::submit(pdf, job)` without pre-rendered sheets:
        // write the imposed PDF and rasterize each page via `pdfcraft-cli.exe` at 300 DPI.
        let pdf_path = dir.join("job.pdf");
        std::fs::write(&pdf_path, pdf).map_err(|e| PrintError::Spool(format!("could not write temporary PDF: {e}")))?;
        let page_sizes: Vec<(f64, f64)> = match pdfcraft_cos::Document::open(std::sync::Arc::new(pdf.to_vec())) {
            Ok(doc) => pdfcraft_model::pages(&doc).iter().map(|p| p.display_size(&doc)).collect(),
            Err(e) => return Err(PrintError::Cos(e)),
        };
        if page_sizes.is_empty() {
            return Err(PrintError::NoPages);
        }
        let cli_exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("pdfcraft-cli.exe")))
            .filter(|p| p.is_file())
            .unwrap_or_else(|| std::path::PathBuf::from("pdfcraft-cli.exe"));

        for (idx, &(w_pt, h_pt)) in page_sizes.iter().enumerate() {
            let png_path = dir.join(format!("sheet-{:04}.png", idx + 1));
            let mut cmd = std::process::Command::new(&cli_exe);
            cmd.args([
                "render",
                &pdf_path.to_string_lossy(),
                "--page",
                &(idx + 1).to_string(),
                "--dpi",
                &DEFAULT_PRINT_DPI.to_string(),
                "--out",
                &png_path.to_string_lossy(),
            ]);
            configure_hidden_command(&mut cmd);
            let out = cmd.output().map_err(|e| PrintError::Spool(format!("failed to invoke pdfcraft-cli for print rendering: {e}")))?;
            if !out.status.success() || !png_path.is_file() {
                let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                return Err(PrintError::Spool(if err.is_empty() {
                    format!("failed to rasterize print sheet {}", idx + 1)
                } else {
                    err
                }));
            }
            manifest.push_str(&format!("{}\t{:.4}\t{:.4}\t{}\n", png_path.display(), w_pt, h_pt, DEFAULT_PRINT_DPI));
        }
    }

    std::fs::write(&manifest_path, &manifest).map_err(|e| PrintError::Spool(format!("could not write print manifest: {e}")))?;

    let duplex_str = match job.duplex {
        Duplex::Off => "Simplex",
        Duplex::LongEdge => "Vertical",
        Duplex::ShortEdge => "Horizontal",
    };

    let ps_print = r#"
param(
    [string]$ManifestPath,
    [string]$PrinterName,
    [int]$Copies,
    [string]$Collate,
    [string]$DuplexMode,
    [string]$Grayscale,
    [string]$DocTitle
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Add-Type -AssemblyName System.Drawing

$lines = Get-Content -LiteralPath $ManifestPath -Encoding UTF8 | Where-Object { $_.Trim().Length -gt 0 }
if ($lines.Count -eq 0) {
    throw "No sheets to print."
}

$sheets = @()
foreach ($l in $lines) {
    $parts = $l -split "`t"
    $sheets += [PSCustomObject]@{
        Path     = $parts[0]
        WidthPt  = [double]::Parse($parts[1], [System.Globalization.CultureInfo]::InvariantCulture)
        HeightPt = [double]::Parse($parts[2], [System.Globalization.CultureInfo]::InvariantCulture)
        Dpi      = [int]$parts[3]
    }
}

$doc = New-Object System.Drawing.Printing.PrintDocument
$doc.DocumentName = $DocTitle
$doc.PrintController = New-Object System.Drawing.Printing.StandardPrintController

if ($PrinterName -ne "") {
    $doc.PrinterSettings.PrinterName = $PrinterName
}
if (-not $doc.PrinterSettings.IsValid) {
    throw "Printer '$PrinterName' is not valid or not available."
}

$doc.PrinterSettings.Copies = [Math]::Max(1, [Math]::Min(999, $Copies))
if ($Copies -gt 1) {
    $doc.PrinterSettings.Collate = ($Collate -eq "true")
}
if ($doc.PrinterSettings.CanDuplex) {
    switch ($DuplexMode) {
        "Vertical"   { $doc.PrinterSettings.Duplex = [System.Drawing.Printing.Duplex]::Vertical }
        "Horizontal" { $doc.PrinterSettings.Duplex = [System.Drawing.Printing.Duplex]::Horizontal }
        default      { $doc.PrinterSettings.Duplex = [System.Drawing.Printing.Duplex]::Simplex }
    }
}
$doc.DefaultPageSettings.Color = ($Grayscale -ne "true")

$script:sheetIndex = 0

$doc.add_QueryPageSettings({
    param($sender, $e)
    $s = $sheets[$script:sheetIndex]
    $isLandscape = ($s.WidthPt -gt $s.HeightPt)
    $e.PageSettings.Landscape = $isLandscape
    $e.PageSettings.Margins = New-Object System.Drawing.Printing.Margins(0, 0, 0, 0)
    $pw = [Math]::Min($s.WidthPt, $s.HeightPt)
    $ph = [Math]::Max($s.WidthPt, $s.HeightPt)
    $wHundredths = [int][Math]::Round($pw * 100.0 / 72.0)
    $hHundredths = [int][Math]::Round($ph * 100.0 / 72.0)
    $matched = $null
    foreach ($ps in $sender.PrinterSettings.PaperSizes) {
        if ([Math]::Abs($ps.Width - $wHundredths) -le 6 -and [Math]::Abs($ps.Height - $hHundredths) -le 6) {
            $matched = $ps
            break
        }
    }
    if ($null -ne $matched) {
        $e.PageSettings.PaperSize = $matched
    } else {
        $e.PageSettings.PaperSize = New-Object System.Drawing.Printing.PaperSize("PDFSheet", $wHundredths, $hHundredths)
    }
})

$doc.add_PrintPage({
    param($sender, $e)
    $s = $sheets[$script:sheetIndex]
    $img = [System.Drawing.Image]::FromFile($s.Path)
    try {
        $e.Graphics.PageUnit = [System.Drawing.GraphicsUnit]::Point
        $e.Graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $e.Graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $e.Graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
        $e.Graphics.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
        $hx = [single]($e.PageSettings.HardMarginX * 72.0 / 100.0)
        $hy = [single]($e.PageSettings.HardMarginY * 72.0 / 100.0)
        $e.Graphics.TranslateTransform(-$hx, -$hy)
        $rect = New-Object System.Drawing.RectangleF(0.0, 0.0, [single]$s.WidthPt, [single]$s.HeightPt)
        $e.Graphics.DrawImage($img, $rect)
    } finally {
        $img.Dispose()
    }
    $script:sheetIndex++
    $e.HasMorePages = ($script:sheetIndex -lt $sheets.Count)
})

try {
    $doc.Print()
    $dpiUsed = $sheets[0].Dpi
    Write-Output ("printed {0} sheet(s) at {1} DPI to {2}" -f $sheets.Count, $dpiUsed, $doc.PrinterSettings.PrinterName)
} finally {
    $doc.Dispose()
}
"#;

    let script_path = dir.join("print-job.ps1");
    std::fs::write(&script_path, ps_print).map_err(|e| PrintError::Spool(format!("could not write print script: {e}")))?;

    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        &script_path.to_string_lossy(),
        "-ManifestPath",
        &manifest_path.to_string_lossy(),
        "-PrinterName",
        target_printer,
        "-Copies",
        &job.copies.clamp(1, 999).to_string(),
        "-Collate",
        if job.collate { "true" } else { "false" },
        "-DuplexMode",
        duplex_str,
        "-Grayscale",
        if job.grayscale { "true" } else { "false" },
        "-DocTitle",
        &job.title,
    ]);
    configure_hidden_command(&mut cmd);

    let out = cmd.output().map_err(|e| PrintError::Spool(format!("the Windows print spooler could not be started: {e}")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let msg = if !stderr.is_empty() {
            stderr.lines().next().unwrap_or(&stderr).to_string()
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("the print job to \"{target_printer}\" failed")
        };
        Err(PrintError::Spool(msg))
    }
}
