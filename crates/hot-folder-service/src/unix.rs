use crate::{Status, Worker};
use std::{
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    process::{Command, Output},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[cfg(target_os = "macos")]
const DEFINITION: &str = "/Library/LaunchDaemons/ai.storyteller.pdfcraft.hot-folder.plist";
#[cfg(not(target_os = "macos"))]
const DEFINITION: &str = "/etc/systemd/system/pdfcraft-hot-folder.service";

fn command(program: &str, args: &[&str]) -> Result<Output, String> {
    Command::new(program).args(args).output().map_err(|e| format!("Could not run {program}: {e}"))
}
fn checked(program: &str, args: &[&str]) -> Result<(), String> {
    let output = command(program, args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("{program} failed. Administrator permission may be required: {}", String::from_utf8_lossy(&output.stderr).trim()))
    }
}

fn account_uid(account: &str) -> Result<u32, String> {
    crate::definitions::account(account)?;
    let output = command("/usr/bin/id", &["-u", account])?;
    let uid = String::from_utf8_lossy(&output.stdout).trim().parse::<u32>().map_err(|_| "The service account does not exist")?;
    if !output.status.success() || uid == 0 {
        return Err("The service must run under an existing non-root account".into());
    }
    Ok(uid)
}

fn contents(executable: &Path, config: &Path, account: &str) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        crate::definitions::launchd(crate::path_text(executable)?, crate::path_text(config)?, account)
    }
    #[cfg(not(target_os = "macos"))]
    {
        crate::definitions::systemd(crate::path_text(executable)?, crate::path_text(config)?, account)
    }
}

fn read_definition() -> Result<Option<String>, String> {
    match std::fs::symlink_metadata(DEFINITION) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
        Ok(meta) => {
            if !meta.is_file() || meta.uid() != 0 || meta.mode() & 0o022 != 0 || meta.len() > 16384 {
                return Err("The service definition must be a root-owned file, not writable by other accounts".into());
            }
            std::fs::read_to_string(DEFINITION).map(Some).map_err(|e| e.to_string())
        }
    }
}

fn registered_account(text: &str) -> Result<&str, String> {
    #[cfg(target_os = "macos")]
    let value = text.split_once("<key>UserName</key><string>").and_then(|(_, tail)| tail.split_once("</string>")).map(|(value, _)| value);
    #[cfg(not(target_os = "macos"))]
    let value = text.lines().find_map(|line| line.strip_prefix("User="));
    value.ok_or_else(|| "The installed service definition is not managed by PdfCraft".into())
}

pub fn status(executable: &Path, config: &Path) -> Result<Status, String> {
    let Some(text) = read_definition()? else {
        return Ok(Status::absent());
    };
    let account = registered_account(&text)?;
    if text != contents(executable, config, account)? {
        return Err("The installed service uses a different executable or task file, or has been edited externally".into());
    }
    #[cfg(target_os = "macos")]
    let (enabled, running) = {
        let disabled = command("/bin/launchctl", &["print-disabled", "system"])?;
        if !disabled.status.success() {
            return Err("Could not read launchd startup state".into());
        }
        let enabled = crate::definitions::launchd_enabled(&String::from_utf8_lossy(&disabled.stdout))?;
        let state = command("/bin/launchctl", &["print", &format!("system/{}", crate::UNIX_LABEL)])?;
        (enabled, state.status.success() && String::from_utf8_lossy(&state.stdout).contains("state = running"))
    };
    #[cfg(not(target_os = "macos"))]
    let (enabled, running) = {
        let enabled = command("/usr/bin/systemctl", &["is-enabled", "pdfcraft-hot-folder.service"])?;
        let running = command("/usr/bin/systemctl", &["is-active", "pdfcraft-hot-folder.service"])?;
        (enabled.status.success(), running.status.success())
    };
    Ok(Status { installed: true, enabled, running, account: account.into() })
}

pub fn set_enabled(enabled: bool, _: &Path, _: &Path) -> Result<(), String> {
    change_startup(enabled)
}
fn change_startup(enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        checked("/bin/launchctl", &[if enabled { "enable" } else { "disable" }, &format!("system/{}", crate::UNIX_LABEL)])
    }
    #[cfg(not(target_os = "macos"))]
    {
        checked("/usr/bin/systemctl", &[if enabled { "enable" } else { "disable" }, "pdfcraft-hot-folder.service"])
    }
}
pub fn start() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let loaded = command("/bin/launchctl", &["print", &format!("system/{}", crate::UNIX_LABEL)])?;
        if loaded.status.success() {
            checked("/bin/launchctl", &["kickstart", &format!("system/{}", crate::UNIX_LABEL)])
        } else {
            checked("/bin/launchctl", &["bootstrap", "system", DEFINITION])
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        checked("/usr/bin/systemctl", &["start", "pdfcraft-hot-folder.service"])
    }
}
pub fn stop() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        checked("/bin/launchctl", &["bootout", &format!("system/{}", crate::UNIX_LABEL)])
    }
    #[cfg(not(target_os = "macos"))]
    {
        checked("/usr/bin/systemctl", &["stop", "pdfcraft-hot-folder.service"])
    }
}

pub fn install(executable: &Path, config: &Path, account: &str) -> Result<(), String> {
    let uid = account_uid(account)?;
    // System startup must never execute a binary replaceable by a less-privileged user.
    let executable = std::fs::canonicalize(executable).map_err(|e| e.to_string())?;
    for ancestor in executable.ancestors() {
        let meta = std::fs::metadata(ancestor).map_err(|e| e.to_string())?;
        let trusted_admin_group = cfg!(target_os = "macos") && meta.gid() == 80;
        if meta.uid() != 0 || meta.mode() & 0o002 != 0 || (meta.mode() & 0o020 != 0 && !trusted_admin_group) {
            return Err(format!(
                "Install the executable in a root-owned directory without group/other write access before registering its service: {}",
                ancestor.display()
            ));
        }
    }
    let config_meta = std::fs::symlink_metadata(config).map_err(|e| format!("Create the task file for the service account first: {e}"))?;
    if !config_meta.is_file() || config_meta.uid() != uid || config_meta.mode() & 0o077 != 0 {
        return Err("The task file must be owned by the selected service account with owner-only permissions (chmod 600)".into());
    }
    let text = contents(&executable, config, account)?;
    if let Some(previous) = read_definition()? {
        if previous != text {
            return Err("A different Hot Folder system service is already installed; remove it before changing its account or paths".into());
        }
    } else {
        let parent = Path::new(DEFINITION).parent().ok_or("The service directory is unavailable")?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|e| format!("Administrator permission is required to install the service: {e}"))?;
        temporary.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        temporary.as_file().set_permissions(std::fs::Permissions::from_mode(0o644)).map_err(|e| e.to_string())?;
        temporary.as_file().sync_all().map_err(|e| e.to_string())?;
        temporary.persist_noclobber(DEFINITION).map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "macos"))]
    checked("/usr/bin/systemctl", &["daemon-reload"])?;
    change_startup(true)?;
    start()
}

pub fn uninstall() -> Result<(), String> {
    change_startup(false)?;
    // It is safe to remove only our fixed definition, never the user's tasks or inputs.
    #[cfg(not(target_os = "macos"))]
    stop()?;
    #[cfg(target_os = "macos")]
    {
        if command("/bin/launchctl", &["print", &format!("system/{}", crate::UNIX_LABEL)])?.status.success() {
            stop()?;
        }
    }
    std::fs::remove_file(DEFINITION).map_err(|e| e.to_string())?;
    #[cfg(not(target_os = "macos"))]
    checked("/usr/bin/systemctl", &["daemon-reload"])?;
    Ok(())
}

pub fn run(config: &Path, worker: Worker) -> Result<(), String> {
    let identity = command("/usr/bin/id", &["-u"])?;
    let uid = String::from_utf8_lossy(&identity.stdout).trim().parse::<u32>().map_err(|_| "Could not determine the service account")?;
    if !identity.status.success() || uid == 0 {
        return Err("The Hot Folder worker must run under a non-root account; use the installed system service".into());
    }
    let stopping = Arc::new(AtomicBool::new(false));
    let signal = stopping.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst)).map_err(|e| e.to_string())?;
    worker(config, &stopping)
}
