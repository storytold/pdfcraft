//! Modal dialogs: Document Properties, Keyboard Shortcuts, About (with the Contributors and Models credits).

use egui::{Align, Layout};

use crate::theme::{self, Tokens};
use pdfcraft_engine::Edit;

use crate::{CloseRequest, Dialog, PdfCraftApp, PropsTab, panels::human_size, widgets};

pub(super) mod appearance;
pub mod bookmark_styles_ui;
pub(super) mod comment_props;
pub(super) mod credits;
pub(super) mod marks_ui;
pub(super) mod pageboxes;
pub(super) mod palette;
mod prompts;
pub(super) mod protect;
pub(super) mod standards_ui;
pub(super) mod word_count_ui;

pub(crate) use prompts::{PromptButton, save_prompt_key, show_save_prompt_modal};
use prompts::{full_screen_prompt, link_prompt, password, remote_link_prompt, save_prompt, unlock};

const INFO_KEYS: [&str; 4] = ["Title", "Author", "Subject", "Keywords"];

/// What the dialog's widgets asked for this frame; acted on after the modal is drawn.
#[derive(Default)]
struct Pending {
    apply: bool,
    split_now: Option<crate::SplitPlan>,
    split_ready: Option<crate::SplitPlan>,
    extract_now: bool,
    rotate_now: bool,
    link_now: Option<Edit>,
    recover: Option<bool>,
    number_now: Option<Edit>,
    apply_number: bool,
    link_command: Option<&'static str>,
    protect_now: bool,
    boxes_now: bool,
    marks_now: bool,
    export_now: bool,
    props_now: bool,
    field_props_now: bool,
    bulk_field_props_now: bool,
    redact_now: Option<Dialog>,
    import_words: bool,
    print_go: bool,
    revert_now: bool,
    overwrite_now: bool,
    overwrite_elsewhere: bool,
    summarize_now: bool,
    optimize_now: bool,
    duplicate_now: Option<Edit>,
    page_props_now: Option<(pdfcraft_engine::DocId, u64, Edit)>,
    replace_now: bool,
    open_revision: Option<usize>,
    a11y_now: bool,
    ocr_now: bool,
    compare_now: bool,
    images_now: bool,
    stamp_now: bool,
    alt_now: bool,
    close: bool,
    /// The dialog to switch to, when a widget asked for another one.
    next: Option<Dialog>,
}

pub fn show(app: &mut PdfCraftApp, ctx: &egui::Context) {
    // The page sets hover cursors from the raw pointer position (form fields, links, comments)
    // before any dialog is drawn, and nothing under a modal's backdrop takes clicks. Start from
    // the arrow; the modal's own widgets set theirs as they are drawn.
    if app.modal_open() || ctx.memory(|m| m.top_modal_layer().is_some()) {
        ctx.set_cursor_icon(egui::CursorIcon::Default);
    }
    dialogs(app, ctx);
    // Last, so it sits above Document Properties when Security's Unlock… asked for it.
    unlock(app, ctx);
}

/// Draw the active modal dialog and apply the action the user chose.
fn dialogs(app: &mut PdfCraftApp, ctx: &egui::Context) {
    password(app, ctx);
    save_prompt(app, ctx);
    link_prompt(app, ctx);
    remote_link_prompt(app, ctx);
    full_screen_prompt(app, ctx);
    crate::updates::dialog(app, ctx);
    if app.dialog != Some(Dialog::WordCount) {
        app.word_count_draft.cancel();
    }
    if app.dialog != Some(Dialog::FieldTable) {
        app.field_table_draft = None;
    }
    // A movable window, not this modal: the page under it shows the table as it changes.
    if app.dialog == Some(Dialog::FieldTable) {
        crate::field_table_ui::show(app, ctx);
        return;
    }
    let Some(dialog) = app.dialog else {
        app.props_draft = None;
        app.view_draft = None;
        app.custom_draft = None;
        return;
    };
    // Properties ▸ Fonts shows the fonts as they are now, after any content edits.
    if let (Dialog::Properties(PropsTab::Fonts), Some((_, id))) = (dialog, app.active_ids()) {
        app.session.refresh_fonts(id);
    }
    // Properties ▸ Space Audit walks the whole file, so it is measured once per edit, not per frame.
    if dialog == Dialog::Properties(PropsTab::SpaceAudit) {
        app.refresh_space_audit();
    }
    // Seed the editable Description fields from the document when the dialog opens.
    if let (Dialog::Properties(_), Some((_, id))) = (dialog, app.active_ids())
        && app.props_draft.as_ref().is_none_or(|(d, _)| *d != id)
        && let Some(doc) = app.session.get(id)
    {
        app.props_draft = Some((id, INFO_KEYS.map(|k| doc.info_value(k).unwrap_or_default())));
        app.view_draft = Some((id, doc.initial_view()));
        app.custom_draft = Some((id, crate::CustomDraft { props: doc.custom_info(), ..Default::default() }));
    }
    let mut req = Pending::default();
    let modal = widgets::show_modal(ctx, "dialog", |ui| {
        ui.set_width(match dialog {
            Dialog::Properties(_) => 640.0,
            Dialog::Print => 820.0,
            Dialog::FieldProps | Dialog::BulkFieldProps => 600.0,
            Dialog::About => 780.0,
            _ => 520.0,
        });
        if dialog_body(ui, app, dialog, &mut req) {
            dialog_footer(ui, app, dialog, &mut req);
        }
    });
    finish_dialog(app, dialog, modal.should_close(), &mut req);
}

/// The dialog's content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, dialog: Dialog, req: &mut Pending) -> bool {
    let t = Tokens::get(ui.ctx());
    match dialog {
        Dialog::WordCount => {
            req.close = crate::word_count_ui::body(ui, app);
        }
        Dialog::CreateImages => {
            let (go, cancel) = crate::create_ui::image_import_body(ui, app);
            req.images_now = go;
            req.close = go || cancel;
            return false;
        }
        Dialog::Properties(tab) => {
            if !dialog_properties(ui, app, req, tab) {
                return false;
            }
        }
        Dialog::Split => {
            if !dialog_split(ui, app, req) {
                return false;
            }
        }
        Dialog::ReplacePages => {
            if !dialog_replace_pages(ui, app, req) {
                return false;
            }
        }
        Dialog::RedactPages => {
            let n = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len());
            let (ok, cancel) = crate::redact_ui::pages_body(ui, &mut app.redact_pages_draft, n, &t);
            if ok {
                req.redact_now = Some(dialog);
            }
            req.close = ok || cancel;
            return false;
        }
        Dialog::RedactSearch => {
            let (go, cancel, import) = crate::redact_ui::search_body(ui, &mut app.redact_search, &t);
            if go {
                req.redact_now = Some(dialog);
            }
            req.import_words = import;
            req.close = cancel;
            return false;
        }
        Dialog::RedactProps => {
            if !dialog_redact_props(ui, app, req) {
                return false;
            }
        }
        Dialog::RedactApply => {
            let marks = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(0, |d| d.redaction_marks());
            let (ok, cancel) = crate::redact_ui::apply_body(ui, marks, &t);
            if ok {
                req.redact_now = Some(dialog);
            }
            req.close = ok || cancel;
            return false;
        }
        Dialog::LinkProps => {
            let pages = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len());
            let Some(d) = app.link_draft.as_mut() else {
                req.close = true;
                return false;
            };
            let (ok, cancel) = crate::link_ui::body(ui, d, pages, &t);
            if ok {
                req.link_now = Some(crate::link_ui::edit_for(d));
            }
            req.close = ok || cancel;
            return false;
        }
        Dialog::Extract => {
            if !dialog_extract(ui, app, req) {
                return false;
            }
        }
        Dialog::RotatePages => {
            if !dialog_rotate_pages(ui, app, req) {
                return false;
            }
        }
        Dialog::PageProps => {
            let Some(d) = app.page_props_draft.as_mut() else {
                req.close = true;
                return false;
            };
            let (ok, cancel) = crate::organize_panel::page_props_body(ui, d);
            // An order every page already has adds no undo step.
            if ok && let Some(order) = d.tab_order.filter(|o| d.before != Some(*o)) {
                req.page_props_now = Some((d.doc, d.generation, Edit::SetTabOrder { pages: d.pages.clone(), order }));
            }
            req.close = ok || cancel;
            return false;
        }
        Dialog::DuplicateField => {
            if !dialog_duplicate_field(ui, app, req) {
                return false;
            }
        }
        // Never reached: `dialogs` shows this movable window above and returns before the modal.
        // The arm keeps the match exhaustive.
        Dialog::FieldTable => req.close = true,
        Dialog::Optimize => {
            let (ok, cancel) = crate::optimize_ui::body(ui, &mut app.optimize_draft, &t);
            req.optimize_now = ok;
            req.close = ok || cancel;
            if std::mem::take(&mut app.optimize_draft.audit) {
                app.refresh_space_audit();
                req.next = Some(Dialog::AuditSpace);
            }
            return false;
        }
        Dialog::CertificateViewer => {
            if !dialog_certificate_viewer(ui, app, req) {
                return false;
            }
        }
        Dialog::AuditSpace => {
            if crate::optimize_ui::audit_body(ui, &app.space_audit, &t) {
                req.next = Some(Dialog::Optimize);
            }
            return false;
        }
        Dialog::Sign => {
            if crate::sign_ui::dialog(ui, app, &t) {
                app.sign_draft = None;
                req.close = true;
            }
            return false;
        }
        Dialog::SummarizeComments => {
            if !dialog_summarize_comments(ui, app, req) {
                return false;
            }
        }
        Dialog::Revert => {
            if !dialog_revert(ui, app, req) {
                return false;
            }
        }
        Dialog::OverwriteChanged => {
            if !dialog_overwrite_changed(ui, app, req) {
                return false;
            }
        }
        Dialog::Print => {
            if !dialog_print(ui, app, req) {
                return false;
            }
        }
        Dialog::RemoveHidden => {
            let (ok, cancel) = crate::redact_ui::hidden_body(ui, &mut app.hidden_draft, &t);
            if ok {
                req.redact_now = Some(dialog);
            }
            req.close = ok || cancel;
            return false;
        }
        Dialog::Sanitize => {
            let (ok, cancel) = crate::redact_ui::sanitize_body(ui, &t);
            if ok {
                req.redact_now = Some(dialog);
            }
            req.close = ok || cancel;
            return false;
        }
        Dialog::FieldProps => {
            let Some(d) = app.field_props.as_mut() else {
                req.close = true;
                return false;
            };
            let (go, cancel) = crate::prepare::body(ui, d, &t);
            req.apply = go;
            req.field_props_now = req.apply;
            req.close = req.apply || cancel;
            return false;
        }
        Dialog::BulkFieldProps => {
            let Some(d) = app.bulk_field_props.as_mut() else {
                req.close = true;
                return false;
            };
            let (go, cancel) = crate::bulk_fields::body(ui, d, &t);
            req.apply = go;
            req.bulk_field_props_now = req.apply;
            req.close = cancel;
            return false;
        }
        Dialog::CommentProps => {
            let (go, cancel) = crate::comment_props::body(ui, app, &t);
            req.apply = go;
            req.props_now = req.apply;
            req.close = req.apply || cancel;
            return false;
        }
        Dialog::Signature => {
            if !dialog_signature(ui, app, req) {
                return false;
            }
        }
        Dialog::AltText => {
            let (save, cancel) = crate::a11y_ui::alt_body(ui, app, &t);
            req.alt_now = save;
            req.close = save || cancel;
            return false;
        }
        Dialog::CreateStamp => {
            let (save, cancel) = crate::stamps_ui::create_body(ui, app, &t);
            req.stamp_now = save;
            req.close = save || cancel;
            return false;
        }
        Dialog::PdfA => {
            req.close = crate::standards_ui::body(ui, app, &t);
            return false;
        }
        Dialog::ActionWizard => {
            ui.set_width(620.0);
            req.close = crate::actions_ui::body(ui, app, &t);
            return false;
        }
        Dialog::CompareFiles => {
            let (go, cancel) = crate::compare_ui::body(ui, app, &t);
            req.compare_now = go;
            req.close = go || cancel;
            return false;
        }
        Dialog::JsConsole => {
            ui.set_width(640.0);
            req.close = crate::js_ui::console_body(ui, app, &t);
            return false;
        }
        Dialog::DocumentJs => {
            ui.set_width(640.0);
            req.close = crate::js_ui::document_js_body(ui, app, &t);
            return false;
        }
        Dialog::Preferences => {
            if !dialog_preferences(ui, app, req) {
                return false;
            }
        }
        Dialog::RecognizeText => {
            let (go, cancel) = crate::ocr_ui::body(ui, app, &t);
            req.ocr_now = go;
            req.close = go || cancel;
            return false;
        }
        Dialog::AccessibilityOptions => {
            let (start, cancel) = crate::a11y_ui::options_body(ui, app, &t);
            req.a11y_now = start;
            req.close = start || cancel;
            return false;
        }
        Dialog::Export(kind) => {
            let (go, cancel) = crate::export_ui::body(ui, app, &t, kind);
            req.apply = go;
            req.export_now = req.apply;
            req.close = req.apply || cancel;
            return false;
        }
        Dialog::Marks(kind) => {
            ui.set_width(720.0);
            let (go, cancel) = crate::marks_ui::body(ui, app, &t, kind);
            req.apply = go;
            req.marks_now = req.apply;
            req.close = req.apply || cancel;
            return false;
        }
        Dialog::PageBoxes => {
            let (go, cancel) = crate::pageboxes::body(ui, app, &t);
            req.apply = go;
            req.boxes_now = req.apply;
            req.close = req.apply || cancel;
            return false;
        }
        Dialog::Protect => {
            let (go, cancel) = crate::protect::body(ui, app, &t);
            req.apply = go;
            req.protect_now = req.apply;
            req.close = req.apply || cancel;
            return false;
        }
        Dialog::BookmarksFromTextStyles => {
            req.close = crate::bookmark_styles_ui::body(ui, app, &t);
            return false;
        }
        Dialog::NumberPages => {
            if !dialog_number_pages(ui, app, req) {
                return false;
            }
        }
        Dialog::Recovery => {
            if !dialog_recovery(ui, app) {
                return false;
            }
        }
        Dialog::Shortcuts => {
            if !dialog_shortcuts(ui) {
                return false;
            }
        }
        Dialog::About => {
            if !dialog_about(ui, req) {
                return false;
            }
        }
    }
    true
}

/// `Dialog::Properties(tab)` content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_properties(ui: &mut egui::Ui, app: &mut PdfCraftApp, req: &mut Pending, tab: PropsTab) -> bool {
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(tl!("Document Properties")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        for (tb, label) in [
            (PropsTab::Description, tl!("Description")),
            (PropsTab::InitialView, tl!("Initial View")),
            (PropsTab::Security, tl!("Security")),
            (PropsTab::Fonts, tl!("Fonts")),
            (PropsTab::Custom, tl!("Custom")),
            (PropsTab::Advanced, tl!("Advanced")),
            (PropsTab::SpaceAudit, tl!("Space Audit")),
        ] {
            if widgets::mode_tab(ui, label, tab == tb).clicked() {
                req.next = Some(Dialog::Properties(tb));
            }
        }
    });
    ui.separator();
    let units = app.units;
    let Some((_, id)) = app.active_ids() else { return false };
    let Some(doc) = app.session.get(id) else { return false };
    let i = &doc.info;
    // Description shows plain values among text fields; they start where the fields' text does.
    let editable_info = tab == PropsTab::Description && doc.allows_modification() && app.props_draft.is_some();
    let inset = if editable_info { widgets::FIELD_TEXT_INSET } else { 0.0 };
    let row = |ui: &mut egui::Ui, k: &str, v: String| {
        // Row labels are UI chrome; values are document data and stay as they are.
        ui.label(egui::RichText::new(tl!(k)).color(t.text_muted));
        ui.horizontal(|ui| {
            ui.add_space(inset);
            ui.add(egui::Label::new(if v.is_empty() { egui::RichText::new("—").color(t.text_faint) } else { egui::RichText::new(v) }).wrap());
        });
        ui.end_row();
    };
    // A file name or path is read left to right even in an RTL UI: the extension
    // stays at its visual end, as in the tab strip.
    let path_row = |ui: &mut egui::Ui, k: &str, v: String| {
        ui.label(egui::RichText::new(tl!(k)).color(t.text_muted));
        ui.scope(|ui| {
            ui.set_max_width(420.0);
            // Align path text with editable fields while keeping its LTR text order.
            ui.add_space(inset);
            if v.is_empty() {
                ui.label(egui::RichText::new("—").color(t.text_faint));
            } else {
                crate::widgets::label_ltr(ui, v, crate::theme::regular(13.0), t.text);
            }
        });
        ui.end_row();
    };
    egui::ScrollArea::vertical().max_height(460.0).auto_shrink([false, true]).show(ui, |ui| {
        if tab == PropsTab::SpaceAudit {
            ui.add_space(4.0);
            crate::optimize_ui::audit_table(ui, &app.space_audit, &t);
            return;
        }
        egui::Grid::new("props").num_columns(2).spacing([18.0, 8.0]).min_col_width(140.0).show(ui, |ui| match tab {
            PropsTab::Description => {
                path_row(ui, "File", doc.name.clone());
                match app.props_draft.as_mut() {
                    Some((_, draft)) if editable_info => {
                        for (k, v) in INFO_KEYS.iter().zip(draft.iter_mut()) {
                            let l = ui.label(egui::RichText::new(tl!(k)).color(t.text_muted));
                            // Subject and Keywords are often a sentence or a list: they wrap and grow
                            // instead of hiding most of the value past the field's edge.
                            let field = if matches!(*k, "Subject" | "Keywords") { widgets::block(v).desired_rows(1) } else { widgets::line(v) };
                            ui.add(field.desired_width(f32::INFINITY).background_color(t.field).id_salt(("info", *k))).labelled_by(l.id);
                            ui.end_row();
                        }
                    }
                    _ => {
                        row(ui, "Title", i.title.clone().unwrap_or_default());
                        row(ui, "Author", i.author.clone().unwrap_or_default());
                        row(ui, "Subject", i.subject.clone().unwrap_or_default());
                        row(ui, "Keywords", i.keywords.clone().unwrap_or_default());
                    }
                }
                row(ui, "Application", i.creator.clone().unwrap_or_default());
                row(ui, "PDF producer", i.producer.clone().unwrap_or_default());
            }
            PropsTab::InitialView => {
                use pdfcraft_engine::{InitialLayout as L, Magnification as M, Navigation as N};
                let editable = doc.allows_modification();
                let pages = i.pages.len();
                let Some((_, v)) = app.view_draft.as_mut() else { return };
                ui.label(egui::RichText::new(tl!("Layout and Magnification")).font(theme::semibold(12.5)));
                ui.end_row();
                let label = ui.label(tl!("Navigation tab"));
                ui.add_enabled_ui(editable, |ui| {
                    craft_ui::ComboBox::from_id_salt("iv-nav")
                        .selected_text(tl!(format!("{:?}", v.navigation)
                            .replace("PageOnly", "Page Only")
                            .replace("Pages", "Pages Panel and Page")
                            .as_str()))
                        .show_ui(ui, |ui| {
                            for (n, l) in [
                                (N::PageOnly, tl!("Page Only")),
                                (N::Bookmarks, tl!("Bookmarks Panel and Page")),
                                (N::Pages, tl!("Pages Panel and Page")),
                                (N::Attachments, tl!("Attachments Panel and Page")),
                                (N::Layers, tl!("Layers Panel and Page")),
                            ] {
                                ui.selectable_value(&mut v.navigation, n, l);
                            }
                        })
                        .response
                        .labelled_by(label.id);
                });
                ui.end_row();
                let label = ui.label(tl!("Page layout"));
                ui.add_enabled_ui(editable, |ui| {
                    let names = [
                        (L::Default, tl!("Default")),
                        (L::SinglePage, tl!("Single Page")),
                        (L::SinglePageContinuous, tl!("Single Page Continuous")),
                        (L::TwoUp, tl!("Two-Up (Facing)")),
                        (L::TwoUpContinuous, tl!("Two-Up Continuous (Facing)")),
                        (L::TwoUpCoverPage, tl!("Two-Up (Cover Page)")),
                        (L::TwoUpContinuousCoverPage, tl!("Two-Up Continuous (Cover Page)")),
                    ];
                    let shown = names.iter().find(|(l, _)| *l == v.layout).map_or(tl!("Default"), |(_, n)| n);
                    craft_ui::ComboBox::from_id_salt("iv-layout")
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            for (l, n) in names {
                                ui.selectable_value(&mut v.layout, l, n);
                            }
                        })
                        .response
                        .labelled_by(label.id);
                });
                ui.end_row();
                let label = ui.label(tl!("Magnification"));
                ui.add_enabled_ui(editable, |ui| {
                    ui.horizontal(|ui| {
                        let names = [
                            (M::Default, tl!("Default")),
                            (M::ActualSize, tl!("Actual Size")),
                            (M::FitPage, tl!("Fit Page")),
                            (M::FitWidth, tl!("Fit Width")),
                            (M::FitHeight, tl!("Fit Height")),
                            (M::FitVisible, tl!("Fit Visible")),
                        ];
                        let shown = match v.magnification {
                            M::Percent(p) => format!("{p:.0}%"),
                            m => names.iter().find(|(x, _)| *x == m).map_or(tl!("Default").to_string(), |(_, n)| n.to_string()),
                        };
                        craft_ui::ComboBox::from_id_salt("iv-mag")
                            .selected_text(shown)
                            .show_ui(ui, |ui| {
                                for (m, n) in names {
                                    ui.selectable_value(&mut v.magnification, m, n);
                                }
                                for p in [50.0, 75.0, 125.0, 150.0, 200.0] {
                                    ui.selectable_value(&mut v.magnification, M::Percent(p), format!("{p:.0}%"));
                                }
                            })
                            .response
                            .labelled_by(label.id);
                    });
                });
                ui.end_row();
                let label = ui.label(tl!("Open to page"));
                ui.add_enabled_ui(editable, |ui| {
                    let mut p = v.page + 1;
                    if ui.add(egui::DragValue::new(&mut p).range(1..=pages.max(1))).labelled_by(label.id).changed() {
                        v.page = p - 1;
                    }
                    ui.label(crate::i18n::fmt(tl!("of {pages}"), &[("pages", &pages.to_string())]));
                });
                ui.end_row();
                ui.label(egui::RichText::new(tl!("Window Options")).font(theme::semibold(12.5)));
                ui.end_row();
                ui.label("");
                ui.add_enabled_ui(editable, |ui| {
                    ui.vertical(|ui| {
                        ui.add(widgets::checkbox(&mut v.fit_window, tl!("Resize window to initial page")));
                        ui.add(widgets::checkbox(&mut v.center_window, tl!("Center window on screen")));
                        ui.add(widgets::checkbox(&mut v.full_screen, tl!("Open in Full Screen mode")));
                        ui.horizontal(|ui| {
                            ui.label(tl!("Show:"));
                            widgets::radio_value(ui, &mut v.display_title, false, tl!("File Name"));
                            widgets::radio_value(ui, &mut v.display_title, true, tl!("Document Title"));
                        });
                    });
                });
                ui.end_row();
                ui.label(egui::RichText::new(tl!("User Interface Options")).font(theme::semibold(12.5)));
                ui.end_row();
                ui.label("");
                ui.add_enabled_ui(editable, |ui| {
                    ui.vertical(|ui| {
                        ui.add(widgets::checkbox(&mut v.hide_menubar, tl!("Hide menu bar")));
                        ui.add(widgets::checkbox(&mut v.hide_toolbar, tl!("Hide toolbars")));
                        ui.add(widgets::checkbox(&mut v.hide_window_ui, tl!("Hide window controls")));
                    });
                });
                ui.end_row();
            }
            PropsTab::Security => match doc.security_summary() {
                None => {
                    row(ui, "Security method", tl!("No security").to_string());
                    row(ui, "Restrictions", tl!("None — everything is allowed").to_string());
                    if doc.allows_security_change() && ui.button(tl!("Protect using password…")).clicked() {
                        req.link_command = Some("protect.password");
                    }
                }
                Some(sec) => {
                    row(ui, "Security method", tl!("Password security").to_string());
                    row(ui, "Encryption", sec.method.clone());
                    row(
                        ui,
                        "Opened with",
                        if sec.pending {
                            tl!("— (protection is applied when you save)").to_string()
                        } else if sec.owner {
                            tl!("Owner password (no restrictions apply)").to_string()
                        } else {
                            tl!("User password").to_string()
                        },
                    );
                    // A restricted document asks for its permissions password first.
                    if doc.allows_security_change() || doc.can_unlock() {
                        ui.horizontal(|ui| {
                            if doc.can_unlock() && ui.button(tl!("Unlock…")).clicked() {
                                req.link_command = Some("protect.unlock");
                            }
                            if ui.button(tl!("Change settings…")).clicked() {
                                req.link_command = Some("protect.password");
                            }
                            if ui.button(tl!("Remove security")).clicked() {
                                req.link_command = Some("protect.remove");
                            }
                        });
                    }
                    let p = sec.permissions;
                    let yes = |b: bool| tl!(if b { "Allowed" } else { "Not allowed" }).to_string();
                    row(
                        ui,
                        "Printing",
                        if !p.print() {
                            tl!("Not allowed").to_string()
                        } else if p.print_high_quality() {
                            tl!("High resolution").to_string()
                        } else {
                            tl!("Low resolution").to_string()
                        },
                    );
                    row(ui, "Changing the document", yes(p.modify()));
                    row(ui, "Document assembly", yes(p.assemble()));
                    row(ui, "Content copying", yes(p.copy()));
                    row(ui, "Content copying for accessibility", yes(p.extract_for_accessibility()));
                    row(ui, "Commenting", yes(p.annotate()));
                    row(ui, "Filling of form fields", yes(p.fill_forms()));
                }
            },
            PropsTab::Fonts => {
                if i.fonts.is_empty() {
                    row(ui, "Fonts", tl!("No fonts are referenced by the pages.").to_string());
                }
                for f in &i.fonts {
                    let mut detail = f.kind.clone();
                    if let Some(e) = &f.encoding {
                        detail.push_str(&format!(" · {e}"));
                    }
                    detail.push_str(if f.subset {
                        " · Embedded subset"
                    } else if f.embedded {
                        " · Embedded"
                    } else {
                        " · Not embedded (substituted)"
                    });
                    row(ui, &f.name, detail);
                }
            }
            PropsTab::Custom => {
                // Add takes a new name, Update changes the value of the listed property the
                // Name field names, Delete removes it; OK writes the changes as one step.
                let editable = doc.allows_modification();
                let Some((_, d)) = app.custom_draft.as_mut() else { return };
                let l = ui.label(egui::RichText::new(tl!("Name:")).color(t.text_muted));
                ui.add_enabled(editable, widgets::line(&mut d.name).desired_width(420.0).background_color(t.field).id_salt("custom-name"))
                    .labelled_by(l.id);
                ui.end_row();
                let l = ui.label(egui::RichText::new(tl!("Value:")).color(t.text_muted));
                ui.add_enabled(editable, widgets::line(&mut d.value).desired_width(420.0).background_color(t.field).id_salt("custom-value"))
                    .labelled_by(l.id);
                ui.end_row();
                // Untrimmed: a file's `/Project#20` is not `/Project`, and a typed space is refused.
                let name = d.name.clone();
                let value = d.value.trim().to_owned();
                let listed = d.props.iter().position(|(k, _)| *k == name);
                // An entry the list leaves out (not text, only padding, or past its 256) keeps its value.
                let file_props = doc.custom_info();
                let unlisted = doc.has_info(&name) && !file_props.iter().any(|(k, _)| *k == name);
                let refused = pdfcraft_engine::check_custom_key(&name).err().filter(|_| !name.is_empty()).map(|e| e.to_string());
                ui.label("");
                // A file can hold a custom name that `check_custom_key` refuses, such as one
                // with a space: the tab lists it, and Add, Update and Delete stay off for it.
                let why = |b: egui::Response| match &refused {
                    Some(why) => b.on_disabled_hover_text(crate::i18n::words(why)),
                    None => b,
                };
                // A name that isn't UTF-8 is listed with U+FFFD and can't be written back as it is.
                let editable = editable && refused.is_none() && !name.contains(char::REPLACEMENT_CHARACTER);
                // `custom_info` lists at most 256 properties, and a new name goes after the file's
                // entries, so Add is off once the draft or the file's own list holds 256.
                let room = d.props.len() < 256 && file_props.len() < 256;
                ui.horizontal(|ui| {
                    let add = editable && room && listed.is_none() && !unlisted && !name.is_empty() && !value.is_empty();
                    if why(ui.add_enabled(add, egui::Button::new(tl!("Add")))).clicked() {
                        d.props.push((name.clone(), value.clone()));
                        d.name.clear();
                        d.value.clear();
                    }
                    let update = editable && !value.is_empty() && listed.and_then(|n| d.props.get(n)).is_some_and(|(_, v)| *v != value);
                    if why(ui.add_enabled(update, egui::Button::new(tl!("Update")))).clicked()
                        && let Some(p) = listed.and_then(|n| d.props.get_mut(n))
                    {
                        p.1 = value.clone();
                    }
                    if why(ui.add_enabled(editable && listed.is_some(), egui::Button::new(tl!("Delete")))).clicked()
                        && let Some(n) = listed.filter(|n| *n < d.props.len())
                    {
                        d.props.remove(n);
                        d.name.clear();
                        d.value.clear();
                    }
                });
                ui.end_row();
                if d.props.is_empty() {
                    ui.label("");
                    ui.label(egui::RichText::new(tl!("None")).color(t.text_faint));
                    ui.end_row();
                }
                // A click on a property fills the fields with it; values are document data.
                let mut picked = None;
                for (k, v) in &d.props {
                    if ui.selectable_label(*k == name, k.as_str()).clicked() {
                        picked = Some((k.clone(), v.clone()));
                    }
                    ui.scope(|ui| {
                        ui.set_max_width(420.0);
                        ui.add(egui::Label::new(v.as_str()).wrap());
                    });
                    ui.end_row();
                }
                if let Some((k, v)) = picked {
                    d.name = k;
                    d.value = v;
                }
            }
            PropsTab::Advanced => {
                row(ui, "PDF version", i.pdf_version.clone());
                path_row(ui, "Location", doc.path.clone().unwrap_or_default());
                row(ui, "File size", format!("{} ({})", human_size(i.file_size), crate::panels::byte_count(i.file_size)));
                let p = &i.pages[0];
                row(ui, "Page size", units.size(f64::from(p.width), f64::from(p.height)));
                row(ui, "Number of pages", i.pages.len().to_string());
                row(ui, "Tagged PDF", yes(i.tagged));
                row(ui, "Form fields", i.fields.len().to_string());
                row(ui, "Comments", i.annotations.len().to_string());
                row(ui, "Layers", i.layers.len().to_string());
                row(ui, "Attachments", i.attachments.len().to_string());
                row(ui, "JavaScript", yes(i.has_javascript));
                // Each incremental update is a revision; earlier ones open as their own document.
                let ends = doc.revision_ends();
                if ends.len() > 1 {
                    ui.label(egui::RichText::new(tl!("Revisions")).color(t.text_muted));
                    // Reserve the full wrapped height before the grid starts the next row.
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(ends.len().to_string());
                            for n in (1..ends.len()).rev().take(12) {
                                if ui
                                    .small_button(crate::i18n::fmt(tl!("View revision {n}"), &[("n", &n.to_string())]))
                                    .on_hover_text(tl!("Open the file as it was saved then"))
                                    .clicked()
                                {
                                    req.open_revision = Some(n);
                                }
                            }
                        });
                    });
                    ui.end_row();
                }
                // Reading Options: binding and language.
                if let Some((_, v)) = app.view_draft.as_mut() {
                    let editable = doc.allows_modification();
                    ui.label(egui::RichText::new(tl!("Binding")).color(t.text_muted));
                    ui.add_enabled_ui(editable, |ui| {
                        ui.horizontal(|ui| {
                            widgets::radio_value(ui, &mut v.right_to_left, false, tl!("Left Edge"));
                            widgets::radio_value(ui, &mut v.right_to_left, true, tl!("Right Edge"));
                        });
                    });
                    ui.end_row();
                    let l = ui.label(egui::RichText::new(tl!("Language")).color(t.text_muted));
                    let mut lang = v.language.clone().unwrap_or_default();
                    if ui
                        .add_enabled(editable, widgets::line(&mut lang).hint_text(tl!("e.g. en-US")).desired_width(160.0))
                        .labelled_by(l.id)
                        .changed()
                    {
                        v.language = (!lang.trim().is_empty()).then(|| lang.trim().to_string());
                    }
                    ui.end_row();
                }
                // What was repaired while reading a damaged file (fidelity: never silent).
                let repairs = doc.repair_log();
                row(ui, "Repairs", if repairs.is_empty() { tl!("None").to_string() } else { repairs.len().to_string() });
                if !repairs.is_empty() {
                    ui.label("");
                    egui::CollapsingHeader::new(tl!("Repair log")).show(ui, |ui| {
                        for r in &repairs {
                            ui.add(egui::Label::new(egui::RichText::new(r).small()).wrap());
                        }
                    });
                    ui.end_row();
                }
            }
            // Laid out above, outside the two-column grid.
            PropsTab::SpaceAudit => {}
        });
    });
    true
}

/// `Dialog::Split` content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_split(ui: &mut egui::Ui, app: &mut PdfCraftApp, req: &mut Pending) -> bool {
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(tl!("Split document")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let Some((vi, id)) = app.active_ids() else { return false };
    let n = app.session.get(id).map(|d| d.info.pages.len()).unwrap_or(0);
    let selected: Vec<usize> = app.views[vi].selected.iter().copied().filter(|p| *p > 0).collect();
    let marks = app.session.bookmark_splits(id);
    let draft = &mut app.split_draft;
    use crate::SplitMode as M;
    if selected.is_empty() && draft.mode == M::Selection {
        draft.mode = M::Pages;
    }
    widgets::radio_value(ui, &mut draft.mode, M::Pages, tl!("Number of pages"));
    ui.add_enabled_ui(draft.mode == M::Pages, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.label(tl!("Pages per file"));
            ui.add(egui::DragValue::new(&mut draft.every).range(1..=n.max(1)));
        });
    });
    widgets::radio_value(ui, &mut draft.mode, M::Size, tl!("File size"));
    ui.add_enabled_ui(draft.mode == M::Size, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.label(tl!("At most"));
            ui.add(egui::DragValue::new(&mut draft.size_mb).range(0.05..=2000.0).speed(0.1).suffix(" MB"));
        });
    });
    ui.add_enabled_ui(!marks.is_empty(), |ui| {
        widgets::radio_value(
            ui,
            &mut draft.mode,
            M::Bookmarks,
            crate::i18n::fmt(tl!("Top-level bookmarks ({n})"), &[("n", &marks.len().to_string())]),
        )
    });
    ui.add_enabled_ui(!selected.is_empty(), |ui| {
        widgets::radio_value(ui, &mut draft.mode, M::Selection, tl!("Before each selected page (select pages in Organize)"))
    });
    let plan = match draft.mode {
        M::Pages => crate::SplitPlan::By(pdfcraft_engine::SplitBy::PageCount(draft.every)),
        M::Selection => crate::SplitPlan::By(pdfcraft_engine::SplitBy::Before(selected)),
        M::Size => crate::SplitPlan::Size((draft.size_mb * 1_048_576.0) as usize),
        M::Bookmarks => crate::SplitPlan::Bookmarks,
    };
    let files = match &plan {
        crate::SplitPlan::By(by) => Some(pdfcraft_engine::split_ranges(n, by).len()),
        crate::SplitPlan::Bookmarks => {
            Some(pdfcraft_engine::split_ranges(n, &pdfcraft_engine::SplitBy::Before(marks.iter().map(|m| m.0).collect())).len())
        }
        crate::SplitPlan::Size(_) => None,
    };
    ui.add_space(8.0);
    let text = match files {
        Some(f) => {
            if f == 1 {
                crate::i18n::fmt(tl!("Creates 1 file from {n} pages."), &[("n", &n.to_string())])
            } else {
                crate::i18n::fmt(tl!("Creates {f} files from {n} pages."), &[("f", &f.to_string()), ("n", &n.to_string())])
            }
        }
        None => crate::i18n::fmt(tl!("Each file holds as many of the {n} pages as fit."), &[("n", &n.to_string())]),
    };
    ui.label(egui::RichText::new(text).color(t.text_muted));
    if files.is_none_or(|f| f > 1) {
        req.split_ready = Some(plan);
    }
    true
}

/// `Dialog::ReplacePages` content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_replace_pages(ui: &mut egui::Ui, app: &mut PdfCraftApp, req: &mut Pending) -> bool {
    let t = Tokens::get(ui.ctx());
    let count = app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|d| d.info.pages.len()).unwrap_or(1).max(1);
    let Some(d) = app.replace_draft.as_mut() else {
        req.close = true;
        return false;
    };
    ui.label(egui::RichText::new(tl!("Replace Pages")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    d.to = d.to.clamp(1, count);
    d.from = d.from.clamp(1, d.to);
    ui.horizontal(|ui| {
        let from_label = ui.label(tl!("Original: replace pages"));
        let from = ui.add(egui::DragValue::new(&mut d.from).range(1..=count)).labelled_by(from_label.id);
        let to_label = ui.label(tl!("to"));
        let to = ui.add(egui::DragValue::new(&mut d.to).range(1..=count)).labelled_by(to_label.id);
        if from.changed() || to.changed() {
            d.range_edited = true;
        }
        ui.label(egui::RichText::new(crate::i18n::fmt(tl!("of {name}"), &[("name", &count.to_string())])).color(t.text_muted));
    });
    let n = if d.range_edited { d.to - d.from + 1 } else { d.pages.len().max(1) };
    let max_start = d.src_pages.saturating_sub(n) + 1;
    d.src_from = d.src_from.clamp(1, max_start.max(1));
    ui.horizontal(|ui| {
        let replacement_label = ui.label(crate::i18n::fmt(tl!("Replacement: pages of {name}"), &[("name", &d.name)]));
        ui.add(egui::DragValue::new(&mut d.src_from).range(1..=max_start.max(1))).labelled_by(replacement_label.id);
        ui.label(crate::i18n::fmt(tl!("to {n}"), &[("n", &(d.src_from + n - 1).to_string())]));
        ui.label(egui::RichText::new(crate::i18n::fmt(tl!("of {name}"), &[("name", &d.src_pages.to_string())])).color(t.text_muted));
    });
    let fits = n <= d.src_pages;
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(if fits {
            tl!("Only the page content changes: links, comments, form fields and bookmarks on the original pages stay.")
        } else {
            tl!("The replacement file doesn't have that many pages.")
        })
        .small()
        .color(t.text_faint),
    );
    ui.add_space(10.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.add_enabled_ui(fits, |ui| widgets::pill_button(ui, tl!("OK"), true)).inner.clicked() {
            req.replace_now = true;
            req.close = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            req.close = true;
        }
    });
    false
}

/// `Dialog::RedactProps` content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_redact_props(ui: &mut egui::Ui, app: &mut PdfCraftApp, req: &mut Pending) -> bool {
    let t = Tokens::get(ui.ctx());
    let mut prefs = app.redact_prefs.clone();
    let (ok, cancel) = crate::redact_ui::props_body(ui, &mut prefs, &t);
    app.redact_prefs = prefs;
    if ok && app.redact_prefs.remember {
        app.redact_defaults = app.redact_prefs.clone();
    }
    if cancel && let Some(before) = app.redact_before.take() {
        app.redact_prefs = before;
    }
    if ok {
        app.redact_before = None;
    }
    req.close = ok || cancel;
    false
}

/// `Dialog::Extract` content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_extract(ui: &mut egui::Ui, app: &mut PdfCraftApp, req: &mut Pending) -> bool {
    let (selected, total) = app.active_ids().map_or((Vec::new(), 0), |(i, _)| (app.views[i].target_pages(), app.views[i].page_count()));
    ui.label(egui::RichText::new(tl!("Extract pages")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    // Page ranges to take instead of the selection (#3593); the hint is the selection.
    let hint = match crate::combine_ui::range_of(&selected, total) {
        all if all.is_empty() && total > 1 => format!("1-{total}"),
        all if all.is_empty() => "1".to_string(),
        some => some,
    };
    ui.horizontal(|ui| {
        let l = ui.label(tl!("Pages"));
        ui.add(egui::TextEdit::singleline(&mut app.extract_draft.pages).hint_text(hint).desired_width(240.0)).labelled_by(l.id);
    });
    let chosen = app.extract_draft.pages_to_extract(selected, total);
    match &chosen {
        Ok(pages) if pages.len() == 1 => {
            ui.label(tl!("1 page selected."));
        }
        Ok(pages) => {
            ui.label(crate::i18n::fmt(tl!("{n} pages selected."), &[("n", &pages.len().to_string())]));
        }
        Err(e) => {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
    }
    ui.add_space(4.0);
    // The name the pages are saved under (#737); separate files add " (page N)".
    ui.horizontal(|ui| {
        let l = ui.label(tl!("File name"));
        ui.add(egui::TextEdit::singleline(&mut app.extract_draft.name).desired_width(240.0)).labelled_by(l.id);
    });
    ui.add(widgets::checkbox(&mut app.extract_draft.delete, tl!("Delete pages after extracting")));
    ui.add(widgets::checkbox(&mut app.extract_draft.separate, tl!("Extract pages as separate files")));
    ui.add_space(12.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.add_enabled_ui(chosen.is_ok(), |ui| widgets::pill_button(ui, tl!("Extract"), true)).inner.clicked() {
            req.extract_now = true;
            req.close = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            req.close = true;
        }
    });
    false
}

/// `Dialog::RotatePages` content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_rotate_pages(ui: &mut egui::Ui, app: &mut PdfCraftApp, req: &mut Pending) -> bool {
    use pdfcraft_engine::{PageOrientation as O, PageParity as P};
    let n = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len());
    let d = &mut app.rotate_draft;
    ui.label(egui::RichText::new(tl!("Rotate Pages")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    egui::Grid::new("rotate-pages").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label(tl!("Direction:"));
        craft_ui::ComboBox::from_id_salt("rotate-dir")
            .selected_text(match d.degrees {
                270 => tl!("Counterclockwise 90 degrees"),
                180 => tl!("180 degrees"),
                _ => tl!("Clockwise 90 degrees"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut d.degrees, 90, tl!("Clockwise 90 degrees"));
                ui.selectable_value(&mut d.degrees, 270, tl!("Counterclockwise 90 degrees"));
                ui.selectable_value(&mut d.degrees, 180, tl!("180 degrees"));
            });
        ui.end_row();
        ui.label(tl!("Pages:"));
        ui.vertical(|ui| {
            widgets::radio_value(ui, &mut d.which, 0, tl!("All"));
            widgets::radio_value(ui, &mut d.which, 1, tl!("Selection"));
            ui.horizontal(|ui| {
                widgets::radio_value(ui, &mut d.which, 2, tl!("From"));
                ui.add_enabled(d.which == 2, egui::DragValue::new(&mut d.from).range(1..=n));
                ui.label(tl!("to"));
                ui.add_enabled(d.which == 2, egui::DragValue::new(&mut d.to).range(1..=n));
                ui.label(crate::i18n::fmt(tl!("of {name}"), &[("name", &n.to_string())]));
            });
        });
        ui.end_row();
        ui.label(tl!("Rotate:"));
        craft_ui::ComboBox::from_id_salt("rotate-parity")
            .selected_text(match d.parity {
                P::Both => tl!("Even and Odd Pages"),
                P::Even => tl!("Even Pages Only"),
                P::Odd => tl!("Odd Pages Only"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut d.parity, P::Both, tl!("Even and Odd Pages"));
                ui.selectable_value(&mut d.parity, P::Even, tl!("Even Pages Only"));
                ui.selectable_value(&mut d.parity, P::Odd, tl!("Odd Pages Only"));
            });
        ui.end_row();
        ui.label("");
        craft_ui::ComboBox::from_id_salt("rotate-orient")
            .selected_text(match d.orientation {
                O::Both => tl!("Landscape and Portrait Pages"),
                O::Landscape => tl!("Landscape Pages"),
                O::Portrait => tl!("Portrait Pages"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut d.orientation, O::Both, tl!("Landscape and Portrait Pages"));
                ui.selectable_value(&mut d.orientation, O::Landscape, tl!("Landscape Pages"));
                ui.selectable_value(&mut d.orientation, O::Portrait, tl!("Portrait Pages"));
            });
        ui.end_row();
    });
    d.to = d.to.max(d.from);
    ui.add_space(12.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, tl!("OK"), true).clicked() {
            req.rotate_now = true;
            req.close = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            req.close = true;
        }
    });
    false
}

/// `Dialog::DuplicateField` content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_duplicate_field(ui: &mut egui::Ui, app: &mut PdfCraftApp, req: &mut Pending) -> bool {
    let n = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len());
    let Some(d) = app.duplicate_draft.as_mut() else {
        req.close = true;
        return false;
    };
    ui.label(egui::RichText::new(tl!("Duplicate Field")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.label(crate::i18n::fmt(tl!("Duplicate \"{name}\" onto:"), &[("name", &d.name)]));
    widgets::radio_value(ui, &mut d.all, true, tl!("All pages"));
    ui.horizontal(|ui| {
        widgets::radio_value(ui, &mut d.all, false, tl!("From"));
        ui.add_enabled(!d.all, egui::DragValue::new(&mut d.from).range(1..=n));
        ui.label(tl!("to"));
        ui.add_enabled(!d.all, egui::DragValue::new(&mut d.to).range(1..=n));
        ui.label(crate::i18n::fmt(tl!("of {name}"), &[("name", &n.to_string())]));
    });
    d.to = d.to.max(d.from);
    ui.add_space(12.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, tl!("OK"), true).clicked() {
            let pages: Vec<usize> = if d.all { (0..n).collect() } else { (d.from - 1..d.to.min(n)).collect() };
            req.duplicate_now = Some(Edit::DuplicateField { name: d.name.clone(), pages });
            req.close = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            req.close = true;
        }
    });
    false
}

/// `Dialog::CertificateViewer` content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_certificate_viewer(ui: &mut egui::Ui, app: &mut PdfCraftApp, req: &mut Pending) -> bool {
    let t = Tokens::get(ui.ctx());
    ui.set_width(700.0);
    let trusted = app.session.trusted_certificates().to_vec();
    let Some(v) = app.cert_viewer.as_mut() else {
        req.close = true;
        return false;
    };
    let (done, action) = crate::sign_ui::cert_viewer(ui, v, &trusted, &t);
    req.close = done;
    match action {
        Some(crate::sign_ui::CertAction::Trust(c)) => app.trust_certificate(*c),
        Some(crate::sign_ui::CertAction::Export(c)) => {
            app.export_certificate(&c);
        }
        None => {}
    }
    false
}

/// `Dialog::SummarizeComments` content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_summarize_comments(ui: &mut egui::Ui, app: &mut PdfCraftApp, req: &mut Pending) -> bool {
    ui.label(egui::RichText::new(tl!("Summarize Options")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.label(tl!("Choose a layout:"));
    let _ = ui.add(widgets::radio(true, tl!("Comments only")));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(tl!("Sort comments by:"));
        craft_ui::ComboBox::from_id_salt("summary-sort").selected_text(tl!(app.summary_sort.name())).show_ui(ui, |ui| {
            for s in pdfcraft_engine::SummarySort::ALL {
                ui.selectable_value(&mut app.summary_sort, s, tl!(s.name()));
            }
        });
    });
    ui.add_space(12.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, tl!("Create PDF Comment Summary"), true).clicked() {
            req.summarize_now = true;
            req.close = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            req.close = true;
        }
    });
    false
}

/// `Dialog::Revert` content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_revert(ui: &mut egui::Ui, app: &mut PdfCraftApp, req: &mut Pending) -> bool {
    let name = app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|d| d.name.clone()).unwrap_or_default();
    ui.label(egui::RichText::new(tl!("Revert")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.label(crate::i18n::fmt(
        tl!("Revert to the previously saved version of “{name}”? Changes since then can't be undone afterwards."),
        &[("name", &name)],
    ));
    ui.add_space(12.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, tl!("Revert"), true).clicked() {
            req.revert_now = true;
            req.close = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            req.close = true;
        }
    });
    false
}

/// `Dialog::OverwriteChanged` content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_overwrite_changed(ui: &mut egui::Ui, app: &mut PdfCraftApp, req: &mut Pending) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    let name = app.pending_overwrite.as_ref().and_then(|p| app.session.get(p.id)).map(|d| d.name.clone()).unwrap_or_default();
    #[cfg(target_arch = "wasm32")]
    let name = String::new();
    ui.label(egui::RichText::new(tl!("Save over the changed file?")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.label(crate::i18n::fmt(
        tl!("“{name}” changed on disk after it was opened or last saved. Saving now replaces that newer version with this one."),
        &[("name", &name)],
    ));
    ui.add_space(12.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, tl!("Save anyway"), true).clicked() {
            req.overwrite_now = true;
            req.close = true;
        }
        if widgets::pill_button(ui, tl!("Save as…"), false).clicked() {
            req.overwrite_elsewhere = true;
            req.close = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            req.close = true;
        }
    });
    false
}

/// `Dialog::Print` content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_print(ui: &mut egui::Ui, app: &mut PdfCraftApp, req: &mut Pending) -> bool {
    let t = Tokens::get(ui.ctx());
    let Some((i, id)) = app.active_ids() else {
        req.close = true;
        return false;
    };
    if app.print_draft.handling == crate::print_ui::Handling::Label {
        app.read_label_crops(id);
    }
    let Some(doc) = app.session.get(id) else { return false };
    let sizes: Vec<(f64, f64)> = doc.info.pages.iter().map(|p| (p.width as f64, p.height as f64)).collect();
    let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
    let rasters = crate::print_ui::preview_rasters(
        &app.print_draft,
        &sizes,
        &labels,
        ui.ctx().pixels_per_point(),
        ui.ctx().input(|i| i.max_texture_side) as f32,
    );
    let view = &mut app.views[i];
    view.set_print_grayscale(app.print_draft.grayscale);
    view.queue_print_previews(&rasters);
    let (go, cancel) = crate::print_ui::body(ui, &mut app.print_draft, &t, &sizes, &labels, &mut |p| {
        view.need_thumbnail(p, true);
        view.page_preview(p)
    });
    req.print_go = go;
    req.close = go || cancel;
    false
}

/// `Dialog::Signature` content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_signature(ui: &mut egui::Ui, app: &mut PdfCraftApp, req: &mut Pending) -> bool {
    let t = Tokens::get(ui.ctx());
    let (go, cancel, browse) =
        crate::fill_sign::signature_pad(ui, &t, &mut app.signature_draft, &mut app.signature_preview, &mut app.signature_photo);
    req.apply = go;
    if browse {
        app.pick_signature_image();
    }
    if req.apply {
        let d = std::mem::take(&mut app.signature_draft);
        let tool = if d.initials {
            app.initials = d.saved();
            crate::fill_sign::FillTool::Initials
        } else {
            app.signature = d.saved();
            crate::fill_sign::FillTool::Signature
        };
        app.quick_tool = crate::QuickTool::Fill(tool);
        app.toast = None;
    }
    req.close = req.apply || cancel;
    false
}

/// `Dialog::Preferences` content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_preferences(ui: &mut egui::Ui, app: &mut PdfCraftApp, req: &mut Pending) -> bool {
    let t = Tokens::get(ui.ctx());
    // The settings outgrow a short window: scroll them rather than run off the screen.
    // The OK row stays below the scroll, in reach however far the settings are scrolled.
    let max = (ui.ctx().content_rect().height() - 120.0 - crate::js_ui::PREFERENCES_BUTTON_ROW).max(crate::js_ui::PREFERENCES_MIN_SCROLL);
    // A scroll area starts from the room the dialog offers it, and a modal offers more
    // each frame for the first few, so with OK outside the scroll the dialog would grow
    // over those frames. Asking for the full height as the least it may scroll in
    // (which only applies when the settings overflow it) opens the dialog at full size.
    // A scrollbar of its own: the floating one lies over the right edge of the theme
    // cards and other full-width rows (#3557).
    ui.scope(|ui| {
        ui.spacing_mut().scroll.floating = false;
        egui::ScrollArea::vertical().max_height(max).min_scrolled_height(max).show(ui, |ui| crate::js_ui::preferences_body(ui, app, &t));
    });
    req.close = crate::js_ui::preferences_buttons(ui);
    false
}

/// `Dialog::NumberPages` content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_number_pages(ui: &mut egui::Ui, app: &mut PdfCraftApp, req: &mut Pending) -> bool {
    let t = Tokens::get(ui.ctx());
    use pdfcraft_engine::LabelStyle as L;
    ui.label(egui::RichText::new(tl!("Number pages")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let Some((_, id)) = app.active_ids() else { return false };
    let n = app.session.get(id).map(|d| d.info.pages.len()).unwrap_or(1).max(1);
    let d = &mut app.number_draft;
    d.to = d.to.clamp(1, n);
    d.from = d.from.clamp(1, d.to);
    egui::Grid::new("number_pages").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label(tl!("Pages"));
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut d.from).range(1..=n));
            ui.label(tl!("to"));
            ui.add(egui::DragValue::new(&mut d.to).range(1..=n));
            ui.label(egui::RichText::new(crate::i18n::fmt(tl!("of {name}"), &[("name", &n.to_string())])).color(t.text_muted));
        });
        ui.end_row();
        ui.label(tl!("Style"));
        let styles = [
            (L::Decimal, "1, 2, 3"),
            (L::LowerRoman, "i, ii, iii"),
            (L::UpperRoman, "I, II, III"),
            (L::LowerAlpha, "a, b, c"),
            (L::UpperAlpha, "A, B, C"),
            (L::None, tl!("None (prefix only)")),
        ];
        let current = styles.iter().find(|(s, _)| *s == d.style).map_or("1, 2, 3", |(_, l)| *l);
        craft_ui::ComboBox::from_id_salt("label_style").selected_text(current).show_ui(ui, |ui| {
            for (s, l) in styles {
                ui.selectable_value(&mut d.style, s, l);
            }
        });
        ui.end_row();
        let l = ui.label(tl!("Prefix"));
        ui.add(widgets::line(&mut d.prefix).desired_width(160.0)).labelled_by(l.id);
        ui.end_row();
        ui.label(tl!("Start"));
        ui.add(egui::DragValue::new(&mut d.start).range(1..=99_999));
        ui.end_row();
    });
    d.to = d.to.max(d.from);
    let label = |k: u32| format!("{}{}", d.prefix, d.style.format(k));
    ui.add_space(8.0);
    let preview = if d.from == d.to {
        label(d.start)
    } else {
        format!("{}, {} … {}", label(d.start), label(d.start + 1), label(d.start + (d.to - d.from) as u32))
    };
    ui.label(
        egui::RichText::new(crate::i18n::fmt(tl!("Labels: {preview}. Later pages keep their labels."), &[("preview", &preview)])).color(t.text_muted),
    );
    req.number_now = Some(Edit::NumberPages { from: d.from - 1, to: d.to - 1, style: d.style, prefix: d.prefix.clone(), first: d.start });
    true
}

/// `Dialog::Recovery` content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_recovery(ui: &mut egui::Ui, app: &mut PdfCraftApp) -> bool {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        ui.add(crate::icons::image("clock-3", 22.0, t.accent));
        ui.label(egui::RichText::new(tl!("Recover unsaved documents?")).font(theme::semibold(18.0)));
    });
    ui.add_space(6.0);
    ui.label(tl!("PdfCraft didn't shut down normally. These documents had changes that were autosaved:"));
    ui.add_space(8.0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    egui::Grid::new("recoverable").num_columns(2).spacing([18.0, 6.0]).show(ui, |ui| {
        for m in &app.recoverable {
            ui.label(egui::RichText::new(&m.name).font(theme::medium(13.0)));
            let mins = now.saturating_sub(m.saved_at) / 60;
            let when = match mins {
                0 => tl!("just now").to_string(),
                1..=59 => crate::i18n::fmt(tl!("{n} min ago"), &[("n", &mins.to_string())]),
                _ => crate::i18n::fmt(tl!("{n} h ago"), &[("n", &(mins / 60).to_string())]),
            };
            let lock = if m.encrypted { tl!(" · password-protected") } else { "" };
            ui.label(egui::RichText::new(format!("{when}{lock}")).color(t.text_muted));
            ui.end_row();
        }
    });
    true
}

/// `Dialog::Shortcuts` content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_shortcuts(ui: &mut egui::Ui) -> bool {
    ui.label(egui::RichText::new(tl!("Keyboard shortcuts")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    use crate::commands::{
        ACTUAL_SIZE, COPY, FIND_NEXT, FIND_PREV, FIT_WIDTH, PAGE_LEVEL, PAGE_NEXT, PAGE_PREV, ROTATE_CCW, ROTATE_CW, SELECT_ALL, ZOOM_IN, ZOOM_OUT,
    };
    // Registered commands first (always in sync with the real bindings), then the
    // keys the document view handles itself.
    let mut rows: Vec<(String, String)> = pdfcraft_engine::commands::COMMANDS
        .iter()
        .filter_map(|c| c.shortcut.map(|k| (crate::commands::shortcut_label(ui.ctx(), k), tl!(c.label).trim_end_matches('…').to_string())))
        .collect();
    let key = |s: pdfcraft_engine::commands::Shortcut| crate::commands::shortcut_label(ui.ctx(), s);
    let pair = |a, b| format!("{} / {}", key(a), key(b));
    // Key names, translated where a catalog has them (not scanned as UI literals).
    let named = |k: &str| tl!(k).to_string();
    // Alt+← / Alt+→ (`canvas::shortcuts`): not a registered command, which has no Alt.
    let alt_arrows = if crate::commands::mac_shortcuts(ui.ctx()) {
        "⌥← / ⌥→".to_owned()
    } else {
        let alt = crate::i18n::key_name("Alt");
        format!("{alt}+← / {alt}+→")
    };
    let scroll = crate::i18n::fmt(tl!("Zoom in / out (also pinch or {key}-scroll)"), &[("key", crate::commands::command_modifier_label(ui.ctx()))]);
    for (k, v) in [
        (pair(FIND_NEXT, FIND_PREV), tl!("Next / previous match").to_string()),
        (key(COPY), tl!("Copy selected text").to_string()),
        (named("Double-click"), tl!("Select a word").to_string()),
        (crate::i18n::key_name("Esc").to_string(), tl!("Clear selection / close find").to_string()),
        (key(ACTUAL_SIZE), tl!("Actual size").to_string()),
        (key(PAGE_LEVEL), tl!("Zoom to page level").to_string()),
        (key(FIT_WIDTH), tl!("Fit to width").to_string()),
        (pair(ZOOM_IN, ZOOM_OUT), scroll),
        (pair(ROTATE_CW, ROTATE_CCW), tl!("Rotate view").to_string()),
        (format!("{} / {}", crate::i18n::key_name("Home"), crate::i18n::key_name("End")), tl!("First / last page").to_string()),
        (format!("← / →, {}", pair(PAGE_PREV, PAGE_NEXT)), tl!("Previous / next page").to_string()),
        (alt_arrows, format!("{} / {}", tl!("Previous view"), tl!("Next view"))),
        (named("V"), tl!("Select (V)").to_string()),
        (named("H / Space (hold)"), tl!("Hand (H)").to_string()),
        (crate::i18n::key_name("Delete").to_string(), tl!("Delete selected pages (Organize)").to_string()),
        (key(SELECT_ALL), tl!("Select all pages (Organize)").to_string()),
    ] {
        rows.push((k, v));
    }
    egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| {
        egui::Grid::new("keys").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
            for (k, v) in &rows {
                ui.label(egui::RichText::new(k).font(egui::FontId::monospace(12.5)));
                ui.label(v);
                ui.end_row();
            }
        });
    });
    true
}

/// `Dialog::About` content. Returns `false` when it has its own buttons and no footer is wanted.
fn dialog_about(ui: &mut egui::Ui, req: &mut Pending) -> bool {
    let t = Tokens::get(ui.ctx());
    // Tabs About · Contributors · Models.
    let tab_id = egui::Id::unique("about_tab");
    let mut tab = ui.data_mut(|d| d.get_temp::<u8>(tab_id)).unwrap_or(0);
    ui.horizontal_wrapped(|ui| {
        for (i, label) in ["About", "Contributors", "Models"].into_iter().enumerate() {
            let i = i as u8;
            if widgets::mode_tab(ui, tl!(label), tab == i).clicked() {
                tab = i;
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(tab_id, tab));
    ui.separator();
    match tab {
        1 => crate::credits::contributors_ui(ui),
        2 => crate::credits::models_ui(ui),
        _ => {
            ui.horizontal(|ui| {
                widgets::artcraft_mark(ui, 40.0);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("PdfCraft").font(theme::semibold(20.0)));
                    ui.label(crate::i18n::fmt(tl!("Version {v}"), &[("v", &pdfcraft_engine::build_info::version())]));
                });
            });
            ui.add_space(6.0);
            ui.label(tl!("A clean-room, open-source PDF application written in Rust. MIT OR Apache-2.0."));
            ui.label(
                egui::RichText::new(
                    "Rendering: hayro (bootstrap) · UI: egui · Icons: Lucide (ISC) · Fonts: Inter, JetBrains Mono, Dancing Script (OFL)",
                )
                .color(t.text_muted)
                .small(),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tl!("Part of")).color(t.text_muted));
                widgets::artcraft_logo(ui, 16.0);
            });
            ui.add_space(6.0);
            if let Some(cmd) = widgets::community_links(ui) {
                req.link_command = Some(cmd);
            }
        }
    }
    true
}

fn dialog_footer(ui: &mut egui::Ui, app: &mut PdfCraftApp, dialog: Dialog, req: &mut Pending) {
    ui.add_space(16.0);
    let changed = draft_changes(app).is_some_and(|c| !c.is_empty());
    // One row high: on its own, a right-to-left layout centres the buttons in all the height
    // left, which after a taller Properties tab is the room that tab left behind.
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if dialog == Dialog::Recovery {
                if widgets::pill_button(ui, tl!("Recover"), true).clicked() {
                    req.recover = Some(true);
                    req.close = true;
                }
                if widgets::pill_button(ui, tl!("Discard"), false).clicked() {
                    req.recover = Some(false);
                    req.close = true;
                }
            } else if dialog == Dialog::NumberPages {
                if widgets::pill_button(ui, tl!("OK"), true).clicked() {
                    req.apply_number = true;
                    req.close = true;
                }
                if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                    req.close = true;
                }
            } else if dialog == Dialog::Split {
                if ui.add_enabled_ui(req.split_ready.is_some(), |ui| widgets::pill_button(ui, tl!("Split"), true)).inner.clicked() {
                    req.split_now = req.split_ready.clone();
                    req.close = true;
                }
                if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                    req.close = true;
                }
            } else if changed {
                if widgets::pill_button(ui, tl!("OK"), true).clicked() {
                    req.apply = true;
                    req.close = true;
                }
                if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                    req.close = true;
                }
            } else if widgets::pill_button(ui, tl!("Close"), true).clicked() {
                req.close = true;
            }
        })
    });
}

/// Acts on what the dialog asked for, once the modal is drawn.
fn finish_dialog(app: &mut PdfCraftApp, dialog: Dialog, modal_closed: bool, req: &mut Pending) {
    finish_requests(app, dialog, modal_closed, req);
    finish_actions(app, req);
    finish_redactions_and_fields(app, req);
    finish_edits(app, dialog, req);
    finish_close(app, dialog, modal_closed, req);
}

/// Runs the one-shot requests the dialog made.
fn finish_requests(app: &mut PdfCraftApp, dialog: Dialog, modal_closed: bool, req: &mut Pending) {
    if let Some(yes) = req.recover {
        let keys: Vec<String> = app.recoverable.iter().map(|m| m.key.clone()).collect();
        if yes {
            app.recover(&keys);
        } else {
            app.discard_recovered(&keys);
        }
    }
    // The pages to replace belong to the document the dialog was opened on (#167).
    if req.replace_now
        && let Some(d) = app.replace_draft.take()
        && app.still_pick_target(d.target)
    {
        let pages = if d.range_edited { (d.from - 1..d.to).collect() } else { d.pages.clone() };
        let n = pages.len();
        app.apply_edit(Edit::ReplacePages {
            pages,
            name: d.name.clone(),
            bytes: d.bytes.clone(),
            src_pages: (d.src_from - 1..d.src_from - 1 + n).collect(),
        });
    }
    // A print or save that fails keeps the dialog open, with the reason in a notice.
    if req.print_go && !app.print_now() {
        req.close = false;
    }
    if req.revert_now {
        app.revert_active();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if req.overwrite_now {
            app.overwrite_changed_file();
        }
        if req.overwrite_elsewhere {
            app.save_changed_file_elsewhere();
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (req.overwrite_now, req.overwrite_elsewhere);
    // Cancelled or dismissed: the save is dropped.
    #[cfg(not(target_arch = "wasm32"))]
    if dialog == Dialog::OverwriteChanged && (modal_closed || req.close) {
        app.pending_overwrite = None;
    }
}

/// Runs the document actions the dialog asked for.
fn finish_actions(app: &mut PdfCraftApp, req: &mut Pending) {
    if req.summarize_now {
        app.summarize_comments();
    }
    if req.optimize_now {
        app.optimize_with_draft();
    }
    if let Some(e) = req.duplicate_now.take() {
        app.duplicate_draft = None;
        app.apply_edit(e);
    }
    // Another document became active, or the document changed, while the dialog was open: the
    // pages at those indices may not be the ones chosen.
    if let Some((doc, generation, e)) = req.page_props_now.take() {
        if app.active_ids().is_some_and(|(_, id)| id == doc) && app.session.get(doc).is_some_and(|d| d.edit_generation() == generation) {
            app.apply_edit(e);
        } else {
            app.notify_tr("The document changed while the dialog was open, so nothing was changed. Open Page properties again to set the tab order.");
        }
    }
    if req.extract_now {
        app.extract_selection();
    }
    if let Some(e) = req.link_now.take() {
        app.apply_edit(e);
        app.link_draft = None;
    }
    if req.rotate_now {
        app.rotate_with_draft();
    }
    if req.import_words {
        app.pick_files(crate::files::FilePurpose::RedactWords, false);
    }
}

/// Redaction, hidden-data and field-properties requests.
fn finish_redactions_and_fields(app: &mut PdfCraftApp, req: &mut Pending) {
    match req.redact_now.take() {
        Some(Dialog::RedactPages) => app.redact_pages(),
        Some(Dialog::RedactSearch) => {
            let n = app.redact_search();
            app.redact_search.found = Some(n);
        }
        Some(Dialog::RemoveHidden) => {
            let which: Vec<pdfcraft_engine::Hidden> = app.hidden_draft.found.iter().filter(|f| f.2 && f.1 > 0).map(|f| f.0).collect();
            let n: usize = app.hidden_draft.found.iter().filter(|f| f.2).map(|f| f.1).sum();
            if app.apply_edit(Edit::RemoveHidden { which }) {
                if n == 1 {
                    app.notify_tr("Removed 1 hidden item. Save to remove them from the file.");
                } else {
                    app.notify_fmt("Removed {n} hidden items. Save to remove them from the file.", &[("n", &n.to_string())]);
                }
            }
        }
        Some(Dialog::Sanitize) if app.apply_edit(Edit::Sanitize) => {
            app.notify_tr("Document sanitized. Save to finish: saving rewrites the whole file.");
        }
        Some(Dialog::RedactApply) => {
            let marks = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(0, |d| d.redaction_marks());
            if app.apply_edit(Edit::ApplyRedactions { pages: None }) {
                if marks == 1 {
                    app.notify_tr("Applied 1 redaction mark. Save to remove the content from the file.");
                } else {
                    app.notify_fmt("Applied {n} redaction marks. Save to remove the content from the file.", &[("n", &marks.to_string())]);
                }
            }
        }
        _ => {}
    }
    if req.bulk_field_props_now {
        req.close = app.apply_bulk_field_props();
    }
    if req.field_props_now
        && let Some(d) = app.field_props.take()
        && let Some(props) = d.props()
    {
        let form = |app: &PdfCraftApp| app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|doc| doc.form.clone());
        // A rename also renames the field in the formulas that read it.
        let before = props.name.as_ref().and_then(|_| form(app));
        // The widget being edited, by object: a radio button that joins a group by its new name
        // is appended after the group's own buttons, so its index changes.
        let widget = before.as_ref().and_then(|all| all.iter().find(|f| f.name == d.field)?.widgets.get(d.widget).map(|w| w.obj));
        let applied = app.apply_edit(Edit::SetFieldProps { name: d.field.clone(), props: Box::new(props) });
        if !applied && app.active_ids().is_some() {
            // A refused edit (its reason is in the notice) leaves the dialog open with what was
            // typed into it, as the several-fields dialog does, so nothing is lost meanwhile.
            app.field_props = Some(d);
            req.close = false;
        } else if applied && let Some(i) = app.active {
            // Keep the (possibly renamed) field selected.
            // A dotted name is the full name; a plain one renames the field within its parent.
            let typed = d.name.trim();
            let prefix = d.field.rsplit_once('.').filter(|_| !typed.contains('.')).map(|(p, _)| format!("{p}.")).unwrap_or_default();
            let name = format!("{prefix}{typed}");
            let after = form(app);
            let index =
                widget.and_then(|obj| after.as_ref()?.iter().find(|f| f.name == name)?.widgets.iter().position(|w| w.obj == obj)).unwrap_or(d.widget);
            if let (Some(before), Some(after)) = (before, after) {
                let n = pdfcraft_engine::form_calculations_changed(&before, &after).iter().filter(|f| **f != name).count();
                if n > 0 {
                    app.notify(crate::i18n::trn(
                        crate::i18n::current(),
                        n as u64,
                        "{n} calculation now uses the new name.",
                        "{n} calculations now use the new name.",
                    ));
                }
            }
            app.views[i].prepare.selected = Some((name, index));
        }
    }
}

/// Edits built from the dialog's drafts.
fn finish_edits(app: &mut PdfCraftApp, dialog: Dialog, req: &mut Pending) {
    if req.props_now
        && let Some(d) = app.comment_props.take()
    {
        let mut edits = crate::comment_props::edits(&d);
        match edits.len() {
            0 => {}
            1 => {
                app.apply_edit(edits.remove(0));
            }
            _ => {
                app.apply_edit(Edit::Batch { label: "Change comment properties".into(), edits });
            }
        }
        // Make Properties Default: the look chosen here, even when it was already the comment's.
        if d.make_default && d.edited.restylable {
            app.make_props_default(d.page, d.index, &d.edited);
        }
    }
    if req.export_now
        && let Dialog::Export(kind) = dialog
    {
        app.start_export(kind);
    }
    if req.marks_now
        && let (Dialog::Marks(kind), Some((_, id))) = (dialog, app.active_ids())
    {
        let count = app.session.get(id).map(|d| d.info.pages.len()).unwrap_or(0);
        let edit = crate::marks_ui::edit(&app.marks_draft, kind, count);
        app.apply_edit(edit);
    }
    if req.boxes_now
        && let Some((i, id)) = app.active_ids()
    {
        let count = app.session.get(id).map(|d| d.info.pages.len()).unwrap_or(0);
        let edit = app.boxes_draft.edit(app.views[i].current, count);
        app.apply_edit(edit);
        app.boxes_draft.seeded = None;
    }
    if req.protect_now && app.apply_edit(app.protect_draft.edit()) {
        app.protect_draft = Default::default();
        app.notify_tr("Password protection will be applied when you save");
    }
    if req.apply_number
        && let Some(edit) = req.number_now.take()
    {
        app.apply_edit(edit);
    }
    if let Some(by) = req.split_now.take() {
        app.split_active(&by);
    }
    if req.apply
        && let Some(edits) = draft_changes(app)
    {
        app.apply_edit(Edit::Batch { label: "Change document properties".into(), edits });
    }
}

/// Closes (or switches) the dialog, then runs what closing it asked for.
fn finish_close(app: &mut PdfCraftApp, dialog: Dialog, modal_closed: bool, req: &mut Pending) {
    // Protect / Remove security replace the Properties dialog.
    let replaces = req.link_command.is_some_and(|c| c.starts_with("protect.")) || req.open_revision.is_some();
    if modal_closed || req.close || replaces {
        app.dialog = None;
        app.props_draft = None;
        app.view_draft = None;
        app.custom_draft = None;
        app.bulk_field_props = None;
        app.unmarked_page = Default::default();
    } else {
        app.dialog = Some(req.next.unwrap_or(dialog));
    }
    if let Some(cmd) = req.link_command {
        app.execute(cmd);
    }
    if let Some(n) = req.open_revision {
        app.open_revision(n);
    }
    if req.alt_now {
        app.save_alt_text();
    }
    if req.stamp_now {
        app.save_custom_stamp();
    }
    if req.images_now {
        app.finish_image_import();
    } else if dialog == Dialog::CreateImages && app.dialog != Some(Dialog::CreateImages) {
        app.image_import = None;
    }
    if req.ocr_now {
        app.start_ocr();
    }
    if req.compare_now {
        app.run_compare();
    }
    if req.a11y_now {
        if let Some((_, id)) = app.active_ids() {
            app.a11y_skipped.retain(|(doc, _)| *doc != id);
        }
        app.run_accessibility_check();
    }
}

/// Info edits needed to make the document match the Description draft.
fn draft_changes(app: &PdfCraftApp) -> Option<Vec<Edit>> {
    let (id, draft) = app.props_draft.as_ref()?;
    let doc = app.session.get(*id)?;
    let mut edits: Vec<Edit> = INFO_KEYS
        .iter()
        .zip(draft.iter())
        .filter(|(k, v)| doc.info_value(k).unwrap_or_default().trim() != v.trim())
        .map(|(k, v)| Edit::SetInfo { key: (*k).to_string(), value: v.clone() })
        .collect();
    if let Some((vid, v)) = &app.view_draft
        && vid == id
        && *v != doc.initial_view()
    {
        edits.push(Edit::SetInitialView(Box::new(v.clone())));
    }
    // Custom: deleted properties are cleared, then added and changed ones are set.
    if let Some((cid, d)) = &app.custom_draft
        && cid == id
    {
        let before = doc.custom_info();
        for (k, _) in before.iter().filter(|(k, _)| !d.props.iter().any(|(n, _)| n == k)) {
            edits.push(Edit::SetInfo { key: k.clone(), value: String::new() });
        }
        for (k, v) in d.props.iter().filter(|(k, v)| !before.iter().any(|(n, w)| n == k && w == v)) {
            edits.push(Edit::SetInfo { key: k.clone(), value: v.clone() });
        }
    }
    Some(edits)
}

fn yes(b: bool) -> String {
    tl!(if b { "Yes" } else { "No" }).to_string()
}
