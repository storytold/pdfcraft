//! Windows only: embed the app icon and version info (VERSIONINFO) into `pdfcraft.exe`, so it
//! shows in Explorer, the taskbar, the Start menu and Alt-Tab. A separate PDF document icon
//! uses resource ID 2; the installer references it as `pdfcraft.exe,-2`.
//!
//! On every other target this does nothing. A missing resource compiler is a warning, so a
//! cross-compile from macOS or Linux still links, unless `PDFCRAFT_REQUIRE_WINRES=1` turns it
//! into an error (for release builds).

#[path = "src/windows_manifest.rs"]
mod windows_manifest;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/pdfcraft.ico");
    println!("cargo:rerun-if-changed=../../assets/app-icon/pdfcraft_nightly.ico");
    println!("cargo:rerun-if-changed=../../assets/app-icon/pdfcraft-document.ico");
    println!("cargo:rerun-if-changed=src/windows_manifest.rs");
    println!("cargo:rerun-if-env-changed=PDFCRAFT_REQUIRE_WINRES");
    println!("cargo:rerun-if-env-changed=PDFCRAFT_NIGHTLY");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let is_nightly = std::env::var_os("PDFCRAFT_NIGHTLY").is_some();
    let ico = if is_nightly {
        "../../assets/app-icon/pdfcraft_nightly.ico"
    } else {
        "../../assets/app-icon/pdfcraft.ico"
    };
    let name = if is_nightly { "PdfCraft Nightly" } else { "PdfCraft" };
    let filename = if is_nightly { "pdfcraft-nightly.exe" } else { "pdfcraft.exe" };
    let internal = if is_nightly { "pdfcraft-nightly" } else { "pdfcraft" };
    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico)
        .set_icon_with_id("../../assets/app-icon/pdfcraft-document.ico", "2")
        .set_manifest(windows_manifest::WINDOWS_MANIFEST)
        .set("ProductName", name)
        .set("FileDescription", &format!("{name} PDF workbench"))
        .set("LegalCopyright", "Copyright (c) the PdfCraft contributors. MIT OR Apache-2.0.")
        .set("OriginalFilename", filename)
        .set("InternalName", internal);
    if let Err(e) = res.compile() {
        if std::env::var_os("PDFCRAFT_REQUIRE_WINRES").is_some() {
            println!("cargo::error=embedding Windows resources failed: {e}");
            return;
        }
        println!("cargo:warning=pdfcraft.exe built without icon/version resources: {e}");
    }
}
