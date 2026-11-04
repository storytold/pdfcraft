//! Windows only: embed the app icon and version info (VERSIONINFO) into `pdfcraft.exe`, so it
//! shows in Explorer, the taskbar, the Start menu and Alt-Tab. A separate PDF document icon
//! uses resource ID 2; the installer references it as `pdfcraft.exe,-2`.
//!
//! On every other target this does nothing. A missing resource compiler is a warning, so a
//! cross-compile from macOS or Linux still links, unless `PDFCRAFT_REQUIRE_WINRES=1` turns it
//! into an error (for release builds).

#[path = "src/shell_resources.rs"]
mod shell_resources;
#[path = "src/windows_manifest.rs"]
mod windows_manifest;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(pdfcraft_shell_resources)");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/pdfcraft.ico");
    println!("cargo:rerun-if-changed=../../assets/app-icon/pdfcraft-document.ico");
    println!("cargo:rerun-if-changed=src/windows_manifest.rs");
    println!("cargo:rerun-if-changed=src/shell_resources.rs");
    println!("cargo:rerun-if-env-changed=CRAFT_I18N_DIR");
    println!("cargo:rerun-if-env-changed=PDFCRAFT_REQUIRE_WINRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/app-icon/pdfcraft.ico")
        .set_icon_with_id("../../assets/app-icon/pdfcraft-document.ico", "2")
        .set_manifest(windows_manifest::WINDOWS_MANIFEST)
        .set("ProductName", "PdfCraft")
        .set("FileDescription", "PdfCraft PDF workbench")
        .set("LegalCopyright", "Copyright (c) the PdfCraft contributors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "pdfcraft.exe")
        .set("InternalName", "pdfcraft");
    // The Explorer labels come from the craft-i18n checkout the interface embeds; without one the
    // menu is English, as the interface is.
    let craft_i18n = std::env::var_os("CRAFT_I18N_DIR").filter(|d| !d.is_empty()).map(std::path::PathBuf::from);
    if let Some(dir) = &craft_i18n {
        println!("cargo:rerun-if-changed={}", dir.join("locales").display());
        for (locale, _) in shell_resources::LOCALES {
            println!("cargo:rerun-if-changed={}", shell_resources::catalog(dir, locale).display());
        }
    }
    match shell_resources::string_tables(craft_i18n.as_deref()) {
        Ok(tables) => {
            res.append_rc_content(&tables);
        }
        Err(error) => {
            println!("cargo::error=embedding Explorer labels failed: {error}");
            return;
        }
    }
    if let Err(e) = res.compile() {
        if std::env::var_os("PDFCRAFT_REQUIRE_WINRES").is_some() {
            println!("cargo::error=embedding Windows resources failed: {e}");
            return;
        }
        println!("cargo:warning=pdfcraft.exe built without icon/version resources: {e}");
    } else {
        println!("cargo::rustc-cfg=pdfcraft_shell_resources");
    }
}
