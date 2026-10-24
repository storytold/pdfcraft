#![cfg(target_os = "android")]

//! PrintCraft on Android — GameActivity + eframe/wgpu, reusing the desktop engine and egui UI.
//!
//! Nothing here is Android-specific beyond the entry point, the storage directories, the file
//! dialogs, the touch adjustments and the font fallback: the same `pdfcraft_engine::Session` and
//! `pdfcraft_ui_egui::PdfCraftApp` the desktop shell builds are wired up here, and every action
//! goes through the app's own command registry (`PdfCraftApp::execute`) or its own document
//! methods. What the desktop shell gets from the operating system, this crate takes from the
//! activity:
//!
//! | Desktop (`apps/pdfcraft`) | Android (this crate) |
//! |---|---|
//! | `std::env::args()` launch flags | [`winit::platform::android::activity::AndroidApp`] |
//! | settings under `~/Library/Application Support/PdfCraft` | [`AndroidApp::internal_data_path`] |
//! | `rfd` file dialogs | the Storage Access Framework through the `fd` host seam ([`saf`]) |
//! | `RecoveryStore::default_dir()` | `<data>/recovery`, so unsaved work survives a restart |
//! | Finder's Apple events | not applicable |
//! | 1440×920 desktop layout, 11 pt menus | [`TouchShell`]: 40 pt targets, Read mode, a long-press command palette |
//!
//! PrintCraft is pure Rust end to end — the PDF parser and rasterizer are its own crates (hayro)
//! and the fonts are Skrifa — so there is no native library to cross-compile and no platform FFI
//! beyond the Java picker. [`selftest`] proves that on start-up by parsing, rasterizing and reading
//! the text of an embedded PDF through the engine's own render path.
//!
//! The engine, the UI and the theme are untouched: the menus, the commands and the canvas are the
//! desktop ones. The shell only adjusts egui's spacing *after* PrintCraft installs its own theme
//! on the first frame (adding fonts any earlier drops the theme's `FontFamily::Name("semibold")`
//! binding and egui panics).

use std::sync::{Arc, Mutex, OnceLock};

use pdfcraft_ui_egui::{Mode, PdfCraftApp};
use winit::platform::android::activity::AndroidApp;

mod saf;
mod selftest;

/// How much to scale the desktop UI up on a phone before the layout reflows.
///
/// Measured on a 1080×2400 @ 420 dpi phone: egui's logical size is 411×914 pt at
/// `pixels_per_point = 2.625`, while PrintCraft's layout is designed for ≥820×520 pt (the desktop
/// window's minimum). Scaling up only shrinks the logical viewport further and makes the panels
/// overlap more, so the shell does **not** scale: the answer is fewer panels at once
/// ([`TouchShell`] starts in Read mode with the tool panel closed) and finger-sized targets, not a
/// zoom. Keep this at 1.0.
const TOUCH_SCALE: f32 = 1.0;

/// The device font appended to egui's fallback chain (see [`ensure_system_font`]).
const SYSTEM_FONT: &str = "system-cjk";

/// The UI context of the running app, published on the first frame: the picker threads use it to
/// ask for a repaint, so a picked document is opened right away instead of waiting for the next
/// touch (an idle egui app runs no frames of its own).
static UI_CTX: OnceLock<egui::Context> = OnceLock::new();

/// Ask for a frame (from any thread).
fn wake() {
    if let Some(ctx) = UI_CTX.get() {
        ctx.request_repaint();
    }
}

/// Notices from the picker threads to the UI thread: they are shown with the app's own toast.
///
/// Picked documents need no queue — PrintCraft's asynchronous picker delivers them itself (see
/// `pickers::Pickers::process_picked`, which runs every frame and calls `open_path`), which is the
/// desktop path with the paths coming from the Storage Access Framework.
pub type Notices = Arc<Mutex<Vec<String>>>;

/// Touch shell: forward everything to [`PdfCraftApp`], then apply the phone adjustments once
/// PrintCraft's own theme and fonts are in place.
struct TouchShell {
    inner: PdfCraftApp,
    /// The touch adjustments have been applied (after the first frame).
    tuned: bool,
    /// The touch command palette is open.
    palette: bool,
    /// Press bookkeeping for the long-press gesture.
    press: Option<(std::time::Instant, egui::Pos2)>,
    /// Notices for the UI thread, pushed by picker threads.
    notices: Notices,
}

impl TouchShell {
    /// Phone adjustments: a comfortable hit target (40 pt ≈ 105 px at 420 dpi), roomier padding and
    /// a little more separation so neighbouring controls are harder to hit by accident. Called
    /// after the first frame, i.e. after PrintCraft's theme install.
    fn tune(ctx: &egui::Context) {
        let before = ctx.pixels_per_point();
        if (TOUCH_SCALE - 1.0).abs() > f32::EPSILON {
            ctx.set_pixels_per_point((before * TOUCH_SCALE).min(4.0));
        }
        ctx.all_styles_mut(|style| {
            style.spacing.interact_size = egui::vec2(40.0, 40.0);
            style.spacing.button_padding = egui::vec2(10.0, 8.0);
            style.spacing.item_spacing = egui::vec2(8.0, 8.0);
            style.spacing.slider_width = style.spacing.slider_width.max(160.0);
            style.spacing.scroll.bar_width = 14.0;
            // A resting finger must not pop tooltips (they cover the control they describe).
            style.interaction.show_tooltips_only_when_still = true;
        });
        let rect = ctx.viewport_rect();
        log::info!(
            "printcraft-android: touch mode applied (ppp {before}, logical {}×{} pt, interact {} pt)",
            rect.width().round(),
            rect.height().round(),
            ctx.style_of(ctx.theme()).spacing.interact_size.x,
        );
    }

    /// Whether the theme's own fonts are live (it defines the named families "medium"/"semibold"),
    /// i.e. the theme install has happened.
    fn theme_fonts_live(ctx: &egui::Context) -> bool {
        ctx.fonts(|f| f.definitions().families.keys().any(|k| matches!(k, egui::FontFamily::Name(_))))
    }

    /// The active document's page count, if one is open.
    fn page_count(&self) -> Option<usize> {
        let (_, id) = self.inner.active_ids()?;
        let pages = self.inner.session.get(id)?.page_boxes().len();
        (pages > 0).then_some(pages)
    }

    /// Show what the picker threads reported since the last frame, with the app's own toast.
    fn drain_notices(&mut self) {
        let notices = match self.notices.lock() {
            Ok(mut queue) => std::mem::take(&mut *queue),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        };
        for text in notices {
            log::info!("printcraft-android: {text}");
            self.inner.notify(text);
        }
    }
}

/// Add the device's CJK font to egui's *current* definitions when it is missing: PrintCraft's
/// bundled Inter has no CJK glyphs and the craft-fonts build input is optional, so Chinese and
/// Japanese interface text would render as replacement boxes.
///
/// Read-modify-write on the *current* definitions, so the theme's own families (including
/// `FontFamily::Name("medium")` and `"semibold"`, which egui panics over when they end up bound to
/// no font) stay; the font is appended to every family, so text drawn with those styles has CJK
/// glyphs too. PrintCraft replaces its fonts when the interface language changes, so this is
/// checked every frame rather than installed once.
fn ensure_system_font(ctx: &egui::Context) {
    if ctx.fonts(|f| f.definitions().font_data.contains_key(SYSTEM_FONT)) {
        return;
    }
    if !TouchShell::theme_fonts_live(ctx) {
        return; // the theme's install would drop it again
    }
    const CANDIDATES: &[&str] = &[
        "/system/fonts/NotoSansCJK-Regular.ttc",
        "/system/fonts/NotoSerifCJK-Regular.ttc",
        "/system/fonts/NotoSansSC-Regular.otf",
        "/system/fonts/DroidSansFallback.ttf",
    ];
    for path in CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else { continue };
        let mut fonts = ctx.fonts(|f| f.definitions().clone());
        fonts.font_data.insert(SYSTEM_FONT.to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
        for family in fonts.families.values_mut() {
            if !family.iter().any(|name| name == SYSTEM_FONT) {
                family.push(SYSTEM_FONT.to_owned());
            }
        }
        ctx.set_fonts(fonts);
        log::info!("printcraft-android: loaded system CJK font {path}");
        return;
    }
    log::warn!("printcraft-android: no system CJK font found; CJK text will be tofu");
}

impl eframe::App for TouchShell {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        // Preferences (theme, language, recent files) keep working.
        eframe::App::save(&mut self.inner, storage);
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let _ = UI_CTX.set(ctx.clone());
        eframe::App::logic(&mut self.inner, ctx, frame);
    }

    /// Diagnostic: whether winit/eframe delivers input at all (a phone that draws but never reacts
    /// is almost always an input problem, and this makes that visible in logcat).
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if !raw_input.events.is_empty() {
            log::debug!("printcraft-android: input: {} event(s), first {:?}", raw_input.events.len(), raw_input.events.first());
        }
        eframe::App::raw_input_hook(&mut self.inner, ctx, raw_input);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        eframe::App::ui(&mut self.inner, ui, frame);
        if !self.tuned {
            // The first pass runs PrintCraft's theme install, so adjust after it.
            self.tuned = true;
            Self::tune(ui.ctx());
        }
        ensure_system_font(ui.ctx());
        self.drain_notices();
        self.gestures(ui.ctx());
        if self.palette {
            self.command_palette(ui.ctx());
        }
    }
}

impl TouchShell {
    /// Long-press on the strip where the desktop menu bar sits opens the command palette: those
    /// menu items are 11–12 pt, far too small for a finger, and the palette gives the same commands
    /// finger-sized buttons.
    fn gestures(&mut self, ctx: &egui::Context) {
        let (down, pos, released) = ctx.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos(), i.pointer.primary_released()));
        if !down || released {
            self.press = None;
            return;
        }
        let Some(p) = pos else { return };
        // While the finger is down, keep frames coming so the 500 ms threshold can pass.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        let strip = ctx.viewport_rect().height() * 0.09;
        match self.press {
            None => self.press = Some((std::time::Instant::now(), p)),
            Some((started, origin)) => {
                if origin.distance(p) > 16.0 {
                    self.press = None; // a drag, not a press
                } else if started.elapsed() > std::time::Duration::from_millis(500) {
                    if p.y < strip && !self.palette {
                        self.palette = true;
                        log::info!("printcraft-android: long-press on the menu strip → command palette");
                    }
                    self.press = None;
                }
            }
        }
    }

    /// The touch replacement for the menu bar and the page rail: the commands a reader needs on a
    /// phone, as full-width buttons. Each one is the app's own registered command
    /// (`PdfCraftApp::execute`, the same entry point the menus, the shortcuts and the control
    /// channel use) or the same `DocView` call the desktop rail makes — there is no second
    /// implementation to keep in step.
    fn command_palette(&mut self, ctx: &egui::Context) {
        let pages = self.page_count();
        let current = self.inner.active.and_then(|i| self.inner.views.get(i)).map(|v| v.current + 1);
        let mut close = false;
        egui::Area::new(egui::Id::new("printcraft-touch-palette"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
            // PrintCraft draws its menus and dialogs in the foreground layer; the touch palette has
            // to sit above them (verified on the emulator: with the default order the app's own
            // menu covered the palette and swallowed its touches).
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(288.0);
                    ui.label(egui::RichText::new("Commands (long-press the menu strip)").size(13.0));
                    let subtitle = match (self.inner.active.is_some(), current, pages) {
                        (true, Some(page), Some(count)) => format!("page {page} of {count}"),
                        (true, _, _) => "document open".to_owned(),
                        _ => "no document open".to_owned(),
                    };
                    ui.label(egui::RichText::new(subtitle).size(12.0).weak());
                    ui.separator();

                    for (label, id) in [
                        ("Open PDF…", "file.open"),
                        ("Save", "file.save"),
                        ("Save As…", "file.save_as"),
                        ("Undo", "edit.undo"),
                        ("Redo", "edit.redo"),
                        ("Find", "edit.find"),
                        ("Comments", "comment.list"),
                        ("Export image…", "export.image"),
                        ("Close document", "file.close"),
                    ] {
                        if touch_button(ui, label, 52.0) {
                            log::info!("palette: {label} → {id}");
                            let done = self.inner.execute(id);
                            if !done {
                                self.inner.notify(format!("{label} is not available right now"));
                            }
                            close = true;
                        }
                    }

                    ui.separator();
                    // Page and zoom controls drive `DocView` directly, exactly as the desktop rail
                    // does (`step_page`, `zoom_step`, `fit`).
                    if let Some(i) = self.inner.active {
                        ui.horizontal(|ui| {
                            if ui.add_sized([68.0, 48.0], egui::Button::new("‹ page")).clicked()
                                && let Some(view) = self.inner.views.get_mut(i)
                            {
                                view.step_page(false);
                            }
                            if ui.add_sized([68.0, 48.0], egui::Button::new("page ›")).clicked()
                                && let Some(view) = self.inner.views.get_mut(i)
                            {
                                view.step_page(true);
                            }
                            if ui.add_sized([68.0, 48.0], egui::Button::new("zoom −")).clicked()
                                && let Some(view) = self.inner.views.get_mut(i)
                            {
                                view.zoom_step(false);
                            }
                            if ui.add_sized([68.0, 48.0], egui::Button::new("zoom +")).clicked()
                                && let Some(view) = self.inner.views.get_mut(i)
                            {
                                view.zoom_step(true);
                            }
                        });
                        if touch_button(ui, "Fit page / fit width", 44.0)
                            && let Some(view) = self.inner.views.get_mut(i)
                        {
                            view.fit = if view.fit == pdfcraft_ui_egui::canvas::Fit::Width {
                                pdfcraft_ui_egui::canvas::Fit::Page
                            } else {
                                pdfcraft_ui_egui::canvas::Fit::Width
                            };
                            view.goto = Some((view.current, 0.0));
                        }
                    }
                    if touch_button(ui, "Close menu", 44.0) {
                        close = true;
                    }
                });
            });
        if close {
            self.palette = false;
            ctx.request_repaint();
        }
    }
}

/// One full-width palette button (a finger-sized target).
fn touch_button(ui: &mut egui::Ui, label: &str, height: f32) -> bool {
    ui.add_sized([288.0, height], egui::Button::new(egui::RichText::new(label).size(17.0))).clicked()
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    // A tag of our own: the default is this crate's library name (`main`), which says nothing in
    // a logcat dump and cannot be filtered for.
    android_logger::init_once(android_logger::Config::default().with_tag("PrintCraft").with_max_level(log::LevelFilter::Info));
    log::info!("printcraft-android: android_main entered (PdfCraft {})", env!("CARGO_PKG_VERSION"));
    // Application-private storage: settings, recovery snapshots, imported documents.
    let data_dir = app.internal_data_path();
    // Exports and "Save As" write here; the SAF host publishes the finished file to wherever the
    // user points (and folder questions answer with this directory).
    let exports = app.external_data_path().map(|dir| dir.join("exports")).or_else(|| data_dir.clone());
    if let Some(dir) = &exports
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        log::warn!("printcraft-android: cannot create {}: {e}", dir.display());
    }
    if let Some(dir) = &exports {
        log::info!("printcraft-android: export scratch directory {}", dir.display());
    }
    // The Java activity object: `ndk_context` only hands out the Application, which cannot start
    // the document picker for a result.
    let activity = app.activity_as_ptr() as usize;
    // PrintCraft's own file dialogs (File ▸ Open, Combine files, Save As, export destinations) ask
    // the `fd` seam, which on Android is this host: the pickers run on the Storage Access Framework
    // and hand back real paths (a picked document is copied into the app's storage first).
    let notices: Notices = Arc::new(Mutex::new(Vec::new()));
    if let Some(dir) = data_dir.clone() {
        pdfcraft_ui_egui::fd::set_host(Arc::new(saf::SafHost::new(activity, dir, exports.clone(), notices.clone())));
        log::info!("printcraft-android: SAF file dialogs installed (host_installed={})", pdfcraft_ui_egui::fd::host_installed());
    } else {
        log::warn!("printcraft-android: no private data directory; file dialogs are unavailable");
    }
    // Parse, rasterize and read the text of an embedded PDF through the engine's own render path:
    // logcat evidence that the PDF stack works on this device.
    std::thread::spawn(|| log::info!("printcraft-android: {}", selftest::run()));
    // Settings and unsaved work live in the app's private directory; eframe's default (derived from
    // the OS's per-user directories) does not exist on Android.
    let persistence_path = data_dir.as_ref().map(|dir| dir.join("app.ron"));
    let recovery_dir = data_dir.as_ref().map(|dir| dir.join("recovery"));
    let options = eframe::NativeOptions { android_app: Some(app), persistence_path, ..Default::default() };
    let started = eframe::run_native(
        "PrintCraft",
        options,
        Box::new(move |cc| {
            let mut app = PdfCraftApp::new();
            // Settings from the previous run (theme, language, recent files).
            if let Some(json) = cc.storage.and_then(|s| s.get_string("pdfcraft").or_else(|| s.get_string("printcraft"))) {
                app.restore(&json);
            }
            // A phone shows one panel at a time: start in Read mode with the tool panel closed so
            // the page fills the screen. Both are the desktop app's own fields (its View menu sets
            // them the same way) and the user can change them again from the menus.
            app.mode = Mode::Read;
            app.left_open = false;
            // Auto-save unsaved changes, and offer to recover documents a killed app left behind.
            if let Some(dir) = recovery_dir.clone() {
                match std::fs::create_dir_all(&dir) {
                    Ok(()) => app.enable_recovery(pdfcraft_ui_egui::RecoveryStore::new(dir)),
                    Err(e) => log::warn!("printcraft-android: no recovery directory {}: {e}", dir.display()),
                }
            }
            Ok(Box::new(TouchShell { inner: app, tuned: false, palette: false, press: None, notices }))
        }),
    );
    if let Err(e) = started {
        log::error!("printcraft-android: eframe exited: {e}");
    }
}
