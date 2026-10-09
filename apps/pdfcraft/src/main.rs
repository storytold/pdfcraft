//! PdfCraft desktop app.
//!
//! Usage: `pdfcraft [options] [files…]`
//! `--create-images [images…]` stages the images in one PDF and asks for the page DPI.
//!
//! View options (applied after the files open; also the seed of the UI control channel):
//! `--page N  --zoom 150  --layout continuous|two-up|single  --panel comments|bookmarks|pages|fields|layers|attachments|none
//!  --theme light|dark|system  --language auto|<code>  --mode all|read|edit|convert|sign  --tool <catalogue id>  --left open|closed
//!  --organize on  --fields on  --dialog properties|shortcuts|about  --palette <query>  --home on
//!  --cover on|off  --default-layout continuous|two-up|single  --default-zoom fit-width|fit-page|<percent>`
//!
//! `--control <file>` enables the UI control channel (off by default): the app listens on a random
//! loopback port and writes `{"port", "token", "pid"}` to `<file>` (owner-only permissions).
//! Agents then drive it with `pdfcraft-cli ui --control <file> <method> …`.

// Release builds on Windows are GUI-subsystem programs, so launching the app doesn't open a console
// window next to it (#57). `--version` and diagnostics then go nowhere when started from a terminal
// (std ignores the missing console handles, so nothing fails); debug builds keep the console.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_ui_egui::PdfCraftApp;

#[cfg(target_os = "macos")]
mod apple_events;
mod logging;
mod updates;

/// Freedesktop app id: the `.desktop` file name and the hicolor icon name.
const APP_ID: &str = "ai.storyteller.pdfcraft";

/// The app icon (assets/app-icon/README.md). macOS gets the version on Apple's icon grid, with a
/// transparent margin; Windows and Linux get the full-bleed tile.
#[cfg(target_os = "macos")]
const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/app-icon/pdfcraft-1024.png");
#[cfg(not(target_os = "macos"))]
const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/ai.storyteller.pdfcraft.png");

/// The app was called PrintCraft before; settings saved then are under this key.
const LEGACY_STORAGE_KEY: &str = "printcraft";

/// The settings folder: `app.ron` and the `logs` folder (docs/development.md). In portable mode
/// it is `PdfCraftData` beside the executable (#157). eframe would otherwise derive it from the
/// app id; keep it under "PdfCraft".
fn settings_dir() -> Option<std::path::PathBuf> {
    if let Some(dir) = pdfcraft_ui_egui::portable::data_dir() {
        return Some(dir.to_path_buf());
    }
    eframe::storage_dir("PdfCraft")
}

/// Move the settings and crash-recovery folders of the app's former name, PrintCraft, to the new
/// name once, so an upgrade keeps recent files, preferences and unsaved work. Best effort: a
/// folder is left alone when the new one already exists or the move fails. A portable copy leaves
/// the per-user folders alone.
fn migrate_legacy_folders() {
    if pdfcraft_ui_egui::portable::data_dir().is_some() {
        return;
    }
    let mut moves = vec![(eframe::storage_dir("PrintCraft"), settings_dir())];
    // Recovery lives in the settings folder except on Windows, where it is under %LOCALAPPDATA%.
    if cfg!(windows) {
        let local = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()).map(std::path::PathBuf::from);
        moves.push((local.as_ref().map(|d| d.join("PrintCraft")), local.map(|d| d.join("PdfCraft"))));
    }
    for (old, new) in moves {
        let (Some(old), Some(new)) = (old, new) else { continue };
        if !old.is_dir() || new.exists() {
            continue;
        }
        if let Some(parent) = new.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::rename(&old, &new) {
            Ok(()) => log::info!("moved {} to {}", old.display(), new.display()),
            Err(e) => log::warn!("moving {} to {}: {e}", old.display(), new.display()),
        }
    }
}

/// Desktop launchers (GNOME Files, KDE Dolphin…) only recognise an app as the default handler for a
/// mime type if its `Exec` takes URIs (`%u`/`%U`), not just paths (`%F`); the packaged `.desktop`
/// file uses `%U` accordingly (packaging/linux/ai.storyteller.pdfcraft.desktop). Decode a local
/// `file://` argument (`file:///path` or `file://localhost/path`) to a plain path here so the rest
/// of the app, which only ever opens paths, is unaffected. Other schemes (`http://`, `mailto:`…),
/// URIs naming another host, and plain paths pass through untouched.
fn path_from_arg(arg: String) -> String {
    let Some(rest) = arg.strip_prefix("file://") else { return arg };
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return arg;
    }
    // `file:///C:/x.pdf` on Windows names `C:/x.pdf`. `rest` starts with the one-byte '/', so
    // byte 1 is a char boundary.
    let rest = if cfg!(windows) && rest.as_bytes().get(2) == Some(&b':') { &rest[1..] } else { rest };
    let mut out = Vec::with_capacity(rest.len());
    let mut bytes = rest.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = bytes.clone().take(2).collect::<Vec<u8>>();
            if let Some(byte) = std::str::from_utf8(&hex).ok().filter(|h| h.len() == 2).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                bytes.nth(1);
                continue;
            }
        }
        out.push(b);
    }
    String::from_utf8(out).unwrap_or(arg)
}

fn main() -> eframe::Result {
    // First, so the panic hook and every start-up warning are recorded (`logging`).
    let logger = logging::install();
    // Last-resort guard (AGENTS.md §4): commands, edits, opens and saves catch panics and report
    // them; this hook logs every panic, caught or not, with a backtrace when RUST_BACKTRACE is set.
    std::panic::set_hook(Box::new(|info| {
        let trace = std::backtrace::Backtrace::capture();
        let report = if trace.status() == std::backtrace::BacktraceStatus::Captured {
            format!("internal error: {info}\n{trace}")
        } else {
            format!("internal error: {info}")
        };
        // Standard error and the log file; standard error alone when RUST_LOG turned errors off.
        if log::log_enabled!(log::Level::Error) {
            log::error!("{report}");
        } else {
            // `eprintln!` panics on a broken stderr pipe, and a panic inside the panic hook aborts.
            let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("pdfcraft: {report}\n"));
        }
    }));
    let mut files = Vec::new();
    let mut options: Vec<(String, String)> = Vec::new();
    let mut control_file: Option<String> = None;
    let mut create_images = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--version" => {
                println!("pdfcraft {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--control" => control_file = args.next(),
            "--create-images" => create_images = true,
            flag if flag.starts_with("--") => {
                let value = args.next().unwrap_or_default();
                options.push((flag.trim_start_matches("--").to_string(), value));
            }
            _ => files.push(path_from_arg(a)),
        }
    }
    let integrated = cfg!(target_os = "macos");
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("PdfCraft")
        .with_inner_size([1440.0, 920.0])
        .with_min_inner_size([820.0, 520.0])
        .with_drag_and_drop(true)
        // Wayland app id: matches packaging/linux/ai.storyteller.pdfcraft.desktop.
        .with_app_id(APP_ID);
    // Dock, taskbar, Alt-Tab and launcher icon when running unbundled.
    match eframe::icon_data::from_png_bytes(APP_ICON_PNG) {
        Ok(icon) => viewport = viewport.with_icon(icon),
        Err(e) => log::warn!("app icon: {e}"),
    }
    if integrated {
        viewport = viewport.with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false);
    }
    migrate_legacy_folders();
    // The log file lives in the settings folder; opened after the arguments (so `--version` leaves
    // no file behind) and after the PrintCraft migration (which a fresh folder would block).
    // Records logged until now are written to it first.
    if let (Some(logger), Some(dir)) = (logger, settings_dir()) {
        match logger.attach_dir(&dir.join("logs")) {
            Ok(path) => log::info!("PdfCraft {}, log file {}", env!("CARGO_PKG_VERSION"), path.display()),
            // Standard error only by now (`attach_dir` gave up on the file); unlike `eprintln!`, never panics.
            Err(e) => log::warn!("no log file: {e}"),
        }
    }
    let persistence_path = settings_dir().map(|d| d.join("app.ron"));
    let mut native = eframe::NativeOptions { viewport, persistence_path, ..Default::default() };
    configure_gpu(&mut native);
    // Finder, Open With and the Dock deliver files as Apple events, not arguments; catch the one
    // that launched us as well as later ones. Lives until the event loop returns.
    #[cfg(target_os = "macos")]
    let apple_events = apple_events::AppleEvents::install();
    #[cfg(target_os = "macos")]
    let apple_events = &apple_events;
    eframe::run_native(
        "PdfCraft",
        native,
        Box::new(move |cc| {
            let mut app = PdfCraftApp::new();
            if let Some(json) = cc.storage.and_then(|s| s.get_string("pdfcraft").or_else(|| s.get_string(LEGACY_STORAGE_KEY))) {
                app.restore(&json);
            }
            app.integrated_titlebar = integrated;
            app.update_source = Some(std::sync::Arc::new(updates::latest_release));
            app.os_key_store_ids = cfg!(any(target_os = "macos", target_os = "windows"));
            #[cfg(target_os = "macos")]
            {
                app.os_events = Some(apple_events.connect(&cc.egui_ctx));
            }
            if let Some(file) = &control_file {
                let client = app.attach_control(&cc.egui_ctx);
                match pdfcraft_ui_egui::control::serve(client).and_then(|ep| write_control_file(file, ep.port, &ep.token).map(|()| ep.port)) {
                    // Never the token (AGENTS.md §3): it stays in the owner-only file.
                    Ok(port) => log::info!("UI control channel on 127.0.0.1:{port} (connection details in {file})"),
                    Err(e) => log::error!("--control {file}: {e}"),
                }
            }
            // Autosave unsaved changes; offer to recover documents a crashed session left behind.
            if let Some(dir) = pdfcraft_ui_egui::RecoveryStore::default_dir() {
                app.enable_recovery(pdfcraft_ui_egui::RecoveryStore::new(dir));
            }
            // A portable marker whose data folder can't be written (#157): say where settings went.
            if let Some(w) = &pdfcraft_ui_egui::portable::current().unwritable {
                app.notify_fmt(
                    "Portable mode is off: {folder} can't be written ({error}). Settings are kept in your user folder instead.",
                    &[("folder", &w.folder.display().to_string()), ("error", &w.error)],
                );
            }
            if create_images {
                if let Err(e) = app.begin_image_import_paths(&files) {
                    app.notify(e);
                }
            } else {
                // With the preference on, last session's files come back first; files named on
                // the command line open after them, in front (#442).
                app.reopen_last_files(&files);
                for f in files {
                    app.open_path(&f);
                }
            }
            for (k, v) in options {
                if let Err(e) = app.set_option(&k, &v) {
                    log::warn!("--{k} {v}: {e}");
                }
            }
            Ok(Box::new(app))
        }),
    )
}

/// Write the control endpoint so that only the current user can read the token.
///
/// The JSON goes to a new file next to `path`, created fresh (owner-only on Unix), which then
/// replaces `path`. Opening `path` itself would follow a link planted there and truncate whatever
/// it points at; a rename replaces the link and leaves its target alone. A reader polling for the
/// file also never sees it half-written.
fn write_control_file(path: &str, port: u16, token: &str) -> std::io::Result<()> {
    use std::io::{ErrorKind, Write};
    let json = serde_json::json!({ "port": port, "token": token, "pid": std::process::id() }).to_string();
    let path = std::path::Path::new(path);
    let name = path.file_name().ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "not a file name"))?;
    // Unpredictable suffixes, so the name can't be planted in advance. `RandomState` is keyed from
    // the operating system's random source.
    let random = std::hash::RandomState::new();
    for i in 0..16u32 {
        let mut staged = std::ffi::OsString::from(".");
        staged.push(name);
        staged.push(format!(".{:016x}.tmp", std::hash::BuildHasher::hash_one(&random, i)));
        let staged = path.with_file_name(staged);
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = match opts.open(&staged) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            // Windows refuses `create_new` on a folder with "access denied"; the name is taken all
            // the same. A folder we can't write to, with nothing at the name, still fails here.
            Err(e) if e.kind() == ErrorKind::PermissionDenied && staged.symlink_metadata().is_ok() => continue,
            Err(e) => return Err(e),
        };
        // The block closes the file before it is renamed.
        let written = {
            let mut file = file;
            file.write_all(json.as_bytes())
        };
        let done = written.and_then(|()| std::fs::rename(&staged, path));
        if done.is_err() {
            let _ = std::fs::remove_file(&staged);
        }
        return done;
    }
    Err(std::io::Error::new(ErrorKind::AlreadyExists, "no free name for the control file's temporary copy"))
}

/// How wgpu finds a GPU. Each choice yields to its wgpu environment variable.
///
/// - Draw on the integrated GPU unless `WGPU_POWER_PREF` says otherwise. A PDF viewer has no use
///   for a discrete GPU, and on hybrid-graphics laptops (NVIDIA Optimus) the discrete one can lose
///   or corrupt its memory across suspend and screen lock, leaving the window illegible (issue #8).
///   It also saves battery. Machines with one GPU are unaffected.
/// - On Linux, draw on a GPU that a monitor is plugged into. On a desktop whose monitors all hang
///   off the discrete GPU, drawing on the integrated one leaves the window black under Wayland
///   compositors on NVIDIA. Among the GPUs that drive a display, the integrated one still wins.
/// - On Windows, use Direct3D 12, falling back to OpenGL, and never load Vulkan drivers unless
///   `WGPU_BACKEND` asks for them. Creating a Vulkan instance loads every installed Vulkan driver
///   into the process, and a faulty one (an Intel driver in issue #37) crashed PdfCraft before
///   its window appeared. D3D12 is the native, best-supported backend there.
fn configure_gpu(native: &mut eframe::NativeOptions) {
    let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut native.wgpu_options.wgpu_setup else { return };
    if std::env::var_os("WGPU_POWER_PREF").is_none() {
        setup.power_preference = eframe::wgpu::PowerPreference::LowPower;
        #[cfg(target_os = "linux")]
        {
            let displays = linux_display_gpus(std::path::Path::new("/sys/class/drm"));
            // Without sysfs (containers, remote sessions) the power preference alone decides.
            if !displays.is_empty() {
                setup.native_adapter_selector = Some(std::sync::Arc::new(move |adapters, surface| {
                    let usable: Vec<&eframe::wgpu::Adapter> = adapters.iter().filter(|a| surface.is_none_or(|s| a.is_surface_supported(s))).collect();
                    let infos: Vec<(u32, u32, eframe::wgpu::DeviceType)> = usable
                        .iter()
                        .map(|a| {
                            let info = a.get_info();
                            (info.vendor, info.device, info.device_type)
                        })
                        .collect();
                    pick_adapter(&infos, &displays)
                        .and_then(|i| usable.get(i))
                        .map(|a| (*a).clone())
                        .ok_or_else(|| "no GPU can draw to this window".to_string())
                }));
            }
        }
    }
    if cfg!(target_os = "windows") && std::env::var_os("WGPU_BACKEND").is_none() {
        setup.instance_descriptor.backends = eframe::wgpu::Backends::DX12 | eframe::wgpu::Backends::GL;
    }
}

/// PCI `(vendor, device)` ids of the GPUs with a connected monitor, read from the DRM connectors
/// under `drm` (`card1-DP-3/status` is `connected`, `card1/device/{vendor,device}` hold `0x10de`).
#[cfg(target_os = "linux")]
fn linux_display_gpus(drm: &std::path::Path) -> Vec<(u32, u32)> {
    let read_hex = |p: std::path::PathBuf| -> Option<u32> {
        let s = std::fs::read_to_string(p).ok()?;
        u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
    };
    let mut gpus = Vec::new();
    let Ok(entries) = std::fs::read_dir(drm) else { return gpus };
    // A machine has a handful of connectors; the cap only bounds a pathological sysfs.
    for entry in entries.flatten().take(256) {
        let name = entry.file_name();
        let Some((card, _connector)) = name.to_str().and_then(|n| n.split_once('-')) else { continue };
        let connected = std::fs::read_to_string(entry.path().join("status")).is_ok_and(|s| s.trim() == "connected");
        if !connected {
            continue;
        }
        let device = drm.join(card).join("device");
        if let (Some(v), Some(d)) = (read_hex(device.join("vendor")), read_hex(device.join("device")))
            && !gpus.contains(&(v, d))
        {
            gpus.push((v, d));
        }
    }
    gpus
}

/// Index of the adapter to draw with: one that drives a display (by PCI ids) first, then the most
/// frugal kind — integrated, discrete, other, virtual, software.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn pick_adapter(adapters: &[(u32, u32, eframe::wgpu::DeviceType)], displays: &[(u32, u32)]) -> Option<usize> {
    use eframe::wgpu::DeviceType;
    adapters
        .iter()
        .enumerate()
        .min_by_key(|(_, (vendor, device, kind))| {
            let drives_display = displays.contains(&(*vendor, *device));
            let frugality = match kind {
                DeviceType::IntegratedGpu => 0,
                DeviceType::DiscreteGpu => 1,
                DeviceType::Other => 2,
                DeviceType::VirtualGpu => 3,
                DeviceType::Cpu => 4,
            };
            (!drives_display, frugality)
        })
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    #[test]
    fn path_from_arg_decodes_file_uris() {
        assert_eq!(super::path_from_arg("file:///home/alice/report.pdf".to_string()), "/home/alice/report.pdf");
        assert_eq!(super::path_from_arg("file:///home/alice/my%20report.pdf".to_string()), "/home/alice/my report.pdf");
        assert_eq!(super::path_from_arg("file://localhost/tmp/a%C3%A9.pdf".to_string()), "/tmp/aé.pdf");
        // Malformed or truncated escapes are kept as written; bytes that aren't UTF-8 keep the URI.
        assert_eq!(super::path_from_arg("file:///tmp/100%.pdf".to_string()), "/tmp/100%.pdf");
        assert_eq!(super::path_from_arg("file:///tmp/a%2".to_string()), "/tmp/a%2");
        assert_eq!(super::path_from_arg("file:///tmp/%zz%".to_string()), "/tmp/%zz%");
        assert_eq!(super::path_from_arg("file:///tmp/%FF.pdf".to_string()), "file:///tmp/%FF.pdf");
    }

    #[test]
    fn path_from_arg_leaves_plain_paths_and_other_schemes_alone() {
        assert_eq!(super::path_from_arg("report.pdf".to_string()), "report.pdf");
        assert_eq!(super::path_from_arg("/home/alice/report.pdf".to_string()), "/home/alice/report.pdf");
        assert_eq!(super::path_from_arg("https://example.com/report.pdf".to_string()), "https://example.com/report.pdf");
        assert_eq!(super::path_from_arg("file://server/share/a.pdf".to_string()), "file://server/share/a.pdf");
        assert_eq!(super::path_from_arg("file://".to_string()), "file://");
    }

    #[test]
    fn gpu_backends_avoid_vulkan_on_windows_and_prefer_low_power() {
        let mut native = eframe::NativeOptions::default();
        super::configure_gpu(&mut native);
        let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &native.wgpu_options.wgpu_setup else {
            panic!("default setup creates its own instance")
        };
        if std::env::var_os("WGPU_POWER_PREF").is_none() {
            assert_eq!(setup.power_preference, eframe::wgpu::PowerPreference::LowPower);
        }
        if cfg!(target_os = "windows") && std::env::var_os("WGPU_BACKEND").is_none() {
            let backends = setup.instance_descriptor.backends;
            assert!(backends.contains(eframe::wgpu::Backends::DX12), "{backends:?}");
            assert!(!backends.contains(eframe::wgpu::Backends::VULKAN), "issue #37: {backends:?}");
        }
    }

    const NVIDIA: (u32, u32) = (0x10de, 0x2684);
    const AMD_IGPU: (u32, u32) = (0x1002, 0x164e);

    fn adapters() -> Vec<(u32, u32, eframe::wgpu::DeviceType)> {
        use eframe::wgpu::DeviceType;
        vec![(NVIDIA.0, NVIDIA.1, DeviceType::DiscreteGpu), (AMD_IGPU.0, AMD_IGPU.1, DeviceType::IntegratedGpu), (0, 0, DeviceType::Cpu)]
    }

    #[test]
    fn pick_adapter_prefers_the_gpu_driving_the_monitors() {
        // A desktop whose monitors are all on the discrete GPU: the integrated one shows black.
        assert_eq!(super::pick_adapter(&adapters(), &[NVIDIA]), Some(0));
    }

    #[test]
    fn pick_adapter_keeps_the_integrated_gpu_on_hybrid_laptops() {
        // Issue #8: the panel is on the integrated GPU, an external monitor on the discrete one.
        assert_eq!(super::pick_adapter(&adapters(), &[NVIDIA, AMD_IGPU]), Some(1));
        assert_eq!(super::pick_adapter(&adapters(), &[AMD_IGPU]), Some(1));
    }

    #[test]
    fn pick_adapter_falls_back_to_low_power_without_a_match() {
        assert_eq!(super::pick_adapter(&adapters(), &[(0x8086, 0x1234)]), Some(1));
        assert_eq!(super::pick_adapter(&[], &[NVIDIA]), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_display_gpus_reads_connected_connectors() -> std::io::Result<()> {
        let dir = std::env::temp_dir().join(format!("pdfcraft-drm-{}", std::process::id()));
        let card = |name: &str, (vendor, device): (u32, u32)| -> std::io::Result<()> {
            std::fs::create_dir_all(dir.join(name).join("device"))?;
            std::fs::write(dir.join(name).join("device/vendor"), format!("{vendor:#06x}\n"))?;
            std::fs::write(dir.join(name).join("device/device"), format!("{device:#06x}\n"))
        };
        let connector = |name: &str, status: &str| -> std::io::Result<()> {
            std::fs::create_dir_all(dir.join(name))?;
            std::fs::write(dir.join(name).join("status"), format!("{status}\n"))
        };
        card("card1", NVIDIA)?;
        card("card2", AMD_IGPU)?;
        connector("card1-DP-3", "connected")?;
        connector("card1-DP-4", "connected")?;
        connector("card2-HDMI-A-1", "disconnected")?;
        connector("card2-Writeback-1", "unknown")?;
        let gpus = super::linux_display_gpus(&dir);
        std::fs::remove_dir_all(&dir)?;
        assert_eq!(gpus, vec![NVIDIA]);
        assert!(super::linux_display_gpus(std::path::Path::new("/nonexistent/drm")).is_empty());
        Ok(())
    }

    /// A fresh folder per call: tests run in parallel.
    fn scratch() -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("pdfcraft-control-file-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The control file's port and token, and every name in its folder.
    fn written(dir: &std::path::Path) -> (serde_json::Value, Vec<String>) {
        let json = serde_json::from_str(&std::fs::read_to_string(dir.join("ctl.json")).unwrap()).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        (json, names)
    }

    #[test]
    fn the_control_file_replaces_a_hard_link_instead_of_writing_through_it() {
        // The file used to be opened with create + truncate, so a link planted at the path had its
        // target truncated and overwritten with the token.
        let dir = scratch();
        std::fs::write(dir.join("victim.txt"), "keep me").unwrap();
        std::fs::hard_link(dir.join("victim.txt"), dir.join("ctl.json")).unwrap();
        super::write_control_file(&dir.join("ctl.json").to_string_lossy(), 4242, "t0ken").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("victim.txt")).unwrap(), "keep me");
        let (json, names) = written(&dir);
        assert_eq!((json["port"].as_u64(), json["token"].as_str()), (Some(4242), Some("t0ken")));
        assert_eq!(names, ["ctl.json", "victim.txt"], "no temporary file left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_control_file_replaces_a_symlink_instead_of_writing_through_it() {
        let dir = scratch();
        let (link, target) = (dir.join("ctl.json"), dir.join("victim.txt"));
        std::fs::write(&target, "keep me").unwrap();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target, &link);
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&target, &link);
        if let Err(e) = made {
            // Windows needs Developer Mode (or admin) for symlinks.
            eprintln!("skipped: can't create a symlink here: {e}");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        super::write_control_file(&link.to_string_lossy(), 4242, "t0ken").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep me");
        assert!(!std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(written(&dir).0["token"].as_str(), Some("t0ken"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_control_file_replaces_the_one_a_previous_run_left() {
        let dir = scratch();
        std::fs::write(dir.join("ctl.json"), r#"{"port":1,"token":"old"}"#).unwrap();
        super::write_control_file(&dir.join("ctl.json").to_string_lossy(), 4242, "t0ken").unwrap();
        let (json, names) = written(&dir);
        assert_eq!(json["token"].as_str(), Some("t0ken"));
        assert_eq!(names, ["ctl.json"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("ctl.json")).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "only the owner can read the token: {mode:o}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
