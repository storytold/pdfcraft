//! Modal dialogs: Document Properties, Keyboard Shortcuts, About.

use crate::SplitMode as M;
use std::fmt::Write as _;

use egui::{Align, Layout};

use crate::theme::{self, Tokens};
use printcraft_engine::{DocId, Document, Edit};

use crate::{CloseRequest, Dialog, PrintCraftApp, PropsTab, panels::human_size, widgets};

const INFO_KEYS: [&str; 4] = ["Title", "Author", "Subject", "Keywords"];

/// `PrintCraftApp::props_draft`, spelled out for the Properties tabs.
type PropsDraft = Option<(DocId, [String; 4])>;
/// `PrintCraftApp::view_draft`, spelled out for the Properties tabs.
type ViewDraft = Option<(DocId, printcraft_engine::InitialView)>;

/// One "label: value" row of the Properties grid.
fn prop_row(ui: &mut egui::Ui, t: &Tokens, k: &str, v: String) {
    ui.label(egui::RichText::new(k).color(t.text_muted));
    ui.label(if v.is_empty() { egui::RichText::new("—").color(t.text_faint) } else { egui::RichText::new(v) });
    ui.end_row();
}

/// What a dialog body asks [`show`] to do once the modal is closed: every "OK" a dialog draws
/// records what it meant here, and [`run_edits`] carries the answers out in the order the dialogs
/// used to.
#[derive(Default)]
#[expect(clippy::struct_excessive_bools, reason = "one flag per button in the dialog set; `run_edits` reads each of them once")]
struct Pending {
    close: bool,
    recover: Option<bool>,
    apply: bool,
    split_now: Option<crate::SplitPlan>,
    split_ready: Option<crate::SplitPlan>,
    apply_number: bool,
    number_now: Option<Edit>,
    replace_now: bool,
    print_go: bool,
    revert_now: bool,
    summarize_now: bool,
    optimize_now: bool,
    duplicate_now: Option<Edit>,
    extract_now: bool,
    link_now: Option<Edit>,
    rotate_now: bool,
    redact_now: Option<Dialog>,
    field_props_now: bool,
    props_now: bool,
    export_now: bool,
    marks_now: bool,
    boxes_now: bool,
    protect_now: bool,
    link_command: Option<&'static str>,
    open_revision: Option<usize>,
    alt_now: bool,
    stamp_now: bool,
    combine_now: bool,
    ocr_now: bool,
    compare_now: bool,
    a11y_now: bool,
}

pub fn show(app: &mut PrintCraftApp, ctx: &egui::Context) {
    password(app, ctx);
    save_prompt(app, ctx);
    crate::updates::dialog(app, ctx);
    let Some(dialog) = app.dialog else {
        app.props_draft = None;
        app.view_draft = None;
        return;
    };
    // Seed the editable Description fields from the document when the dialog opens.
    if let (Dialog::Properties(_), Some((_, id))) = (dialog, app.active_ids())
        && app.props_draft.as_ref().is_none_or(|(d, _)| *d != id)
        && let Some(doc) = app.session.get(id)
    {
        app.props_draft = Some((id, INFO_KEYS.map(|k| doc.info_value(k).unwrap_or_default())));
        app.view_draft = Some((id, doc.initial_view()));
    }
    let mut pending = Pending::default();
    let t = Tokens::get(ctx);
    let mut next = dialog;
    let modal = egui::Modal::new(egui::Id::new("dialog")).show(ctx, |ui| {
        ui.set_width(match dialog {
            Dialog::Properties(_) => 640.0,
            Dialog::Print => 820.0,
            Dialog::FieldProps => 600.0,
            _ => 520.0,
        });
        // Dialog controls are outlined (radio buttons, check boxes, combo boxes and number fields
        // would otherwise blend into the dialog, whose fill matches the theme's field colour).
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.bg_stroke = egui::Stroke::new(1.0, t.border);
        widgets.inactive.weak_bg_fill = t.field;
        // Slider rails and check-box interiors use the plain fill.
        widgets.inactive.bg_fill = t.hover;
        widgets.hovered.bg_stroke = egui::Stroke::new(1.0, t.text_muted);
        if dialog_body(ui, app, &t, dialog, &mut pending, &mut next) {
            return;
        }
        dialog_footer(ui, dialog, app, &mut pending);
    });
    run_edits(app, dialog, &mut pending);
    close_dialog(app, &mut pending, modal.should_close(), next);
}

/// The dialog's own body. `true` when it drew its own buttons, so the common footer below
/// must not be drawn.
fn dialog_body(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, dialog: Dialog, pending: &mut Pending, next: &mut Dialog) -> bool {
    match dialog {
        Dialog::Properties(tab) => show_properties(ui, app, t, tab, pending, next),
        Dialog::Split => show_split(ui, app, t, pending),
        Dialog::ReplacePages => show_replace_pages(ui, app, t, pending),
        Dialog::RedactPages => show_redact_pages(ui, app, t, dialog, pending),
        Dialog::RedactSearch => show_redact_search(ui, app, t, dialog, pending),
        Dialog::RedactProps => show_redact_props(ui, app, t, pending),
        Dialog::RedactApply => show_redact_apply(ui, app, t, dialog, pending),
        Dialog::LinkProps => show_link_props(ui, app, t, pending),
        Dialog::Extract => show_extract(ui, app, pending),
        Dialog::RotatePages => show_rotate_pages(ui, app, pending),
        Dialog::DuplicateField => show_duplicate_field(ui, app, pending),
        Dialog::Optimize => show_optimize(ui, app, t, pending, next),
        Dialog::CertificateViewer => show_certificate_viewer(ui, app, t, pending),
        Dialog::AuditSpace => show_audit_space(ui, app, t, next),
        Dialog::Sign => show_sign(ui, app, t, pending),
        Dialog::SummarizeComments => show_summarize_comments(ui, app, pending),
        Dialog::Revert => show_revert(ui, app, pending),
        Dialog::Print => show_print(ui, app, t, pending),
        Dialog::RemoveHidden => show_remove_hidden(ui, app, t, dialog, pending),
        Dialog::Sanitize => show_sanitize(ui, t, dialog, pending),
        Dialog::FieldProps => show_field_props(ui, app, t, pending),
        Dialog::CommentProps => show_comment_props(ui, app, t, pending),
        Dialog::Signature => show_signature(ui, app, t, pending),
        Dialog::AltText => show_alt_text(ui, app, t, pending),
        Dialog::CreateStamp => show_create_stamp(ui, app, t, pending),
        Dialog::Combine => show_combine(ui, app, t, pending),
        Dialog::PdfA => show_pdfa(ui, app, t, pending),
        Dialog::ActionWizard => show_action_wizard(ui, app, t, pending),
        Dialog::CompareFiles => show_compare_files(ui, app, t, pending),
        Dialog::JsConsole => show_js_console(ui, app, t, pending),
        Dialog::DocumentJs => show_document_js(ui, app, t, pending),
        Dialog::Preferences => show_preferences(ui, app, t, pending),
        Dialog::RecognizeText => show_recognize_text(ui, app, t, pending),
        Dialog::AccessibilityOptions => show_accessibility_options(ui, app, t, pending),
        Dialog::Export(kind) => show_export(ui, app, t, kind, pending),
        Dialog::Marks(kind) => show_marks(ui, app, t, kind, pending),
        Dialog::PageBoxes => show_page_boxes(ui, app, t, pending),
        Dialog::Protect => show_protect(ui, app, t, pending),
        Dialog::NumberPages => show_number_pages(ui, app, t, pending),
        Dialog::Recovery => show_recovery(ui, app, t),
        Dialog::Shortcuts => show_shortcuts(ui),
        Dialog::About => show_about(ui, t, pending),
    }
}

/// The buttons a dialog shares, for those that did not draw their own.
fn dialog_footer(ui: &mut egui::Ui, dialog: Dialog, app: &PrintCraftApp, pending: &mut Pending) {
    ui.add_space(12.0);
    let changed = draft_changes(app).is_some_and(|c| !c.is_empty());
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if dialog == Dialog::Recovery {
            if widgets::pill_button(ui, "Recover", true).clicked() {
                pending.recover = Some(true);
                pending.close = true;
            }
            if widgets::pill_button(ui, "Discard", false).clicked() {
                pending.recover = Some(false);
                pending.close = true;
            }
        } else if dialog == Dialog::NumberPages {
            if widgets::pill_button(ui, "OK", true).clicked() {
                pending.apply_number = true;
                pending.close = true;
            }
            if widgets::pill_button(ui, "Cancel", false).clicked() {
                pending.close = true;
            }
        } else if dialog == Dialog::Split {
            if ui.add_enabled_ui(pending.split_ready.is_some(), |ui| widgets::pill_button(ui, "Split", true)).inner.clicked() {
                pending.split_now.clone_from(&pending.split_ready);
                pending.close = true;
            }
            if widgets::pill_button(ui, "Cancel", false).clicked() {
                pending.close = true;
            }
        } else if changed {
            if widgets::pill_button(ui, "OK", true).clicked() {
                pending.apply = true;
                pending.close = true;
            }
            if widgets::pill_button(ui, "Cancel", false).clicked() {
                pending.close = true;
            }
        } else if widgets::pill_button(ui, "Close", true).clicked() {
            pending.close = true;
        }
    });
}

/// Document Properties: the tab bar and the grid; each tab draws its own rows.
fn show_properties(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, tab: PropsTab, pending: &mut Pending, next: &mut Dialog) -> bool {
    ui.label(egui::RichText::new("Document Properties").font(theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        for (tb, label) in [
            (PropsTab::Description, "Description"),
            (PropsTab::InitialView, "Initial View"),
            (PropsTab::Security, "Security"),
            (PropsTab::Fonts, "Fonts"),
            (PropsTab::Advanced, "Advanced"),
        ] {
            if widgets::mode_tab(ui, label, tab == tb).clicked() {
                *next = Dialog::Properties(tb);
            }
        }
    });
    ui.separator();
    let Some((_, id)) = app.active_ids() else { return true };
    let Some(doc) = app.session.get(id) else { return true };
    egui::ScrollArea::vertical().max_height(460.0).auto_shrink([false, true]).show(ui, |ui| {
        egui::Grid::new("props").num_columns(2).spacing([18.0, 8.0]).min_col_width(140.0).show(ui, |ui| match tab {
            PropsTab::Description => props_description(ui, doc, &mut app.props_draft, t),
            PropsTab::InitialView => props_initial_view(ui, doc, &mut app.view_draft),
            PropsTab::Security => props_security(ui, doc, t, pending),
            PropsTab::Fonts => props_fonts(ui, doc, t),
            PropsTab::Advanced => props_advanced(ui, doc, &mut app.view_draft, t, pending),
        })
    });
    false
}

/// Description: the info fields, editable when the document allows it.
fn props_description(ui: &mut egui::Ui, doc: &Document, props_draft: &mut PropsDraft, t: &Tokens) {
    let i = &doc.info;
    prop_row(ui, t, "File", doc.name.clone());
    match props_draft.as_mut() {
        Some((_, draft)) if doc.allows_modification() => {
            for (k, v) in INFO_KEYS.iter().zip(draft.iter_mut()) {
                let l = ui.label(egui::RichText::new(*k).color(t.text_muted));
                ui.add(
                    egui::TextEdit::singleline(v)
                        .desired_width(420.0)
                        .background_color(t.field)
                        .margin(egui::Margin::symmetric(6, 4))
                        .id_salt(("info", *k)),
                )
                .labelled_by(l.id);
                ui.end_row();
            }
        }
        _ => {
            prop_row(ui, t, "Title", i.title.clone().unwrap_or_default());
            prop_row(ui, t, "Author", i.author.clone().unwrap_or_default());
            prop_row(ui, t, "Subject", i.subject.clone().unwrap_or_default());
            prop_row(ui, t, "Keywords", i.keywords.clone().unwrap_or_default());
        }
    }
    prop_row(ui, t, "Application", i.creator.clone().unwrap_or_default());
    prop_row(ui, t, "PDF producer", i.producer.clone().unwrap_or_default());
}

/// Initial View: layout, magnification, window and interface options.
fn props_initial_view(ui: &mut egui::Ui, doc: &Document, view_draft: &mut ViewDraft) {
    use printcraft_engine::{InitialLayout as L, Magnification as M, Navigation as N};
    let i = &doc.info;
    let editable = doc.allows_modification();
    let pages = i.pages.len();
    let Some((_, view)) = view_draft.as_mut() else { return };
    ui.label(egui::RichText::new("Layout and Magnification").font(theme::semibold(12.5)));
    ui.end_row();
    ui.label("Navigation tab");
    ui.add_enabled_ui(editable, |ui| {
        egui::ComboBox::from_id_salt("iv-nav")
            .selected_text(format!("{:?}", view.navigation).replace("PageOnly", "Page Only").replace("Pages", "Pages Panel and Page"))
            .show_ui(ui, |ui| {
                for (n, l) in [
                    (N::PageOnly, "Page Only"),
                    (N::Bookmarks, "Bookmarks Panel and Page"),
                    (N::Pages, "Pages Panel and Page"),
                    (N::Attachments, "Attachments Panel and Page"),
                    (N::Layers, "Layers Panel and Page"),
                ] {
                    ui.selectable_value(&mut view.navigation, n, l);
                }
            });
    });
    ui.end_row();
    ui.label("Page layout");
    ui.add_enabled_ui(editable, |ui| {
        let names = [
            (L::Default, "Default"),
            (L::SinglePage, "Single Page"),
            (L::SinglePageContinuous, "Single Page Continuous"),
            (L::TwoUp, "Two-Up (Facing)"),
            (L::TwoUpContinuous, "Two-Up Continuous (Facing)"),
            (L::TwoUpCoverPage, "Two-Up (Cover Page)"),
            (L::TwoUpContinuousCoverPage, "Two-Up Continuous (Cover Page)"),
        ];
        let shown = names.iter().find(|(l, _)| *l == view.layout).map_or("Default", |(_, n)| n);
        egui::ComboBox::from_id_salt("iv-layout").selected_text(shown).show_ui(ui, |ui| {
            for (l, n) in names {
                ui.selectable_value(&mut view.layout, l, n);
            }
        });
    });
    ui.end_row();
    ui.label("Magnification");
    ui.add_enabled_ui(editable, |ui| {
        ui.horizontal(|ui| {
            let names = [
                (M::Default, "Default"),
                (M::ActualSize, "Actual Size"),
                (M::FitPage, "Fit Page"),
                (M::FitWidth, "Fit Width"),
                (M::FitHeight, "Fit Height"),
                (M::FitVisible, "Fit Visible"),
            ];
            let shown = match view.magnification {
                M::Percent(pct) => format!("{pct:.0}%"),
                other => names.iter().find(|(x, _)| *x == other).map_or("Default", |(_, n)| n).to_string(),
            };
            egui::ComboBox::from_id_salt("iv-mag").selected_text(shown).show_ui(ui, |ui| {
                for (mode, label) in names {
                    ui.selectable_value(&mut view.magnification, mode, label);
                }
                for pct in [50.0, 75.0, 125.0, 150.0, 200.0] {
                    ui.selectable_value(&mut view.magnification, M::Percent(pct), format!("{pct:.0}%"));
                }
            });
        });
    });
    ui.end_row();
    ui.label("Open to page");
    ui.add_enabled_ui(editable, |ui| {
        let mut page = view.page + 1;
        if ui.add(egui::DragValue::new(&mut page).range(1..=pages.max(1))).changed() {
            view.page = page - 1;
        }
        ui.label(format!("of {pages}"));
    });
    ui.end_row();
    ui.label(egui::RichText::new("Window Options").font(theme::semibold(12.5)));
    ui.end_row();
    ui.label("");
    ui.add_enabled_ui(editable, |ui| {
        ui.vertical(|ui| {
            ui.checkbox(&mut view.fit_window, "Resize window to initial page");
            ui.checkbox(&mut view.center_window, "Center window on screen");
            ui.checkbox(&mut view.full_screen, "Open in Full Screen mode");
            ui.horizontal(|ui| {
                ui.label("Show:");
                ui.radio_value(&mut view.display_title, false, "File Name");
                ui.radio_value(&mut view.display_title, true, "Document Title");
            });
        });
    });
    ui.end_row();
    ui.label(egui::RichText::new("User Interface Options").font(theme::semibold(12.5)));
    ui.end_row();
    ui.label("");
    ui.add_enabled_ui(editable, |ui| {
        ui.vertical(|ui| {
            ui.checkbox(&mut view.hide_menubar, "Hide menu bar");
            ui.checkbox(&mut view.hide_toolbar, "Hide toolbars");
            ui.checkbox(&mut view.hide_window_ui, "Hide window controls");
        });
    });
    ui.end_row();
}

/// Security: the encryption in force and what it allows.
fn props_security(ui: &mut egui::Ui, doc: &Document, t: &Tokens, pending: &mut Pending) {
    match doc.security_summary() {
        None => {
            prop_row(ui, t, "Security method", "No security".into());
            prop_row(ui, t, "Restrictions", "None — everything is allowed".into());
            if doc.allows_security_change() && ui.button("Protect using password…").clicked() {
                pending.link_command = Some("protect.password");
            }
        }
        Some(sec) => {
            prop_row(ui, t, "Security method", "Password security".into());
            prop_row(ui, t, "Encryption", sec.method.clone());
            prop_row(
                ui,
                t,
                "Opened with",
                if sec.pending {
                    "— (protection is applied when you save)".into()
                } else if sec.owner {
                    "Owner password (no restrictions apply)".into()
                } else {
                    "User password".into()
                },
            );
            if doc.allows_security_change() {
                ui.horizontal(|ui| {
                    if ui.button("Change settings…").clicked() {
                        pending.link_command = Some("protect.password");
                    }
                    if ui.button("Remove security").clicked() {
                        pending.link_command = Some("protect.remove");
                    }
                });
            }
            let p = sec.permissions;
            let yes = |b: bool| if b { "Allowed".to_string() } else { "Not allowed".to_string() };
            prop_row(
                ui,
                t,
                "Printing",
                if !p.print() {
                    "Not allowed".into()
                } else if p.print_high_quality() {
                    "High resolution".into()
                } else {
                    "Low resolution".into()
                },
            );
            prop_row(ui, t, "Changing the document", yes(p.modify()));
            prop_row(ui, t, "Document assembly", yes(p.assemble()));
            prop_row(ui, t, "Content copying", yes(p.copy()));
            prop_row(ui, t, "Content copying for accessibility", yes(p.extract_for_accessibility()));
            prop_row(ui, t, "Commenting", yes(p.annotate()));
            prop_row(ui, t, "Filling of form fields", yes(p.fill_forms()));
        }
    }
}

/// Fonts: what the pages reference.
fn props_fonts(ui: &mut egui::Ui, doc: &Document, t: &Tokens) {
    let i = &doc.info;
    if i.fonts.is_empty() {
        prop_row(ui, t, "Fonts", "No fonts are referenced by the pages.".into());
    }
    for f in &i.fonts {
        let mut detail = f.kind.clone();
        if let Some(e) = &f.encoding {
            let _ = write!(detail, " · {e}");
        }
        detail.push_str(if f.subset {
            " · Embedded subset"
        } else if f.embedded {
            " · Embedded"
        } else {
            " · Not embedded (substituted)"
        });
        prop_row(ui, t, &f.name, detail);
    }
}

/// Advanced: PDF version, file facts, revisions and reading options.
fn props_advanced(ui: &mut egui::Ui, doc: &Document, view_draft: &mut ViewDraft, t: &Tokens, pending: &mut Pending) {
    let i = &doc.info;
    prop_row(ui, t, "PDF version", i.pdf_version.clone());
    prop_row(ui, t, "Location", doc.path.clone().unwrap_or_default());
    prop_row(ui, t, "File size", format!("{} ({} bytes)", human_size(i.file_size), i.file_size));
    let page_info = &i.pages[0];
    prop_row(ui, t, "Page size", format!("{:.2} × {:.2} in", page_info.width / 72.0, page_info.height / 72.0));
    prop_row(ui, t, "Number of pages", i.pages.len().to_string());
    prop_row(ui, t, "Tagged PDF", yes(i.tagged));
    prop_row(ui, t, "Form fields", i.fields.len().to_string());
    prop_row(ui, t, "Comments", i.annotations.len().to_string());
    prop_row(ui, t, "Layers", i.layers.len().to_string());
    prop_row(ui, t, "Attachments", i.attachments.len().to_string());
    prop_row(ui, t, "JavaScript", yes(i.has_javascript));
    // Each incremental update is a revision; earlier ones open as their own document.
    let ends = doc.revision_ends();
    if ends.len() > 1 {
        ui.label(egui::RichText::new("Revisions").color(t.text_muted));
        ui.horizontal_wrapped(|ui| {
            ui.label(ends.len().to_string());
            for n in (1..ends.len()).rev().take(12) {
                if ui.small_button(format!("View revision {n}")).on_hover_text("Open the file as it was saved then").clicked() {
                    pending.open_revision = Some(n);
                }
            }
        });
        ui.end_row();
    }
    // Reading Options: binding and language.
    if let Some((_, view)) = view_draft.as_mut() {
        let editable = doc.allows_modification();
        ui.label(egui::RichText::new("Binding").color(t.text_muted));
        ui.add_enabled_ui(editable, |ui| {
            ui.horizontal(|ui| {
                ui.radio_value(&mut view.right_to_left, false, "Left Edge");
                ui.radio_value(&mut view.right_to_left, true, "Right Edge");
            });
        });
        ui.end_row();
        let lang_label = ui.label(egui::RichText::new("Language").color(t.text_muted));
        let mut lang = view.language.clone().unwrap_or_default();
        if ui
            .add_enabled(editable, egui::TextEdit::singleline(&mut lang).hint_text("e.g. en-US").desired_width(160.0))
            .labelled_by(lang_label.id)
            .changed()
        {
            view.language = (!lang.trim().is_empty()).then(|| lang.trim().to_string());
        }
        ui.end_row();
    }
    // What was repaired while reading a damaged file (fidelity: never silent).
    let repairs = doc.repair_log();
    prop_row(ui, t, "Repairs", if repairs.is_empty() { "None".to_string() } else { repairs.len().to_string() });
    if !repairs.is_empty() {
        ui.label("");
        egui::CollapsingHeader::new("Repair log").show(ui, |ui| {
            for r in &repairs {
                ui.add(egui::Label::new(egui::RichText::new(r).small()).wrap());
            }
        });
        ui.end_row();
    }
}

/// Split document: how to cut it up, and what that makes.
fn show_split(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    ui.label(egui::RichText::new("Split document").font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let Some((vi, id)) = app.active_ids() else { return true };
    let n = app.session.get(id).map_or(0, |d| d.info.pages.len());
    let selected: Vec<usize> = app.views[vi].selected.iter().copied().filter(|p| *p > 0).collect();
    let marks = app.session.bookmark_splits(id);
    let draft = &mut app.split_draft;
    if selected.is_empty() && draft.mode == M::Selection {
        draft.mode = M::Pages;
    }
    ui.radio_value(&mut draft.mode, M::Pages, "Number of pages");
    ui.add_enabled_ui(draft.mode == M::Pages, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.label("Pages per file");
            ui.add(egui::DragValue::new(&mut draft.every).range(1..=n.max(1)));
        });
    });
    ui.radio_value(&mut draft.mode, M::Size, "File size");
    ui.add_enabled_ui(draft.mode == M::Size, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.label("At most");
            ui.add(egui::DragValue::new(&mut draft.size_mb).range(0.05..=2000.0).speed(0.1).suffix(" MB"));
        });
    });
    ui.add_enabled_ui(!marks.is_empty(), |ui| ui.radio_value(&mut draft.mode, M::Bookmarks, format!("Top-level bookmarks ({})", marks.len())));
    ui.add_enabled_ui(!selected.is_empty(), |ui| {
        ui.radio_value(&mut draft.mode, M::Selection, "Before each selected page (select pages in Organize)")
    });
    let plan = match draft.mode {
        M::Pages => crate::SplitPlan::By(printcraft_engine::SplitBy::PageCount(draft.every)),
        M::Selection => crate::SplitPlan::By(printcraft_engine::SplitBy::Before(selected)),
        M::Size => crate::SplitPlan::Size((draft.size_mb * 1_048_576.0) as usize),
        M::Bookmarks => crate::SplitPlan::Bookmarks,
    };
    let files = match &plan {
        crate::SplitPlan::By(by) => Some(printcraft_engine::split_ranges(n, by).len()),
        crate::SplitPlan::Bookmarks => {
            Some(printcraft_engine::split_ranges(n, &printcraft_engine::SplitBy::Before(marks.iter().map(|m| m.0).collect())).len())
        }
        crate::SplitPlan::Size(_) => None,
    };
    ui.add_space(8.0);
    let text = match files {
        Some(f) => format!("Creates {f} file{} from {n} pages.", if f == 1 { "" } else { "s" }),
        None => format!("Each file holds as many of the {n} pages as fit."),
    };
    ui.label(egui::RichText::new(text).color(t.text_muted));
    if files.is_none_or(|f| f > 1) {
        pending.split_ready = Some(plan);
    }
    false
}

/// Replace Pages: the range of the replacement file.
fn show_replace_pages(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    let count = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len()).max(1);
    let Some(d) = app.replace_draft.as_mut() else {
        pending.close = true;
        return true;
    };
    ui.label(egui::RichText::new("Replace Pages").font(theme::semibold(18.0)));
    ui.add_space(8.0);
    d.to = d.to.clamp(1, count);
    d.from = d.from.clamp(1, d.to);
    let n = d.to - d.from + 1;
    ui.horizontal(|ui| {
        ui.label("Original: replace pages");
        ui.add(egui::DragValue::new(&mut d.from).range(1..=count));
        ui.label("to");
        ui.add(egui::DragValue::new(&mut d.to).range(1..=count));
        ui.label(egui::RichText::new(format!("of {count}")).color(t.text_muted));
    });
    let max_start = d.src_pages.saturating_sub(n) + 1;
    d.src_from = d.src_from.clamp(1, max_start.max(1));
    ui.horizontal(|ui| {
        ui.label(format!("Replacement: pages of {}", d.name));
        ui.add(egui::DragValue::new(&mut d.src_from).range(1..=max_start.max(1)));
        ui.label(format!("to {}", d.src_from + n - 1));
        ui.label(egui::RichText::new(format!("of {}", d.src_pages)).color(t.text_muted));
    });
    let fits = n <= d.src_pages;
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(if fits {
            "Only the page content changes: links, comments, form fields and bookmarks on the original pages stay."
        } else {
            "The replacement file doesn't have that many pages."
        })
        .small()
        .color(t.text_faint),
    );
    ui.add_space(10.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.add_enabled_ui(fits, |ui| widgets::pill_button(ui, "OK", true)).inner.clicked() {
            pending.replace_now = true;
            pending.close = true;
        }
        if widgets::pill_button(ui, "Cancel", false).clicked() {
            pending.close = true;
        }
    });
    true
}

/// Redact a PDF: Redact pages.
fn show_redact_pages(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, dialog: Dialog, pending: &mut Pending) -> bool {
    let n = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len());
    let (ok, cancel) = crate::redact_ui::pages_body(ui, &mut app.redact_pages_draft, n, t);
    if ok {
        pending.redact_now = Some(dialog);
    }
    pending.close = ok || cancel;
    true
}

/// Redact a PDF: find text and redact.
fn show_redact_search(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, dialog: Dialog, pending: &mut Pending) -> bool {
    let (go, cancel) = crate::redact_ui::search_body(ui, &mut app.redact_search, t);
    if go {
        pending.redact_now = Some(dialog);
    }
    pending.close = cancel;
    true
}

/// Redact a PDF: set properties.
fn show_redact_props(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    let mut prefs = app.redact_prefs.clone();
    let (ok, cancel) = crate::redact_ui::props_body(ui, &mut prefs, t);
    app.redact_prefs = prefs;
    pending.close = ok || cancel;
    true
}

/// Redact a PDF: the confirmation before applying.
fn show_redact_apply(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, dialog: Dialog, pending: &mut Pending) -> bool {
    let marks = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(0, printcraft_engine::Document::redaction_marks);
    let (ok, cancel) = crate::redact_ui::apply_body(ui, marks, t);
    if ok {
        pending.redact_now = Some(dialog);
    }
    pending.close = ok || cancel;
    true
}

/// Link Properties.
fn show_link_props(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    let pages = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len());
    let Some(d) = app.link_draft.as_mut() else {
        pending.close = true;
        return true;
    };
    let (ok, cancel) = crate::link_ui::body(ui, d, pages, t);
    if ok {
        pending.link_now = Some(crate::link_ui::edit_for(d));
    }
    pending.close = ok || cancel;
    true
}

/// Organize: extract the selected pages.
fn show_extract(ui: &mut egui::Ui, app: &mut PrintCraftApp, pending: &mut Pending) -> bool {
    let count = app.active_ids().map_or(0, |(i, _)| app.views[i].target_pages().len());
    ui.label(egui::RichText::new("Extract pages").font(theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.label(format!("{count} page{} selected.", if count == 1 { "" } else { "s" }));
    ui.checkbox(&mut app.extract_draft.delete, "Delete pages after extracting");
    ui.checkbox(&mut app.extract_draft.separate, "Extract pages as separate files");
    ui.add_space(12.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, "Extract", true).clicked() {
            pending.extract_now = true;
            pending.close = true;
        }
        if widgets::pill_button(ui, "Cancel", false).clicked() {
            pending.close = true;
        }
    });
    true
}

/// Organize: rotate pages.
fn show_rotate_pages(ui: &mut egui::Ui, app: &mut PrintCraftApp, pending: &mut Pending) -> bool {
    use printcraft_engine::{PageOrientation as O, PageParity as P};
    let n = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len());
    let d = &mut app.rotate_draft;
    ui.label(egui::RichText::new("Rotate Pages").font(theme::semibold(18.0)));
    ui.add_space(8.0);
    egui::Grid::new("rotate-pages").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label("Direction:");
        egui::ComboBox::from_id_salt("rotate-dir")
            .selected_text(match d.degrees {
                270 => "Counterclockwise 90 degrees",
                180 => "180 degrees",
                _ => "Clockwise 90 degrees",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut d.degrees, 90, "Clockwise 90 degrees");
                ui.selectable_value(&mut d.degrees, 270, "Counterclockwise 90 degrees");
                ui.selectable_value(&mut d.degrees, 180, "180 degrees");
            });
        ui.end_row();
        ui.label("Pages:");
        ui.vertical(|ui| {
            ui.radio_value(&mut d.which, 0, "All");
            ui.radio_value(&mut d.which, 1, "Selection");
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.which, 2, "From");
                ui.add_enabled(d.which == 2, egui::DragValue::new(&mut d.from).range(1..=n));
                ui.label("to");
                ui.add_enabled(d.which == 2, egui::DragValue::new(&mut d.to).range(1..=n));
                ui.label(format!("of {n}"));
            });
        });
        ui.end_row();
        ui.label("Rotate:");
        egui::ComboBox::from_id_salt("rotate-parity")
            .selected_text(match d.parity {
                P::Both => "Even and Odd Pages",
                P::Even => "Even Pages Only",
                P::Odd => "Odd Pages Only",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut d.parity, P::Both, "Even and Odd Pages");
                ui.selectable_value(&mut d.parity, P::Even, "Even Pages Only");
                ui.selectable_value(&mut d.parity, P::Odd, "Odd Pages Only");
            });
        ui.end_row();
        ui.label("");
        egui::ComboBox::from_id_salt("rotate-orient")
            .selected_text(match d.orientation {
                O::Both => "Landscape and Portrait Pages",
                O::Landscape => "Landscape Pages",
                O::Portrait => "Portrait Pages",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut d.orientation, O::Both, "Landscape and Portrait Pages");
                ui.selectable_value(&mut d.orientation, O::Landscape, "Landscape Pages");
                ui.selectable_value(&mut d.orientation, O::Portrait, "Portrait Pages");
            });
        ui.end_row();
    });
    d.to = d.to.max(d.from);
    ui.add_space(12.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, "OK", true).clicked() {
            pending.rotate_now = true;
            pending.close = true;
        }
        if widgets::pill_button(ui, "Cancel", false).clicked() {
            pending.close = true;
        }
    });
    true
}

/// Prepare a form: duplicate a field onto other pages.
fn show_duplicate_field(ui: &mut egui::Ui, app: &mut PrintCraftApp, pending: &mut Pending) -> bool {
    let n = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len());
    let Some(d) = app.duplicate_draft.as_mut() else {
        pending.close = true;
        return true;
    };
    ui.label(egui::RichText::new("Duplicate Field").font(theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.label(format!("Duplicate \"{}\" onto:", d.name));
    ui.radio_value(&mut d.all, true, "All pages");
    ui.horizontal(|ui| {
        ui.radio_value(&mut d.all, false, "From");
        ui.add_enabled(!d.all, egui::DragValue::new(&mut d.from).range(1..=n));
        ui.label("to");
        ui.add_enabled(!d.all, egui::DragValue::new(&mut d.to).range(1..=n));
        ui.label(format!("of {n}"));
    });
    d.to = d.to.max(d.from);
    ui.add_space(12.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, "OK", true).clicked() {
            let pages: Vec<usize> = if d.all { (0..n).collect() } else { (d.from - 1..d.to.min(n)).collect() };
            pending.duplicate_now = Some(Edit::DuplicateField { name: d.name.clone(), pages });
            pending.close = true;
        }
        if widgets::pill_button(ui, "Cancel", false).clicked() {
            pending.close = true;
        }
    });
    true
}

/// PDF Optimizer: Advanced optimization.
fn show_optimize(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending, next: &mut Dialog) -> bool {
    let (ok, cancel) = crate::optimize_ui::body(ui, &mut app.optimize_draft, t);
    pending.optimize_now = ok;
    pending.close = ok || cancel;
    if std::mem::take(&mut app.optimize_draft.audit) {
        app.space_audit = app.active_ids().and_then(|(_, id)| app.session.get(id)).map(printcraft_engine::Document::audit_space).unwrap_or_default();
        *next = Dialog::AuditSpace;
    }
    true
}

/// Signatures: show a certificate.
fn show_certificate_viewer(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    ui.set_width(700.0);
    let trusted = app.session.trusted_certificates().to_vec();
    let Some(v) = app.cert_viewer.as_mut() else {
        pending.close = true;
        return true;
    };
    let (done, action) = crate::sign_ui::cert_viewer(ui, v, &trusted, t);
    pending.close = done;
    match action {
        Some(crate::sign_ui::CertAction::Trust(c)) => app.trust_certificate(*c),
        Some(crate::sign_ui::CertAction::Export(c)) => {
            let pem = crate::sign_ui::certificate_pem(&c);
            app.write_files(&[(format!("{}.cer", c.display_name()), std::sync::Arc::new(pem.into_bytes()))], "Export certificate");
        }
        None => {}
    }
    true
}

/// PDF Optimizer: audit space usage.
fn show_audit_space(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, next: &mut Dialog) -> bool {
    if crate::optimize_ui::audit_body(ui, &app.space_audit, t) {
        *next = Dialog::Optimize;
    }
    true
}

/// Sign with a Digital ID: Configure / Sign as.
fn show_sign(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    if crate::sign_ui::dialog(ui, app, t) {
        app.sign_draft = None;
        pending.close = true;
    }
    true
}

/// Comments: Summarize comments (options).
fn show_summarize_comments(ui: &mut egui::Ui, app: &mut PrintCraftApp, pending: &mut Pending) -> bool {
    ui.label(egui::RichText::new("Summarize Options").font(theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.label("Choose a layout:");
    let _ = ui.radio(true, "Comments only");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("Sort comments by:");
        egui::ComboBox::from_id_salt("summary-sort").selected_text(app.summary_sort.name()).show_ui(ui, |ui| {
            for s in printcraft_engine::SummarySort::ALL {
                ui.selectable_value(&mut app.summary_sort, s, s.name());
            }
        });
    });
    ui.add_space(12.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, "Create PDF Comment Summary", true).clicked() {
            pending.summarize_now = true;
            pending.close = true;
        }
        if widgets::pill_button(ui, "Cancel", false).clicked() {
            pending.close = true;
        }
    });
    true
}

/// File: the Revert confirmation.
fn show_revert(ui: &mut egui::Ui, app: &mut PrintCraftApp, pending: &mut Pending) -> bool {
    let name = app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|d| d.name.clone()).unwrap_or_default();
    ui.label(egui::RichText::new("Revert").font(theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.label(format!("Revert to the previously saved version of \"{name}\"? Changes since then can't be undone afterwards."));
    ui.add_space(12.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, "Revert", true).clicked() {
            pending.revert_now = true;
            pending.close = true;
        }
        if widgets::pill_button(ui, "Cancel", false).clicked() {
            pending.close = true;
        }
    });
    true
}

/// File: Print.
fn show_print(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    let Some((i, id)) = app.active_ids() else {
        pending.close = true;
        return true;
    };
    let Some(doc) = app.session.get(id) else { return true };
    let sizes: Vec<(f64, f64)> = doc.info.pages.iter().map(|p| (f64::from(p.width), f64::from(p.height))).collect();
    let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
    let thumbs: std::collections::HashMap<usize, egui::TextureId> =
        (0..sizes.len()).filter_map(|p| app.views[i].thumb_id(p).map(|t| (p, t))).collect();
    let (go, cancel) = crate::print_ui::body(ui, &mut app.print_draft, t, &sizes, &labels, &|p| thumbs.get(&p).copied());
    pending.print_go = go;
    pending.close = go || cancel;
    true
}

/// Remove Hidden Information.
fn show_remove_hidden(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, dialog: Dialog, pending: &mut Pending) -> bool {
    let (ok, cancel) = crate::redact_ui::hidden_body(ui, &mut app.hidden_draft, t);
    if ok {
        pending.redact_now = Some(dialog);
    }
    pending.close = ok || cancel;
    true
}

/// Sanitize Document.
fn show_sanitize(ui: &mut egui::Ui, t: &Tokens, dialog: Dialog, pending: &mut Pending) -> bool {
    let (ok, cancel) = crate::redact_ui::sanitize_body(ui, t);
    if ok {
        pending.redact_now = Some(dialog);
    }
    pending.close = ok || cancel;
    true
}

/// Prepare a form: Field Properties.
fn show_field_props(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    let Some(d) = app.field_props.as_mut() else {
        pending.close = true;
        return true;
    };
    let (apply, cancel) = crate::prepare::body(ui, d, t);
    pending.field_props_now = apply;
    pending.close = apply || cancel;
    true
}

/// Comment Properties.
fn show_comment_props(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    let (apply, cancel) = crate::comment_props::body(ui, app, t);
    pending.props_now = apply;
    pending.close = apply || cancel;
    true
}

/// Fill & Sign: Create signature (the drawing pad).
fn show_signature(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    let (apply, cancel) = crate::fill_sign::signature_pad(ui, t, &mut app.signature_draft, &mut app.signature_preview);
    if apply {
        let d = std::mem::take(&mut app.signature_draft);
        let tool = if d.initials {
            app.initials = Some(d.saved());
            crate::fill_sign::FillTool::Initials
        } else {
            app.signature = Some(d.saved());
            crate::fill_sign::FillTool::Signature
        };
        app.quick_tool = crate::QuickTool::Fill(tool);
        app.toast = None;
    }
    pending.close = apply || cancel;
    true
}

/// Prepare for accessibility: Add alternate text.
fn show_alt_text(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    let (save, cancel) = crate::a11y_ui::alt_body(ui, app, t);
    pending.alt_now = save;
    pending.close = save || cancel;
    true
}

/// Custom stamps: Create.
fn show_create_stamp(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    let (save, cancel) = crate::stamps_ui::create_body(ui, app, t);
    pending.stamp_now = save;
    pending.close = save || cancel;
    true
}

/// Combine files.
fn show_combine(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    ui.set_width(620.0);
    let (go, cancel) = crate::combine_ui::body(ui, app, t);
    pending.combine_now = go;
    if cancel {
        app.combine_draft.clear();
    }
    pending.close = go || cancel;
    true
}

/// Standards: PDF/A.
fn show_pdfa(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    pending.close = crate::standards_ui::body(ui, app, t);
    true
}

/// Action Wizard.
fn show_action_wizard(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    ui.set_width(620.0);
    pending.close = crate::actions_ui::body(ui, app, t);
    true
}

/// Compare files.
fn show_compare_files(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    let (go, cancel) = crate::compare_ui::body(ui, app, t);
    pending.compare_now = go;
    pending.close = go || cancel;
    true
}

/// The JavaScript console.
fn show_js_console(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    ui.set_width(640.0);
    pending.close = crate::js_ui::console_body(ui, app, t);
    true
}

/// The document's `/JavaScripts` array.
fn show_document_js(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    ui.set_width(640.0);
    pending.close = crate::js_ui::document_js_body(ui, app, t);
    true
}

/// Preferences.
fn show_preferences(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    pending.close = crate::js_ui::preferences_body(ui, app, t);
    true
}

/// Scan & OCR: Recognize text.
fn show_recognize_text(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    let (go, cancel) = crate::ocr_ui::body(ui, app, t);
    pending.ocr_now = go;
    pending.close = go || cancel;
    true
}

/// Check for accessibility: Accessibility Checker Options.
fn show_accessibility_options(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    let (start, cancel) = crate::a11y_ui::options_body(ui, app, t);
    pending.a11y_now = start;
    pending.close = start || cancel;
    true
}

/// Export to another format.
fn show_export(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, kind: crate::export_ui::ExportKind, pending: &mut Pending) -> bool {
    let (apply, cancel) = crate::export_ui::body(ui, app, t, kind);
    pending.export_now = apply;
    pending.close = apply || cancel;
    true
}

/// Add / Update Header and Footer, Watermark, Background.
fn show_marks(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, kind: printcraft_engine::MarkKind, pending: &mut Pending) -> bool {
    ui.set_width(720.0);
    let (apply, cancel) = crate::marks_ui::body(ui, app, t, kind);
    pending.marks_now = apply;
    pending.close = apply || cancel;
    true
}

/// Set Page Boxes.
fn show_page_boxes(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    let (apply, cancel) = crate::pageboxes::body(ui, app, t);
    pending.boxes_now = apply;
    pending.close = apply || cancel;
    true
}

/// Protect Using Password.
fn show_protect(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    let (apply, cancel) = crate::protect::body(ui, app, t);
    pending.protect_now = apply;
    pending.close = apply || cancel;
    true
}

/// Pages: Number pages (the page labels).
fn show_number_pages(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens, pending: &mut Pending) -> bool {
    use printcraft_engine::LabelStyle as L;
    ui.label(egui::RichText::new("Number pages").font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let Some((_, id)) = app.active_ids() else { return true };
    let total = app.session.get(id).map_or(1, |d| d.info.pages.len()).max(1);
    let d = &mut app.number_draft;
    d.to = d.to.clamp(1, total);
    d.from = d.from.clamp(1, d.to);
    // Editable values get a visible border (the dialog and field fills are alike).
    let boxed = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui) -> egui::Response| {
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, t.border))
            .corner_radius(egui::CornerRadius::same(5))
            .inner_margin(egui::Margin::symmetric(4, 1))
            .show(ui, |ui| add(ui))
            .inner
    };
    egui::Grid::new("number_pages").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label("Pages");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut d.from).range(1..=total));
            ui.label("to");
            ui.add(egui::DragValue::new(&mut d.to).range(1..=total));
            ui.label(egui::RichText::new(format!("of {total}")).color(t.text_muted));
        });
        ui.end_row();
        ui.label("Style");
        let styles = [
            (L::Decimal, "1, 2, 3"),
            (L::LowerRoman, "i, ii, iii"),
            (L::UpperRoman, "I, II, III"),
            (L::LowerAlpha, "a, b, c"),
            (L::UpperAlpha, "A, B, C"),
            (L::None, "None (prefix only)"),
        ];
        let current = styles.iter().find(|(s, _)| *s == d.style).map_or("1, 2, 3", |(_, l)| *l);
        egui::ComboBox::from_id_salt("label_style").selected_text(current).show_ui(ui, |ui| {
            for (s, l) in styles {
                ui.selectable_value(&mut d.style, s, l);
            }
        });
        ui.end_row();
        let prefix_label = ui.label("Prefix");
        boxed(ui, &mut |ui| ui.add(egui::TextEdit::singleline(&mut d.prefix).desired_width(160.0).frame(egui::Frame::NONE)))
            .labelled_by(prefix_label.id);
        ui.end_row();
        ui.label("Start");
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
    ui.label(egui::RichText::new(format!("Labels: {preview}. Later pages keep their labels.")).color(t.text_muted));
    pending.number_now = Some(Edit::NumberPages { from: d.from - 1, to: d.to - 1, style: d.style, prefix: d.prefix.clone(), first: d.start });
    false
}

/// Documents from a session that ended unexpectedly.
fn show_recovery(ui: &mut egui::Ui, app: &mut PrintCraftApp, t: &Tokens) -> bool {
    ui.horizontal(|ui| {
        ui.add(crate::icons::image("clock-3", 22.0, t.accent));
        ui.label(egui::RichText::new("Recover unsaved documents?").font(theme::semibold(18.0)));
    });
    ui.add_space(6.0);
    ui.label("PrintCraft didn't shut down normally. These documents had changes that were autosaved:");
    ui.add_space(8.0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    egui::Grid::new("recoverable").num_columns(2).spacing([18.0, 6.0]).show(ui, |ui| {
        for m in &app.recoverable {
            ui.label(egui::RichText::new(&m.name).font(theme::medium(13.0)));
            let mins = now.saturating_sub(m.saved_at) / 60;
            let when = match mins {
                0 => "just now".to_string(),
                1..=59 => format!("{mins} min ago"),
                _ => format!("{} h ago", mins / 60),
            };
            let lock = if m.encrypted { " · password-protected" } else { "" };
            ui.label(egui::RichText::new(format!("{when}{lock}")).color(t.text_muted));
            ui.end_row();
        }
    });
    false
}

/// Keyboard shortcuts.
fn show_shortcuts(ui: &mut egui::Ui) -> bool {
    ui.label(egui::RichText::new("Keyboard shortcuts").font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let mac = cfg!(target_os = "macos") || cfg!(target_arch = "wasm32");
    // Registered commands first (always in sync with the real bindings), then the
    // keys the document view handles itself.
    let mut rows: Vec<(String, String)> = printcraft_engine::commands::COMMANDS
        .iter()
        .filter_map(|c| c.shortcut.map(|k| (k.label(mac), c.label.trim_end_matches('…').to_string())))
        .collect();
    for (k, v) in [
        ("⌘G / ⇧⌘G", "Next / previous match"),
        ("⌘C", "Copy selected text"),
        ("Double-click", "Select a word"),
        ("Esc", "Clear selection / close find"),
        ("⌘1", "Actual size"),
        ("⌘0", "Zoom to page level"),
        ("⌘2", "Fit to width"),
        ("⌘3", "Fit visible"),
        ("⌘+ / ⌘−", "Zoom in / out (also pinch or ⌘-scroll)"),
        ("⇧⌘+ / ⇧⌘−", "Rotate view"),
        ("Home / End", "First / last page"),
        ("⌘← / ⌘→", "Previous / next page"),
        ("Delete", "Delete selected pages (Organize)"),
        ("⌘A", "Select all pages (Organize)"),
    ] {
        rows.push((k.to_string(), v.to_string()));
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
    false
}

/// The About box: version, licences and credits.
fn show_about(ui: &mut egui::Ui, t: &Tokens, pending: &mut Pending) -> bool {
    ui.horizontal(|ui| {
        widgets::artcraft_mark(ui, 40.0);
        ui.vertical(|ui| {
            ui.label(egui::RichText::new("PrintCraft").font(theme::semibold(20.0)));
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
        });
    });
    ui.add_space(6.0);
    ui.label("A clean-room, open-source PDF application written in Rust. MIT OR Apache-2.0.");
    ui.label(
        egui::RichText::new("Rendering: hayro (bootstrap) · UI: egui · Icons: Lucide (ISC) · Fonts: Inter, JetBrains Mono, Dancing Script (OFL)")
            .color(t.text_muted)
            .small(),
    );
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Part of").color(t.text_muted));
        widgets::artcraft_logo(ui, 16.0);
    });
    ui.add_space(6.0);
    if let Some(cmd) = widgets::community_links(ui) {
        pending.link_command = Some(cmd);
    }
    false
}

/// Everything the dialog asked for, carried out once the modal is closed, in order.
fn run_edits(app: &mut PrintCraftApp, dialog: Dialog, pending: &mut Pending) {
    if let Some(yes) = pending.recover {
        let keys: Vec<String> = app.recoverable.iter().map(|m| m.key.clone()).collect();
        if yes {
            app.recover(&keys);
        } else {
            app.discard_recovered(&keys);
        }
    }
    if pending.replace_now
        && let Some(d) = app.replace_draft.take()
    {
        let n = d.to - d.from + 1;
        app.apply_edit(Edit::ReplacePages {
            pages: (d.from - 1..d.to).collect(),
            name: d.name.clone(),
            bytes: d.bytes.clone(),
            src_pages: (d.src_from - 1..d.src_from - 1 + n).collect(),
        });
    }
    if pending.print_go {
        app.print_now();
    }
    if pending.revert_now {
        app.revert_active();
    }
    if pending.summarize_now {
        app.summarize_comments();
    }
    if pending.optimize_now {
        app.optimize_with_draft();
    }
    if let Some(e) = pending.duplicate_now.take() {
        app.duplicate_draft = None;
        app.apply_edit(e);
    }
    if pending.extract_now {
        app.extract_selection();
    }
    if let Some(e) = pending.link_now.take() {
        app.apply_edit(e);
        app.link_draft = None;
    }
    if pending.rotate_now {
        app.rotate_with_draft();
    }
    match pending.redact_now {
        Some(Dialog::RedactPages) => app.redact_pages(),
        Some(Dialog::RedactSearch) => {
            let n = app.redact_search();
            app.redact_search.found = Some(n);
        }
        Some(Dialog::RemoveHidden) => {
            let which: Vec<printcraft_engine::Hidden> = app.hidden_draft.found.iter().filter(|f| f.2 && f.1 > 0).map(|f| f.0).collect();
            let n: usize = app.hidden_draft.found.iter().filter(|f| f.2).map(|f| f.1).sum();
            if app.apply_edit(Edit::RemoveHidden { which }) {
                app.notify(format!("Removed {n} hidden item{}. Save to remove them from the file.", if n == 1 { "" } else { "s" }));
            }
        }
        Some(Dialog::Sanitize) => {
            if app.apply_edit(Edit::Sanitize) {
                app.notify("Document sanitized. Save to finish: saving rewrites the whole file.");
            }
        }
        Some(Dialog::RedactApply) => {
            let marks = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(0, printcraft_engine::Document::redaction_marks);
            if app.apply_edit(Edit::ApplyRedactions { pages: None }) {
                app.notify(format!("Applied {marks} redaction mark{}. Save to remove the content from the file.", if marks == 1 { "" } else { "s" }));
            }
        }
        _ => {}
    }
    if pending.field_props_now
        && let Some(d) = app.field_props.take()
        && let Some(props) = d.props()
        && app.apply_edit(Edit::SetFieldProps { name: d.field.clone(), props: Box::new(props) })
        && let Some(i) = app.active
    {
        // Keep the (possibly renamed) field selected.
        let prefix = d.field.rsplit_once('.').map(|(p, _)| format!("{p}.")).unwrap_or_default();
        app.views[i].prepare.selected = Some((format!("{prefix}{}", d.name.trim()), d.widget));
    }
    if pending.props_now
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
    }
    if pending.export_now
        && let Dialog::Export(kind) = dialog
    {
        app.start_export(kind);
    }
    if pending.marks_now
        && let (Dialog::Marks(kind), Some((_, id))) = (dialog, app.active_ids())
    {
        let count = app.session.get(id).map_or(0, |d| d.info.pages.len());
        let edit = crate::marks_ui::edit(&app.marks_draft, kind, count);
        app.apply_edit(edit);
    }
    if pending.boxes_now
        && let Some((i, id)) = app.active_ids()
    {
        let count = app.session.get(id).map_or(0, |d| d.info.pages.len());
        let edit = app.boxes_draft.edit(app.views[i].current, count);
        app.apply_edit(edit);
        app.boxes_draft.seeded = None;
    }
    if pending.protect_now && app.apply_edit(app.protect_draft.edit()) {
        app.protect_draft = crate::protect::ProtectDraft::default();
        app.notify("Password protection will be applied when you save");
    }
    if pending.apply_number
        && let Some(edit) = pending.number_now.take()
    {
        app.apply_edit(edit);
    }
    if let Some(by) = pending.split_now.take() {
        app.split_active(&by);
    }
    if pending.apply
        && let Some(edits) = draft_changes(app)
    {
        app.apply_edit(Edit::Batch { label: "Change document properties".into(), edits });
    }
}

/// Close (or swap in) the dialog, then do what its buttons asked for.
fn close_dialog(app: &mut PrintCraftApp, pending: &mut Pending, modal_closed: bool, next: Dialog) {
    // Protect / Remove security replace the Properties dialog.
    let replaces = pending.link_command.is_some_and(|c| c.starts_with("protect.")) || pending.open_revision.is_some();
    if modal_closed || pending.close || replaces {
        app.dialog = None;
        app.props_draft = None;
        app.view_draft = None;
    } else {
        app.dialog = Some(next);
    }
    if let Some(cmd) = pending.link_command {
        app.execute(cmd);
    }
    if let Some(n) = pending.open_revision {
        app.open_revision(n);
    }
    if pending.alt_now {
        app.save_alt_text();
    }
    if pending.stamp_now {
        app.save_custom_stamp();
    }
    if pending.combine_now {
        app.combine_staged();
    }
    if pending.ocr_now {
        app.start_ocr();
    }
    if pending.compare_now {
        app.run_compare();
    }
    if pending.a11y_now {
        app.a11y_skipped.clear();
        app.run_accessibility_check();
    }
}

/// Info edits needed to make the document match the Description draft.
fn draft_changes(app: &PrintCraftApp) -> Option<Vec<Edit>> {
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
    Some(edits)
}

/// The answer to "Save changes?": save, discard the changes, or cancel the close.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    Save,
    Discard,
    Cancel,
}

impl Answer {
    /// As `PrintCraftApp::resolve_close` takes it: save, discard, or cancel.
    pub(crate) fn resolve(self) -> Option<bool> {
        match self {
            Answer::Save => Some(true),
            Answer::Discard => Some(false),
            Answer::Cancel => None,
        }
    }
}

/// The "Save changes?" prompt's keys (issue #8), read before anything else can take them: Enter
/// saves (the default button) and Escape cancels; Don't save takes ⌘D / Ctrl+D, the macOS
/// convention, or Alt+D / Alt+N, the mnemonics Windows and Linux desktops use for it.
pub(crate) fn save_prompt_key(ctx: &egui::Context) -> Option<Answer> {
    use egui::{Key, Modifiers};
    ctx.input_mut(|i| {
        if i.consume_key(Modifiers::NONE, Key::Enter) {
            Some(Answer::Save)
        } else if i.consume_key(Modifiers::NONE, Key::Escape) {
            Some(Answer::Cancel)
        } else if [(Modifiers::COMMAND, Key::D), (Modifiers::ALT, Key::D), (Modifiers::ALT, Key::N)].into_iter().any(|(m, k)| i.consume_key(m, k)) {
            Some(Answer::Discard)
        } else {
            None
        }
    })
}

/// "Save changes?" when closing a tab or quitting with unsaved edits.
fn save_prompt(app: &mut PrintCraftApp, ctx: &egui::Context) {
    let Some(req) = app.close_request else { return };
    let index = match req {
        CloseRequest::Tab(i) => Some(i),
        CloseRequest::Quit | CloseRequest::All => app.first_dirty(),
    };
    let Some(name) = index.and_then(|i| app.views.get(i)).and_then(|v| app.session.get(v.id)).map(|d| d.name.clone()) else {
        // Nothing left to ask about (tab already gone or no dirty documents).
        app.resolve_close(ctx, Some(false));
        return;
    };
    let t = Tokens::get(ctx);
    let mut choice: Option<Answer> = None;
    let modal = egui::Modal::new(egui::Id::new("save_prompt")).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("save", 22.0, t.accent));
            ui.label(egui::RichText::new(format!("Save changes to “{name}” before closing?")).font(theme::semibold(16.0)));
        });
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Your changes will be lost if you don't save them.").color(t.text_muted));
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, "Save", true).clicked() {
                choice = Some(Answer::Save);
            }
            if widgets::pill_button(ui, "Cancel", false).clicked() {
                choice = Some(Answer::Cancel);
            }
            ui.add_space(24.0);
            if widgets::pill_button(ui, "Don't save", false).clicked() {
                choice = Some(Answer::Discard);
            }
        });
    });
    if choice.is_none() && modal.should_close() {
        choice = Some(Answer::Cancel);
    }
    if let Some(c) = choice {
        app.resolve_close(ctx, c.resolve());
    }
}

fn yes(b: bool) -> String {
    if b { "Yes".into() } else { "No".into() }
}

/// Password prompt for encrypted documents (Acrobat: "Password" dialog on open).
fn password(app: &mut PrintCraftApp, ctx: &egui::Context) {
    let Some(prompt) = app.password_prompt.as_mut() else { return };
    let t = Tokens::get(ctx);
    let mut submit = false;
    let mut cancel = false;
    let modal = egui::Modal::new(egui::Id::new("password")).show(ctx, |ui| {
        ui.set_width(400.0);
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("lock", 22.0, t.accent));
            ui.label(egui::RichText::new("Password required").font(theme::semibold(17.0)));
        });
        ui.add_space(6.0);
        ui.label(format!("“{}” is protected. Enter a password to open it.", prompt.name));
        ui.add_space(8.0);
        let r = ui.add(egui::TextEdit::singleline(&mut prompt.input).password(true).hint_text("Password").desired_width(f32::INFINITY));
        // Enter submits. The field keeps focus (we request it every frame), so check the key
        // while it is focused as well as on the frame focus is lost.
        if (r.has_focus() || r.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            submit = true;
        }
        r.request_focus();
        if let Some(e) = &prompt.error {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(e).color(egui::Color32::from_rgb(0xD1, 0x3B, 0x3B)));
        }
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, "Open", true).clicked() {
                submit = true;
            }
            if widgets::pill_button(ui, "Cancel", false).clicked() {
                cancel = true;
            }
        });
    });
    if submit {
        let pw = prompt.input.clone();
        app.submit_password(Some(pw));
    } else if cancel || modal.should_close() {
        app.submit_password(None);
    }
}
