//! Background processing loop used by the manual worker and the system-service host.
use pdfcraft_engine::hot_folder as hf;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use crate::worker_process::{self, Limits, Outcome};

pub fn check_identity() -> Result<(), String> {
    #[cfg(unix)]
    if rustix::process::geteuid().is_root() {
        return Err("Run folder processing as the configured non-root service account; administrator mode is only for setup".into());
    }
    Ok(())
}

pub fn run(config: &Path, should_stop: impl Fn() -> bool) -> Result<(), String> {
    let Some(_lock) = prepare(config, false)? else { return Ok(()) };
    let limits = Limits::load(config).inspect_err(|error| report(config, Some(error)))?;
    while !should_stop() {
        if let Err(error) = pass(config, &limits, &should_stop) {
            report(config, Some(&error));
            // A process that could not be reaped must not be left alive while another pass starts.
            return Err(error);
        }
        let sleeping = Instant::now();
        while !should_stop() && sleeping.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Ok(())
}

pub fn once(config: &Path) -> Result<hf::State, String> {
    let _lock = prepare(config, true)?.ok_or("another Hot Folder supervisor is already running")?;
    hf::validate_structure(&hf::load_config(config)?, None)?;
    pass(config, &Limits::load(config)?, &|| false)?;
    hf::read_state(config)
}

fn prepare(config: &Path, exclusive: bool) -> Result<Option<std::fs::File>, String> {
    check_identity()?;
    local_file(config)?;
    for extension in ["worker.lock", "lock", "state.json", "state.pending.json"] {
        local_file(&config.with_extension(extension))?;
    }
    if !config.exists() {
        hf::initialize_config(config)?;
    }
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(config.with_extension("worker.lock"))
        .map_err(|e| e.to_string())?;
    match lock.try_lock() {
        Ok(()) => Ok(Some(lock)),
        Err(std::fs::TryLockError::WouldBlock) if !exclusive => Ok(None),
        Err(error) => Err(format!("Cannot own the Hot Folder supervisor lock: {error}")),
    }
}

/// This is intentionally lexical until every ancestor has been checked: canonicalizing
/// a junction to an unavailable SMB share would already block the supervisor.
pub fn local_file(path: &Path) -> Result<(), String> {
    if !path.is_absolute() || path.components().any(|component| matches!(component, Component::ParentDir | Component::CurDir)) {
        return Err("worker configuration and journals need an absolute local path without '.' or '..'".into());
    }
    #[cfg(windows)]
    {
        let drive = match path.components().next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                std::path::Prefix::Disk(drive) | std::path::Prefix::VerbatimDisk(drive) => drive,
                _ => return Err("worker configuration and journals must be on a local disk, never a UNC share".into()),
            },
            _ => return Err("worker configuration needs a local drive letter".into()),
        };
        if winsafe::GetDriveType(Some(&format!("{}:\\", char::from(drive)))) != winsafe::co::DRIVE::FIXED {
            return Err("worker configuration and journals must be on a fixed local disk, never a mapped network drive".into());
        }
    }
    let mut ancestor = PathBuf::new();
    for component in path.components() {
        ancestor.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        match std::fs::symlink_metadata(&ancestor) {
            Ok(metadata) => {
                #[cfg(windows)]
                let redirected = {
                    use std::os::windows::fs::MetadataExt;
                    metadata.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT, including junctions.
                };
                #[cfg(not(windows))]
                let redirected = false;
                // macOS /var and /tmp are normal system symlinks. Only Windows guarantees
                // local volumes here; Unix deployments must keep the config on local storage.
                if metadata.file_type().is_symlink() && (cfg!(windows) || ancestor == path) || redirected {
                    return Err("worker configuration and journal paths cannot traverse symbolic links or reparse points".into());
                }
                if ancestor == path && !metadata.is_file() {
                    return Err("worker configuration and journal targets must be regular files".into());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(format!("Cannot inspect local worker path: {error}")),
        }
    }
    Ok(())
}

fn report(config: &Path, error: Option<&str>) {
    if let Err(write_error) = local_file(&config.with_extension("error.json")).and_then(|()| hf::record_worker_error(config, error)) {
        // A full/unwritable error sidecar must never stop scheduling otherwise valid tasks.
        let _ = writeln!(std::io::stderr(), "Cannot write Hot Folder diagnostics: {write_error}; {}", error.unwrap_or("worker pass completed"));
    }
}

fn pass(config: &Path, limits: &Limits, should_stop: &dyn Fn() -> bool) -> Result<(), String> {
    pass_with(config, should_stop, |task, cancel| worker_process::run(config, &task.id, limits, cancel))
}

fn pass_with(
    path: &Path,
    should_stop: &dyn Fn() -> bool,
    mut run_task: impl FnMut(&hf::Task, &dyn Fn() -> bool) -> Result<Outcome, String>,
) -> Result<(), String> {
    let config = match local_file(path).and_then(|()| hf::load_config(path)).and_then(|config| hf::validate_structure(&config, None).map(|()| config))
    {
        Ok(config) => config,
        Err(error) => {
            report(path, Some(&error));
            return Ok(()); // An atomic later edit can fix configuration without restarting the service.
        }
    };
    let state = match local_file(&path.with_extension("state.json"))
        .and_then(|()| local_file(&path.with_extension("state.pending.json")))
        .and_then(|()| hf::read_state(path))
    {
        Ok(state) => state,
        Err(error) => {
            report(path, Some(&error));
            return Ok(()); // Never replace a corrupt receipt journal with empty state.
        }
    };
    let mut failures = Vec::new();
    for task in &config.tasks {
        if should_stop() {
            break;
        }
        if !task.enabled
            || task.start_at > hf::now()
            || state.tasks.get(&task.id).is_some_and(|status| status.finished || status.blocked || status.next_check > hf::now())
        {
            continue;
        }
        let snapshot = serde_json::to_vec(task).map_err(|e| e.to_string())?;
        let checked = std::cell::Cell::new(None::<Instant>);
        let cancelled = std::cell::Cell::new(false);
        let cancel = || {
            if should_stop() || cancelled.get() {
                return true;
            }
            if checked.get().is_none_or(|last| last.elapsed() >= Duration::from_millis(500)) {
                checked.set(Some(Instant::now()));
                let unchanged = local_file(path).and_then(|()| hf::load_config(path)).is_ok_and(|config| {
                    config
                        .tasks
                        .iter()
                        .any(|current| current.id == task.id && current.enabled && serde_json::to_vec(current).is_ok_and(|bytes| bytes == snapshot))
                });
                cancelled.set(!unchanged);
            }
            cancelled.get()
        };
        match run_task(task, &cancel)? {
            Outcome::Completed | Outcome::Cancelled => {}
            Outcome::Failed(error) => {
                failures.push(format!("{}: {error}", task.id));
                if let Err(error) = hf::record_task_failure(path, None, &task.id, &error) {
                    failures.push(format!("{}: cannot persist task failure: {error}", task.id));
                }
            }
        }
    }
    report(path, (!failures.is_empty()).then(|| failures.join("\n")).as_deref());
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn hot_folder_worker_initializes_missing_stopped_configuration_as_its_account() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/tasks.json");
        #[cfg(unix)]
        if rustix::process::geteuid().is_root() {
            assert!(super::run(&path, || true).unwrap_err().contains("non-root"));
            assert!(!path.exists());
            return;
        }
        super::run(&path, || true).unwrap();
        assert!(pdfcraft_engine::hot_folder::load_config(&path).unwrap().tasks.is_empty());
    }

    #[test]
    fn diagnostics_write_failure_and_failed_task_do_not_block_another_task() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("tasks.json");
        let mut tasks = Vec::new();
        for id in ["first", "second"] {
            let input = dir.path().join(format!("{id}-in"));
            let output = dir.path().join(format!("{id}-out"));
            std::fs::create_dir(&input).unwrap();
            std::fs::create_dir(&output).unwrap();
            tasks.push(hf::Task { id: id.into(), name: id.into(), input, output, enabled: true, analysis_only: true, ..Default::default() });
        }
        hf::save_config(&config, &hf::Config { tasks }, None).unwrap();
        std::fs::create_dir(config.with_extension("error.json")).unwrap();
        let mut called = Vec::new();
        pass_with(&config, &|| false, |task, _| {
            called.push(task.id.clone());
            Ok(if task.id == "first" { Outcome::Failed("simulated timeout".into()) } else { Outcome::Completed })
        })
        .unwrap();
        assert_eq!(called, ["first", "second"]);
        assert!(hf::read_state(&config).unwrap().tasks["first"].next_check > hf::now());
    }

    #[test]
    fn changing_a_task_cancels_its_isolated_snapshot() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        let input = dir.path().join("input");
        let output = dir.path().join("output");
        std::fs::create_dir(&input).unwrap();
        std::fs::create_dir(&output).unwrap();
        let mut config = hf::Config {
            tasks: vec![hf::Task { id: "task".into(), name: "task".into(), input, output, enabled: true, analysis_only: true, ..Default::default() }],
        };
        hf::save_config(&path, &config, None).unwrap();
        pass_with(&path, &|| false, |_, stop| {
            config.tasks[0].enabled = false;
            hf::save_config(&path, &config, None).unwrap();
            assert!(stop());
            Ok(Outcome::Cancelled)
        })
        .unwrap();
    }

    #[test]
    fn supervisor_paths_refuse_remote_paths_and_non_files() {
        use std::path::Path;
        let dir = tempfile::tempdir().unwrap();
        assert!(super::local_file(dir.path()).is_err());
        assert!(super::local_file(&dir.path().join("tasks.json")).is_ok());
        assert!(super::local_file(Path::new("relative.json")).is_err());
        #[cfg(windows)]
        for path in [r"\\server\share\tasks.json", r"\\?\UNC\server\share\tasks.json", r"\\.\pipe\tasks.json"] {
            assert!(super::local_file(Path::new(path)).is_err());
        }
    }
}
