//! Windows (MSVC) only: delay-load the print DLLs in this crate's own test executable, as the
//! applications do (`apps/pdfcraft{,-cli}/build.rs`), so it starts on a Windows installation that
//! lacks them (#2630) and its tests of the missing-DLL guard in `src/spool.rs` run there. Link
//! arguments from a build script reach only this package's own executables (its tests), never
//! the crates that depend on it; each application sets its own.
//!
//! On every other target this does nothing.

/// The in-box DLLs only printing uses (Windows.Data.Pdf draws the job, prntvpt reads the
/// driver's print capabilities). Same list in `apps/pdfcraft{,-cli}/build.rs` and
/// `src/spool.rs`.
const DELAY_LOADED: [&str; 2] = ["windows.data.pdf.dll", "prntvpt.dll"];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    for dll in DELAY_LOADED {
        println!("cargo:rustc-link-arg=/DELAYLOAD:{dll}");
    }
    println!("cargo:rustc-link-arg=delayimp.lib");
}
