//! PdfCraft desktop app.
//!
//! Usage: `pdfcraft [options] [files…]` (`-h` / `--help` prints a short version of this, [`USAGE`]).
//! `--two-paged` opens each command-line PDF in noncontinuous two-page view, without changing defaults.
//! An explicit `--layout` takes precedence for the active document, regardless of argument order.
//! `--create-images [images…]` stages the images in one PDF and asks for the page DPI.
//!
//! View options (applied after the files open; also the seed of the UI control channel):
//! `--page N  --zoom 150  --layout single|continuous|two-up|two-page  --continuous on|off  --panel comments|bookmarks|pages|fields|layers|attachments|none
//!  --theme light|dark|system  --language auto|<code>  --mode all|read|edit|convert|sign  --tool <catalogue id>  --left open|closed
//!  --organize on  --fields on  --dialog properties|shortcuts|about  --palette <query>  --home on
//!  --cover on|off  --default-layout single|continuous|two-up|two-page  --default-continuous on|off  --default-zoom fit-width|fit-page|<percent>  --always-default-view on|off
//!  --page-gaps on|off  --default-page-gaps on|off
//!  --combine-view grid|list  --combine-zoom 60..200`
//!
//! `--new-window` opens a window of its own. Without it, on Windows, a launch that only names files
//! hands them to the PdfCraft already running, where they open as tabs (`single_instance`).
//!
//! `--control <file>` enables the UI control channel (off by default): the app listens on a random
//! loopback port and writes `{"port", "token", "pid"}` to `<file>` (owner-only permissions). The
//! app doesn't start if it can't.
//! Agents then drive it with `pdfcraft-cli ui --control <file> <method> …`.

// Release builds on Windows are GUI-subsystem programs, so launching the app doesn't open a console
// window next to it (#57). `--version` and diagnostics then go nowhere when started from a terminal
// (std ignores the missing console handles, so nothing fails); debug builds keep the console.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::cell::Cell;
use std::rc::Rc;

use pdfcraft_ui_egui::PdfCraftApp;

#[cfg(target_os = "macos")]
mod apple_events;
mod compositor_gpu;
mod decorations;
mod logging;
#[cfg(target_os = "windows")]
mod motion;
mod reopen;
mod settings;
mod single_instance;
mod startup;
mod text_scale;
#[cfg(test)]
#[path = "windows_manifest.rs"]
mod windows_manifest;

/// Freedesktop app id: the `.desktop` file name and the hicolor icon name.
const APP_ID: &str = "ai.storyteller.pdfcraft";

/// The app icon (assets/app-icon/README.md). macOS gets the version on Apple's icon grid, with a
/// transparent margin; Windows and Linux get the full-bleed tile.
#[cfg(target_os = "macos")]
const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/app-icon/pdfcraft-1024.png");
#[cfg(not(target_os = "macos"))]
const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/ai.storyteller.pdfcraft.png");

/// What `-h` / `--help` prints (#4076): a hint, not the full option list (that is this file's
/// doc comment and the README). Scripts want `pdfcraft-cli`, which never opens a window.
const USAGE: &str = "\
Usage: pdfcraft [options] [files…]

Opens the files in the PdfCraft desktop app.

Options:
  --two-paged                 open command-line PDFs in noncontinuous two-page view
  --new-window                open a window of its own instead of passing the files to a running PdfCraft
  --create-images [images…]   put the images in one PDF and ask for the page DPI
  --control <file>            enable the UI control channel (pdfcraft-cli ui --control <file> …)
  --page N, --zoom 150, --layout continuous, --theme dark, …
                              view settings applied once the files are open
  -h, --help                  print this and exit
  --version                   print the version and exit

To inspect, convert or edit PDFs from a script, use pdfcraft-cli (pdfcraft-cli --help).";

/// Print `text` for `--help` and `--version`. `println!` would panic on a full or closed
/// standard output; this exits normally instead.
fn print_and_exit(text: &str) -> eframe::Result {
    use std::io::Write;
    let _ = writeln!(std::io::stdout().lock(), "{text}");
    Ok(())
}

/// The app was called PrintCraft before; settings saved then are under this key.
const LEGACY_STORAGE_KEY: &str = "printcraft";

/// The preferences folder (`app.ron`, docs/development.md). In portable mode
/// it is `PdfCraftData` beside the executable (#157). XDG desktops use the configuration base;
/// other platforms retain eframe's PdfCraft folder. Logs and recovery stay in the data folder.
fn settings_dir() -> Option<std::path::PathBuf> {
    let legacy = eframe::storage_dir("PdfCraft");
    match settings::directory(
        cfg!(all(unix, not(target_os = "macos"))),
        pdfcraft_ui_egui::portable::data_dir(),
        legacy.clone(),
        std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from),
        std::env::home_dir(),
    ) {
        Ok(dir) => dir,
        Err(e) => {
            log::warn!("could not migrate preferences to the XDG configuration folder; using the original location: {e}");
            legacy
        }
    }
}

/// Move the settings and crash-recovery folders of the app's former name, PrintCraft, to the new
/// name once, so an upgrade keeps recent files, preferences and unsaved work. Best effort: a
/// folder is left alone when the new one already exists or the move fails. A portable copy leaves
/// the per-user folders alone.
fn migrate_legacy_folders() {
    if pdfcraft_ui_egui::portable::data_dir().is_some() {
        return;
    }
    let mut moves = vec![(eframe::storage_dir("PrintCraft"), eframe::storage_dir("PdfCraft"))];
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

/// Some programs start the PDF reader with the path unquoted (`pdfcraft.exe P:\Docs\01 Contract.PDF`),
/// so a path with spaces arrives split into several arguments (Acrobat accepts this). When an
/// argument names no file but it and the ones after it, joined with one space, do, rejoin them
/// (the longest such run). Anything else is kept as given. Runs of several spaces can't be recovered: the split has lost them.
fn rejoin_split_paths(args: Vec<String>, exists: impl Fn(&str) -> bool) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut i = 0;
    while let Some(first) = args.get(i) {
        if exists(first) {
            out.push(first.clone());
            i += 1;
            continue;
        }
        // The longest run starting here that names a file; capped so a long list stays cheap.
        let end = (i + 2..=args.len().min(i + 32)).rev().find(|&j| args.get(i..j).is_some_and(|run| exists(&run.join(" "))));
        match end.and_then(|j| Some((j, args.get(i..j)?.join(" ")))) {
            Some((j, joined)) => {
                out.push(joined);
                i = j;
            }
            None => {
                out.push(first.clone());
                i += 1;
            }
        }
    }
    out
}

fn main() -> eframe::Result {
    // Windows: the update helper is a copy of this program started with `--finish-update`.
    #[cfg(windows)]
    if let Some(result) = craft_update::self_update::finish(&pdfcraft_ui_egui::UPDATE_APP, &std::env::args_os().skip(1).collect::<Vec<_>>()) {
        std::process::exit(i32::from(result.is_err()));
    }
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
    let mut startup = startup::Arguments::parse(std::env::args().skip(1));
    if startup.version {
        return print_and_exit(&format!("pdfcraft {}", pdfcraft_engine::build_info::version()));
    }
    if startup.help {
        return print_and_exit(USAGE);
    }
    startup.files = rejoin_split_paths(startup.files, |p| std::path::Path::new(p).is_file());
    let may_hand_off = startup.may_hand_off();
    let startup::Arguments { files, options, control_file, create_images, two_paged, .. } = startup;
    let integrated = cfg!(target_os = "macos");
    // GNOME on Wayland leaves the window frame to the app (src/decorations.rs).
    let window_controls = if cfg!(any(target_os = "macos", target_os = "windows")) { None } else { decorations::app_drawn() };
    migrate_legacy_folders();
    // The log file stays in the data folder; opened after the arguments (so `--version` leaves
    // no file behind) and after the PrintCraft migration (which a fresh folder would block).
    // Records logged until now are written to it first.
    if let (Some(logger), Some(dir)) =
        (logger, pdfcraft_ui_egui::portable::data_dir().map(std::path::Path::to_path_buf).or_else(|| eframe::storage_dir("PdfCraft")))
    {
        match logger.attach_dir(&dir.join("logs")) {
            Ok(path) => log::info!("PdfCraft {}, log file {}", pdfcraft_engine::build_info::version(), path.display()),
            // Standard error only by now (`attach_dir` gave up on the file); unlike `eprintln!`, never panics.
            Err(e) => log::warn!("no log file: {e}"),
        }
    }
    // Windows, Linux and the BSDs: a launch that only names files (an Outlook attachment, an
    // Explorer double-click, a file manager or `xdg-open`) hands them to the PdfCraft already
    // running and exits (#282, #317, #1726). Any option means the caller wants this launch's own
    // window.
    // Absolute for this instance too: links relative to a document resolve against its folder,
    // never the working directory.
    let files = single_instance::absolute(&files);
    let instance = match single_instance::claim(&files, may_hand_off) {
        single_instance::Claim::HandedOff => return Ok(()),
        single_instance::Claim::Primary(server) => Some(server),
        single_instance::Claim::Alone => None,
    };
    let choice = renderer_choice(std::env::var("PDFCRAFT_RENDERER").ok().as_deref());
    let launch = Launch { files, options, control_file, create_images, integrated, window_controls, two_paged };
    // Finder, Open With and the Dock deliver files as Apple events, not arguments; catch the one
    // that launched us as well as later ones. Lives until the event loop returns.
    #[cfg(target_os = "macos")]
    let apple_events = apple_events::AppleEvents::install();
    // Set once the app is created, which is after the renderer has started.
    let started = Rc::new(Cell::new(false));
    let first = if choice == RendererChoice::Gl { eframe::Renderer::Glow } else { eframe::Renderer::Wgpu };
    let result = eframe::run_native(
        "PdfCraft",
        native_options(launch.integrated, launch.window_controls.is_some(), first),
        app_creator(
            launch.clone(),
            Rc::clone(&started),
            instance.as_ref(),
            #[cfg(target_os = "macos")]
            &apple_events,
        ),
    );
    match result {
        // Only a renderer that couldn't start: without a window or display at all, OpenGL can't
        // help either, and winit's own error says more.
        Err(e @ eframe::Error::Wgpu(_)) if retry_with_gl(choice, started.get()) => {
            // Old or unusual GPUs and drivers (#461, #435, #392) can't give wgpu a device; OpenGL
            // usually still works there, so that's better than quitting.
            log::error!("the GPU renderer (wgpu) didn't start: {e}. Starting with OpenGL instead; set PDFCRAFT_RENDERER=gl to skip wgpu.");
            eframe::run_native(
                "PdfCraft",
                native_options(launch.integrated, launch.window_controls.is_some(), eframe::Renderer::Glow),
                app_creator(
                    launch,
                    started,
                    instance.as_ref(),
                    #[cfg(target_os = "macos")]
                    &apple_events,
                ),
            )
        }
        other => other,
    }
}

/// Which renderer to start with, from `PDFCRAFT_RENDERER`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RendererChoice {
    /// wgpu, then OpenGL if wgpu can't start (unset, or anything unrecognised).
    Auto,
    /// wgpu only (`wgpu`): a failure is reported, not worked around.
    Wgpu,
    /// OpenGL only (`gl`, `opengl` or `glow`): for drivers where wgpu starts but misbehaves.
    Gl,
}

fn renderer_choice(value: Option<&str>) -> RendererChoice {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("wgpu") => RendererChoice::Wgpu,
        Some("gl" | "opengl" | "glow") => RendererChoice::Gl,
        _ => RendererChoice::Auto,
    }
}

/// Whether a failed run should be retried with OpenGL: only when wgpu was tried first by choice of
/// nobody, and it failed before the app was created (so while starting the renderer, not later).
fn retry_with_gl(choice: RendererChoice, app_started: bool) -> bool {
    choice == RendererChoice::Auto && !app_started
}

/// The window and renderer settings for one run.
fn native_options(integrated: bool, app_frame: bool, renderer: eframe::Renderer) -> eframe::NativeOptions {
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
    if app_frame {
        // The tab strip is the title bar (pdfcraft_ui_egui::window_controls).
        viewport = viewport.with_decorations(false);
    }
    let persistence_path = settings_dir().map(|d| d.join("app.ron"));
    // Set explicitly, so the renderer never depends on which one eframe defaults to.
    let mut native = eframe::NativeOptions { viewport, persistence_path, renderer, ..Default::default() };
    // PdfCraft installs its own last-resort system face (`PDFCRAFT_SYSTEM_FONTS`); eframe's
    // provider would read every installed face, including proprietary ones, into every UI pass.
    native.system_font_fallback = false;
    if renderer == eframe::Renderer::Wgpu {
        configure_gpu(&mut native);
    }
    native
}

/// What the command line asked for, kept so a second run (with OpenGL) can start the same way.
#[derive(Clone)]
struct Launch {
    files: Vec<String>,
    options: Vec<(String, String)>,
    control_file: Option<String>,
    create_images: bool,
    integrated: bool,
    /// The window buttons the app draws, when the desktop leaves the frame to it.
    window_controls: Option<pdfcraft_ui_egui::window_controls::Layout>,
    two_paged: bool,
}

/// Whether the window is a native Wayland one (not X11 or XWayland).
#[cfg(all(unix, not(target_os = "macos")))]
fn native_wayland(cc: &eframe::CreationContext<'_>) -> bool {
    use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    cc.display_handle().is_ok_and(|handle| matches!(handle.as_raw(), RawDisplayHandle::Wayland(_)))
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn native_wayland(_: &eframe::CreationContext<'_>) -> bool {
    false
}

fn app_creator<'a>(
    launch: Launch,
    started: Rc<Cell<bool>>,
    instance: Option<&'a single_instance::Server>,
    #[cfg(target_os = "macos")] apple_events: &'a apple_events::AppleEvents,
) -> eframe::AppCreator<'a> {
    let Launch { files, options, control_file, create_images, integrated, window_controls, two_paged } = launch;
    Box::new(move |cc| {
        started.set(true);
        let mut app = PdfCraftApp::new();
        // First start (or settings from before Units existed): the region's unit.
        app.units = pdfcraft_ui_egui::units::Unit::system();
        #[cfg(target_os = "linux")]
        app.enable_desktop_theme();
        // The system's reopen-windows choice (macOS) starts the preference; a saved choice replaces it.
        app.reopen_last_session = reopen::system_default();
        if let Some(json) = cc.storage.and_then(|s| s.get_string("pdfcraft").or_else(|| s.get_string(LEGACY_STORAGE_KEY))) {
            app.restore(&json);
        }
        app.window_state = pdfcraft_ui_egui::window_state::WindowState::started_with(cc.storage);
        app.integrated_titlebar = integrated;
        app.window_controls = window_controls;
        app.system_text_scale = text_scale::system();
        // Windows' Animation effects setting, which Preferences ▸ Animation effects follows unless
        // it is set to On or Off.
        #[cfg(target_os = "windows")]
        {
            app.motion.source = Some(motion::source());
        }
        app.set_startup_update_check(true);
        app.printer_source = Some(pdfcraft_ui_egui::system_printers());
        app.os_key_store_ids = cfg!(any(target_os = "macos", target_os = "windows"));
        #[cfg(target_os = "macos")]
        {
            app.os_events = Some(apple_events.connect(&cc.egui_ctx));
            // The native menu bar. Its Quit (⌘Q) closes the window rather than ending the process as
            // winit's does (#1136), so `guard_quit` asks about unsaved changes, as for the close
            // button.
            if !app.install_native_menu(&cc.egui_ctx) {
                log::warn!("couldn't install the macOS menu bar; the menus stay in the window");
            }
        }
        if let Some(server) = instance {
            app.os_events = Some(server.connect(&cc.egui_ctx, native_wayland(cc)));
        }
        if let Some(file) = &control_file {
            let client = app.attach_control(&cc.egui_ctx);
            open_control_channel(file, pdfcraft_ui_egui::control::serve(client))?;
        }
        // Autosave unsaved changes; offer to recover documents a crashed session left behind.
        if let Some(dir) = pdfcraft_ui_egui::RecoveryStore::default_dir() {
            app.enable_recovery(pdfcraft_ui_egui::RecoveryStore::new(dir));
        }
        if let Some(state) = &cc.wgpu_render_state {
            let adapter = state.adapter.get_info();
            let max_texture_dimension_2d = state.device.limits().max_texture_dimension_2d;
            log::info!(
                "renderer: wgpu; adapter: {} ({:?}, {:?}); max_texture_dimension_2d: {max_texture_dimension_2d}",
                adapter.name,
                adapter.device_type,
                adapter.backend
            );
            notify_software_renderer(&mut app, adapter.device_type);
        } else {
            log::info!("renderer: OpenGL (glow)");
        }
        // A portable marker whose data folder can't be written (#157): say where settings went.
        if let Some(w) = &pdfcraft_ui_egui::portable::current().unwritable {
            app.notify_fmt(
                "Portable mode is off: {folder} can't be written ({error}). Settings are kept in your user folder instead.",
                &[("folder", &w.folder.display().to_string()), ("error", &w.error)],
            );
        }
        let defer_layout = two_paged && !files.is_empty() && !create_images;
        if create_images {
            if let Err(e) = app.begin_image_import_paths(&files) {
                app.notify(e);
            }
        } else {
            // With the preference on, last session's files come back first; files named on
            // the command line open after them, in front (#442).
            app.reopen_last_files(&files);
            startup::open_files(&mut app, &files, two_paged);
        }
        for (k, v) in options {
            let result = if defer_layout && k == "layout" { app.set_launch_layout(&v) } else { app.set_option(&k, &v) };
            if let Err(e) = result {
                log::warn!("--{k} {v}: {e}");
            }
        }
        Ok(Box::new(app))
    })
}

/// Tell the user when wgpu draws on the processor (WARP on Windows, llvmpipe on Linux) rather than
/// a GPU, which makes everything slower. egui-wgpu only logs it, so the reporter of #519 had to find
/// it in pdfcraft.log. The OpenGL fallback isn't covered: glow only reports its renderer through
/// `unsafe` calls.
fn notify_software_renderer(app: &mut PdfCraftApp, device_type: eframe::wgpu::DeviceType) {
    if device_type == eframe::wgpu::DeviceType::Cpu {
        app.notify_tr("PdfCraft is drawing without a graphics processor, so it may be slow. Updating the graphics driver may help.");
    }
}

/// Publish a started control channel in `file`.
///
/// A failure stops the app. `--control` was asked for, so carrying on without it would leave a
/// script driving nothing, and the file that couldn't be replaced may be another user's, planted
/// at a shared path such as `/tmp/pc.json` to receive the commands. This runs before any document
/// opens, so nothing is lost.
fn open_control_channel(file: &str, endpoint: std::io::Result<pdfcraft_ui_egui::control::Endpoint>) -> Result<(), String> {
    match endpoint.and_then(|ep| write_control_file(file, ep.port, &ep.token).map(|()| ep.port)) {
        // Never the token (AGENTS.md §3): it stays in the owner-only file.
        Ok(port) => {
            log::info!("UI control channel on 127.0.0.1:{port} (connection details in {file})");
            Ok(())
        }
        Err(e) => {
            let message = format!("--control {file}: {e}. Use a control file in a folder only you can write, such as ~/.pdfcraft-control.json");
            log::error!("{message}");
            Err(message)
        }
    }
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
/// - Draw on a GPU that a monitor is plugged into. Drawing on another one means every frame is
///   handed to the GPU that drives the display, and that path fails: on a desktop whose monitors
///   all hang off the discrete GPU, the integrated one leaves the window black under Wayland
///   compositors on NVIDIA, and on Windows an AMD Ryzen integrated GPU takes the display driver
///   down with it, blacking out every monitor for minutes (issue #378). When several GPUs drive a
///   display, the one driving the display the user most likely looks at wins: a built-in panel on
///   Linux (a hybrid laptop, issue #8), the primary display on Windows. Without either (a Linux
///   desktop with a monitor on each GPU, issue #445) the discrete one wins, as the integrated one
///   may fail to present there. Where the system doesn't say which GPU drives a display (macOS,
///   containers, remote sessions) the power preference alone decides.
/// - Under Wayland, the GPU the compositor draws with comes before all of that: the compositor
///   says which one it is ([`compositor_gpu::compositor_gpu`]), and frames from another GPU may
///   never reach the screen, even from the GPU behind the built-in panel when the compositor runs
///   on the discrete one.
/// - On Windows, use Direct3D 12, falling back to OpenGL, and never load Vulkan drivers unless
///   `WGPU_BACKEND` asks for them. Creating a Vulkan instance loads every installed Vulkan driver
///   into the process, and a faulty one (an Intel driver in issue #37) crashed PdfCraft before
///   its window appeared. D3D12 is the native, best-supported backend there.
/// - On Windows, Direct3D 12 presents through a DirectComposition visual rather than a swap chain
///   on the window itself, unless `WGPU_DX12_PRESENTATION_SYSTEM` says otherwise. While the window
///   is being resized, Windows stretched the window's swap chain, still at the old size, to the
///   new one, so the text shimmered and the left panel's width bounced for as long as the drag
///   lasted (issue #2738). A visual's last frame is shown at its own size instead.
fn configure_gpu(native: &mut eframe::NativeOptions) {
    let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut native.wgpu_options.wgpu_setup else { return };
    // Nothing draws or dispatches indirectly, so wgpu's check of indirect arguments only costs a
    // compute shader when the device is created, one that Metal can't compile on an Intel Iris
    // Pro: the device is lost and the window falls back to OpenGL. `WGPU_VALIDATION_INDIRECT_CALL=1`
    // turns the check back on.
    setup.instance_descriptor.flags = setup.instance_descriptor.flags.difference(eframe::wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL).with_env();
    // egui-wgpu requests only 8192 pixels even when the adapter can render larger surfaces.
    // A restored 3440-point window at 250% DPI requests 8600 pixels and otherwise panics in
    // Surface::configure before the app starts. Keep the renderer's other compatible requirements,
    // but enable the adapter's actual 2D texture extent (also for smaller/downlevel adapters).
    let device_descriptor = std::sync::Arc::clone(&setup.device_descriptor);
    setup.device_descriptor = std::sync::Arc::new(move |adapter| {
        let mut descriptor = device_descriptor(adapter);
        descriptor.required_limits = surface_texture_limits(descriptor.required_limits, &adapter.limits());
        descriptor
    });
    if std::env::var_os("WGPU_POWER_PREF").is_none() {
        setup.power_preference = eframe::wgpu::PowerPreference::LowPower;
        let displays = display_gpus();
        let compositor = compositor_gpu::compositor_gpu();
        if !displays.is_empty() || compositor.is_some() {
            setup.native_adapter_selector = Some(std::sync::Arc::new(move |adapters, surface| {
                let usable: Vec<&eframe::wgpu::Adapter> = adapters.iter().filter(|a| surface.is_none_or(|s| a.is_surface_supported(s))).collect();
                let infos: Vec<(u32, u32, eframe::wgpu::DeviceType)> = usable
                    .iter()
                    .map(|a| {
                        let info = a.get_info();
                        (info.vendor, info.device, info.device_type)
                    })
                    .collect();
                let picked = pick_adapter(&infos, &displays, compositor).and_then(|i| usable.get(i)).map(|a| (*a).clone());
                match &picked {
                    // Which GPU draws is the first question when a window stays black (#8, #378, #445).
                    Some(a) => {
                        let info = a.get_info();
                        log::info!("drawing on {} ({:?}, {:?})", info.name, info.device_type, info.backend);
                    }
                    None => log::warn!("no GPU can draw to this window; {} were considered", usable.len()),
                }
                picked.ok_or_else(|| "no GPU can draw to this window".to_string())
            }));
        }
    }
    if cfg!(target_os = "windows") && std::env::var_os("WGPU_BACKEND").is_none() {
        setup.instance_descriptor.backends = eframe::wgpu::Backends::DX12 | eframe::wgpu::Backends::GL;
    }
    if cfg!(target_os = "windows") {
        setup.instance_descriptor.backend_options.dx12.presentation_system = dx12_presentation(eframe::wgpu::Dx12SwapchainKind::from_env());
    }
}

/// How Direct3D 12 presents on Windows: through a DirectComposition visual (issue #2738), unless
/// `WGPU_DX12_PRESENTATION_SYSTEM` asked for something else (`asked`).
fn dx12_presentation(asked: Option<eframe::wgpu::Dx12SwapchainKind>) -> eframe::wgpu::Dx12SwapchainKind {
    asked.unwrap_or(eframe::wgpu::Dx12SwapchainKind::DxgiFromVisual)
}

/// Surface textures must fit the device's enabled limits, not just the physical GPU's limits.
/// Clamp egui-wgpu's backend-specific requirements before enabling the adapter's full 2D extent.
fn surface_texture_limits(mut required: eframe::wgpu::Limits, supported: &eframe::wgpu::Limits) -> eframe::wgpu::Limits {
    required = required.or_worse_values_from(supported);
    required.max_texture_dimension_2d = supported.max_texture_dimension_2d;
    required
}

/// A GPU with a connected monitor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DisplayGpu {
    /// PCI `(vendor, device)` ids.
    pci: (u32, u32),
    /// Whether it drives the display the user most likely looks at: on Linux a built-in panel
    /// (`eDP`, `LVDS` or `DSI`), on Windows the primary display.
    primary: bool,
}

/// The GPUs with a connected monitor, as far as the system says: Linux reads the DRM connectors
/// in sysfs, Windows asks for the display devices attached to the desktop. Empty elsewhere.
fn display_gpus() -> Vec<DisplayGpu> {
    #[cfg(target_os = "linux")]
    {
        linux_display_gpus(std::path::Path::new("/sys/class/drm"))
    }
    #[cfg(target_os = "windows")]
    {
        windows_display_gpus()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        Vec::new()
    }
}

/// Records one display of GPU `pci`; a GPU driving several displays is one entry.
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
fn add_display_gpu(gpus: &mut Vec<DisplayGpu>, pci: (u32, u32), primary: bool) {
    match gpus.iter_mut().find(|g| g.pci == pci) {
        Some(gpu) => gpu.primary |= primary,
        None => gpus.push(DisplayGpu { pci, primary }),
    }
}

/// The GPUs driving a display attached to the desktop, from `EnumDisplayDevices`. Each entry it
/// lists is one display device of an adapter (`\\.\DISPLAY1`, "NVIDIA GeForce RTX 3090"), with the
/// adapter's PCI ids in its device id (`PCI\VEN_10DE&DEV_2204&SUBSYS_40421458&REV_A1`); a GPU with
/// no monitor lists its devices as not attached. Issue #378: a Ryzen desktop with the monitors on
/// an NVIDIA card, where the integrated GPU would otherwise be chosen.
#[cfg(target_os = "windows")]
fn windows_display_gpus() -> Vec<DisplayGpu> {
    use winsafe::co::DISPLAY_DEVICE as Flags;
    let mut gpus: Vec<DisplayGpu> = Vec::new();
    // A machine has a handful of display devices; the cap only bounds a runaway enumeration.
    for device in winsafe::EnumDisplayDevices(None, None).take(256) {
        let device = match device {
            Ok(d) => d,
            // The enumeration ends with "no more items" or, from a driver, with any error.
            Err(e) => {
                log::debug!("display devices: {e}");
                break;
            }
        };
        if !device.StateFlags.has(Flags::ATTACHED_TO_DESKTOP) {
            continue;
        }
        let Some(pci) = pci_ids(&device.DeviceID()) else { continue };
        add_display_gpu(&mut gpus, pci, device.StateFlags.has(Flags::PRIMARY_DEVICE));
    }
    gpus
}

/// The PCI `(vendor, device)` ids in a Windows device id such as
/// `PCI\VEN_10DE&DEV_2204&SUBSYS_40421458&REV_A1`; `None` for any other kind of device.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn pci_ids(device_id: &str) -> Option<(u32, u32)> {
    let field = |key: &str| {
        device_id.split(['\\', '&']).find_map(|part| {
            let (name, hex) = part.split_at_checked(key.len())?;
            if name.eq_ignore_ascii_case(key) { u32::from_str_radix(hex, 16).ok() } else { None }
        })
    };
    Some((field("VEN_")?, field("DEV_")?))
}

/// Whether a DRM connector name (`eDP-1`, `LVDS-1`, `DSI-1`) is a laptop's built-in panel.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn is_internal_panel(connector: &str) -> bool {
    ["eDP", "LVDS", "DSI"].iter().any(|kind| connector.starts_with(kind))
}

/// The GPUs with a connected monitor, read from the DRM connectors under `drm` (`card1-DP-3/status`
/// is `connected`, `card1/device/{vendor,device}` hold `0x10de`).
#[cfg(target_os = "linux")]
fn linux_display_gpus(drm: &std::path::Path) -> Vec<DisplayGpu> {
    let read_hex = |p: std::path::PathBuf| -> Option<u32> {
        let s = std::fs::read_to_string(p).ok()?;
        u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
    };
    let mut gpus: Vec<DisplayGpu> = Vec::new();
    let Ok(entries) = std::fs::read_dir(drm) else { return gpus };
    // A machine has a handful of connectors; the cap only bounds a pathological sysfs.
    for entry in entries.flatten().take(256) {
        let name = entry.file_name();
        let Some((card, connector)) = name.to_str().and_then(|n| n.split_once('-')) else { continue };
        let connected = std::fs::read_to_string(entry.path().join("status")).is_ok_and(|s| s.trim() == "connected");
        if !connected {
            continue;
        }
        let device = drm.join(card).join("device");
        let (Some(v), Some(d)) = (read_hex(device.join("vendor")), read_hex(device.join("device"))) else { continue };
        add_display_gpu(&mut gpus, (v, d), is_internal_panel(connector));
    }
    gpus
}

/// Index of the adapter to draw with: the GPU the Wayland compositor draws with (`compositor`, by
/// PCI ids) first, then one that drives a display. Among several of those, the one driving the
/// primary display (a hybrid laptop's panel, #8; the primary display on Windows, #378), or else,
/// with no primary display known (a Linux desktop with a monitor on each GPU, #445), the discrete
/// one. Otherwise the most frugal kind: integrated, discrete, other, virtual, software.
fn pick_adapter(adapters: &[(u32, u32, eframe::wgpu::DeviceType)], displays: &[DisplayGpu], compositor: Option<(u32, u32)>) -> Option<usize> {
    use eframe::wgpu::DeviceType;
    let display = |pci: (u32, u32)| displays.iter().find(|g| g.pci == pci);
    let multi_gpu_desktop = displays.len() > 1 && !displays.iter().any(|g| g.primary);
    adapters
        .iter()
        .enumerate()
        .min_by_key(|(_, (vendor, device, kind))| {
            let shown = display((*vendor, *device));
            let drives_display = shown.is_some();
            let drives_primary = shown.is_some_and(|g| g.primary);
            let rank = match kind {
                // On a desktop with monitors on both, the integrated GPU can accept the window and
                // still fail to present to it (#445).
                DeviceType::DiscreteGpu if drives_display && multi_gpu_desktop => 0,
                DeviceType::IntegratedGpu => 1,
                DeviceType::DiscreteGpu => 2,
                DeviceType::Other => 3,
                DeviceType::VirtualGpu => 4,
                DeviceType::Cpu => 5,
            };
            (compositor != Some((*vendor, *device)), !drives_display, !drives_primary, rank)
        })
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests;
