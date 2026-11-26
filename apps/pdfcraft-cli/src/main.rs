//! The `pdfcraft-cli` binary: hands the process's arguments and streams to [`pdfcraft_cli::run`].

use std::process::ExitCode;

fn main() -> ExitCode {
    // Before anything uses aws-lc: an `OPENSSL_ia32cap` naming a CPU feature this processor lacks
    // would stop it, so the CLI runs again without it (#2916, `pdfcraft_platform::crypto_env`).
    if let Some(code) = pdfcraft_platform::crypto_env::run_without_cpu_override() {
        std::process::exit(code);
    }
    // aws-lc detects the CPU's features here, where it used to before `main` (`init_crypto`).
    pdfcraft_engine::sign::init_crypto();
    // `args_os`: a file name that is not UTF-8 reaches clap, which reports it as a usage error,
    // where `std::env::args` would panic (photocraft#1108).
    let status = pdfcraft_cli::run(std::env::args_os(), &mut std::io::stdout().lock(), &mut std::io::stderr().lock());
    ExitCode::from(status)
}
