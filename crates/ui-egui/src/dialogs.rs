//! The app's dialogs: every dialog is a framework dialog on the app's `DialogHost` or one of the windows `host::run`
//! draws beside it; each area's module opens its dialogs and reads their results in its own pass.

use crate::PdfCraftApp;

mod about;
pub(crate) mod attachment_description;
pub mod bookmark_styles_ui;
pub(super) mod comment_props;
pub(super) mod convert_colours_ui;
pub(crate) mod duplicate_field;
pub(super) mod hairlines_ui;
pub(crate) mod host;
pub(super) mod marks_ui;
pub(crate) mod page_dialogs;
pub(crate) mod page_props;
pub(super) mod pageboxes;
mod pin_tools;
pub(super) mod preferences;
pub(super) mod printer_marks_ui;
pub(crate) mod prompts;
pub(crate) mod properties;
pub(super) mod protect;
pub(crate) mod rename_ui;
pub(crate) mod shortcuts;
pub(crate) mod signature;
pub(super) mod standards_ui;
pub(crate) mod summarize;
pub(super) mod word_count_ui;

pub(crate) use prompts::shorten_middle;

/// Drop (and zeroize) the Sign password draft once Sign is no longer the active window: Sign is a window beside the
/// dialog host, so any dialog open on the host ends it. `host::open` and `host::open_over` call this after opening,
/// and commands and control requests after they run, since a close and a reopen can happen between frames; a command
/// that starts signing opens no dialog, so its draft stays.
pub(crate) fn discard_inactive_password_drafts(app: &mut PdfCraftApp) {
    if app.dialogs.stack().top().is_some() {
        app.sign_draft = None;
    }
}

/// Draws the dialogs over the window and acts on the answers they gave this pass.
pub fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    // The page sets hover cursors from the raw pointer position (form fields, links, comments)
    // before any dialog is drawn, and nothing under a modal's backdrop takes clicks. Start from
    // the arrow; the modal's own widgets set theirs as they are drawn.
    if app.modal_open() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
    }
    host::run(app, ui);
}

#[cfg(test)]
mod password_draft_tests {
    use super::*;

    fn seed_password_drafts(app: &mut PdfCraftApp) {
        app.start_signing(&egui::Context::default(), 0, None, None, None);
        let sign = app.sign_draft.as_mut().unwrap();
        sign.step = crate::sign_ui::SignStep::Choose;
        sign.password = "sign password".into();
        sign.new_id.password = "new ID password".into();
        sign.new_id.confirm = "new ID password".into();
        sign.new_id.file_password = "import password".into();
    }

    #[test]
    fn inactive_password_drafts_are_cleared_before_early_returns() {
        let mut app = PdfCraftApp::for_unit_tests();
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        seed_password_drafts(&mut app);
        app.set_option(&egui::Context::default(), "dialog", "about").unwrap();
        assert!(app.sign_draft.is_none(), "opening About ends the Sign draft");
    }

    #[test]
    fn control_close_and_reopen_clears_passwords_without_a_render_between_requests() {
        let mut app = PdfCraftApp::for_unit_tests();
        // The control queue can handle several ui.set requests before the next rendered frame.
        // Repeat the flows so a stale draft cannot return on a later open.
        for _ in 0..2 {
            for replacement in ["none", "about", "protect"] {
                seed_password_drafts(&mut app);
                app.set_option(&egui::Context::default(), "dialog", replacement).unwrap();
                assert!(app.sign_draft.is_none(), "inactive Sign draft retained after {replacement}");
                app.set_option(&egui::Context::default(), "dialog", "sign").unwrap();
                let sign = app.sign_draft.as_ref().unwrap();
                assert!(sign.password.is_empty());
                assert_eq!(sign.new_id, crate::sign_ui::NewIdDraft::default());
            }
        }
    }

    #[test]
    fn commands_discard_replaced_password_drafts_before_the_next_control_request() {
        let mut app = PdfCraftApp::for_unit_tests();
        for _ in 0..2 {
            seed_password_drafts(&mut app);
            assert!(app.execute(&egui::Context::default(), "help.about"));
            assert!(app.sign_draft.is_none());
            assert!(app.modal_open(), "About is open");
            app.set_option(&egui::Context::default(), "dialog", "sign").unwrap();
            let sign = app.sign_draft.as_ref().unwrap();
            assert!(sign.password.is_empty());
            assert_eq!(sign.new_id, crate::sign_ui::NewIdDraft::default());
        }
    }
}
