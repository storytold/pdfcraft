//! Synthetic service lifecycle probe for platform CI; never starts without explicit arguments.
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

fn worker(config: &Path, stopping: &AtomicBool) -> Result<(), String> {
    while !stopping.load(Ordering::SeqCst) {
        if config.with_extension("fail").exists() {
            std::fs::remove_file(config.with_extension("fail")).map_err(|e| e.to_string())?;
            return Err("Synthetic controlled service failure".into());
        }
        std::fs::write(config.with_extension("heartbeat"), b"running").map_err(|e| e.to_string())?;
        std::thread::sleep(Duration::from_millis(50));
    }
    std::fs::write(config.with_extension("stopped"), b"stopped gracefully").map_err(|e| e.to_string())
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut action = "status".to_owned();
    let mut config = None;
    let mut account = String::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--service" => action = "run".into(),
            "--initialize-config" => action = "initialize".into(),
            "--config" => config = args.next().map(std::path::PathBuf::from),
            "--service-action" => action = args.next().ok_or("missing action")?,
            "--account" => account = args.next().ok_or("missing account")?,
            _ => return Err("unexpected probe argument".into()),
        }
    }
    let config = config.ok_or("--config is required")?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    use pdfcraft_hot_folder_service as service;
    match action.as_str() {
        "initialize" => {
            let parent = config.parent().ok_or("missing configuration directory")?;
            let mut directories = std::fs::DirBuilder::new();
            directories.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                directories.mode(0o700);
            }
            directories.create(parent).map_err(|e| e.to_string())?;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&config) {
                Ok(mut file) => {
                    use std::io::Write;
                    file.write_all(b"{\"tasks\":[]}").map_err(|e| e.to_string())?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.to_string()),
            }
            return Ok(());
        }
        "run" => return service::run(&config, worker),
        "install" => service::install(&executable, &config, &account)?,
        "uninstall" => service::uninstall(&executable, &config)?,
        "start" => service::start(&executable, &config)?,
        "stop" => service::stop(&executable, &config)?,
        "enable" => service::set_enabled(true, &executable, &config)?,
        "disable" => service::set_enabled(false, &executable, &config)?,
        "prepare-account" => service::prepare_account(&executable, &config)?,
        "status" => {}
        _ => return Err("unknown action".into()),
    }
    let state = service::status(&executable, &config)?;
    println!("installed={} enabled={} running={} account={}", state.installed, state.enabled, state.running, state.account);
    Ok(())
}
