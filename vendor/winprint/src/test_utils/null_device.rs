use crate::printer::PrinterDevice;
use sha2::{Digest, Sha256};
use std::{cell::OnceCell, process::Stdio, sync::OnceLock};

thread_local! {
    static NULL_DEVICE: OnceCell<NullPrinterDevice> = const { OnceCell::new() };
}
static DEVICE_ID_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static PORT_AND_DRIVER_INSTALLER: OnceLock<()> = OnceLock::new();

struct NullPrinterDevice {
    printer: PrinterDevice,
}

impl NullPrinterDevice {
    fn new() -> Self {
        PORT_AND_DRIVER_INSTALLER.get_or_init(|| {
            std::process::Command::new("powershell")
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .args([
                    "-Command",
                    "if (-not (Get-PrinterDriver -Name 'Generic / Text Only' -ErrorAction SilentlyContinue)) {Add-PrinterDriver 'Generic / Text Only' -ErrorAction Continue} if (-not (Get-PrinterPort -Name 'nul:' -ErrorAction SilentlyContinue)) {Add-PrinterPort -Name 'nul:' -ErrorAction Continue}",
                ])
                .spawn()
                .unwrap()
                .wait()
                .unwrap();
        });

        let id = DEVICE_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let exe_path = std::env::current_exe().unwrap().canonicalize().unwrap();
        let exe_id =
            bs58::encode(Sha256::digest(exe_path.to_string_lossy().as_bytes())).into_string();

        let printer_name = format!("null-device-{}-{}", exe_id, id);

        let printer = PrinterDevice::all()
            .unwrap()
            .into_iter()
            .find(|p| p.name() == printer_name);
        if let Some(printer) = printer {
            return NullPrinterDevice { printer };
        }

        std::process::Command::new("powershell")
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .args([
                "-Command",
                &format!(
                    "Add-Printer -Name '{}' -PortName 'nul:' -DriverName 'Generic / Text Only'",
                    printer_name
                ),
            ])
            .spawn()
            .unwrap()
            .wait()
            .unwrap();

        let printer = PrinterDevice::all()
            .unwrap()
            .into_iter()
            .find(|p| p.name() == printer_name)
            .unwrap();
        NullPrinterDevice { printer }
    }
}

impl Drop for NullPrinterDevice {
    fn drop(&mut self) {
        std::process::Command::new("powershell")
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .args([
                "-Command",
                &format!("Remove-Printer -Name '{}'", self.printer.name()),
            ])
            .spawn()
            .unwrap()
            .wait()
            .unwrap();
    }
}

/// Get a thread-local null printer device.
/// This device is automatically managed and to be deleted when the thread ends.
pub fn thread_local() -> PrinterDevice {
    NULL_DEVICE.with(|f| f.get_or_init(NullPrinterDevice::new).printer.clone())
}
