//! Optional standalone Hot Folder app. Nothing installs or enables it implicitly.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod i18n;
mod source_ui;
mod startup;
mod ui;
mod widgets;
mod worker;
mod worker_process;

use pdfcraft_engine::hot_folder as hf;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn default_config() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    let path = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).map(|p| p.join("PdfCraft/HotFolder/tasks.json"));
    #[cfg(target_os = "macos")]
    let path = std::env::var_os("HOME").map(PathBuf::from).map(|p| p.join("Library/Application Support/PdfCraft/HotFolder/tasks.json"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let path = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from).map(|p| p.join(".config")))
        .map(|p| p.join("pdfcraft/hot-folder/tasks.json"));
    path.ok_or_else(|| "the user configuration directory is unavailable; use --config <path>".into())
}

fn launch_worker(config: &Path) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    if pdfcraft_hot_folder_service::status(&executable, config)?.installed {
        return pdfcraft_hot_folder_service::start(&executable, config);
    }
    worker::check_identity()?;
    let mut command = Command::new(executable);
    command.arg("--worker").arg("--config").arg(config).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW; no visible command window.
    }
    command.spawn().map_err(|e| e.to_string())?;
    Ok(())
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut path = None;
    let mut background = false;
    let mut once = false;
    let mut service = false;
    let mut service_action = None;
    let mut account = String::new();
    let mut initialize = false;
    let mut task_child = false;
    let mut task_id = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => path = Some(PathBuf::from(args.next().ok_or("--config needs a file path")?)),
            "--worker" => background = true,
            "--once" => once = true,
            "--service" => service = true,
            "--initialize-config" => initialize = true,
            "--task-child" => task_child = true,
            "--task-id" => task_id = Some(args.next().ok_or("--task-id needs an ID")?),
            "--service-action" => {
                service_action = Some(args.next().ok_or("--service-action needs status/start/stop/enable/disable/install/uninstall/prepare-account")?)
            }
            "--account" => account = args.next().ok_or("--account needs a non-root service account")?,
            "--version" => {
                println!("PdfCraft Hot Folder {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!(
                    "PdfCraft Hot Folder\n  pdfcraft-hot-folder [--config FILE]\n  pdfcraft-hot-folder --worker --config FILE\n  pdfcraft-hot-folder --once --config FILE\n  pdfcraft-hot-folder --service --config FILE\n  pdfcraft-hot-folder --initialize-config --config FILE\n  pdfcraft-hot-folder --service-action status|start|stop|enable|disable|install|uninstall|prepare-account --config FILE [--account USER]\nThe GUI saves opt-in tasks. --once runs one supervised scheduler pass; --service hosts a registered boot service. Task passes default to a 300-second deadline; local tasks.worker.json can override worker limits. No network listener is opened."
                );
                return Ok(());
            }
            _ => return Err(format!("unknown option: {arg}")),
        }
    }
    let config = path.map(Ok).unwrap_or_else(default_config)?;
    let config = if config.is_absolute() { config } else { std::env::current_dir().map_err(|e| e.to_string())?.join(config) };
    if usize::from(service)
        + usize::from(once)
        + usize::from(background)
        + usize::from(service_action.is_some())
        + usize::from(initialize)
        + usize::from(task_child)
        > 1
    {
        return Err("choose only one operating mode".into());
    }
    if task_child {
        return worker_process::child_main(&config, &task_id.ok_or("the task child needs --task-id")?);
    }
    if task_id.is_some() {
        return Err("--task-id is reserved for supervised task children".into());
    }
    if initialize {
        return hf::initialize_config(&config);
    }
    if let Some(action) = service_action {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        use pdfcraft_hot_folder_service as svc;
        return match action.as_str() {
            "status" => {
                println!("{}", serde_json::to_string_pretty(&svc::status(&exe, &config)?).map_err(|e| e.to_string())?);
                Ok(())
            }
            "start" => svc::start(&exe, &config),
            "stop" => svc::stop(&exe, &config),
            "enable" => svc::set_enabled(true, &exe, &config),
            "disable" => svc::set_enabled(false, &exe, &config),
            "install" => svc::install(&exe, &config, &account),
            "uninstall" => svc::uninstall(&exe, &config),
            "prepare-account" => svc::prepare_account(&exe, &config),
            _ => Err("unknown service action; use status/start/stop/enable/disable/install/uninstall/prepare-account".into()),
        };
    }
    if service {
        return pdfcraft_hot_folder_service::run(&config, |path, stop| worker::run(path, || stop.load(std::sync::atomic::Ordering::Relaxed)));
    }
    if once {
        let state = worker::once(&config)?;
        println!("{}", serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?);
        return Ok(());
    }
    if background {
        return worker::run(&config, || false);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1080.0, 780.0]).with_min_inner_size([780.0, 560.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "PdfCraft Hot Folder",
        options,
        Box::new(move |cc| {
            ui::install_fonts(&cc.egui_ctx);
            Ok(Box::new(ui::HotFolderApp::new(config)))
        }),
    )
    .map_err(|e| e.to_string())
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("PdfCraft Hot Folder: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
