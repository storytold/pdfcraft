//! State-based waits for native preparation, which completes between UI frames, and pointer
//! input in the frames the platform delivers it in ([`input`]), and the skip for tests whose text
//! the interface fonts cannot draw ([`skip_without_glyphs`]).
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use craft::host::target::ShellInfo;
use craft::host::{AppInfo, Files, Host, InlineJobs, Jobs, ManualJobs, ScriptedFiles, Wake};
use craft::keymap::CommandRef;
use craft::platform::clipboard::{Clipboard, ClipboardBackend, ClipboardError, ClipboardImage, Flavor, TEXT};
use craft::platform::open::FileIntake;
use craft::platform::watch::FileWatch;
use craft::ui::InstallSpec;
use craft::ui::runtime::commands::{FireSource, fire};
use craft::ui::runtime::env::WindowKey;
use craft::ui::runtime::toast;
use craft::ui::shell::embed;
use craft::window::system::NoWindows;
use egui::FontId;
use egui::epaint::text::{Fonts, TextOptions};
use egui_kittest::Harness;
use pdfcraft_ui_egui::{AppServices, PdfCraftApp, RightPanel, i18n};
use std::path::PathBuf;

pub mod fixtures;
pub mod input;

/// Step frames until `ready` holds or 30 seconds pass, and say whether it held. For background work
/// (a worker-thread open, page text arriving) that a test then asserts on itself: a frame count is
/// not a time, and a busy runner may not have finished the work within one. Sleeps briefly between
/// frames so the worker threads get the CPU.
#[must_use]
pub fn step_until(h: &mut Harness<'static, PdfCraftApp>, mut ready: impl FnMut(&mut Harness<'static, PdfCraftApp>) -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        h.step();
        if ready(h) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Step frames until nothing is waiting to be rendered, and fail the test if that takes over 30
/// seconds: what a test checks next is on the rendered page.
pub fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    let settled = step_until(h, |h| {
        h.run_steps(1);
        !h.state().render_pending()
    });
    assert!(settled, "pages rendered");
}

/// [`settle`] for an app whose jobs wait in `jobs`: runs them each frame until nothing is waiting to be rendered.
/// Between calls the page pictures are held back, as a slow render would hold them.
pub fn settle_held(h: &mut Harness<'static, PdfCraftApp>, jobs: &ManualJobs) {
    let settled = step_until(h, |h| {
        jobs.run_all();
        h.run_steps(1);
        !h.state().render_pending()
    });
    assert!(settled, "pages rendered");
}

pub fn wait_until(h: &mut Harness<'static, PdfCraftApp>, what: &str, ready: impl Fn(&PdfCraftApp) -> bool) {
    let held = step_until(h, |h| ready(h.state()));
    assert!(
        held,
        "timed out waiting for {what}: views={}, active={:?}, password={:?}",
        h.state().views.len(),
        h.state().active,
        h.state().password_prompt.as_ref().map(|p| (&p.name, &p.error)),
    );
}

/// Fire `command` as a menu row does, through the board, and run a frame. For a command with
/// parameters (`window.document`), pass a `CommandRef` with them.
pub fn fire_menu(h: &mut Harness<'static, PdfCraftApp>, command: impl Into<CommandRef>) {
    fire(&h.ctx, command.into(), FireSource::Menu);
    h.step();
}

/// The text of the newest notice card the main window shows, if any: what the app said last.
pub fn shown_notice(h: &Harness<'_, PdfCraftApp>) -> Option<String> {
    // A posted notice's text is the card's title; a message and bullet lines follow it.
    toast::cards(&h.ctx, &WindowKey::main()).last().map(|card| {
        let toast = &card.toast;
        let parts = toast.title.iter().chain(std::iter::once(&toast.message)).chain(&toast.lines);
        parts.map(|part| part.as_str()).filter(|text| !text.is_empty()).collect::<Vec<_>>().join("\n")
    })
}

/// The services the tests give the app: no wake, no autosave folder, the GitHub update source, a fresh
/// file intake and an inline file watch, over `clipboard`.
pub fn services(clipboard: RecordingClipboard) -> AppServices {
    let (intake, _sender) = FileIntake::new(Wake::none());
    AppServices::new(Wake::none(), Clipboard::new(clipboard), None, None, intake, FileWatch::new(Wake::none()).inline())
}

fn app_info() -> AppInfo {
    AppInfo { id: "pdfcraft", name: "PdfCraft", version: "0" }
}

/// A host whose threads run its jobs and whose pickers answer Cancelled.
pub fn host() -> Arc<Host> {
    Arc::new(Host::builder(app_info()).build())
}

/// A host over `jobs` and `files`, for a test that scripts either.
pub fn host_with(jobs: Arc<dyn Jobs>, files: Arc<dyn Files>) -> Arc<Host> {
    Arc::new(Host::builder(app_info()).jobs(jobs).files(files).build())
}

/// A host whose pickers answer from `files`.
pub fn host_with_files(files: impl Into<Arc<ScriptedFiles>>) -> Arc<Host> {
    let files: Arc<ScriptedFiles> = files.into();
    Arc::new(Host::builder(app_info()).files(files).build())
}

/// The app over `host` with the tests' services.
pub fn test_app(host: Arc<Host>) -> PdfCraftApp {
    PdfCraftApp::with_host(host, services(RecordingClipboard::default()))
}

/// The app over a host whose jobs wait in `jobs` until the test runs them.
pub fn held_app(jobs: Arc<ManualJobs>) -> PdfCraftApp {
    test_app(Arc::new(Host::builder(app_info()).jobs(jobs).build()))
}

/// The app over a host whose jobs run at once, where they are queued.
pub fn inline_app() -> PdfCraftApp {
    test_app(Arc::new(Host::builder(app_info()).jobs(Arc::new(InlineJobs)).build()))
}

/// The app over the default host and a clipboard the test keeps a clone of.
pub fn app_with_clipboard(clipboard: RecordingClipboard) -> PdfCraftApp {
    PdfCraftApp::with_host(host(), services(clipboard))
}

/// An app built over `host`, with the tests' services: the one way a test gives the app a host, so
/// jobs and picked files run when the test steps.
/// Installs the framework runtime on a harness's context, as the app's window does, with a system
/// that opens no OS windows and the interface fonts of English. Every harness creator calls it.
pub fn install_runtime(ctx: &egui::Context) {
    let spec = InstallSpec::new(ShellInfo::default(), Arc::new(NoWindows));
    embed::install(ctx, spec, &pdfcraft_ui_egui::fonts::ui_fonts(i18n::Lang::EN)).expect("the runtime installs on a fresh context");
}

/// A clipboard backend the tests share: what the app writes is read back through the clone the test keeps.
#[derive(Clone, Default)]
pub struct RecordingClipboard {
    state: Arc<Mutex<Recorded>>,
}

#[derive(Default)]
struct Recorded {
    writes: Vec<Vec<Flavor>>,
    held: Option<String>,
    refused: Option<&'static str>,
}

impl RecordingClipboard {
    /// A clipboard that refuses a write offering a flavour of `mime`; the attempt is recorded.
    pub fn refusing(mime: &'static str) -> Self {
        let clipboard = Self::default();
        clipboard.lock().refused = Some(mime);
        clipboard
    }

    /// The text the app last copied.
    pub fn copied(&self) -> Option<String> {
        self.lock().held.clone()
    }

    /// Every write the app attempted, refused or not, each as its flavours.
    pub fn writes(&self) -> Vec<Vec<Flavor>> {
        self.lock().writes.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Recorded> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl ClipboardBackend for RecordingClipboard {
    fn write(&mut self, flavors: &[Flavor]) -> Result<usize, ClipboardError> {
        let mut state = self.lock();
        state.writes.push(flavors.to_vec());
        if let Some(mime) = state.refused
            && flavors.iter().any(|f| f.mime == mime)
        {
            return Err(ClipboardError::NoFormat);
        }
        let written = flavors.iter().position(|f| f.mime == TEXT).unwrap_or(0);
        state.held = flavors.get(written).filter(|f| f.mime == TEXT).map(|f| String::from_utf8_lossy(&f.data).into_owned());
        Ok(written)
    }

    fn read(&mut self, mimes: &[&str], _limit: u64) -> Result<Option<Flavor>, ClipboardError> {
        let held = self.lock().held.clone();
        Ok(held.filter(|_| mimes.contains(&TEXT)).map(|text| Flavor::text(&text)))
    }

    fn read_image(&mut self, _limit: u64) -> Result<Option<ClipboardImage>, ClipboardError> {
        Ok(None)
    }

    fn read_files(&mut self, _limit: usize) -> Result<Option<Vec<PathBuf>>, ClipboardError> {
        Ok(None)
    }

    fn has(&mut self, mimes: &[&str]) -> bool {
        mimes.contains(&TEXT) && self.lock().held.is_some()
    }
}

/// Step `frames` frames and return every URL the app asked the platform to open, in order.
pub fn step_collecting_urls(h: &mut Harness<'static, PdfCraftApp>, frames: usize) -> Vec<String> {
    let mut urls = Vec::new();
    for _ in 0..frames {
        h.step();
        urls.extend(h.output().platform_output.commands.iter().filter_map(|command| match command {
            egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
            _ => None,
        }));
    }
    urls
}

/// Open the in-window menu bar's `title` menu (File, Edit, View…).
pub fn open_menu(h: &mut Harness<'static, PdfCraftApp>, title: &str) {
    use egui_kittest::kittest::Queryable;
    h.get_by(|n| n.role() == egui::accesskit::Role::MenuItem && n.label().as_deref() == Some(title)).click();
    h.run_steps(2);
}

/// Whether the last frame asked the platform for the clipboard (`RequestPaste`, Edit ▸ Paste's).
pub fn asked_for_the_clipboard(h: &Harness<'static, PdfCraftApp>) -> bool {
    h.output().viewport_output.values().any(|v| v.commands.contains(&egui::ViewportCommand::RequestPaste))
}

/// The menu row labelled `label` in an open menu.
pub fn menu_row<'h>(h: &'h Harness<'static, PdfCraftApp>, label: &str) -> egui_kittest::Node<'h> {
    use egui_kittest::kittest::Queryable;
    let label = label.to_owned();
    h.get_by(move |n| n.role() == egui::accesskit::Role::MenuItem && n.label().as_deref() == Some(label.as_str()))
}

/// The widget labelled `label` outside the menu bar, whose titles (File, Edit, Sign…) may share it.
pub fn outside_menus<'h>(h: &'h Harness<'static, PdfCraftApp>, label: &str) -> egui_kittest::Node<'h> {
    use egui_kittest::kittest::Queryable;
    let label = label.to_owned();
    h.get_by(move |n| n.role() != egui::accesskit::Role::MenuItem && n.label().as_deref() == Some(label.as_str()))
}

/// Whether the test `name` must skip because the interface fonts lack a glyph for a character it
/// draws: one in `text` or in the catalog of the interface `language`. egui 0.37 test harnesses
/// panic on a missing glyph, and Japanese, Chinese and Thai faces come only from the optional
/// craft-fonts build input or a face installed on this machine. Prints the skip line.
///
/// The fonts are the interface stack of `language`, which already holds the language's faces.
///
/// A build that requires craft-fonts (`CRAFT_FONTS_REQUIRED=1`, as the CI gate sets) never skips:
/// a missing glyph there is a failure.
pub fn skip_without_glyphs(name: &str, language: &str, text: &str) -> bool {
    if std::env::var("CRAFT_FONTS_REQUIRED").as_deref() == Ok("1") {
        return false;
    }
    let catalogs: Vec<&str> = i18n::Lang::from_code(language).into_iter().flat_map(i18n::Lang::catalog_texts).collect();
    let translations = catalogs.iter().flat_map(|c| c.lines()).filter(|l| !l.starts_with('#')).filter_map(|l| l.split('\t').nth(2));
    let chars: BTreeSet<char> = std::iter::once(text).chain(translations).flat_map(str::chars).filter(|c| !c.is_control()).collect();
    let defs: egui::FontDefinitions =
        pdfcraft_ui_egui::fonts::ui_fonts(i18n::Lang::from_code(language).unwrap_or(i18n::Lang::EN)).stack().flat_definitions();
    let families: Vec<FontId> = defs.families.keys().map(|f| FontId::new(13.0, f.clone())).collect();
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    let missing = chars.iter().copied().find(|c| families.iter().any(|id| !fonts.has_glyphs(id, &c.to_string())));
    let Some(missing) = missing else {
        return false;
    };
    eprintln!("skipping {name}: no interface font draws {missing:?} (set CRAFT_FONTS_DIR to a craft-fonts checkout with a face for it)");
    true
}

/// Whether `name` is skipped because the build embeds no craft-i18n catalogs (English only).
pub fn skip_without_catalogs(name: &str) -> bool {
    let skip = !pdfcraft_ui_egui::i18n::catalogs_embedded();
    if skip {
        eprintln!("skipping {name}: built without craft-i18n (set CRAFT_I18N_DIR to a craft-i18n checkout)");
    }
    skip
}

/// Open Help ▸ Keyboard shortcuts with a click, as a user would, so whatever had the keyboard loses
/// it as it does then.
pub fn open_shortcuts_from_the_help_menu(h: &mut Harness<'static, PdfCraftApp>) {
    use egui_kittest::kittest::Queryable;
    open_menu(h, "Help");
    h.get_by(|n| n.role() == egui::accesskit::Role::MenuItem && n.label().is_some_and(|l| l.starts_with("Keyboard shortcuts"))).click();
    h.run_steps(4);
    assert!(shortcuts_open(h), "Keyboard shortcuts is open");
}

/// Whether the Keyboard shortcuts window shows: its dialog node is in the window's tree. It is the framework's own
/// window, not one the dialog host holds.
pub fn shortcuts_open(h: &Harness<'_, PdfCraftApp>) -> bool {
    use egui_kittest::kittest::Queryable;
    h.query_by(|n| n.role() == egui::accesskit::Role::Dialog && n.label().is_some_and(|l| l.to_lowercase().starts_with("keyboard shortcuts")))
        .is_some()
}

/// The right panel the dock has open, as the control channel's `ui.state` reports it (`side_panels::state`).
pub fn right(app: &PdfCraftApp) -> Option<RightPanel> {
    use RightPanel::*;
    let state = pdfcraft_ui_egui::side_panels::state(app);
    let name = state["side_panel"].as_str()?;
    [Comments, Bookmarks, Pages, Fields, Layers, Attachments, Signatures, Accessibility, Tags, Search, Compare]
        .into_iter()
        .find(|panel| format!("{panel:?}") == name)
}

/// Whether the dialog host holds a dialog.
pub fn dialog_open(h: &Harness<'_, PdfCraftApp>) -> bool {
    !h.state().dialogs.is_empty()
}

/// The app's command and option entry points, called with the harness's own context (L-208: they take the
/// `ctx` the openers need, and the harness holds it).
pub trait AppHarness {
    fn execute(&mut self, id: &str) -> bool;
    fn set_option(&mut self, key: &str, value: &str) -> Result<(), String>;
    fn undo(&mut self);
    fn redo(&mut self);
    fn submit_password(&mut self, password: Option<String>);
    fn run_button_script(&mut self, id: pdfcraft_engine::DocId, field: &str, script: &str);
    fn snapshot(&mut self, index: usize, page: usize, view_rect: [f32; 4]) -> Result<(u32, u32), String>;
    fn start_signing(&mut self, page: usize, rect: Option<[f64; 4]>, field: Option<String>, certify: Option<u8>);
}

impl AppHarness for Harness<'_, PdfCraftApp> {
    fn execute(&mut self, id: &str) -> bool {
        let ctx = self.ctx.clone();
        self.state_mut().execute(&ctx, id)
    }

    fn set_option(&mut self, key: &str, value: &str) -> Result<(), String> {
        let ctx = self.ctx.clone();
        self.state_mut().set_option(&ctx, key, value)
    }

    fn undo(&mut self) {
        let ctx = self.ctx.clone();
        self.state_mut().undo(&ctx);
    }

    fn redo(&mut self) {
        let ctx = self.ctx.clone();
        self.state_mut().redo(&ctx);
    }

    fn submit_password(&mut self, password: Option<String>) {
        let ctx = self.ctx.clone();
        self.state_mut().submit_password(&ctx, password);
    }

    fn run_button_script(&mut self, id: pdfcraft_engine::DocId, field: &str, script: &str) {
        let ctx = self.ctx.clone();
        self.state_mut().run_button_script(&ctx, id, field, script);
    }

    fn snapshot(&mut self, index: usize, page: usize, view_rect: [f32; 4]) -> Result<(u32, u32), String> {
        let ctx = self.ctx.clone();
        self.state_mut().snapshot(&ctx, index, page, view_rect)
    }

    fn start_signing(&mut self, page: usize, rect: Option<[f64; 4]>, field: Option<String>, certify: Option<u8>) {
        let ctx = self.ctx.clone();
        self.state_mut().start_signing(&ctx, page, rect, field, certify);
    }
}
