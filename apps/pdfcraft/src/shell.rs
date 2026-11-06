//! Open the saved local MIME draft with the user's registered handler. No MAPI send call,
//! recipients, remote URL or shell-command interpolation is involved.

pub fn open_draft(path: &std::path::Path) -> Result<(), String> {
    // Called on eframe's window thread after a user confirmation. native_options enables file
    // drops, and winit's create_window_data initializes OLE/STA for that thread before showing it.
    let path = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    if path.extension().and_then(|s| s.to_str()) != Some("eml") || !path.is_file() {
        return Err("choose a saved .eml draft".into());
    }
    // File handlers do not all understand Win32's extended-path prefix from canonicalize.
    let text = path.to_string_lossy();
    let ordinary =
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") { format!(r"\\{unc}") } else { text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned() };
    winsafe::ShellExecuteEx(&winsafe::SHELLEXECUTEINFO {
        verb: Some("open"),
        file: &ordinary,
        show: winsafe::co::SW::SHOWNORMAL,
        mask: winsafe::co::SEE_MASK::FLAG_NO_UI,
        ..Default::default()
    })
    .map_err(|e| e.to_string())
}
