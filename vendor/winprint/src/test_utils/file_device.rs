use crate::printer::PrinterDevice;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io,
    marker::PhantomData,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
};
use uuid::Uuid;

/// Defines a virtual "print-to-file" device backed by a built-in Windows driver.
///
/// Implement this trait to expose additional file-based drivers. Two built-in providers are
/// shipped: [`PwgRaster`] and [`Pdf`].
pub trait FilePrinterProvider {
    /// Returns the Windows driver name (as accepted by `Add-Printer -DriverName`) to use for the
    /// virtual printer.
    fn driver_name() -> &'static str;
}

/// A built-in [`FilePrinterProvider`] backed by the `Microsoft PWG Raster Class Driver`.
pub struct PwgRaster;
impl FilePrinterProvider for PwgRaster {
    fn driver_name() -> &'static str {
        "Microsoft PWG Raster Class Driver"
    }
}

/// A built-in [`FilePrinterProvider`] backed by the `Microsoft Print To PDF` driver.
pub struct Pdf;
impl FilePrinterProvider for Pdf {
    fn driver_name() -> &'static str {
        "Microsoft Print To PDF"
    }
}

fn ps_quote(s: &str) -> String {
    // PowerShell single-quoted string: escape ' by doubling it.
    format!("'{}'", s.replace('\'', "''"))
}

/// Run a PowerShell script and fail with `io::Error` if PowerShell exits with a non-zero status.
///
/// `context` is prepended to the resulting error message for easier diagnosis.
fn run_powershell(context: &str, script: &str) -> io::Result<()> {
    let status = Command::new("powershell")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .args(["-NoProfile", "-Command", script])
        .status()?;
    if !status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("powershell failed during {context}: exit status {status:?}"),
        ));
    }
    Ok(())
}

fn printer_name_for(port_path: &str) -> String {
    // printer name = file-device-{pid}-{bs58(sha256(port_path))}
    let hash = Sha256::digest(port_path.as_bytes());
    format!(
        "file-device-{}-{}",
        std::process::id(),
        bs58::encode(hash).into_string()
    )
}

fn make_temp_port_path() -> io::Result<PathBuf> {
    let uuid = Uuid::new_v4();
    let mut p = std::env::temp_dir().canonicalize()?;
    p.push(format!(
        "winprint-file-device-{}.prn",
        uuid.as_simple().to_string()
    ));
    Ok(p)
}

/// Run the one-shot init script: install the driver and sweep away any leftover
/// `file-device-*` printers owned by dead pids. Runs at most once per (process, driver).
fn ensure_driver_and_cleanup(driver: &'static str) -> io::Result<()> {
    static INIT: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let map = INIT.get_or_init(|| Mutex::new(HashSet::new()));
    let mut guard = map.lock().expect("INIT mutex poisoned");
    if guard.contains(driver) {
        return Ok(());
    }

    // NOTE: keep this script compatible with Windows PowerShell 5.1.
    //
    // Error-handling policy:
    //   * `$ErrorActionPreference = 'Stop'` by default — any unexpected failure aborts the
    //     script with a non-zero exit code.
    //   * Operations where "failure" is a legitimate expected outcome (e.g. "the printer I
    //     just removed is already gone because a sibling process also cleaned it") are wrapped
    //     in `try { ... } catch { Write-Warning ... }` so they are visibly reported but do
    //     not abort the whole sweep.
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2

# 1. Install the driver if missing.
if (-not (Get-PrinterDriver -Name {driver_q} -ErrorAction SilentlyContinue)) {{
    Add-PrinterDriver -Name {driver_q}
}}

# 2. Scan existing printers and remove leftovers.
#    Minimal base58 decoder (Bitcoin alphabet) adapted from
#    https://gist.github.com/gkostoulias/9e0af1595aaf5e6728443497a7defbe5
function Convert-FromBase58 {{
    param([string]$s)
    $alphabet = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz'
    $num = [System.Numerics.BigInteger]::Zero
    foreach ($c in $s.ToCharArray()) {{
        $i = $alphabet.IndexOf($c)
        if ($i -lt 0) {{ return $null }}
        $num = ($num * [System.Numerics.BigInteger]58) + [System.Numerics.BigInteger]$i
    }}
    $bytes = $num.ToByteArray()
    # BigInteger is little-endian; strip the trailing sign byte (if present) and reverse.
    if ($bytes.Length -gt 1 -and $bytes[$bytes.Length - 1] -eq 0) {{
        $bytes = $bytes[0..($bytes.Length - 2)]
    }}
    [array]::Reverse($bytes)
    # Add leading zero bytes for each leading '1' in the input.
    $leading = 0
    foreach ($c in $s.ToCharArray()) {{
        if ($c -eq '1') {{ $leading++ }} else {{ break }}
    }}
    if ($leading -gt 0) {{
        $pad = New-Object byte[] $leading
        $bytes = $pad + $bytes
    }}
    return ,$bytes
}}

$sha256 = [System.Security.Cryptography.SHA256]::Create()

$printers = Get-Printer | Where-Object {{
    $_.Type -eq 'Local' -and $_.DriverName -eq {driver_q} -and $_.Name -match '^file-device-(\d+)-([1-9A-HJ-NP-Za-km-z]+)$'
}}

foreach ($p in $printers) {{
    $m = [regex]::Match($p.Name, '^file-device-(\d+)-([1-9A-HJ-NP-Za-km-z]+)$')
    $pidValue = [int]$m.Groups[1].Value
    $hashB58  = $m.Groups[2].Value

    # If the owning process is still alive, leave it alone.
    $alive = $true
    try {{ $null = Get-Process -Id $pidValue -ErrorAction Stop }} catch {{ $alive = $false }}
    if ($alive) {{ continue }}

    # Verify bs58(sha256(port)) matches the printer name so that we only remove printers we
    # created. Otherwise: skip and keep going.
    $portName = $p.PortName
    if (-not $portName) {{ continue }}
    $expected = $sha256.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($portName))
    $actual   = Convert-FromBase58 $hashB58
    if ($null -eq $actual -or $actual.Length -ne $expected.Length) {{ continue }}
    $eq = $true
    for ($i = 0; $i -lt $expected.Length; $i++) {{
        if ($actual[$i] -ne $expected[$i]) {{ $eq = $false; break }}
    }}
    if (-not $eq) {{ continue }}

    # Expected-to-possibly-fail operations (racing cleanups, transient spooler state, ...):
    # wrap in try/catch so a single stale entry doesn't abort the whole sweep.
    try {{ Remove-Printer -InputObject $p }}
    catch {{ Write-Warning ("Remove-Printer {{0}} failed: {{1}}" -f $p.Name, $_.Exception.Message) }}

    if ($port = Get-PrinterPort -Name $portName -ErrorAction SilentlyContinue) {{
        try {{ Remove-PrinterPort -InputObject $port }}
        catch {{ Write-Warning ("Remove-PrinterPort {{0}} failed: {{1}}" -f $portName, $_.Exception.Message) }}
    }}
    if (Test-Path -LiteralPath $portName) {{
        try {{ Remove-Item -LiteralPath $portName -Force }}
        catch {{ Write-Warning ("Remove-Item {{0}} failed: {{1}}" -f $portName, $_.Exception.Message) }}
    }}
}}
"#,
        driver_q = ps_quote(driver),
    );
    run_powershell("driver install + leftover cleanup", &script)?;
    guard.insert(driver);
    Ok(())
}

/// A virtual "print-to-file" printer device backed by a built-in Windows driver.
///
/// Each instance installs a freshly-created printer port (a unique temp file) and a printer
/// using the driver supplied by `T`. Dropping the value removes the printer, the port, and the
/// backing temp file. Unlike `null_device`, this type is **not** thread-local or shared — each
/// `FilePrinterDevice` owns its own printer; if you need several, construct several.
///
/// ```no_run
/// use winprint::test_utils::file_device::{FilePrinterDevice, PwgRaster};
///
/// let dev = FilePrinterDevice::<PwgRaster>::new().unwrap();
/// println!("printer: {}", dev.device().name());
/// println!("output:  {}", dev.file_path().display());
/// // ... use dev.device() for printing ...
/// ```
pub struct FilePrinterDevice<T: FilePrinterProvider> {
    device: PrinterDevice,
    port_path: PathBuf,
    _phantom: PhantomData<fn() -> T>,
}

impl<T: FilePrinterProvider> FilePrinterDevice<T> {
    /// Create a new virtual "print-to-file" printer.
    ///
    /// On first call per-process (per driver) this installs the driver and cleans up
    /// leftover printers from previous crashed runs. Subsequent calls only create the port
    /// and printer.
    pub fn new() -> io::Result<Self> {
        let driver = T::driver_name();
        ensure_driver_and_cleanup(driver)?;

        let port_path = make_temp_port_path()?;
        let port_str = port_path
            .to_str()
            .ok_or(io::Error::other("contains non-Unicode chars in path"))?;
        let printer_name = printer_name_for(&port_str);

        let script = format!(
            r#"
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2
if (-not (Get-PrinterPort -Name {port_q} -ErrorAction SilentlyContinue)) {{
    Add-PrinterPort -Name {port_q}
}}
Add-Printer -Name {name_q} -PortName {port_q} -DriverName {driver_q}
"#,
            port_q = ps_quote(&port_str),
            name_q = ps_quote(&printer_name),
            driver_q = ps_quote(driver),
        );
        run_powershell("add printer", &script)?;

        let device = PrinterDevice::all()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("enumerate printers: {e:?}")))?
            .into_iter()
            .find(|p| p.name() == printer_name)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "virtual printer '{printer_name}' not found after creation (port='{port_str}', driver='{driver}')"
                    ),
                )
            })?;

        Ok(FilePrinterDevice {
            device,
            port_path,
            _phantom: PhantomData,
        })
    }

    /// Returns the underlying [`PrinterDevice`].
    pub fn device(&self) -> &PrinterDevice {
        &self.device
    }

    /// Returns the path to the backing file that the driver spools its output into.
    pub fn file_path(&self) -> &Path {
        &self.port_path
    }
}

impl<T: FilePrinterProvider> Drop for FilePrinterDevice<T> {
    fn drop(&mut self) {
        let port_str = self.port_path.to_string_lossy().into_owned();
        let script = format!(
            r#"
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2
if ($p = Get-Printer -Name {name_q} -ErrorAction SilentlyContinue) {{
    try {{ Remove-Printer -InputObject $p }}
    catch {{ Write-Warning ("Remove-Printer {{0}} failed: {{1}}" -f {name_q}, $_.Exception.Message) }}
}}
if ($port = Get-PrinterPort -Name {port_q} -ErrorAction SilentlyContinue) {{
    try {{ Remove-PrinterPort -InputObject $port }}
    catch {{ Write-Warning ("Remove-PrinterPort failed: {{0}}" -f $_.Exception.Message) }}
}}
"#,
            name_q = ps_quote(self.device.name()),
            port_q = ps_quote(&port_str),
        );
        // Drop cannot propagate errors.
        let _ = run_powershell("remove printer", &script);
        let _ = std::fs::remove_file(&self.port_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printer::PrinterDevice;

    fn printer_exists(name: &str) -> bool {
        PrinterDevice::all()
            .unwrap()
            .into_iter()
            .any(|p| p.name() == name)
    }

    #[test]
    fn pwg_raster_device_lifetime() {
        let name;
        let path;
        {
            let dev = FilePrinterDevice::<PwgRaster>::new().unwrap();
            name = dev.device().name().to_string();
            path = dev.file_path().to_path_buf();
            assert!(name.starts_with("file-device-"), "unexpected name: {name}");
            assert!(printer_exists(&name), "printer {name} was not created");
        }
        assert!(
            !printer_exists(&name),
            "printer {name} still present after drop"
        );
        assert!(
            !path.exists(),
            "backing file {} still present after drop",
            path.display()
        );
    }

    #[test]
    fn pdf_device_lifetime() {
        let name;
        {
            let dev = FilePrinterDevice::<Pdf>::new().unwrap();
            name = dev.device().name().to_string();
            assert!(printer_exists(&name), "printer {name} was not created");
        }
        assert!(
            !printer_exists(&name),
            "printer {name} still present after drop"
        );
    }

    #[test]
    fn printer_name_is_stable_for_same_port() {
        let n1 = printer_name_for(r"C:\Temp\foo.prn");
        let n2 = printer_name_for(r"C:\Temp\foo.prn");
        assert_eq!(n1, n2);
        let n3 = printer_name_for(r"C:\Temp\bar.prn");
        assert_ne!(n1, n3);
    }

    #[test]
    fn two_devices_coexist() {
        let a = FilePrinterDevice::<PwgRaster>::new().unwrap();
        let b = FilePrinterDevice::<PwgRaster>::new().unwrap();
        assert_ne!(a.device().name(), b.device().name());
        assert_ne!(a.file_path(), b.file_path());
        assert!(printer_exists(a.device().name()));
        assert!(printer_exists(b.device().name()));
    }
}
