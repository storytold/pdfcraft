//! State-based waits for native preparation, which completes between UI frames, and pointer
//! input in the frames the platform delivers it in ([`input`]), and the skip for tests whose text
//! the interface fonts cannot draw ([`skip_without_glyphs`]).
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use egui::FontId;
use egui::epaint::text::{Fonts, TextOptions};
use egui_kittest::Harness;
use craft::ui::runtime::commands::{FireSource, fire};
use craft::ui::runtime::env::WindowKey;
use craft::ui::runtime::toast;
use craft::host::{Host, Wake};
use craft::keymap::CommandRef;
use pdfcraft_ui_egui::{AppServices, FileIntake, FileWatch, PdfCraftApp, i18n};

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

/// The text of the first notice card the main window shows, if any.
pub fn shown_notice(h: &mut Harness<'static, PdfCraftApp>) -> Option<String> {
    toast::cards(&h.ctx, &WindowKey::main()).first().map(|card| card.toast.message.as_str().to_owned())
}

/// An app built over `host`, with no wake, an inline file watch and a test file intake: the one way a
/// test gives the app a host, so jobs and picked files run when the test steps.
pub fn host_harness(host: Arc<Host>, clipboard: RecordingClipboard) -> Harness<'static, PdfCraftApp> {
    // PLAN: assumed `AppServices::new(wake, clipboard, recovery_dir, update_source, intake, watch)`,
    // `FileIntake::for_tests()`, `FileWatch::inline()` and `PdfCraftApp::with_host(Arc<Host>, AppServices)` (pdf-hub-w1, L-207)
    let services = AppServices::new(Wake::none(), Box::new(clipboard), None, None, FileIntake::for_tests(), FileWatch::inline());
    let app = PdfCraftApp::with_host(host, services);
    Harness::new_ui_state(|ui, app: &mut PdfCraftApp| app.ui(ui), app)
}

/// A clipboard backend the tests share: what the app copies is read back through the clone the test keeps.
#[derive(Clone, Default)]
pub struct RecordingClipboard {
    text: Arc<Mutex<Option<String>>>,
}

impl RecordingClipboard {
    /// The text the app last copied.
    pub fn copied(&self) -> Option<String> {
        self.text.lock().ok().and_then(|text| text.clone())
    }
}

// PLAN: assumed `ClipboardBackend` with `get_text` and `set_text` (craft::platform::clipboard)
impl craft::platform::clipboard::ClipboardBackend for RecordingClipboard {
    fn get_text(&self) -> Option<String> {
        self.copied()
    }

    fn set_text(&self, text: &str) {
        if let Ok(mut held) = self.text.lock() {
            *held = Some(text.to_owned());
        }
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
    // PLAN: assumed `fonts::ui_fonts(language)` (pdf-shell-w10); `fonts` must be `pub mod` for tests.
    let defs: egui::FontDefinitions = pdfcraft_ui_egui::fonts::ui_fonts(language).stack().flat_definitions();
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
    assert!(h.state().dialog.is_some(), "Keyboard shortcuts is open");
}
