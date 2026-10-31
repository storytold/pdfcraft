//! Writes the shell completions and man pages of `pdfcraft-cli` into a folder (RFC-0012 §1.9).
//!
//! `cargo run -p pdfcraft-cli --example man-and-completions -- DIR`
//!
//! One completion file per shell (bash, zsh, fish, elvish, PowerShell) and one man page per
//! command (`pdfcraft-cli.1`, `pdfcraft-cli-render.1`, …). The generators are dev-dependencies of
//! this example, so they are never linked into the binary.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::ValueEnum;
use clap_complete::Shell;

fn generate(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut cmd = pdfcraft_cli::command();
    for &shell in Shell::value_variants() {
        clap_complete::generate_to(shell, &mut cmd, "pdfcraft-cli", dir)?;
    }
    // A fresh command: `generate_to` built `cmd`, adding the `help` subcommand, which clap_mangen
    // would otherwise give pages of its own (`pdfcraft-cli-help-render.1`).
    clap_mangen::generate_to(pdfcraft_cli::command(), dir)
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let (Some(dir), None) = (args.next(), args.next()) else {
        eprintln!("usage: man-and-completions DIR");
        return ExitCode::from(2);
    };
    match generate(&PathBuf::from(dir)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
