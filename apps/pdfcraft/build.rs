//! Windows only: embed the app icon and version info (VERSIONINFO) into `pdfcraft.exe`, so it
//! shows in Explorer, the taskbar, the Start menu and Alt-Tab. A separate PDF document icon
//! uses resource ID 2; the installer references it as `pdfcraft.exe,-2`.
//!
//! The nightly channel is compiled in here (`PDFCRAFT_BUILD_CHANNEL=nightly-v1`), together with the
//! exact prepared version, so `--version`, About and the Windows ProductVersion the Launcher
//! checks all name the same identity. Unset, empty or `stable` builds the stable app; anything
//! else is a build error, and so is a nightly without its version, commit and date.
//!
//! On every other target the channel and version are still compiled in. A missing resource
//! compiler is a warning, so a cross-compile from macOS or Linux still links, unless
//! `PDFCRAFT_REQUIRE_WINRES=1` turns it into an error (for release builds).

#[path = "src/build_channel.rs"]
mod build_channel;

#[path = "src/windows_manifest.rs"]
mod windows_manifest;

/// The in-box DLLs only printing uses (Windows.Data.Pdf draws the job, prntvpt reads the
/// driver's print capabilities). Trimmed Windows installations (NTLite) can lack them, and a
/// load-time import of a missing DLL stops the program before `main` (#2630). Delay-loaded, they
/// load on the first print; `pdfcraft_print::spool` checks they exist before calling in.
/// Same list in `apps/pdfcraft{,-cli}/build.rs`, `crates/print/build.rs` and `crates/print/src/spool.rs`.
const DELAY_LOADED: [&str; 2] = ["windows.data.pdf.dll", "prntvpt.dll"];

fn die(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/pdfcraft.ico");
    println!("cargo:rerun-if-changed=../../assets/app-icon/pdfcraft-nightly.ico");
    println!("cargo:rerun-if-changed=../../assets/app-icon/pdfcraft-document.ico");
    println!("cargo:rerun-if-changed=src/build_channel.rs");
    println!("cargo:rerun-if-changed=src/windows_manifest.rs");
    println!("cargo:rerun-if-env-changed=PDFCRAFT_REQUIRE_WINRES");
    println!("cargo:rerun-if-env-changed=PDFCRAFT_BUILD_CHANNEL");
    println!("cargo:rustc-check-cfg=cfg(pdfcraft_nightly_v1)");
    println!("cargo:rerun-if-env-changed=PDFCRAFT_VERSION");
    println!("cargo:rerun-if-env-changed=PDFCRAFT_OVERRIDE_VERSION");
    println!("cargo:rerun-if-env-changed=PDFCRAFT_BUILD_SHA");
    println!("cargo:rerun-if-env-changed=PDFCRAFT_BUILD_DATE");

    let nightly = match std::env::var("PDFCRAFT_BUILD_CHANNEL").as_deref() {
        Ok("nightly-v1") => {
            println!("cargo:rustc-cfg=pdfcraft_nightly_v1");
            true
        }
        // An empty value (an unset CI input) means the default stable build.
        Ok("stable" | "") | Err(std::env::VarError::NotPresent) => false,
        _ => die("unsupported PDFCRAFT_BUILD_CHANNEL (expected stable or nightly-v1)"),
    };
    let stable_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let built_version = if nightly {
        let version = std::env::var("PDFCRAFT_VERSION")
            .or_else(|_| std::env::var("PDFCRAFT_OVERRIDE_VERSION"))
            .unwrap_or_else(|_| die("nightly-v1 requires an explicit PDFCRAFT_VERSION feed identity"));
        if !build_channel::is_nightly_version(&version) {
            die("nightly-v1 requires an exact X.Y.Z-nightly.YYYYMMDD.<run>.<attempt>.g<sha12> version");
        }
        let sha = std::env::var("PDFCRAFT_BUILD_SHA").unwrap_or_else(|_| die("nightly-v1 requires PDFCRAFT_BUILD_SHA (40 lowercase hex)"));
        if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            die("PDFCRAFT_BUILD_SHA must be 40 lowercase hex characters");
        }
        if !version.ends_with(&sha[..12]) {
            die("PDFCRAFT_VERSION does not name PDFCRAFT_BUILD_SHA");
        }
        let date = std::env::var("PDFCRAFT_BUILD_DATE").unwrap_or_else(|_| die("nightly-v1 requires PDFCRAFT_BUILD_DATE (YYYY-MM-DD)"));
        let digits: String = date.chars().filter(char::is_ascii_digit).collect();
        let version_date = version.split_once("-nightly.").map(|(_, rest)| rest.chars().take(8).collect::<String>()).unwrap_or_default();
        if date.len() != 10 || digits.len() != 8 || digits != version_date {
            die("PDFCRAFT_BUILD_DATE must be the run-created UTC date named by PDFCRAFT_VERSION");
        }
        println!("cargo:rustc-env=PDFCRAFT_BUILD_COMMIT={sha}");
        version
    } else {
        stable_version.clone()
    };
    println!("cargo:rustc-env=PDFCRAFT_BUILT_VERSION={built_version}");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        for dll in DELAY_LOADED {
            println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
        }
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon(build_channel::windows_icon_path(nightly))
        .set_icon_with_id("../../assets/app-icon/pdfcraft-document.ico", "2")
        .set_manifest(windows_manifest::WINDOWS_MANIFEST)
        .set("ProductName", if nightly { "PdfCraft Nightly" } else { "PdfCraft" })
        .set("FileDescription", if nightly { "PdfCraft Nightly PDF workbench" } else { "PdfCraft PDF workbench" })
        .set("LegalCopyright", "Copyright (c) the PdfCraft contributors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "pdfcraft.exe")
        .set("InternalName", "pdfcraft");
    if nightly {
        // The Launcher verifies the PE ProductVersion against the exact feed identity, not FileVersion.
        res.set("ProductVersion", &built_version);
    }
    if let Err(e) = res.compile() {
        if std::env::var_os("PDFCRAFT_REQUIRE_WINRES").is_some() {
            println!("cargo::error=embedding Windows resources failed: {e}");
            return;
        }
        println!("cargo:warning=pdfcraft.exe built without icon/version resources: {e}");
    }
}
