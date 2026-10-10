//! The system print spooler.
//!
//! - On **macOS and Linux**, printers are enumerated via CUPS (`lpstat -p -d` and `lpstat -e` for
//!   driverless network destinations) and print jobs are piped to `lp` on stdin with job options
//!   (copies, collation, duplex, grayscale, driver PPD options, and `fit-to-page=false` because
//!   sheets are already imposed at their target paper size).
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
    /// The printer driver's own job options (a PPD keyword and the chosen value), such as the
    /// paper tray or paper type; see [`printer_options`]. CUPS only.
    pub options: Vec<(String, String)>,
}

impl Default for Job {
    fn default() -> Self {
        Job { printer: None, copies: 1, collate: true, duplex: Duplex::Off, grayscale: false, title: "Linkco PDF Editor".into(), options: Vec::new() }
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

/// The `lpstat -d` line's destination name, when the spooler has a default.
fn default_destination(out: &str) -> Option<&str> {
    out.lines().find_map(|l| l.strip_prefix("system default destination:")).map(str::trim)
}

/// Parse `lpstat -p -d` output into detailed printer entries.
pub fn parse_lpstat_detailed(out: &str) -> Vec<PrinterDetail> {
    let default = default_destination(out);
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
                default: default == Some(name),
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

/// Parse `lpstat -e` output: destination names, one per line.
pub fn parse_lpstat_e(out: &str) -> Vec<String> {
    out.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// The `lp` arguments for a job. There is no file argument: `lp` reads the job from stdin.
pub fn lp_args(job: &Job) -> Vec<String> {
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
    for (key, value) in &job.options {
        // Driver options are PPD keywords; anything else is not something lp can be given.
        if is_ppd_keyword(key) && is_ppd_keyword(value) && !HANDLED_OPTIONS.contains(&key.as_str()) {
            a.extend(["-o".to_string(), format!("{key}={value}")]);
        }
    }
    a
}

/// One of the printer driver's job options (a PPD `OpenUI` entry), such as the paper tray.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrinterOption {
    /// The PPD keyword (`InputSlot`).
    pub key: String,
    /// What the driver calls it (`Paper tray`); the keyword when it gives no name.
    pub label: String,
    /// The driver's group (`Paper Handling`); empty outside a group.
    pub group: String,
    /// The choices: keyword and label (`Tray2`, `Tray 2`).
    pub choices: Vec<(String, String)>,
    /// The choice the printer uses when a job doesn't say.
    pub default: String,
}

/// Options PdfCraft's Print dialog sets itself (paper, two-sided, collation), so the driver's
/// copies of them are neither shown nor sent.
pub const HANDLED_OPTIONS: &[&str] = &["PageSize", "PageRegion", "Duplex", "Collate", "ImageableArea", "PaperDimension"];

/// Caps on what a PPD may make the dialog hold (PPDs are text files from the printer vendor).
const MAX_OPTIONS: usize = 400;
const MAX_CHOICES: usize = 400;

/// A PPD keyword: printable ASCII without spaces, `/`, `:` or `=`.
fn is_ppd_keyword(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'/' | b':' | b'=' | b'"'))
}

/// `Name/Label` → (`Name`, `Label`), the label falling back to the name.
fn name_label(s: &str) -> (&str, &str) {
    match s.split_once('/') {
        Some((n, l)) if !l.trim().is_empty() => (n.trim(), l.trim()),
        Some((n, _)) => (n.trim(), n.trim()),
        None => (s.trim(), s.trim()),
    }
}

/// The job options a PPD offers, in file order, without the installable-hardware group and the
/// options PdfCraft sets itself ([`HANDLED_OPTIONS`]). Lenient: malformed lines are skipped.
pub fn parse_ppd(ppd: &str) -> Vec<PrinterOption> {
    let mut out: Vec<PrinterOption> = Vec::new();
    let (mut group, mut group_key) = (String::new(), String::new());
    let mut open: Option<PrinterOption> = None;
    for line in ppd.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("*OpenGroup:") {
            let (k, l) = name_label(rest);
            (group_key, group) = (k.to_string(), l.to_string());
        } else if line.starts_with("*CloseGroup") {
            (group_key, group) = (String::new(), String::new());
        } else if let Some(rest) = line.strip_prefix("*OpenUI") {
            let Some((head, _kind)) = rest.trim().rsplit_once(':') else { continue };
            let (key, label) = name_label(head.trim().trim_start_matches('*'));
            open = is_ppd_keyword(key).then(|| PrinterOption {
                key: key.to_string(),
                label: label.to_string(),
                group: group.clone(),
                choices: Vec::new(),
                default: String::new(),
            });
        } else if line.starts_with("*CloseUI") {
            if let Some(o) = open.take()
                && !o.choices.is_empty()
                && group_key != "InstallableOptions"
                && !HANDLED_OPTIONS.contains(&o.key.as_str())
                && out.len() < MAX_OPTIONS
            {
                out.push(o);
            }
        } else if let Some(o) = open.as_mut() {
            let Some(rest) = line.strip_prefix('*') else { continue };
            if let Some(value) = rest.strip_prefix("Default").and_then(|r| r.strip_prefix(o.key.as_str())).and_then(|r| r.strip_prefix(':')) {
                o.default = value.trim().to_string();
            } else if let Some(rest) = rest.strip_prefix(o.key.as_str()).and_then(|r| r.strip_prefix(' ')) {
                // `*InputSlot Tray2/Tray 2: "<< ... >>"`; the code after the colon is the driver's.
                let head = rest.split_once(':').map_or(rest, |(h, _)| h);
                let (choice, label) = name_label(head);
                if is_ppd_keyword(choice) && o.choices.len() < MAX_CHOICES && o.choices.iter().all(|(c, _)| c != choice) {
                    o.choices.push((choice.to_string(), label.to_string()));
                }
            }
        }
    }
    for o in &mut out {
        if !o.choices.iter().any(|(c, _)| *c == o.default) {
            o.default = o.choices.first().map(|(c, _)| c.clone()).unwrap_or_default();
        }
    }
    out
}

/// The current choice of each option in `lpoptions -p NAME -l` output (`Key/Label: a *b c`): the
/// queue's defaults, including the user's own (`~/.cups/lpoptions`).
pub fn parse_lpoptions(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| {
            let (head, choices) = l.split_once(':')?;
            let (key, _) = name_label(head);
            let current = choices.split_whitespace().find_map(|c| c.strip_prefix('*'))?;
            Some((key.to_string(), current.to_string()))
        })
        .collect()
}

/// A PPD's text: UTF-8, or ISO Latin-1 (the PPD default) when it isn't valid UTF-8.
pub fn ppd_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    }
}

/// The driver's job options for a printer (CUPS: its PPD, with the queue's current defaults);
/// empty when it has none or on other platforms. Reads files and runs `lpoptions`, so call it
/// when the user asks, not every frame.
pub fn printer_options(printer: &str) -> Vec<PrinterOption> {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        // Queue names never contain `/`; anything else is not a queue and not a path to follow.
        if !is_ppd_keyword(printer) {
            return Vec::new();
        }
        const PPD_CAP: u64 = 16 << 20;
        let ppd = std::fs::File::open(format!("/etc/cups/ppd/{printer}.ppd")).ok().and_then(|f| {
            use std::io::Read as _;
            let mut bytes = Vec::new();
            f.take(PPD_CAP).read_to_end(&mut bytes).ok().map(|_| bytes)
        });
        let mut options = ppd.map(|b| parse_ppd(&ppd_text(&b))).unwrap_or_default();
        let current = std::process::Command::new("lpoptions")
            .args(["-p", printer, "-l"])
            .output()
            .map(|o| parse_lpoptions(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_default();
        for (key, value) in current {
            if let Some(o) = options.iter_mut().find(|o| o.key == key)
                && o.choices.iter().any(|(c, _)| *c == value)
            {
                o.default = value;
            }
        }
        options
    }
    #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
    {
        let _ = printer;
        Vec::new()
    }
}

/// Whether [`open_printer_preferences`] can show the driver's own settings window (Windows).
pub const HAS_PRINTER_PREFERENCES: bool = cfg!(windows);

/// Open the printer driver's Printing Preferences window (Windows: `printui.dll`). What the user
/// sets there is saved as that printer's defaults for this Windows user, which every job starts
/// from; the Print dialog's copies, two-sided, grayscale and paper still apply on top.
pub fn open_printer_preferences(printer: &str) -> Result<(), PrintError> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // printui takes the name in quotes; Windows printer names can't contain one anyway.
        if printer.is_empty() || printer.contains('"') {
            return Err(PrintError::Spool("not a printer name".into()));
        }
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let rundll = std::path::Path::new(&root).join("System32").join("rundll32.exe");
        std::process::Command::new(rundll)
            .raw_arg("printui.dll,PrintUIEntry")
            .raw_arg("/e")
            .raw_arg("/n")
            .raw_arg(format!("\"{printer}\""))
            .spawn()
            .map(|_| ())
            .map_err(|e| PrintError::Spool(format!("the printer's preferences could not be opened: {e}")))
    }
    #[cfg(not(windows))]
    {
        let _ = printer;
        Err(PrintError::Spool("printer preferences open only on Windows; use the printer's options instead".into()))
    }
}

/// `lpstat`, forced to print untranslated messages so [`parse_lpstat`] and [`parse_lpstat_e`] can
/// read them.
///
/// `LC_ALL`/`LANG=C` is enough on Linux. macOS CUPS ignores them and follows the user's
/// interface language (`AppleLanguages`) unless `SOFTWARE` is set, in which case it uses `LANG`.
fn lpstat_with(args: &[&str]) -> std::process::Command {
    let mut c = std::process::Command::new("lpstat");
    c.args(args).env("LC_ALL", "C").env("LANG", "C").env("SOFTWARE", "Linkco PDF Editor");
    c
}

/// `lpstat -p -d`: the spooler's queues and the default destination.
pub fn lpstat_command() -> std::process::Command {
    lpstat_with(&["-p", "-d"])
}

/// `lpstat -e`: every destination CUPS can print to, including driverless network printers no
/// permanent queue exists for (a queue is built only when a job needs one). GTK's and macOS's
/// print dialogs list them the same way. CUPS ≥ 1.7 (2013).
///
/// Gated like its only caller, `printers_detailed`'s unix block: off unix this is dead code, and the
/// Windows clippy gate builds with `-D warnings`.
#[cfg(all(unix, not(target_arch = "wasm32")))]
fn lpstat_e_command() -> std::process::Command {
    lpstat_with(&["-e"])
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

/// Detailed list of installed printers from lpstat's two answers: the spooler's queues (`-p -d`),
/// then the driverless destinations of `available` (`-e`) that have no queue.
pub fn printers_detailed_parsed(queues: &str, available: Option<&str>) -> Vec<PrinterDetail> {
    let mut printers = parse_lpstat_detailed(queues);
    let default = default_destination(queues);
    if let Some(names) = available {
        for name in parse_lpstat_e(names) {
            if !printers.iter().any(|p| p.name == name) {
                printers.push(PrinterDetail {
                    default: default == Some(name.as_str()),
                    name,
                    status: PrinterStatus::Ready,
                    port: String::new(),
                    driver: String::new(),
                    network: true,
                });
            }
        }
    }
    printers
}

/// The Print dialog's printers from lpstat's two answers: the spooler's queues (`-p -d`), then
/// the driverless destinations of `available` (`-e`) that have no queue — CUPS builds a temporary
/// queue when `lp` sends them a job. Each destination once, queues first; the default still comes
/// from `-d`, so a driverless default keeps its marker. `None` when the spooler doesn't know `-e`
/// (CUPS < 1.7, 2013), which loses only the driverless names.
pub fn printers_parsed(queues: &str, available: Option<&str>) -> Vec<Printer> {
    printers_detailed_parsed(queues, available).into_iter().map(|p| p.to_printer()).collect()
}

/// Detailed list of installed printers from the OS print spooler (including status and port).
pub fn printers_detailed() -> Vec<PrinterDetail> {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        let Some(queues) = lpstat_command().output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()) else {
            return Vec::new();
        };
        // A spooler without `-e`, or one whose discovery times out, only loses the driverless names.
        let available = lpstat_e_command().output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
        printers_detailed_parsed(&queues, available.as_deref())
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

/// The printers the system knows (empty when there are none or no spooler): its queues, then the
/// driverless destinations CUPS can print to without one.
pub fn printers() -> Vec<Printer> {
    printers_detailed().into_iter().map(|p| p.to_printer()).collect()
}

/// Send pre-rendered high-DPI sheets (or vector PDF on CUPS) to the system print spooler.
pub fn submit_sheets(pdf: &[u8], sheets: &[RenderedSheet], job: &Job) -> Result<String, PrintError> {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        let _ = sheets;
        if let Some(target) = &job.printer {
            let installed = printers_detailed();
            if let Some(found) = installed.iter().find(|p| p.name == *target)
                && found.status == PrinterStatus::Offline
            {
                return Err(PrintError::Spool(format!("printer \"{target}\" is currently offline")));
            }
        }
        submit_via(std::process::Command::new("lp"), pdf, job)
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

/// [`submit`] with the spooler command given, so tests can stand in for `lp`.
///
/// The job goes to `lp` on stdin, never through a file: a predictable job folder in a shared
/// temp directory lets another local user read printed documents or swap the file before `lp`
/// reads it.
#[cfg(any(test, all(unix, not(target_arch = "wasm32"))))]
pub(crate) fn submit_via(mut lp: std::process::Command, pdf: &[u8], job: &Job) -> Result<String, PrintError> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = lp
        .args(lp_args(job))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| PrintError::Spool(format!("the print spooler is not available: {e}")))?;
    let stdin = child.stdin.take();
    // Feed the job on its own thread while collecting the output, so neither side can fill a
    // pipe and wait on the other. Dropping `stdin` at the end closes it: the end of the job.
    let fed_and_out = std::thread::scope(|s| {
        let feeder = std::thread::Builder::new().name("print job".into()).spawn_scoped(s, move || match stdin {
            Some(mut stdin) => stdin.write_all(pdf),
            None => Err(std::io::Error::other("no pipe to the spooler")),
        });
        match feeder {
            Ok(feeder) => {
                let out = child.wait_with_output();
                Ok((feeder.join(), out))
            }
            Err(e) => {
                // No thread to feed it: stop `lp` rather than leave it with an empty job.
                let _ = child.kill();
                let _ = child.wait();
                Err(PrintError::Spool(format!("the print job could not be sent to the spooler: {e}")))
            }
        }
    });
    let (fed, out) = fed_and_out?;
    let out = out.map_err(|e| PrintError::Spool(format!("the print spooler is not available: {e}")))?;
    if !out.status.success() {
        // `lp` may refuse before reading the job (an unknown printer); its message says why,
        // and the broken pipe that leaves behind does not.
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(PrintError::Spool(if err.is_empty() { "the print job was refused".into() } else { err }));
    }
    match fed {
        Ok(Ok(())) => Ok(job_message(&out.stdout)),
        Ok(Err(e)) => Err(PrintError::Spool(format!("the print job could not be sent to the spooler: {e}"))),
        Err(_) => Err(PrintError::Spool("the print job could not be sent to the spooler".into())),
    }
}

/// `lp`'s reply ("request id is Office-12"), without the "(0 file(s))" CUPS adds when the job came
/// on stdin: it reads as if nothing was sent.
#[cfg(any(test, all(unix, not(target_arch = "wasm32"))))]
pub(crate) fn job_message(stdout: &[u8]) -> String {
    let s = String::from_utf8_lossy(stdout);
    let s = s.trim();
    s.strip_suffix("(0 file(s))").map_or(s, str::trim_end).to_string()
}
