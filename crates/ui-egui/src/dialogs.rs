//! Modal dialogs: Document Properties, Keyboard Shortcuts, About (with the Contributors and Models credits).

use egui::{Align, Layout};

use crate::theme::{self, Tokens};
use pdfcraft_engine::Edit;

use crate::{CloseRequest, Dialog, PdfCraftApp, PropsTab, panels::human_size, widgets};

const INFO_KEYS: [&str; 4] = ["Title", "Author", "Subject", "Keywords"];

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
    let mut apply = false;
    let mut split_now: Option<crate::SplitPlan> = None;
    let mut split_ready: Option<crate::SplitPlan> = None;
    let mut extract_now = false;
    let mut rotate_now = false;
    let mut link_now: Option<Edit> = None;
    let mut recover: Option<bool> = None;
    let mut number_now: Option<Edit> = None;
    let mut apply_number = false;
    let mut link_command: Option<&'static str> = None;
    let mut protect_now = false;
    let mut boxes_now = false;
    let mut marks_now = false;
    let mut export_now = false;
    let mut props_now = false;
    let mut field_props_now = false;
    let mut bulk_field_props_now = false;
    let mut redact_now: Option<Dialog> = None;
    let mut import_words = false;
    let mut print_go = false;
    let mut revert_now = false;
    let mut overwrite_now = false;
    let mut overwrite_elsewhere = false;
    let mut summarize_now = false;
    let mut optimize_now = false;
    let mut duplicate_now: Option<Edit> = None;
    let mut replace_now = false;
    let mut open_revision: Option<usize> = None;
    let mut a11y_now = false;
    let mut ocr_now = false;
    let mut compare_now = false;
    let mut images_now = false;
    let mut stamp_now = false;
    let mut alt_now = false;
    let t = Tokens::get(ctx);
    let mut close = false;
    let mut next = dialog;
    let modal = widgets::show_modal(ctx, "dialog", |ui| {
        ui.set_width(match dialog {
            Dialog::Properties(_) => 640.0,
            Dialog::Print => 820.0,
            Dialog::FieldProps | Dialog::BulkFieldProps => 600.0,
            Dialog::About => 780.0,
            _ => 520.0,
        });
        match dialog {
            Dialog::WordCount => {
                close = crate::word_count_ui::body(ui, app);
            }
            Dialog::CreateImages => {
                let (go, cancel) = crate::create_ui::image_import_body(ui, app);
                images_now = go;
                close = go || cancel;
                return;
            }
            Dialog::Properties(tab) => {
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
                            next = Dialog::Properties(tb);
                        }
                    }
                });
                ui.separator();
                let units = app.units;
                let Some((_, id)) = app.active_ids() else { return };
                let Some(doc) = app.session.get(id) else { return };
                let i = &doc.info;
                // Description shows plain values among text fields; they start where the fields' text does.
                let editable_info = tab == PropsTab::Description && doc.allows_modification() && app.props_draft.is_some();
                let inset = if editable_info { widgets::FIELD_TEXT_INSET } else { 0.0 };
                let row = |ui: &mut egui::Ui, k: &str, v: String| {
                    // Row labels are UI chrome; values are document data and stay as they are.
                    ui.label(egui::RichText::new(tl!(k)).color(t.text_muted));
                    ui.horizontal(|ui| {
                        ui.add_space(inset);
                        ui.add(
                            egui::Label::new(if v.is_empty() { egui::RichText::new("—").color(t.text_faint) } else { egui::RichText::new(v) }).wrap(),
                        );
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
                                        let field =
                                            if matches!(*k, "Subject" | "Keywords") { widgets::block(v).desired_rows(1) } else { widgets::line(v) };
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
                                egui::ComboBox::from_id_salt("iv-nav")
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
                                egui::ComboBox::from_id_salt("iv-layout")
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
                                    egui::ComboBox::from_id_salt("iv-mag")
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
                                    link_command = Some("protect.password");
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
                                            link_command = Some("protect.unlock");
                                        }
                                        if ui.button(tl!("Change settings…")).clicked() {
                                            link_command = Some("protect.password");
                                        }
                                        if ui.button(tl!("Remove security")).clicked() {
                                            link_command = Some("protect.remove");
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
                            ui.add_enabled(
                                editable,
                                widgets::line(&mut d.name).desired_width(420.0).background_color(t.field).id_salt("custom-name"),
                            )
                            .labelled_by(l.id);
                            ui.end_row();
                            let l = ui.label(egui::RichText::new(tl!("Value:")).color(t.text_muted));
                            ui.add_enabled(
                                editable,
                                widgets::line(&mut d.value).desired_width(420.0).background_color(t.field).id_salt("custom-value"),
                            )
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
                                                open_revision = Some(n);
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
            }
            Dialog::Split => {
                ui.label(egui::RichText::new(tl!("Split document")).font(theme::semibold(18.0)));
                ui.add_space(8.0);
                let Some((vi, id)) = app.active_ids() else { return };
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
                    split_ready = Some(plan);
                }
            }
            Dialog::ReplacePages => {
                let count = app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|d| d.info.pages.len()).unwrap_or(1).max(1);
                let Some(d) = app.replace_draft.as_mut() else {
                    close = true;
                    return;
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
                        replace_now = true;
                        close = true;
                    }
                    if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                        close = true;
                    }
                });
                return;
            }
            Dialog::RedactPages => {
                let n = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len());
                let (ok, cancel) = crate::redact_ui::pages_body(ui, &mut app.redact_pages_draft, n, &t);
                if ok {
                    redact_now = Some(dialog);
                }
                close = ok || cancel;
                return;
            }
            Dialog::RedactSearch => {
                let (go, cancel, import) = crate::redact_ui::search_body(ui, &mut app.redact_search, &t);
                if go {
                    redact_now = Some(dialog);
                }
                import_words = import;
                close = cancel;
                return;
            }
            Dialog::RedactProps => {
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
                close = ok || cancel;
                return;
            }
            Dialog::RedactApply => {
                let marks = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(0, |d| d.redaction_marks());
                let (ok, cancel) = crate::redact_ui::apply_body(ui, marks, &t);
                if ok {
                    redact_now = Some(dialog);
                }
                close = ok || cancel;
                return;
            }
            Dialog::LinkProps => {
                let pages = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len());
                let Some(d) = app.link_draft.as_mut() else {
                    close = true;
                    return;
                };
                let (ok, cancel) = crate::link_ui::body(ui, d, pages, &t);
                if ok {
                    link_now = Some(crate::link_ui::edit_for(d));
                }
                close = ok || cancel;
                return;
            }
            Dialog::Extract => {
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
                        extract_now = true;
                        close = true;
                    }
                    if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                        close = true;
                    }
                });
                return;
            }
            Dialog::RotatePages => {
                use pdfcraft_engine::{PageOrientation as O, PageParity as P};
                let n = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len());
                let d = &mut app.rotate_draft;
                ui.label(egui::RichText::new(tl!("Rotate Pages")).font(theme::semibold(18.0)));
                ui.add_space(8.0);
                egui::Grid::new("rotate-pages").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label(tl!("Direction:"));
                    egui::ComboBox::from_id_salt("rotate-dir")
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
                    egui::ComboBox::from_id_salt("rotate-parity")
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
                    egui::ComboBox::from_id_salt("rotate-orient")
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
                        rotate_now = true;
                        close = true;
                    }
                    if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                        close = true;
                    }
                });
                return;
            }
            Dialog::DuplicateField => {
                let n = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len());
                let Some(d) = app.duplicate_draft.as_mut() else {
                    close = true;
                    return;
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
                        duplicate_now = Some(Edit::DuplicateField { name: d.name.clone(), pages });
                        close = true;
                    }
                    if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                        close = true;
                    }
                });
                return;
            }
            // Never reached: `dialogs` shows this movable window above and returns before the modal.
            // The arm keeps the match exhaustive.
            Dialog::FieldTable => close = true,
            Dialog::Optimize => {
                let (ok, cancel) = crate::optimize_ui::body(ui, &mut app.optimize_draft, &t);
                optimize_now = ok;
                close = ok || cancel;
                if std::mem::take(&mut app.optimize_draft.audit) {
                    app.refresh_space_audit();
                    next = Dialog::AuditSpace;
                }
                return;
            }
            Dialog::CertificateViewer => {
                ui.set_width(700.0);
                let trusted = app.session.trusted_certificates().to_vec();
                let Some(v) = app.cert_viewer.as_mut() else {
                    close = true;
                    return;
                };
                let (done, action) = crate::sign_ui::cert_viewer(ui, v, &trusted, &t);
                close = done;
                match action {
                    Some(crate::sign_ui::CertAction::Trust(c)) => app.trust_certificate(*c),
                    Some(crate::sign_ui::CertAction::Export(c)) => {
                        app.export_certificate(&c);
                    }
                    None => {}
                }
                return;
            }
            Dialog::AuditSpace => {
                if crate::optimize_ui::audit_body(ui, &app.space_audit, &t) {
                    next = Dialog::Optimize;
                }
                return;
            }
            Dialog::Sign => {
                if crate::sign_ui::dialog(ui, app, &t) {
                    app.sign_draft = None;
                    close = true;
                }
                return;
            }
            Dialog::SummarizeComments => {
                ui.label(egui::RichText::new(tl!("Summarize Options")).font(theme::semibold(18.0)));
                ui.add_space(8.0);
                ui.label(tl!("Choose a layout:"));
                let _ = ui.add(widgets::radio(true, tl!("Comments only")));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(tl!("Sort comments by:"));
                    egui::ComboBox::from_id_salt("summary-sort").selected_text(tl!(app.summary_sort.name())).show_ui(ui, |ui| {
                        for s in pdfcraft_engine::SummarySort::ALL {
                            ui.selectable_value(&mut app.summary_sort, s, tl!(s.name()));
                        }
                    });
                });
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::pill_button(ui, tl!("Create PDF Comment Summary"), true).clicked() {
                        summarize_now = true;
                        close = true;
                    }
                    if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                        close = true;
                    }
                });
                return;
            }
            Dialog::Revert => {
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
                        revert_now = true;
                        close = true;
                    }
                    if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                        close = true;
                    }
                });
                return;
            }
            Dialog::OverwriteChanged => {
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
                        overwrite_now = true;
                        close = true;
                    }
                    if widgets::pill_button(ui, tl!("Save as…"), false).clicked() {
                        overwrite_elsewhere = true;
                        close = true;
                    }
                    if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                        close = true;
                    }
                });
                return;
            }
            Dialog::Print => {
                let Some((i, id)) = app.active_ids() else {
                    close = true;
                    return;
                };
                if app.print_draft.handling == crate::print_ui::Handling::Label {
                    app.read_label_crops(id);
                }
                let Some(doc) = app.session.get(id) else { return };
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
                print_go = go;
                close = go || cancel;
                return;
            }
            Dialog::RemoveHidden => {
                let (ok, cancel) = crate::redact_ui::hidden_body(ui, &mut app.hidden_draft, &t);
                if ok {
                    redact_now = Some(dialog);
                }
                close = ok || cancel;
                return;
            }
            Dialog::Sanitize => {
                let (ok, cancel) = crate::redact_ui::sanitize_body(ui, &t);
                if ok {
                    redact_now = Some(dialog);
                }
                close = ok || cancel;
                return;
            }
            Dialog::FieldProps => {
                let Some(d) = app.field_props.as_mut() else {
                    close = true;
                    return;
                };
                let (apply, cancel) = crate::prepare::body(ui, d, &t);
                field_props_now = apply;
                close = apply || cancel;
                return;
            }
            Dialog::BulkFieldProps => {
                let Some(d) = app.bulk_field_props.as_mut() else {
                    close = true;
                    return;
                };
                let (apply, cancel) = crate::bulk_fields::body(ui, d, &t);
                bulk_field_props_now = apply;
                close = cancel;
                return;
            }
            Dialog::CommentProps => {
                let (apply, cancel) = crate::comment_props::body(ui, app, &t);
                props_now = apply;
                close = apply || cancel;
                return;
            }
            Dialog::Signature => {
                let (apply, cancel, browse) =
                    crate::fill_sign::signature_pad(ui, &t, &mut app.signature_draft, &mut app.signature_preview, &mut app.signature_photo);
                if browse {
                    app.pick_signature_image();
                }
                if apply {
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
                close = apply || cancel;
                return;
            }
            Dialog::AltText => {
                let (save, cancel) = crate::a11y_ui::alt_body(ui, app, &t);
                alt_now = save;
                close = save || cancel;
                return;
            }
            Dialog::CreateStamp => {
                let (save, cancel) = crate::stamps_ui::create_body(ui, app, &t);
                stamp_now = save;
                close = save || cancel;
                return;
            }
            Dialog::PdfA => {
                close = crate::standards_ui::body(ui, app, &t);
                return;
            }
            Dialog::ActionWizard => {
                ui.set_width(620.0);
                close = crate::actions_ui::body(ui, app, &t);
                return;
            }
            Dialog::CompareFiles => {
                let (go, cancel) = crate::compare_ui::body(ui, app, &t);
                compare_now = go;
                close = go || cancel;
                return;
            }
            Dialog::JsConsole => {
                ui.set_width(640.0);
                close = crate::js_ui::console_body(ui, app, &t);
                return;
            }
            Dialog::DocumentJs => {
                ui.set_width(640.0);
                close = crate::js_ui::document_js_body(ui, app, &t);
                return;
            }
            Dialog::Preferences => {
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
                close = crate::js_ui::preferences_buttons(ui);
                return;
            }
            Dialog::RecognizeText => {
                let (go, cancel) = crate::ocr_ui::body(ui, app, &t);
                ocr_now = go;
                close = go || cancel;
                return;
            }
            Dialog::AccessibilityOptions => {
                let (start, cancel) = crate::a11y_ui::options_body(ui, app, &t);
                a11y_now = start;
                close = start || cancel;
                return;
            }
            Dialog::Export(kind) => {
                let (apply, cancel) = crate::export_ui::body(ui, app, &t, kind);
                export_now = apply;
                close = apply || cancel;
                return;
            }
            Dialog::Marks(kind) => {
                ui.set_width(720.0);
                let (apply, cancel) = crate::marks_ui::body(ui, app, &t, kind);
                marks_now = apply;
                close = apply || cancel;
                return;
            }
            Dialog::PageBoxes => {
                let (apply, cancel) = crate::pageboxes::body(ui, app, &t);
                boxes_now = apply;
                close = apply || cancel;
                return;
            }
            Dialog::Protect => {
                let (apply, cancel) = crate::protect::body(ui, app, &t);
                protect_now = apply;
                close = apply || cancel;
                return;
            }
            Dialog::NumberPages => {
                use pdfcraft_engine::LabelStyle as L;
                ui.label(egui::RichText::new(tl!("Number pages")).font(theme::semibold(18.0)));
                ui.add_space(8.0);
                let Some((_, id)) = app.active_ids() else { return };
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
                    egui::ComboBox::from_id_salt("label_style").selected_text(current).show_ui(ui, |ui| {
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
                    egui::RichText::new(crate::i18n::fmt(tl!("Labels: {preview}. Later pages keep their labels."), &[("preview", &preview)]))
                        .color(t.text_muted),
                );
                number_now = Some(Edit::NumberPages { from: d.from - 1, to: d.to - 1, style: d.style, prefix: d.prefix.clone(), first: d.start });
            }
            Dialog::Recovery => {
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
            }
            Dialog::Shortcuts => {
                ui.label(egui::RichText::new(tl!("Keyboard shortcuts")).font(theme::semibold(18.0)));
                ui.add_space(8.0);
                use crate::commands::{
                    ACTUAL_SIZE, COPY, FIND_NEXT, FIND_PREV, FIT_WIDTH, PAGE_LEVEL, PAGE_NEXT, PAGE_PREV, ROTATE_CCW, ROTATE_CW, SELECT_ALL, ZOOM_IN,
                    ZOOM_OUT,
                };
                // Registered commands first (always in sync with the real bindings), then the
                // keys the document view handles itself.
                let mut rows: Vec<(String, String)> = pdfcraft_engine::commands::COMMANDS
                    .iter()
                    .filter_map(|c| {
                        c.shortcut.map(|k| (crate::commands::shortcut_label(ui.ctx(), k), tl!(c.label).trim_end_matches('…').to_string()))
                    })
                    .collect();
                let key = |s: pdfcraft_engine::commands::Shortcut| crate::commands::shortcut_label(ui.ctx(), s);
                let pair = |a, b| format!("{} / {}", key(a), key(b));
                // Key names, translated where a catalog has them (not scanned as UI literals).
                let named = |k: &str| tl!(k).to_string();
                let scroll = crate::i18n::fmt(
                    tl!("Zoom in / out (also pinch or {key}-scroll)"),
                    &[("key", crate::commands::command_modifier_label(ui.ctx()))],
                );
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
            }
            Dialog::About => {
                // Tabs About · Contributors · Models (craftrules standards/contributors.md).
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
                            link_command = Some(cmd);
                        }
                    }
                }
            }
        }
        ui.add_space(16.0);
        let changed = draft_changes(app).is_some_and(|c| !c.is_empty());
        // One row high: on its own, a right-to-left layout centres the buttons in all the height
        // left, which after a taller Properties tab is the room that tab left behind.
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if dialog == Dialog::Recovery {
                    if widgets::pill_button(ui, tl!("Recover"), true).clicked() {
                        recover = Some(true);
                        close = true;
                    }
                    if widgets::pill_button(ui, tl!("Discard"), false).clicked() {
                        recover = Some(false);
                        close = true;
                    }
                } else if dialog == Dialog::NumberPages {
                    if widgets::pill_button(ui, tl!("OK"), true).clicked() {
                        apply_number = true;
                        close = true;
                    }
                    if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                        close = true;
                    }
                } else if dialog == Dialog::Split {
                    if ui.add_enabled_ui(split_ready.is_some(), |ui| widgets::pill_button(ui, tl!("Split"), true)).inner.clicked() {
                        split_now = split_ready.clone();
                        close = true;
                    }
                    if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                        close = true;
                    }
                } else if changed {
                    if widgets::pill_button(ui, tl!("OK"), true).clicked() {
                        apply = true;
                        close = true;
                    }
                    if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                        close = true;
                    }
                } else if widgets::pill_button(ui, tl!("Close"), true).clicked() {
                    close = true;
                }
            })
        });
    });
    if let Some(yes) = recover {
        let keys: Vec<String> = app.recoverable.iter().map(|m| m.key.clone()).collect();
        if yes {
            app.recover(&keys);
        } else {
            app.discard_recovered(&keys);
        }
    }
    // The pages to replace belong to the document the dialog was opened on (#167).
    if replace_now
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
    if print_go && !app.print_now() {
        close = false;
    }
    if revert_now {
        app.revert_active();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if overwrite_now {
            app.overwrite_changed_file();
        }
        if overwrite_elsewhere {
            app.save_changed_file_elsewhere();
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (overwrite_now, overwrite_elsewhere);
    // Cancelled or dismissed: the save is dropped.
    #[cfg(not(target_arch = "wasm32"))]
    if dialog == Dialog::OverwriteChanged && (modal.should_close() || close) {
        app.pending_overwrite = None;
    }
    if summarize_now {
        app.summarize_comments();
    }
    if optimize_now {
        app.optimize_with_draft();
    }
    if let Some(e) = duplicate_now {
        app.duplicate_draft = None;
        app.apply_edit(e);
    }
    if extract_now {
        app.extract_selection();
    }
    if let Some(e) = link_now {
        app.apply_edit(e);
        app.link_draft = None;
    }
    if rotate_now {
        app.rotate_with_draft();
    }
    if import_words {
        app.pick_files(crate::files::FilePurpose::RedactWords, false);
    }
    match redact_now {
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
    if bulk_field_props_now {
        close = app.apply_bulk_field_props();
    }
    if field_props_now
        && let Some(d) = app.field_props.take()
        && let Some(props) = d.props()
    {
        let form = |app: &PdfCraftApp| app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|doc| doc.form.clone());
        // A rename also renames the field in the formulas that read it.
        let before = props.name.as_ref().and_then(|_| form(app));
        if app.apply_edit(Edit::SetFieldProps { name: d.field.clone(), props: Box::new(props) })
            && let Some(i) = app.active
        {
            // Keep the (possibly renamed) field selected.
            // A dotted name is the full name; a plain one renames the field within its parent.
            let typed = d.name.trim();
            let prefix = d.field.rsplit_once('.').filter(|_| !typed.contains('.')).map(|(p, _)| format!("{p}.")).unwrap_or_default();
            let name = format!("{prefix}{typed}");
            if let (Some(before), Some(after)) = (before, form(app)) {
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
            app.views[i].prepare.selected = Some((name, d.widget));
        }
    }
    if props_now && let Some(d) = app.comment_props.take() {
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
    if export_now && let Dialog::Export(kind) = dialog {
        app.start_export(kind);
    }
    if marks_now && let (Dialog::Marks(kind), Some((_, id))) = (dialog, app.active_ids()) {
        let count = app.session.get(id).map(|d| d.info.pages.len()).unwrap_or(0);
        let edit = crate::marks_ui::edit(&app.marks_draft, kind, count);
        app.apply_edit(edit);
    }
    if boxes_now && let Some((i, id)) = app.active_ids() {
        let count = app.session.get(id).map(|d| d.info.pages.len()).unwrap_or(0);
        let edit = app.boxes_draft.edit(app.views[i].current, count);
        app.apply_edit(edit);
        app.boxes_draft.seeded = None;
    }
    if protect_now && app.apply_edit(app.protect_draft.edit()) {
        app.protect_draft = Default::default();
        app.notify_tr("Password protection will be applied when you save");
    }
    if apply_number && let Some(edit) = number_now {
        app.apply_edit(edit);
    }
    if let Some(by) = split_now {
        app.split_active(&by);
    }
    if apply && let Some(edits) = draft_changes(app) {
        app.apply_edit(Edit::Batch { label: "Change document properties".into(), edits });
    }
    // Protect / Remove security replace the Properties dialog.
    let replaces = link_command.is_some_and(|c| c.starts_with("protect.")) || open_revision.is_some();
    if modal.should_close() || close || replaces {
        app.dialog = None;
        app.props_draft = None;
        app.view_draft = None;
        app.custom_draft = None;
        app.bulk_field_props = None;
    } else {
        app.dialog = Some(next);
    }
    if let Some(cmd) = link_command {
        app.execute(cmd);
    }
    if let Some(n) = open_revision {
        app.open_revision(n);
    }
    if alt_now {
        app.save_alt_text();
    }
    if stamp_now {
        app.save_custom_stamp();
    }
    if images_now {
        app.finish_image_import();
    } else if dialog == Dialog::CreateImages && app.dialog != Some(Dialog::CreateImages) {
        app.image_import = None;
    }
    if ocr_now {
        app.start_ocr();
    }
    if compare_now {
        app.run_compare();
    }
    if a11y_now {
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

/// The buttons of the "Save changes?" prompt, in the order they sit on screen, left to right.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PromptButton {
    DontSave,
    Cancel,
    #[default]
    Save,
}

impl PromptButton {
    const ORDER: [PromptButton; 3] = [PromptButton::DontSave, PromptButton::Cancel, PromptButton::Save];

    fn position(self) -> usize {
        Self::ORDER.iter().position(|b| *b == self).unwrap_or(2)
    }

    /// The button `by` places to the right (negative: left). Arrow keys stop at the ends.
    fn step(self, by: isize) -> PromptButton {
        let to = self.position().saturating_add_signed(by).min(Self::ORDER.len() - 1);
        Self::ORDER.get(to).copied().unwrap_or(self)
    }

    /// The button after (or, with `back`, before) this one, wrapping round: Tab and Shift+Tab.
    fn cycle(self, back: bool) -> PromptButton {
        let n = Self::ORDER.len();
        let to = if back { self.position().checked_sub(1).unwrap_or(n - 1) } else { (self.position() + 1) % n };
        Self::ORDER.get(to).copied().unwrap_or(self)
    }

    /// What pressing the button answers.
    fn answer(self) -> Option<bool> {
        match self {
            PromptButton::Save => Some(true),
            PromptButton::DontSave => Some(false),
            PromptButton::Cancel => None,
        }
    }
}

/// The "Save changes?" prompt's keys (issue #8), read before anything else can take them. Save is
/// the button chosen when the prompt opens, so Enter saves; the arrow keys and Tab / Shift+Tab move
/// the choice, and Enter or Space press the chosen button. Escape cancels. Don't save also has
/// ⌘D / Ctrl+D, the macOS convention, and Alt+D / Alt+N, the mnemonics Windows and Linux desktops
/// use for it. Returns the answer once one is given.
pub(crate) fn save_prompt_key(ctx: &egui::Context, chosen: &mut PromptButton) -> Option<Option<bool>> {
    use egui::{Key, Modifiers};
    ctx.input_mut(|i| {
        if i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Space) {
            return Some(chosen.answer());
        }
        if i.consume_key(Modifiers::NONE, Key::Escape) {
            return Some(None);
        }
        if [(Modifiers::COMMAND, Key::D), (Modifiers::ALT, Key::D), (Modifiers::ALT, Key::N)].into_iter().any(|(m, k)| i.consume_key(m, k)) {
            return Some(Some(false));
        }
        // Shift+Tab before Tab: a key press matches a pattern that asks for fewer modifiers.
        if i.consume_key(Modifiers::SHIFT, Key::Tab) {
            *chosen = chosen.cycle(true);
        } else if i.consume_key(Modifiers::NONE, Key::Tab) {
            *chosen = chosen.cycle(false);
        }
        for (key, by) in [(Key::ArrowLeft, -1), (Key::ArrowUp, -1), (Key::ArrowRight, 1), (Key::ArrowDown, 1)] {
            if i.consume_key(Modifiers::NONE, key) {
                *chosen = chosen.step(by);
            }
        }
        None
    })
}

/// "Save changes?" when closing a tab or quitting with unsaved edits.
fn save_prompt(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(req) = app.close_request else { return };
    let index = match req {
        CloseRequest::Tab(id) => app.views.iter().position(|v| v.id == id),
        CloseRequest::Quit | CloseRequest::All => app.first_dirty(),
    };
    let Some(name) = index.and_then(|i| app.views.get(i)).and_then(|v| app.session.get(v.id)).map(|d| d.name.clone()) else {
        // Nothing left to ask about (tab already gone or no dirty documents).
        app.resolve_close(ctx, Some(false));
        return;
    };
    let t = Tokens::get(ctx);
    // Wrapping alone (#161) still let a long enough name (a web `?file=` URL has no limit) grow
    // the prompt taller than the window (#236); 80 characters wrap to a few lines.
    let shown = shorten_middle(&name, 80);
    let mut choice: Option<Option<bool>> = None;
    let modal = widgets::show_modal(ctx, "save_prompt", |ui| {
        ui.set_width(420.0);
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("save", 22.0, t.accent));
            let title = ui.add(
                egui::Label::new(
                    egui::RichText::new(crate::i18n::fmt(tl!("Save changes to “{name}” before closing?"), &[("name", &shown)]))
                        .font(theme::semibold(16.0)),
                )
                .wrap(),
            );
            if shown != name {
                title.on_hover_text(&name);
            }
        });
        ui.add_space(6.0);
        ui.label(egui::RichText::new(tl!("Your changes will be lost if you don't save them.")).color(t.text_muted));
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let chosen = app.save_prompt_button;
            if widgets::pill_button_marked(ui, tl!("Save"), true, chosen == PromptButton::Save).clicked() {
                choice = Some(Some(true));
            }
            if widgets::pill_button_marked(ui, tl!("Cancel"), false, chosen == PromptButton::Cancel).clicked() {
                choice = Some(None);
            }
            ui.add_space(24.0);
            if widgets::pill_button_marked(ui, tl!("Don't save"), false, chosen == PromptButton::DontSave).clicked() {
                choice = Some(Some(false));
            }
        });
    });
    if choice.is_none() && modal.should_close() {
        choice = Some(None);
    }
    if let Some(c) = choice {
        app.resolve_close(ctx, c);
    }
}

/// `name` cut to at most `max` characters by replacing its middle with "…", keeping the start and
/// the end, where the extension and version suffixes sit. Counts `char`s, so it never splits one.
fn shorten_middle(name: &str, max: usize) -> String {
    let count = name.chars().count();
    if count <= max {
        return name.to_string();
    }
    let tail = max / 4;
    let head: String = name.chars().take(max.saturating_sub(tail + 1)).collect();
    let end: String = name.chars().skip(count.saturating_sub(tail)).collect();
    format!("{head}…{end}")
}

/// "Open this web page?" when a document's link, button or script asks to open an address
/// (#90, #91). Shows where the address really goes and the whole address; Cancel is the default,
/// and Escape or clicking outside cancels.
fn link_prompt(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(pending) = app.pending_link.clone() else { return };
    let t = Tokens::get(ctx);
    let email = pending.url.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("mailto:"));
    let (title, open) = if email { (tl!("Write this email?"), tl!("Open email app")) } else { (tl!("Open this web page?"), tl!("Open link")) };
    let mut choice: Option<bool> = None;
    let modal = widgets::show_modal(ctx, "link_prompt", |ui| {
        ui.set_width(460.0);
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("external-link", 22.0, t.accent));
            ui.label(egui::RichText::new(title).font(theme::semibold(16.0)));
        });
        ui.add_space(6.0);
        let template = if email {
            tl!("{who} in this document wants to open your email app with this address:")
        } else {
            tl!("{who} in this document wants to open this address in your browser:")
        };
        ui.label(crate::i18n::fmt(template, &[("who", tl!(pending.origin.noun()))]));
        ui.add_space(6.0);
        // The host as the browser will connect to it, in punycode when it is international, so a
        // lookalike such as `pаypal.com` (Cyrillic `а`) reads as `xn--pypal-4ve.com`. Its Unicode
        // form isn't repeated here: a whole-script lookalike would read as the real site.
        if let Some(host) = pdfcraft_engine::links::display_host(&pending.url) {
            ui.label(egui::RichText::new(&host.ascii).font(theme::semibold(14.0)));
            if host.mixed_scripts {
                let warning = tl!("This web address mixes letters from different alphabets, a common way to imitate another site's name.");
                ui.add(egui::Label::new(egui::RichText::new(warning).color(egui::Color32::from_rgb(0xD1, 0x3B, 0x3B))).wrap());
            } else if host.international {
                let note = tl!("This web address uses letters from another alphabet, which can look like familiar ones.");
                ui.add(egui::Label::new(egui::RichText::new(note).color(t.text)).wrap());
            }
        }
        egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
            ui.add(egui::Label::new(egui::RichText::new(&pending.url).monospace().small()).wrap().selectable(true));
        });
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(tl!(
                "Only continue if you trust this document. An address can carry information from the document, such as what you typed into its form."
            ))
            .color(t.text_muted)
            .small(),
        );
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Cancel"), true).clicked() {
                choice = Some(false);
            }
            if widgets::pill_button(ui, open, false).clicked() {
                choice = Some(true);
            }
        });
    });
    if choice.is_none() && modal.should_close() {
        choice = Some(false);
    }
    if let Some(open) = choice {
        app.resolve_pending_link(open);
    }
}

/// A document that asks to open in Full Screen mode (`/PageMode /FullScreen`) is asked about
/// first, as Acrobat does: full screen hides every control, so it is the person's choice.
fn full_screen_prompt(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(id) = app.pending_full_screen else { return };
    // The document was closed before an answer: nothing to ask any more.
    if app.session.get(id).is_none() {
        app.pending_full_screen = None;
        return;
    }
    let t = Tokens::get(ctx);
    let mut choice = None;
    let modal = widgets::show_modal(ctx, "full_screen_prompt", |ui| {
        ui.set_width(460.0);
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("maximize", 22.0, t.accent));
            ui.label(egui::RichText::new(tl!("Open in Full Screen mode?")).font(theme::semibold(16.0)));
        });
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(tl!(
                "This document asks to open in Full Screen mode: the page fills the screen and nothing else shows. Press Esc at any time to leave it."
            ))
            .wrap(),
        );
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Not now"), true).clicked() {
                choice = Some(false);
            }
            if widgets::pill_button(ui, tl!("Full Screen"), false).clicked() {
                choice = Some(true);
            }
        });
    });
    if choice.is_none() && modal.should_close() {
        choice = Some(false);
    }
    if let Some(allow) = choice {
        app.resolve_full_screen_prompt(ctx, allow);
    }
}

/// Absolute document-controlled local paths require consent before any filesystem access.
fn remote_link_prompt(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(pending) = app.pending_remote_link.clone() else { return };
    let mut choice = None;
    let modal = widgets::show_modal(ctx, "remote_link_prompt", |ui| {
        ui.set_width(460.0);
        ui.label(egui::RichText::new(tl!("Open this local PDF?")).font(theme::semibold(16.0)));
        ui.add_space(6.0);
        ui.label(tl!("This document is trying to open a local file:"));
        egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
            // A local path is file/path chrome: left to right, so its final component stays last.
            let mut job = egui::text::LayoutJob::simple(
                pending.path.to_string_lossy().into_owned(),
                egui::FontId::monospace(13.0),
                crate::theme::Tokens::get(ui.ctx()).text,
                ui.available_width().max(0.0),
            );
            job.direction = egui::epaint::text::TextDirection::LeftToRight;
            let galley = ui.painter().layout_job(job);
            ui.add(egui::Label::new(egui::WidgetText::Galley(galley)).selectable(true));
        });
        ui.add_space(8.0);
        ui.label(tl!("Only allow this if you trust the source document."));
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Cancel"), true).clicked() {
                choice = Some(false);
            }
            if widgets::pill_button(ui, tl!("Allow"), false).clicked() {
                choice = Some(true);
            }
        });
    });
    if choice.is_none() && modal.should_close() {
        choice = Some(false);
    }
    if let Some(open) = choice {
        app.resolve_pending_remote_link(open);
    }
}

fn yes(b: bool) -> String {
    tl!(if b { "Yes" } else { "No" }).to_string()
}

/// What a password box was answered with.
enum PasswordAnswer {
    Submit,
    Cancel,
}

/// A password box: a lock, a title, a message, the masked field, an error and two buttons.
fn password_modal(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    message: String,
    input: &mut String,
    error: Option<&str>,
    ok: &str,
) -> Option<PasswordAnswer> {
    let t = Tokens::get(ctx);
    let mut answer = None;
    let modal = widgets::show_modal(ctx, id, |ui| {
        ui.set_width(400.0);
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("lock", 22.0, t.accent));
            ui.label(egui::RichText::new(title).font(theme::semibold(17.0)));
        });
        ui.add_space(6.0);
        ui.add(egui::Label::new(message).wrap());
        ui.add_space(8.0);
        let r = ui.add(widgets::line(input).password(true).hint_text(tl!("Password")).desired_width(f32::INFINITY));
        // Enter submits. The field keeps focus, so check the key while it is focused as well as
        // on the frame focus is lost.
        if (r.has_focus() || r.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            answer = Some(PasswordAnswer::Submit);
        }
        // Only when it lost focus: each request interrupts the input method, and on Linux typed
        // keys go through it, so asking on every frame lost everything typed (#2818).
        if !r.has_focus() {
            r.request_focus();
        }
        if let Some(e) = error {
            ui.add_space(4.0);
            // Known error text is translated; anything else passes through.
            ui.label(egui::RichText::new(tl!(e)).color(egui::Color32::from_rgb(0xD1, 0x3B, 0x3B)));
        }
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, ok, true).clicked() {
                answer = Some(PasswordAnswer::Submit);
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                answer = Some(PasswordAnswer::Cancel);
            }
        });
    });
    answer.or(modal.should_close().then_some(PasswordAnswer::Cancel))
}

/// Password prompt for encrypted documents (Acrobat: "Password" dialog on open).
fn password(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(prompt) = app.password_prompt.as_mut() else { return };
    let message = crate::i18n::fmt(tl!("“{name}” is protected. Enter a password to open it."), &[("name", &prompt.name)]);
    match password_modal(ctx, "password", tl!("Password required"), message, &mut prompt.input, prompt.error.as_deref(), tl!("Open")) {
        Some(PasswordAnswer::Submit) => {
            let pw = prompt.input.clone();
            app.submit_password(Some(pw));
        }
        Some(PasswordAnswer::Cancel) => app.submit_password(None),
        None => {}
    }
}

/// Permissions password prompt: lifts the restrictions of an open document (#995).
fn unlock(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(prompt) = app.unlock_prompt.as_mut() else { return };
    let message =
        crate::i18n::fmt(tl!("“{name}” has security restrictions. Enter its permissions password to lift them."), &[("name", &prompt.name)]);
    match password_modal(ctx, "unlock", tl!("Permissions password"), message, &mut prompt.input, prompt.error.as_deref(), tl!("Unlock")) {
        Some(PasswordAnswer::Submit) => {
            let pw = prompt.input.clone();
            app.submit_unlock(Some(pw));
        }
        Some(PasswordAnswer::Cancel) => app.submit_unlock(None),
        None => {}
    }
}

#[cfg(test)]
mod prompt_button_tests {
    use super::PromptButton::{self, Cancel, DontSave, Save};

    #[test]
    fn opens_on_save_and_arrows_stop_at_the_ends() {
        assert_eq!(PromptButton::default(), Save);
        assert_eq!(Save.step(1), Save);
        assert_eq!(Save.step(-1), Cancel);
        assert_eq!(Cancel.step(-1), DontSave);
        assert_eq!(DontSave.step(-1), DontSave);
        assert_eq!(DontSave.step(1000), Save, "a long step clamps");
        assert_eq!(Save.step(isize::MIN), DontSave, "and so does a huge negative one");
    }

    #[test]
    fn tab_wraps_both_ways() {
        assert_eq!(Save.cycle(false), DontSave);
        assert_eq!(DontSave.cycle(true), Save);
        assert_eq!(Cancel.cycle(false), Save);
        assert_eq!(Cancel.cycle(true), DontSave);
    }

    #[test]
    fn each_button_answers_as_labelled() {
        assert_eq!(Save.answer(), Some(true));
        assert_eq!(DontSave.answer(), Some(false));
        assert_eq!(Cancel.answer(), None);
    }
}
