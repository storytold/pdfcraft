#![forbid(unsafe_code)]

use windows::Management::Deployment::PackageManager;
use windows_core::HSTRING;

const NAME: &str = "PdfCraft.Printer.Personal";
const PUBLISHER: &str = "CN=gh0sted5456-us, OID.2.25.311729368913984317654407730594956997722=1";

pub fn message(title: &str, text: &str, error: bool) {
    let level = if error { rfd::MessageLevel::Error } else { rfd::MessageLevel::Info };
    let _ = rfd::MessageDialog::new().set_title(title).set_description(text).set_level(level).show();
}

pub fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.iter().any(|a| !["--status", "--remove-printer", "--printer-only", "--quiet", "--install"].contains(&a.as_str())) {
        return Err("Usage: pdfcraft-printer [--status | --remove-printer | --install] [--printer-only] [--quiet]".into());
    }
    if args.iter().any(|a| a == "--status") {
        return status(false);
    }
    if args.iter().any(|a| a == "--remove-printer") {
        return status(true);
    }
    #[cfg(feature = "bundle")]
    {
        install(&args)?;
        Ok(())
    }
    #[cfg(not(feature = "bundle"))]
    Err("This is the PdfCraft printer component. Run the combined PdfCraft installer to install it, or use --status / --remove-printer.".into())
}

fn status(remove: bool) -> Result<(), String> {
    let manager = PackageManager::new().map_err(|e| e.to_string())?;
    let packages = manager
        .FindPackagesByUserSecurityIdNamePublisher(&HSTRING::new(), &HSTRING::from(NAME), &HSTRING::from(PUBLISHER))
        .map_err(|e| e.to_string())?;
    let mut installed = Vec::new();
    for package in packages {
        let id = package.Id().and_then(|id| id.FullName()).map_err(|e| e.to_string())?;
        if remove {
            let result = manager.RemovePackageAsync(&id).and_then(|op| op.join()).map_err(|e| e.to_string())?;
            let error = result.ExtendedErrorCode().map_err(|e| e.to_string())?;
            if error.is_err() {
                return Err(format!("Printer removal failed: {} ({error:?})", result.ErrorText().map_err(|e| e.to_string())?));
            }
        }
        installed.push(id.to_string_lossy());
    }
    println!(
        "{}",
        serde_json::json!({"plugin":NAME,"installed":!remove && !installed.is_empty(),"printer_ready":printer_ready(),"removed":remove,"packages":installed})
    );
    Ok(())
}

#[cfg(feature = "bundle")]
fn install(args: &[String]) -> Result<(), String> {
    use std::io::Write as _;
    use windows::Foundation::{Metadata::ApiInformation, Uri};
    use windows::Management::Deployment::AddPackageOptions;
    if !ApiInformation::IsTypePresent(&HSTRING::from("Windows.Graphics.Printing.Workflow.PrintWorkflowVirtualPrinterSession"))
        .map_err(|e| e.to_string())?
    {
        return Err("PdfCraft Printer requires Windows 11 24H2 (build 26100) or newer. This installer did not change your system.".into());
    }
    let temporary = tempfile::Builder::new().prefix("craftpdf-setup-").tempdir().map_err(|e| e.to_string())?;
    if !args.iter().any(|a| a == "--printer-only") {
        let path = temporary.path().join("PdfCraft.msi");
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| e.to_string())?;
        file.write_all(include_bytes!(env!("PDFCRAFT_SETUP_MSI"))).and_then(|_| file.sync_all()).map_err(|e| e.to_string())?;
        drop(file);
        let system = std::env::var_os("SystemRoot").ok_or("Windows system directory is unavailable")?;
        let status = std::process::Command::new(std::path::PathBuf::from(system).join("System32/msiexec.exe"))
            .arg("/i")
            .arg(&path)
            .args(["/passive", "/norestart"])
            .status()
            .map_err(|e| e.to_string())?;
        if !matches!(status.code(), Some(0 | 3010)) {
            return Err(format!(
                "PdfCraft installation did not finish (Windows Installer result {:?}); the printer was not installed",
                status.code()
            ));
        }
    }
    let path = temporary.path().join("PdfCraft.Printer.msix");
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| e.to_string())?;
    file.write_all(include_bytes!(env!("PDFCRAFT_SETUP_MSIX"))).and_then(|_| file.sync_all()).map_err(|e| e.to_string())?;
    drop(file);
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    let path_text = path.to_string_lossy();
    let path_text = path_text.strip_prefix(r"\\?\").unwrap_or(&path_text);
    let uri = Uri::CreateUri(&HSTRING::from(path_text)).map_err(|e| e.to_string())?;
    let options = AddPackageOptions::new().map_err(|e| e.to_string())?;
    // Applies only to this embedded personal-build package; never changes certificate stores or system policy.
    options.SetAllowUnsigned(true).map_err(|e| e.to_string())?;
    let previous_default = default_printer()?;
    let registration = (|| {
        let result = PackageManager::new()
            .and_then(|p| p.AddPackageByUriAsync(&uri, &options))
            .and_then(|op| op.join())
            .map_err(|e| format!("PdfCraft setup could not register the printer plugin: {e}"))?;
        let error = result.ExtendedErrorCode().map_err(|e| e.to_string())?;
        if error.is_err() {
            return Err(format!("Printer plugin registration failed: {} ({error:?})", result.ErrorText().map_err(|e| e.to_string())?));
        }
        for _ in 0..50 {
            if printer_ready() {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Err("The plugin package installed, but Windows did not register PdfCraft Printer. The printer is not ready; see the setup troubleshooting guide.".to_string())
    })();
    let restored = restore_default(previous_default.as_deref());
    registration.and(restored)?;
    status(false)
}

fn printer_ready() -> bool {
    windows::Devices::Printers::IppPrintDevice::IsIppPrinter(&HSTRING::from("PdfCraft Printer")).unwrap_or(false)
}

#[cfg(feature = "bundle")]
fn default_printer() -> Result<Option<String>, String> {
    let key = windows_registry::CURRENT_USER
        .open(r"Software\Microsoft\Windows NT\CurrentVersion\Windows")
        .map_err(|e| format!("Could not preserve the default printer: {e}"))?;
    match key.get_string("Device") {
        Ok(device) => default_device_name(&device)
            .map(Some)
            .ok_or_else(|| "The current default printer could not be identified; setup did not register a printer".into()),
        Err(error) if error.code().0 == 0x80070002u32 as i32 => Ok(None),
        Err(error) => Err(format!("Could not preserve the default printer: {error}")),
    }
}

#[cfg(any(feature = "bundle", test))]
fn default_device_name(device: &str) -> Option<String> {
    let separator = device.to_ascii_lowercase().rfind(",winspool,")?;
    device.get(..separator).filter(|s| !s.is_empty()).map(str::to_owned)
}

#[cfg(feature = "bundle")]
fn restore_default(previous: Option<&str>) -> Result<(), String> {
    let Some(previous) = previous else {
        return Ok(());
    };
    if default_printer()?.as_deref() == Some(previous) {
        return Ok(());
    }
    let system = std::env::var_os("SystemRoot").ok_or("Windows system directory is unavailable")?;
    let result = std::process::Command::new(std::path::PathBuf::from(system).join("System32/rundll32.exe"))
        .args(["printui.dll,PrintUIEntry", "/y", "/n", previous])
        .status()
        .map_err(|e| format!("Could not restore your default printer: {e}"))?;
    if !result.success() || default_printer()?.as_deref() != Some(previous) {
        return Err("The printer plugin installed, but Windows could not restore your previous default printer".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_printer_names_keep_commas_and_unicode() {
        assert_eq!(default_device_name("Office, étage,winspool,Ne01:"), Some("Office, étage".into()));
        assert_eq!(default_device_name("Printer,WinSpool,port"), Some("Printer".into()));
        assert_eq!(default_device_name("damaged"), None);
        assert_eq!(default_device_name(",winspool,port"), None);
    }
}
