//! The `pdfcraft-cli` binary: hands the process's arguments and streams to [`pdfcraft_cli::run`].

use std::process::ExitCode;

fn main() -> ExitCode {
    // `args_os`: a file name that is not UTF-8 reaches clap, which reports it as a usage error,
    // where `std::env::args` would panic (photocraft#1108).
    let status = pdfcraft_cli::run(std::env::args_os(), &mut std::io::stdout().lock(), &mut std::io::stderr().lock());
    ExitCode::from(status)
}
