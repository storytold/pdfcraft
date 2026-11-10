//! System-boot startup backed by Windows SCM, systemd, or a macOS LaunchDaemon.
use std::path::Path;

pub fn enabled(executable: &Path, config: &Path) -> bool {
    pdfcraft_hot_folder_service::status(executable, config).is_ok_and(|state| state.enabled)
}

pub fn set_enabled(enabled: bool, executable: &Path, config: &Path) -> Result<(), String> {
    pdfcraft_hot_folder_service::set_enabled(enabled, executable, config)
}
