fn main() {
    println!("cargo:rerun-if-changed=../../assets/app-icon/pdfcraft.ico");
    println!("cargo:rerun-if-env-changed=PDFCRAFT_SETUP_MSI");
    println!("cargo:rerun-if-env-changed=PDFCRAFT_SETUP_MSIX");
    for key in ["PDFCRAFT_SETUP_MSI", "PDFCRAFT_SETUP_MSIX"] {
        if let Ok(path) = std::env::var(key) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let level = if std::env::var_os("CARGO_FEATURE_BUNDLE").is_some() { "requireAdministrator" } else { "asInvoker" };
    let manifest = format!(
        r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"><trustInfo xmlns="urn:schemas-microsoft-com:asm.v3"><security><requestedPrivileges><requestedExecutionLevel level="{level}" uiAccess="false"/></requestedPrivileges></security></trustInfo><compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1"><application><supportedOS Id="{{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}}"/></application></compatibility></assembly>"#
    );
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("../../assets/app-icon/pdfcraft.ico")
        .set("ProductName", "PdfCraft Printer")
        .set("FileDescription", "PdfCraft printer component and setup")
        .set_manifest(&manifest);
    if let Err(error) = resource.compile() {
        println!("cargo::error=Could not embed Windows printer resources: {error}");
    }
}
