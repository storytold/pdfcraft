//! An `OPENSSL_ia32cap` left in the environment must not stop PdfCraft (#2916).
//!
//! aws-lc, the cryptography library behind digital signatures (through `aws-lc-rs`) and TLS,
//! reads `OPENSSL_ia32cap` when it detects the CPU's features on x86 and x86-64. A value that
//! asks for a feature the processor doesn't have makes it print "Fatal Error: HW capability
//! found: …, but HW capability requested: …" and call `abort()`. On Windows that is exit code
//! 0xC0000409 with no window, no message and nothing in the log. Tuning guides and other
//! programs leave the variable set for every program, so PdfCraft never started on those PCs
//! while other apps did.
//!
//! Two parts keep PdfCraft running:
//! - `.cargo/config.toml` builds aws-lc with `BORINGSSL_NO_STATIC_INITIALIZER`, so it detects
//!   the CPU when `CRYPTO_library_init` is called instead of in a static initializer before
//!   `main`.
//! - [`run_without_cpu_override`], the first thing each executable's `main` does, runs the
//!   program again without the variable. Then `main` calls `pdfcraft_sign::init_crypto`, so the
//!   CPU is detected at start as before, and every operation gets its fast code.
//!
//! Removing the variable from this process instead wouldn't work: aws-lc reads it through the C
//! runtime's `getenv`, whose copy of the environment `std::env::remove_var` doesn't change on
//! Windows, and the workspace has no `unsafe` to call the C runtime with.

use std::ffi::OsString;
use std::io::Write as _;

/// The variable aws-lc reads on x86 and x86-64.
pub const IA32CAP: &str = "OPENSSL_ia32cap";

/// Set in the program started again to the value it was started without, for its log.
pub const DROPPED: &str = "PDFCRAFT_DROPPED_OPENSSL_IA32CAP";

/// When `OPENSSL_ia32cap` is set: run this executable again, with the same arguments and
/// standard streams but without the variable, and return its exit code once it ends.
///
/// `None` when the variable isn't set, on processors aws-lc doesn't read it for (not x86), on
/// macOS (Finder hands files to the process it started, through Apple events, so a second one
/// would never get them), or when the program can't be started again. Then the caller carries
/// on: a value that asks for a missing CPU feature still stops it, at its first use of aws-lc.
pub fn run_without_cpu_override() -> Option<i32> {
    if !cfg!(any(target_arch = "x86", target_arch = "x86_64")) || cfg!(target_os = "macos") {
        return None;
    }
    let value = std::env::var_os(IA32CAP)?;
    let run = || {
        let exe = std::env::current_exe()?;
        std::process::Command::new(exe).args(std::env::args_os().skip(1)).env_remove(IA32CAP).env(DROPPED, &value).status()
    };
    match run() {
        // A process ended by a signal (Unix) has no code: report a failure.
        Ok(status) => Some(status.code().unwrap_or(1)),
        Err(e) => {
            // No logger yet; unlike `eprintln!`, this never panics on a closed standard error.
            let _ = writeln!(std::io::stderr(), "{IA32CAP} is set to {}, and starting again without it failed: {e}", shown(&value));
            None
        }
    }
}

/// The value this program was started without, when [`run_without_cpu_override`] started it.
pub fn dropped() -> Option<String> {
    std::env::var_os(DROPPED).map(|v| shown(&v))
}

/// A value for a message: lossy where it isn't Unicode, and short.
fn shown(value: &OsString) -> String {
    value.to_string_lossy().chars().take(64).collect()
}
