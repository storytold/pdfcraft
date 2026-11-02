//! Explicit system-boot service management, shared by the companion and automation.
//! No service is installed or started merely by linking this crate.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::Path;
use std::sync::atomic::AtomicBool;

mod definitions;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix;
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod unsupported;
#[cfg(windows)]
mod windows;

pub const SERVICE_NAME: &str = "PdfCraftHotFolder";
pub const UNIX_LABEL: &str = "ai.storyteller.pdfcraft.hot-folder";
pub type Worker = fn(&Path, &AtomicBool) -> Result<(), String>;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Status {
    pub installed: bool,
    pub enabled: bool,
    pub running: bool,
    pub account: String,
}

impl Status {
    fn absent() -> Self {
        Self { installed: false, enabled: false, running: false, account: String::new() }
    }
}

fn path_text(path: &Path) -> Result<&str, String> {
    let value = path.to_str().ok_or("Service paths must contain valid Unicode")?;
    if !path.is_absolute() || value.chars().any(char::is_control) {
        return Err("Service paths must be absolute and contain no control characters".into());
    }
    Ok(value)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
use unix as platform;
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
use unsupported as platform;
#[cfg(windows)]
use windows as platform;

/// Inspect only the registered service matching this executable and task file.
pub fn status(executable: &Path, config: &Path) -> Result<Status, String> {
    path_text(executable)?;
    path_text(config)?;
    platform::status(executable, config)
}

/// Change boot startup, without changing the service account or task file.
pub fn set_enabled(enabled: bool, executable: &Path, config: &Path) -> Result<(), String> {
    if !status(executable, config)?.installed {
        return Err("Install the optional Hot Folder system service first".into());
    }
    platform::set_enabled(enabled, executable, config)
}

pub fn start(executable: &Path, config: &Path) -> Result<(), String> {
    let state = status(executable, config)?;
    if !state.installed {
        return Err("The Hot Folder system service is not installed".into());
    }
    if !state.running {
        platform::start()?;
    }
    Ok(())
}

pub fn stop(executable: &Path, config: &Path) -> Result<(), String> {
    if status(executable, config)?.running {
        platform::stop()?;
    }
    Ok(())
}

/// Unix requires an explicit non-root account. Windows registration belongs to MSI.
pub fn install(executable: &Path, config: &Path, account: &str) -> Result<(), String> {
    path_text(executable)?;
    path_text(config)?;
    platform::install(executable, config, account)
}

pub fn uninstall(executable: &Path, config: &Path) -> Result<(), String> {
    if status(executable, config)?.installed {
        platform::uninstall()?;
    }
    Ok(())
}

/// Prepare the installed Windows service for an administrator-managed logon account.
/// Does not change credentials, start the service, or grant access to watched folders.
pub fn prepare_account(executable: &Path, config: &Path) -> Result<(), String> {
    if !status(executable, config)?.installed {
        return Err("Install the optional Hot Folder system service first".into());
    }
    #[cfg(windows)]
    return windows::prepare_account();
    #[cfg(not(windows))]
    Err("prepare-account is a Windows service administration action".into())
}

/// Blocking service host. The worker must check the cancellation flag between pages.
pub fn run(config: &Path, worker: Worker) -> Result<(), String> {
    path_text(config)?;
    platform::run(config, worker)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn service_paths_reject_relative_and_control_characters() {
        assert!(path_text(Path::new("tasks.json")).is_err());
        assert!(path_text(Path::new("/tmp/tasks\n.json")).is_err());
    }
}
