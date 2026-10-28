#[path = "../pdfcraft/src/windows_manifest.rs"]
mod windows_manifest;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/pdfcraft_nightly.ico");
    println!("cargo:rerun-if-changed=../../assets/app-icon/pdfcraft-document.ico");
    println!("cargo:rerun-if-changed=../pdfcraft/src/windows_manifest.rs");
    println!("cargo:rerun-if-env-changed=PDFCRAFT_REQUIRE_WINRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/app-icon/pdfcraft_nightly.ico")
        .set_icon_with_id("../../assets/app-icon/pdfcraft-document.ico", "2")
        .set_manifest(windows_manifest::WINDOWS_MANIFEST)
        .set("ProductName", "PdfCraft Nightly")
        .set("FileDescription", "PdfCraft Nightly PDF workbench")
        .set("LegalCopyright", "Copyright (c) the PdfCraft contributors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "pdfcraft-nightly.exe")
        .set("InternalName", "pdfcraft-nightly");
    if let Err(e) = res.compile() {
        if std::env::var_os("PDFCRAFT_REQUIRE_WINRES").is_some() {
            println!("cargo::error=embedding Windows resources failed: {e}");
            return;
        }
        println!("cargo:warning=pdfcraft-nightly.exe built without icon/version resources: {e}");
    }
}
