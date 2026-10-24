#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    #[cfg(target_os = "linux")]
    let result = std::panic::catch_unwind(pdfcraft_printer::cups::run).unwrap_or_else(|_| Err("PdfCraft Printer could not finish the job".into()));
    #[cfg(not(target_os = "linux"))]
    let result: Result<(), String> = Err("This printer component is for Linux CUPS".into());
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ERROR: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
