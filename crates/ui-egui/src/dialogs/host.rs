//! The app's dialog host: runs the stack dialogs and the windows beside them each pass, then hands each area its
//! results. Every area opens its dialogs through [`open`] or [`open_over`] and reads its answers in its own pass.

use craft::review::ui::signature::SignaturePadDialog;
use craft::ui::dialog::{DialogId, StackDialog, dialog_id};
use craft::ui::patterns::form_dialogs::{DialogFields, HostedParamDialog};

use crate::PdfCraftApp;

/// Runs the dialogs for one pass: the stack dialogs, then the windows that are not stack dialogs, then the results.
/// Each pass of a frame shows every dialog again and applies only its own input (`DialogHost::run`); a result is
/// stored on the pass its dialog finished and taken once, so a second pass routes nothing twice.
pub(crate) fn run(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    app.dialogs.run(ui);
    windows(app, ui);
    results(app, &ctx);
}

/// Opens `dialog` on the host. Opening a dialog ends the Sign window's password draft.
pub(crate) fn open<D: StackDialog>(app: &mut PdfCraftApp, dialog: D) -> DialogId {
    let id = app.dialogs.open(dialog);
    super::discard_inactive_password_drafts(app);
    id
}

/// Opens `dialog` above the dialog on top of the stack, which stays open under it; with none open, as [`open`].
pub(crate) fn open_over<D: StackDialog>(app: &mut PdfCraftApp, dialog: D) -> DialogId {
    let id = match app.dialogs.stack().top() {
        Some(parent) => app.dialogs.open_child(dialog, parent),
        None => app.dialogs.open(dialog),
    };
    super::discard_inactive_password_drafts(app);
    id
}

/// The windows the app keeps beside the host: movable, so the page under them stays in view.
fn windows(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    app.show_preferences(ui);
    app.show_shortcuts(ui);
    app.password_pass(ui);
    crate::workflows::js_dialogs::show(app, ui);
    crate::redact_ui::dialogs::show_props(app, ui);
    crate::redact_ui::dialogs::show_search(app, ui);
    crate::redact_ui::dialogs::show_hidden(app, ui);
    crate::sign_ui::dialog::show(app, ui);

    crate::sign_ui::cert_viewer::show(app, ui);
}

/// Hands each area the answers its dialogs gave. `DialogHost::take` yields a result only once its dialog finished,
/// so every pass runs whatever is open.
fn results(app: &mut PdfCraftApp, ctx: &egui::Context) {
    // Dialogs.
    app.rename_pass(ctx);
    app.properties_pass(ctx);
    app.page_dialogs_pass(ctx);

    app.page_props_pass(ctx, ctx);
    app.bookmarks_from_styles_pass(ctx, ctx);
    app.about_pass(ctx);
    app.pin_tools_pass(ctx);
    app.marks_pass(ctx);
    app.printer_marks_pass(ctx, ctx);
    app.convert_colours_pass(ctx, ctx);
    app.fix_hairlines_pass(ctx, ctx);
    app.page_boxes_pass(ctx, ctx);
    app.protect_pass(ctx);
    app.pdfa_pass(ctx);
    app.word_count_pass(ctx);
    app.prompts_pass(ctx);
    app.attachment_description_pass(ctx);
    // Comments.
    app.comment_props_pass(ctx);
    app.create_stamp_pass(ctx);
    app.summarize_pass();

    crate::colour_picker::colour_picker_pass(app, ctx);
    // Forms.
    if let Some(result) = app.dialogs.take::<SignaturePadDialog>() {
        super::signature::finish(app, result);
    }
    crate::prepare::dialog::finish(ctx, app);
    crate::bulk_fields::finish(ctx, app);
    crate::field_table_ui::pass(app, ctx);
    if let Some(fields) = param_ok(app, "redact.pages") {
        crate::redact_ui::dialogs::finish_pages(ctx, app, &fields);
    }
    if let Some(fields) = param_ok(app, "form.duplicate_field") {
        super::duplicate_field::finish(ctx, app, &fields);
    }
    // Edit.
    app.link_props_pass(ctx);
    // Print and OCR.
    app.print_pass(ctx);
    app.recognize_text_pass(ctx);
    app.enhance_scan_pass(ctx, ctx);
    // Workflows.
    crate::optimize_ui::pass(app, ctx);
    crate::create_ui::pass(app);
    crate::a11y_ui::pass(ctx, app);
    crate::actions_ui::pass(app);
    crate::export_ui::pass(ctx, app);
    crate::compare_ui::pass(app, ctx);
    // Shell.
    app.recovery_pass(ctx);
    app.close_guard_pass(ctx);
    app.save_flow_pass(ctx, ctx);
}

/// The confirmed values of the parameter dialog opened for `key`, once. A `HostedParamDialog`'s id is
/// `dialog_id(key)` and it keeps its result apart by that id, so no other owner's take reaches it.
fn param_ok(app: &mut PdfCraftApp, key: &str) -> Option<DialogFields> {
    let result = app.dialogs.take_by_id::<HostedParamDialog>(dialog_id(key))?;
    result.ok().map(|r| r.fields)
}
