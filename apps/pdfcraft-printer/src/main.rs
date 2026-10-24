#![cfg_attr(windows, windows_subsystem = "windows")]
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(windows)]
mod setup;

fn main() -> std::process::ExitCode {
    #[cfg(windows)]
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(setup::run)).unwrap_or_else(|_| Err("PdfCraft printer setup could not finish".into()));
    #[cfg(not(windows))]
    let result: Result<(), String> = Err("The PdfCraft printer plugin requires Windows 11 24H2 or newer".into());
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            #[cfg(windows)]
            if !std::env::args().any(|a| a == "--quiet") {
                setup::message("PdfCraft printer setup", &message, true);
            }
            std::process::ExitCode::FAILURE
        }
    }
}
