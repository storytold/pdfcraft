//! Help ▸ Check for updates and the check at start, through `craft_update`: the flow itself is its
//! `Updater`; this module supplies the suite and the quit path, and shows the result in
//! `craft_update`'s `CheckResultDialog`.

#[cfg(not(target_arch = "wasm32"))]
use craft_update::updater::{Action, CheckSource, Status, Updater};
#[cfg(not(target_arch = "wasm32"))]
use craft_update::{CheckResultDialog, CheckResultResponse};

use crate::PdfCraftApp;

/// Where every PdfCraft release is listed.
pub const RELEASES_PAGE: &str = "https://github.com/storytold/pdfcraft/releases";

/// Stands in for the network in tests.
#[cfg(not(target_arch = "wasm32"))]
pub type UpdateSource = CheckSource;

pub(crate) struct Updates {
    #[cfg(not(target_arch = "wasm32"))]
    updater: Updater,
    /// The Updates dialog is showing.
    pub(crate) open: bool,
    /// Ask for a newer release at start, unless Preferences says not to; the desktop app turns
    /// it on, so tests never reach the network.
    #[cfg(not(target_arch = "wasm32"))]
    startup_check: bool,
    #[cfg(not(target_arch = "wasm32"))]
    check_requested: bool,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            updater: Updater::new(&crate::UPDATE_APP, env!("CARGO_PKG_VERSION")).with_translator(translate_update),
            open: false,
            #[cfg(not(target_arch = "wasm32"))]
            startup_check: false,
            #[cfg(not(target_arch = "wasm32"))]
            check_requested: false,
        }
    }
}

impl PdfCraftApp {
    /// Whether the app asks for a newer release on its first frame (off until the desktop app turns it on).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_startup_update_check(&mut self, on: bool) {
        self.updates.startup_check = on;
    }

    /// Replaces the network with `source` (tests).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_update_source(&mut self, source: UpdateSource) {
        self.updates.updater = Updater::new(&crate::UPDATE_APP, env!("CARGO_PKG_VERSION")).with_source(source).with_translator(translate_update);
    }

    /// Help ▸ Check for updates: ask for the latest release and show the outcome.
    pub fn check_for_updates(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.updates.open = true;
            self.updates.check_requested = true;
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.open_url(RELEASES_PAGE);
        }
    }

    /// Starts the checks and picks up their answers (each frame).
    pub(crate) fn poll_updates(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(ctx) = self.ctx.clone() else { return };
            // Settled on the first frame: a preference turned on later waits for the next start.
            let at_start = std::mem::take(&mut self.updates.startup_check) && self.update_check_at_start;
            let updates = &mut self.updates;
            if std::mem::take(&mut updates.check_requested) {
                updates.updater.check_now(&ctx);
            } else if at_start {
                updates.updater.check_at_start(&ctx);
            }
            updates.updater.tick(&ctx);
        }
    }
}

/// The startup prompt, and the Updates dialog that answers Help ▸ Check for updates.
pub(crate) fn dialog(app: &mut PdfCraftApp, ctx: &egui::Context) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        // The restart helper is scheduled before the app closes, so no update starts over unsaved work.
        let clean = app.first_dirty().is_none();
        match app.updates.updater.show_if(ctx, clean) {
            Action::None => {}
            Action::OpenUrl(url) => app.open_url(&url),
            Action::Close => {
                app.request_quit(ctx);
            }
        }
        // A requested check that found a release is answered by the prompt, not by this dialog.
        if app.updates.open && !app.updates.check_requested && matches!(app.updates.updater.status(), Status::Idle) {
            app.updates.open = false;
        }
        if app.updates.open {
            let dialog = CheckResultDialog::new(&crate::UPDATE_APP, env!("CARGO_PKG_VERSION")).with_translator(translate_update);
            match dialog.show(ctx, app.updates.updater.status()) {
                CheckResultResponse::None => {}
                CheckResultResponse::Close => {
                    app.updates.open = false;
                    app.updates.updater.dismiss_status();
                }
                CheckResultResponse::CheckAgain => app.updates.check_requested = true,
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (app, ctx);
}

/// Translates the shared update UI through PdfCraft's active interface catalog.
///
/// The update library's new labels have not all reached the shared Hebrew catalog yet. Prefer
/// catalog entries, and use these Hebrew fallbacks only for missing update-specific entries. Once
/// the shared catalog gains a row, it takes over without another code change.
#[cfg(not(target_arch = "wasm32"))]
fn translate_update(text: &str, args: &[(&str, &str)]) -> String {
    translate_update_for(crate::i18n::current(), text, args)
}

#[cfg(not(target_arch = "wasm32"))]
fn translate_update_for(lang: crate::i18n::Lang, text: &str, args: &[(&str, &str)]) -> String {
    let catalog_text = crate::i18n::tr(lang, text);
    let translated = if lang.code() == "he" && catalog_text == text {
        hebrew_update_fallback(text).unwrap_or(catalog_text)
    } else {
        catalog_text
    };
    crate::i18n::fmt(translated, args)
}

#[cfg(not(target_arch = "wasm32"))]
fn hebrew_update_fallback(text: &str) -> Option<&'static str> {
    Some(match text {
        "v{from} to {to}" => "מגרסה {from} לגרסה {to}",
        "by {developer}" => "מבית {developer}",
        "{app} {version} is available with the latest features and bug fixes." => {
            "גרסה {version} של {app} זמינה עם תכונות חדשות ותיקוני באגים."
        }
        "What's new" => "מה חדש",
        "Update and Restart" => "עדכון והפעלה מחדש",
        "Checking for updates…" => "בודק אם קיימים עדכונים…",
        "{app} {version} is the newest version available." => "גרסה {version} של {app} היא העדכנית ביותר.",
        "Could not check for updates." => "לא ניתן לבדוק אם קיימים עדכונים.",
        "All releases are listed at {url}" => "רשימת כל הגרסאות זמינה בכתובת {url}",
        "Downloading and installing update…" => "מוריד ומתקין את העדכון…",
        "The update failed unexpectedly." => "העדכון נכשל באופן בלתי צפוי.",
        "The update stopped unexpectedly." => "תהליך העדכון נעצר באופן בלתי צפוי.",
        "The update is installed. Close {app} to finish; it restarts by itself." => {
            "העדכון הותקן. סגור את {app} כדי להשלים את התהליך; התוכנה תופעל מחדש באופן אוטומטי."
        }
        "Save or close your open work first; the update restarts the app." => {
            "יש לשמור את העבודה ולסגור מסמכים פתוחים לפני התקנת העדכון, שתפעיל מחדש את התוכנה."
        }
        "Cannot auto-update this installation type. Please download manually." => {
            "לא ניתן לעדכן אוטומטית את סוג ההתקנה הזה. יש להוריד את העדכון באופן ידני."
        }
        "Couldn't start the update ({error})" => "לא ניתן להתחיל את העדכון ({error})",
        _ => return None,
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod update_translation_tests {
    use super::*;

    #[test]
    fn hebrew_update_fallbacks_translate_templates_and_preserve_values() {
        let he = crate::i18n::Lang::from_code("he").unwrap_or(crate::i18n::Lang::EN);
        assert_eq!(he.code(), "he");
        assert_eq!(
            translate_update_for(he, "{app} {version} is available with the latest features and bug fixes.", &[("app", "PdfCraft"), ("version", "0.19.0")]),
            "גרסה 0.19.0 של PdfCraft זמינה עם תכונות חדשות ותיקוני באגים."
        );
        assert_eq!(translate_update_for(he, "What's new", &[]), "מה חדש");
        assert_eq!(translate_update_for(he, "Update and Restart", &[]), "עדכון והפעלה מחדש");
    }

    #[test]
    fn existing_catalog_translations_take_precedence_over_fallbacks() {
        let he = crate::i18n::Lang::from_code("he").unwrap_or(crate::i18n::Lang::EN);
        assert_eq!(translate_update_for(he, "Update", &[]), "עדכון");
        assert_eq!(translate_update_for(he, "Later", &[]), "מאוחר יותר");
    }
}
