//! State-based waits for native preparation, which completes between UI frames, and pointer
//! input in the frames the platform delivers it in ([`input`]), and the skip for tests whose text
//! the interface fonts cannot draw ([`skip_without_glyphs`]).
use std::collections::BTreeSet;

use egui::FontId;
use egui::epaint::text::{Fonts, TextOptions};
use egui_kittest::Harness;
use pdfcraft_ui_egui::{PdfCraftApp, i18n, theme};

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
        "timed out waiting for {what}: views={}, active={:?}, password={:?}, toast={:?}",
        h.state().views.len(),
        h.state().active,
        h.state().password_prompt.as_ref().map(|p| (&p.name, &p.error)),
        h.state().toast
    );
}

/// Open the in-window menu bar's `title` menu (File, Edit, View…).
pub fn open_menu(h: &mut Harness<'static, PdfCraftApp>, title: &str) {
    use egui_kittest::kittest::Queryable;
    h.get_by(|n| n.role() == egui::accesskit::Role::MenuItem && n.label().as_deref() == Some(title)).click();
    h.run_steps(2);
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
/// The fonts are those the app installs for `language` and for the default CJK preference: a
/// switch to Chinese installs its fonts for the next frame, so the first frame in Chinese draws
/// with the fonts of the language before it.
///
/// A build that requires craft-fonts (`CRAFT_FONTS_REQUIRED=1`, as the CI gate sets) never skips:
/// a missing glyph there is a failure.
pub fn skip_without_glyphs(name: &str, language: &str, text: &str) -> bool {
    if std::env::var("CRAFT_FONTS_REQUIRED").as_deref() == Ok("1") {
        return false;
    }
    let catalog = i18n::LANGUAGES.iter().find(|l| l.code == language).map_or("", |l| l.source);
    let translations = catalog.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split('\t').nth(2));
    let chars: BTreeSet<char> = std::iter::once(text).chain(translations).flat_map(str::chars).filter(|c| !c.is_control()).collect();
    let cjk = theme::CjkPreference::for_language(language);
    let missing = [theme::CjkPreference::Default, cjk].into_iter().find_map(|cjk| {
        let defs = theme::installed_font_definitions(cjk);
        let families: Vec<FontId> = defs.families.keys().map(|f| FontId::new(13.0, f.clone())).collect();
        let mut fonts = Fonts::new(TextOptions::default(), defs);
        chars.iter().copied().find(|c| families.iter().any(|id| !fonts.has_glyphs(id, &c.to_string())))
    });
    let Some(missing) = missing else {
        return false;
    };
    eprintln!("skipping {name}: no interface font draws {missing:?} (set CRAFT_FONTS_DIR to a craft-fonts checkout with a face for it)");
    true
}
