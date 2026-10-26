use super::bookmarks::{BmAction, OutlineCtx, outline_item, outline_matches};
use super::pages::{page_tools, pages, thumbnail_size};
use super::{
    Align, Align2, Color32, DocInfo, FieldKind, Layout, Nav, PdfCraftApp, Rect, RightPanel, Sense, Stroke, Tokens, human_size, icons, theme, vec2,
};

/// Draw the selected right-hand panel and apply its navigation, layer, bookmark or edit actions.
pub fn right_panel(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let Some((index, id)) = app.active_ids() else { return };
    let Some(panel) = app.right else { return };
    let mut nav: Option<Nav> = None;
    let mut close = false;
    let mut toggle_layer: Option<(usize, bool)> = None;
    let mut reset_layers = false;
    let mut attachment_action: Option<(usize, bool)> = None; // (index, open instead of save)
    let mut bm_action: Option<BmAction> = None;
    let mut bm_expand: Option<usize> = None;
    let mut panel_edit: Option<pdfcraft_engine::Edit> = None;
    let mut field_properties = None;
    let mut panel_command: Option<&'static str> = None;
    let mut sig_action: Option<crate::sign_ui::PanelAction> = None;
    let mut a11y_action: Option<crate::a11y_ui::PanelAction> = None;
    let mut compare_action: Option<crate::compare_ui::PanelAction> = None;
    let mut bm_rename = app.bookmark_rename.clone();
    let bm_editable = app.session.get(id).is_some_and(|d| d.allows_assembly() && d.read_only_reason.is_none());
    // Prepare a form is open: the Fields panel orders tabs.
    let preparing = app.is_preparing();
    {
        let Some(doc) = app.session.get(id) else { return };
        let info = &doc.info;
        let view = &mut app.views[index];
        let prefs = &app.comment_prefs;
        let sig_expanded = &mut app.sig_expanded;
        let a11y = &mut app.a11y;
        let tags_state = &mut app.tags_state;
        let compare = &app.compare;
        let comment_allowed = doc.allows_annotation();
        // A dialog or the palette owns the keyboard: the panel leaves Escape to it.
        let modal = app.dialog.is_some() || app.palette_open;
        egui::Frame::NONE
            .fill(t.panel)
            .inner_margin(egui::Margin { left: 14, right: 12, top: 12, bottom: 8 })
            .stroke(Stroke::new(1.0, t.divider))
            .show(ui, |ui| {
                let (title, count) = match panel {
                    RightPanel::Comments => ("Comments", Some(crate::comments_panel::count(info))),
                    RightPanel::Bookmarks => ("Bookmarks", None),
                    RightPanel::Pages => ("Pages", Some(info.pages.len())),
                    RightPanel::Fields => ("Fields", Some(info.fields.len())),
                    RightPanel::Layers => ("Layers", Some(info.layers.len())),
                    RightPanel::Attachments => ("Attachments", Some(info.attachments.len())),
                    RightPanel::Signatures => ("Signatures", Some(doc.signatures.iter().filter(|s| s.signed).count())),
                    RightPanel::Accessibility => ("Accessibility Checker", None),
                    RightPanel::Tags => ("Tags", None),
                    RightPanel::Search => ("Search", None),
                    RightPanel::Compare => ("Compare", compare.as_ref().filter(|c| c.new == id).map(|c| c.result.changes.len())),
                };
                // Comments count like Acrobat (no replies); the replies go in small text below.
                let replies = if panel == RightPanel::Comments { crate::comments_panel::reply_count(info) } else { 0 };
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(tl!(title)).font(theme::semibold(15.5)));
                            if let Some(c) = count {
                                ui.label(egui::RichText::new(c.to_string()).font(theme::medium(13.0)).color(t.text_faint));
                            }
                        });
                        if replies > 0 {
                            let text = crate::i18n::trn(crate::i18n::current(), replies as u64, "{n} reply", "{n} replies");
                            ui.label(egui::RichText::new(text).font(theme::regular(11.0)).color(t.text_faint));
                        }
                    });
                    if panel == RightPanel::Pages && view.selected.len() > 1 {
                        let n = view.selected.len().to_string();
                        let picked = crate::i18n::fmt(tl!("{n} pages selected"), &[("n", &n)]);
                        ui.label(egui::RichText::new(picked).font(theme::medium(12.0)).color(t.accent_text));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icons::button(ui, "x", 26.0, false, tl!("Close")).clicked() {
                            close = true;
                        }
                        if panel == RightPanel::Pages
                            && let Some(c) = page_tools(ui, !modal && view.panel_drag.is_none() && bm_editable)
                        {
                            panel_command = Some(c);
                        }
                        // Right to left: close, "…", filter, search (Acrobat's order left to right).
                        if panel == RightPanel::Comments {
                            panel_command = crate::comments_panel::header_controls(ui, info, view, doc.comments_hidden());
                        }
                        if panel == RightPanel::Comments
                            && icons::button(ui, "search", 26.0, view.comments.search.is_some(), tl!("Search comments")).clicked()
                        {
                            view.comments.search = match view.comments.search {
                                Some(_) => None,
                                None => Some(String::new()),
                            };
                            view.comments.search_focus = true;
                        }
                        if panel == RightPanel::Bookmarks && (bm_editable || !info.outline.is_empty()) {
                            let more = icons::button(ui, "ellipsis", 26.0, false, tl!("Bookmark options"));
                            egui::Popup::menu(&more).show(|ui| {
                                ui.set_min_width(200.0);
                                if bm_editable && ui.button(tl!("New bookmarks from structure")).clicked() {
                                    bm_action = Some(BmAction::FromStructure);
                                    ui.close();
                                }
                                if bm_editable && ui.button(tl!("New bookmarks from text styles…")).clicked() {
                                    bm_action = Some(BmAction::FromTextStyles);
                                    ui.close();
                                }
                                if !info.outline.is_empty() {
                                    for (levels, label) in [
                                        (usize::MAX, tl!("Expand all bookmarks")),
                                        (1, tl!("Expand top-level bookmarks")),
                                        (0, tl!("Collapse all bookmarks")),
                                    ] {
                                        if ui.button(label).clicked() {
                                            bm_expand = Some(levels);
                                            ui.close();
                                        }
                                    }
                                }
                            });
                        }
                        if panel == RightPanel::Layers && !info.layers.is_empty() {
                            let more = icons::button(ui, "ellipsis", 26.0, false, tl!("Layer options"));
                            egui::Popup::menu(&more).show(|ui| {
                                ui.set_min_width(200.0);
                                if ui.button(tl!("Reset to Initial Visibility")).clicked() {
                                    reset_layers = true;
                                    ui.close();
                                }
                            });
                        }
                        let new_tip = crate::commands::command_tip(ui.ctx(), tl!("New bookmark ({key})"), "bookmark.add");
                        if panel == RightPanel::Bookmarks && bm_editable && icons::button(ui, "bookmark-plus", 26.0, false, &new_tip).clicked() {
                            bm_action = Some(BmAction::New);
                        }
                    });
                });
                ui.add_space(6.0);
                let search_id = egui::Id::unique(("bookmark_search", id.0));
                let mut bookmark_query = ui.data(|d| d.get_temp::<String>(search_id)).unwrap_or_default();
                if panel == RightPanel::Bookmarks && !info.outline.is_empty() {
                    ui.horizontal(|ui| {
                        const CLEAR: f32 = 26.0;
                        let label = ui.label(tl!("Search"));
                        let previous = bookmark_query.clone();
                        // The panel keeps whatever width its content used, so this row must fit
                        // exactly: any overflow would widen the panel again on every repaint.
                        let width = ui.available_width() - CLEAR - ui.spacing().item_spacing.x;
                        let response = ui.add(egui::TextEdit::singleline(&mut bookmark_query).desired_width(width)).labelled_by(label.id);
                        response.widget_info(|| {
                            let mut info = egui::WidgetInfo::text_edit(ui.is_enabled(), &previous, &bookmark_query, "");
                            info.label = Some(tl!("Search").to_string());
                            info
                        });
                        if icons::button(ui, "x", CLEAR, false, tl!("Clear")).clicked() {
                            bookmark_query.clear();
                        }
                    });
                    ui.data_mut(|d| d.insert_temp(search_id, bookmark_query.clone()));
                    ui.add_space(6.0);
                }
                let bookmark_query = pdfcraft_render::text::fold_case(bookmark_query.trim());
                let mut bookmark_matches =
                    if panel == RightPanel::Bookmarks { outline_matches(&info.outline, &bookmark_query) } else { std::collections::HashSet::new() };
                // A new bookmark must stay nameable even when its initial title doesn't match.
                if let Some((path, _)) = &bm_rename {
                    let mut ancestor = path.clone();
                    while !ancestor.is_empty() {
                        bookmark_matches.insert(ancestor.clone());
                        ancestor.pop();
                    }
                }
                if panel == RightPanel::Pages {
                    let enabled = !modal && view.panel_drag.is_none();
                    thumbnail_size(ui, &t, view, enabled);
                }
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match panel {
                    RightPanel::Comments => {
                        if let Some(e) = crate::comments_panel::show(ui, &t, doc, view, prefs, comment_allowed, &mut nav) {
                            panel_edit = Some(e);
                        }
                    }
                    RightPanel::Bookmarks => {
                        if info.outline.is_empty() {
                            empty(ui, &t, "bookmark", "This document has no bookmarks.");
                        }
                        let mut ctx = OutlineCtx {
                            nav: &mut nav,
                            action: &mut bm_action,
                            rename: &mut bm_rename,
                            editable: bm_editable,
                            current: view.current,
                            expand: bm_expand,
                            matches: &bookmark_matches,
                            searching: !bookmark_query.is_empty(),
                        };
                        if ctx.searching && bookmark_matches.is_empty() && !info.outline.is_empty() {
                            ui.label(tl!("No matches."));
                        }
                        for (i, item) in info.outline.iter().enumerate() {
                            outline_item(ui, &t, info, item, &[i], info.outline.len(), &mut ctx);
                        }
                        if info.outline_more {
                            ui.label(tl!("Some bookmarks are not shown here: the panel lists a limited number, nested a limited depth."));
                        }
                    }
                    RightPanel::Pages => pages(ui, &t, info, view, modal, bm_editable, &mut nav),
                    RightPanel::Fields => {
                        field_properties = fields(
                            ui,
                            FieldsPanel {
                                theme: &t,
                                info,
                                form: doc.form.as_slice(),
                                preparing,
                                selection: &mut view.prepare,
                                nav: &mut nav,
                                edit: &mut panel_edit,
                            },
                        );
                    }
                    RightPanel::Layers => {
                        if info.layers.is_empty() {
                            empty(ui, &t, "layers", "This document has no layers.");
                        }
                        for (li, l) in info.layers.iter().enumerate() {
                            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, l.visible, &l.name));
                            let press = theme::Press::track(ui, &resp);
                            press.wash(ui, rect, 6, false);
                            let body = rect.translate(press.offset());
                            icons::paint(
                                ui,
                                Rect::from_min_size(body.min + vec2(4.0, 7.0), vec2(18.0, 18.0)),
                                if l.visible { "eye" } else { "eye-off" },
                                16.0,
                                t.icon,
                            );
                            let fg = if l.visible { t.text } else { t.text_faint };
                            crate::widgets::paint_left(
                                ui,
                                body.left_center() + vec2(32.0, 0.0),
                                &l.name,
                                theme::regular(13.0),
                                fg,
                                (rect.width() - 40.0).max(0.0),
                            );
                            if theme::hand(resp).on_hover_text(if l.visible { tl!("Hide layer") } else { tl!("Show layer") }).clicked() {
                                toggle_layer = Some((li, !l.visible));
                            }
                        }
                    }
                    RightPanel::Signatures => {
                        sig_action = crate::sign_ui::panel(ui, &t, &doc.signatures, sig_expanded);
                    }
                    RightPanel::Accessibility => {
                        a11y_action = crate::a11y_ui::panel(ui, &t, a11y, id);
                    }
                    RightPanel::Tags => {
                        let tags = tags_state.tags(doc);
                        let content = tags_state.content(doc);
                        let (page, edit) = crate::tags_ui::panel(ui, &t, id, &tags, &content, tags_state, doc.allows_modification());
                        if let Some(page) = page {
                            nav = Some(Nav::Page(page));
                        }
                        panel_edit = edit;
                    }
                    RightPanel::Search => crate::search_ui::panel(ui, &t, view, info.pages.len()),
                    RightPanel::Compare => compare_action = crate::compare_ui::panel(ui, &t, compare, id),
                    RightPanel::Attachments => {
                        let attachable = doc.allows_modification() && doc.read_only_reason.is_none();
                        if attachable {
                            egui::Frame::NONE.inner_margin(egui::Margin::symmetric(4, 6)).show(ui, |ui| {
                                if ui.add(egui::Button::new(tl!("Attach a file…"))).clicked() {
                                    panel_command = Some("edit.attach");
                                }
                            });
                        }
                        if info.attachments.is_empty() {
                            empty(ui, &t, "paperclip", "This document has no attachments.");
                        }
                        for (ai, a) in info.attachments.iter().enumerate() {
                            let is_pdf = a.name.to_lowercase().ends_with(".pdf");
                            // Only an attachment of the document itself can be deleted here.
                            let deletable = match &a.source {
                                pdfcraft_render::AttachmentSource::Document { key } if attachable => Some(key),
                                _ => None,
                            };
                            // The whole row answers a double-click and a right-click (#3466), below
                            // its buttons, which still take their own clicks.
                            let mut buttons: Vec<egui::Response> = Vec::new();
                            let row = ui.scope_builder(egui::UiBuilder::new().sense(Sense::click()), |ui| {
                                egui::Frame::NONE.inner_margin(egui::Margin::symmetric(4, 6)).show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.add(icons::image("paperclip", 16.0, t.icon));
                                        ui.vertical(|ui| {
                                            ui.set_width(ui.available_width() - 70.0);
                                            // Not selectable, so a click on the name or the details reaches the row.
                                            ui.add(
                                                egui::Label::new(egui::RichText::new(&a.name).font(theme::medium(13.0))).truncate().selectable(false),
                                            );
                                            let mut meta = a.size.map(human_size).unwrap_or_default();
                                            if let pdfcraft_render::AttachmentSource::Annotation { page, .. } = a.source {
                                                let on_page = crate::i18n::fmt(
                                                    tl!("on page {label}"),
                                                    &[("label", info.pages.get(page).map(|p| p.label.as_str()).unwrap_or("?"))],
                                                );
                                                meta = format!("{meta}  ·  {on_page}");
                                            }
                                            if let Some(d) = &a.description {
                                                meta = format!("{meta}  ·  {d}");
                                            }
                                            ui.add(
                                                egui::Label::new(egui::RichText::new(meta).color(t.text_faint).small()).truncate().selectable(false),
                                            );
                                        });
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            let save = icons::button(ui, "file-down", 28.0, false, tl!("Save attachment…"));
                                            if save.clicked() {
                                                attachment_action = Some((ai, false));
                                            }
                                            buttons.push(save);
                                            if is_pdf {
                                                let open = icons::button(ui, "file-input", 28.0, false, tl!("Open in a new tab"));
                                                if open.clicked() {
                                                    attachment_action = Some((ai, true));
                                                }
                                                buttons.push(open);
                                            }
                                            if let Some(key) = deletable {
                                                let delete = icons::button(ui, "trash-2", 28.0, false, tl!("Delete attachment"));
                                                if delete.clicked() {
                                                    panel_edit = Some(pdfcraft_engine::Edit::DeleteAttachment { key: key.clone() });
                                                }
                                                buttons.push(delete);
                                            }
                                        });
                                    });
                                })
                            });
                            // A double-click opens a PDF in a new tab, as Acrobat opens an attachment;
                            // any other file is saved, since PdfCraft opens only PDFs.
                            if row.response.double_clicked() {
                                attachment_action = Some((ai, is_pdf));
                            }
                            // A right-click on one of the row's buttons is a right-click on the row:
                            // the union keeps the row's id, so it is one menu either way.
                            let menu = buttons.into_iter().fold(row.response.clone(), |row, button| row | button);
                            menu.context_menu(|ui| {
                                if is_pdf && ui.button(tl!("Open in a new tab")).clicked() {
                                    attachment_action = Some((ai, true));
                                    ui.close();
                                }
                                if ui.button(tl!("Save attachment…")).clicked() {
                                    attachment_action = Some((ai, false));
                                    ui.close();
                                }
                                if let Some(key) = deletable
                                    && ui.button(tl!("Delete attachment")).clicked()
                                {
                                    panel_edit = Some(pdfcraft_engine::Edit::DeleteAttachment { key: key.clone() });
                                    ui.close();
                                }
                            });
                        }
                    }
                });
            });
    }
    if close {
        app.choose_right_panel(None);
    }
    if let Some(e) = panel_edit {
        app.apply_edit(e);
    }
    if let Some((name, widget)) = field_properties {
        app.open_field_props(&name, widget);
    }
    if let Some(c) = panel_command {
        app.run_command(c);
    }
    if let Some(a) = a11y_action {
        app.a11y_action(index, a);
    }
    if let Some(a) = compare_action {
        app.compare_action(index, a);
    }
    match sig_action {
        Some(crate::sign_ui::PanelAction::Validate) => app.run_command("sign.validate"),
        Some(crate::sign_ui::PanelAction::GoTo(p)) => app.views[index].go_to_page(p),
        Some(crate::sign_ui::PanelAction::Trust(c)) => app.trust_certificate(*c),
        #[cfg(not(target_arch = "wasm32"))]
        Some(crate::sign_ui::PanelAction::Import) => app.pick_certificates_to_import(),
        #[cfg(target_arch = "wasm32")]
        Some(crate::sign_ui::PanelAction::Import) => {}
        Some(crate::sign_ui::PanelAction::ViewCertificate(chain)) => {
            app.cert_viewer = Some(crate::sign_ui::CertViewer { chain, selected: 0, tab: crate::sign_ui::CertTab::Summary });
            app.dialog = Some(crate::Dialog::CertificateViewer);
        }
        Some(crate::sign_ui::PanelAction::ViewSigned { len, revision }) => app.view_signed_version(len, revision),
        Some(crate::sign_ui::PanelAction::Sign(field)) => {
            let page = app.views[index].current;
            app.start_signing(page, None, Some(field), None);
        }
        Some(crate::sign_ui::PanelAction::ExportCertificate(c)) => {
            app.export_certificate(&c);
        }
        None => {}
    }
    if let Some((p, i)) = app.views.get_mut(index).and_then(|v| v.comments.default_request.take()) {
        app.make_comment_default(p, i);
    }
    if let Some((p, i, open)) = app.views.get_mut(index).and_then(|v| v.comments.attachment_request.take()) {
        app.comment_attachment_action(id, p, i, open);
    }
    if let Some((p, i)) = app.views.get_mut(index).and_then(|v| v.comments.props_request.take()) {
        app.open_comment_props(p, i);
    }
    app.bookmark_rename = bm_rename;
    if let Some(a) = bm_action {
        app.bookmark_action(a);
    }
    if let Some((ai, open)) = attachment_action {
        app.attachment_action(id, ai, open);
    }
    if let Some((layer, visible)) = toggle_layer
        && app.session.set_layer_visible(id, layer, visible)
    {
        app.views[index].invalidate_content();
    }
    if reset_layers && app.session.reset_layers(id) {
        app.views[index].invalidate_content();
    }
    match nav {
        Some(Nav::Page(p)) => app.views[index].go_to_page(p),
        Some(Nav::Dest(p, dest)) => match app.session.get(id) {
            Some(doc) => app.views[index].go_to_dest(p, dest, &doc.info),
            None => app.views[index].go_to_page(p),
        },
        Some(Nav::Flash(p, r)) => {
            let v = &mut app.views[index];
            if let Some(doc) = app.session.get(id) {
                v.reveal_rect(p, r, &doc.info);
            } else {
                v.go_to_page(p);
            }
            v.flash = Some((p, r, 0.0));
        }
        None => {}
    }
}

fn empty(ui: &mut egui::Ui, t: &Tokens, icon: &str, text: &str) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.add(icons::image(icon, 36.0, t.text_faint));
        ui.add_space(8.0);
        // Every empty-panel message goes through here.
        ui.label(egui::RichText::new(tl!(text)).color(t.text_muted));
    });
}

/// The Fields panel: fields by page, in tab order. While preparing a form, each field can move
/// earlier or later in its page's tab order (Acrobat: Order Tabs Manually).
struct FieldsPanel<'a> {
    theme: &'a Tokens,
    info: &'a DocInfo,
    form: &'a [pdfcraft_engine::FormField],
    preparing: bool,
    selection: &'a mut crate::prepare::PrepareView,
    nav: &'a mut Option<Nav>,
    edit: &'a mut Option<pdfcraft_engine::Edit>,
}

fn fields(ui: &mut egui::Ui, panel: FieldsPanel<'_>) -> Option<(String, usize)> {
    let FieldsPanel { theme: t, info, form, preparing, selection, nav, edit } = panel;
    let mut properties = None;
    if info.fields.is_empty() {
        empty(ui, t, "text-cursor-input", "This document has no form fields.");
        return None;
    }
    let rows = field_rows(&info.fields, form);
    if preparing {
        ui.label(egui::RichText::new(tl!("Tab order: move a field with its arrows.")).small().color(t.text_muted));
        ui.label(egui::RichText::new(tl!("Shift- or Ctrl/Command-click to select several fields.")).small().color(t.text_muted));
        let names = selection.names();
        // Keep the field rows in place when the first click selects a field, so the
        // second click of a double-click still lands on the same row.
        ui.horizontal(|ui| {
            ui.label(crate::i18n::fmt(tl!("{n} selected"), &[("n", &names.len().to_string())]));
            if ui.add_enabled(!names.is_empty(), egui::Button::new(tl!("Properties…"))).clicked() {
                properties = selection.selected.clone();
            }
        });
    }
    for (p, page_rows) in rows {
        // A field's /P can name a page the document doesn't have (#1307); it reads as unplaced.
        let label = p
            .and_then(|p| info.pages.get(p))
            .map(|page| crate::i18n::fmt(tl!("Page {label}"), &[("label", &page.label)]))
            .unwrap_or_else(|| tl!("Unplaced").to_string());
        ui.add_space(4.0);
        ui.label(egui::RichText::new(label).font(theme::semibold(12.5)).color(t.text_muted));
        for (f, key) in page_rows {
            let selected = key.as_ref().is_some_and(|key| selection.selected.as_ref() == Some(key) || selection.also.contains(key));
            let icon = match f.kind {
                FieldKind::Text => "text-cursor-input",
                FieldKind::CheckBox => "check-circle-2",
                FieldKind::Radio => "circle",
                FieldKind::PushButton => "square",
                FieldKind::Combo => "chevron-down",
                FieldKind::List => "list",
                FieldKind::Signature => "signature",
                FieldKind::Unknown => "square",
            };
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, &f.name));
            // A row scrolled out of view still takes clicks and keyboard focus, but isn't drawn:
            // laying out every row of a large form each frame took a tenth of a second a frame.
            let visible = ui.is_rect_visible(rect);
            if visible {
                let press = theme::Press::track(ui, &resp);
                press.wash(ui, rect, 6, selected);
                let body = rect.translate(press.offset());
                icons::paint(ui, Rect::from_min_size(body.min + vec2(8.0, 7.0), vec2(16.0, 16.0)), icon, 15.0, Color32::from_rgb(0x8E, 0x4E, 0xE6));
                let value =
                    f.value.as_ref().map(|v| if v.chars().count() > 18 { format!("{}…", v.chars().take(17).collect::<String>()) } else { v.clone() });
                let value_font = theme::regular(11.5);
                let value_w = value.as_ref().map(|v| crate::widgets::text_width(ui, v, &value_font)).unwrap_or(0.0);
                let arrows = if preparing && f.page.is_some() { 56.0 } else { 0.0 };
                let max_w = (rect.width() - 32.0 - value_w - arrows - 16.0).max(0.0);
                crate::widgets::paint_left(ui, body.left_center() + vec2(32.0, 0.0), &f.name, theme::regular(13.0), t.text, max_w);
                if let Some(v) = value {
                    let x = if arrows > 0.0 { 64.0 } else { 8.0 };
                    ui.painter().text(body.right_center() - vec2(x, 0.0), Align2::RIGHT_CENTER, v, value_font, t.text_faint);
                }
            }
            if preparing && f.page.is_some() {
                let up = Rect::from_center_size(rect.right_center() - vec2(44.0, 0.0), vec2(22.0, 22.0));
                let down = Rect::from_center_size(rect.right_center() - vec2(20.0, 0.0), vec2(22.0, 22.0));
                for (r, icon, earlier, tip) in
                    [(up, "chevron-up", true, tl!("Earlier in tab order")), (down, "chevron-down", false, tl!("Later in tab order"))]
                {
                    let b = ui.interact(r, ui.scope_id().with((&f.name, earlier)), Sense::click());
                    b.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{tip}: {}", f.name)));
                    if visible {
                        let bpress = theme::Press::track(ui, &b);
                        bpress.wash(ui, r, 4, false);
                        icons::paint(ui, r.shrink(3.0).translate(bpress.offset()), icon, 15.0, t.icon);
                    }
                    if theme::hand(b).on_hover_text(tip).clicked() {
                        *edit = Some(pdfcraft_engine::Edit::MoveInTabOrder { name: f.name.clone(), earlier });
                    }
                }
            }
            if preparing && resp.double_clicked() && !ui.input(|i| i.modifiers.shift || i.modifiers.command) {
                if let Some(key) = key {
                    if !selected {
                        selection.select(key.clone(), false);
                    }
                    properties = Some(key);
                }
            } else if preparing
                && resp.clicked()
                && let Some(key) = key
            {
                selection.select(key, ui.input(|i| i.modifiers.shift || i.modifiers.command));
            }
            let resp = theme::hand(resp);
            // Only a row in view can be hovered: its tip isn't built for the others.
            let resp = if visible { resp.on_hover_text(field_tip(f)) } else { resp };
            if resp.clicked()
                && let (Some(p), Some(r)) = (f.page, f.rect)
            {
                *nav = Some(Nav::Flash(p, r));
            }
        }
    }
    properties
}

/// A Fields panel row's tooltip: the field's kind, its own tooltip and whether it has scripts.
fn field_tip(f: &pdfcraft_render::Field) -> String {
    let kind = format!("{:?}", f.kind);
    let mut tip = crate::i18n::fmt(tl!("{kind} field"), &[("kind", tl!(&kind))]);
    if let Some(tt) = &f.tooltip {
        tip.push_str(&format!(" — {tt}"));
    }
    if f.has_actions {
        tip.push_str(&format!("\n{}", tl!("Has JavaScript actions (run in M6)")));
    }
    tip
}

/// One page's rows of the Fields panel: each field, with the form field and widget a click on
/// it selects (`None` when the form has no widget of it on that page).
type FieldRows<'a> = Vec<(&'a pdfcraft_render::Field, Option<(String, usize)>)>;

/// The Fields panel's rows: by page ("Unplaced" first), each page's in tab order. A row is paired
/// with the first form field of its name, as the engine pairs them: one map, built once a frame.
/// Searching the form for each row, and again inside the sort, was O(fields²) every frame.
fn field_rows<'a>(fields: &'a [pdfcraft_render::Field], form: &[pdfcraft_engine::FormField]) -> Vec<(Option<usize>, FieldRows<'a>)> {
    let mut by_name: std::collections::HashMap<&str, &pdfcraft_engine::FormField> = std::collections::HashMap::with_capacity(form.len());
    for field in form {
        by_name.entry(field.name.as_str()).or_insert(field);
    }
    let rank = |name: &str| by_name.get(name).and_then(|f| f.widgets.iter().map(|w| w.tab).min()).unwrap_or(usize::MAX);
    let mut ordered: Vec<&pdfcraft_render::Field> = fields.iter().collect();
    ordered.sort_by_cached_key(|f| (f.page, rank(&f.name)));
    let mut rows: Vec<(Option<usize>, FieldRows<'a>)> = Vec::new();
    for f in ordered {
        let key =
            by_name.get(f.name.as_str()).and_then(|field| field.widgets.iter().position(|w| w.page == f.page).map(|wi| (field.name.clone(), wi)));
        match rows.last_mut() {
            Some((page, page_rows)) if *page == f.page => page_rows.push((f, key)),
            _ => rows.push((f.page, vec![(f, key)])),
        }
    }
    rows
}

#[cfg(test)]
mod field_rows {
    use super::field_rows;
    use std::sync::Arc;

    /// A PDF of `objects` (numbered from 1; object 1 the catalog) with a valid xref table.
    fn pdf(objects: &[String]) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for o in offsets {
            out.extend(format!("{o:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
        out
    }

    /// The fields the engine reads from `bytes`: the inspector's list and the form.
    fn open(bytes: Vec<u8>) -> (Vec<pdfcraft_render::Field>, Arc<Vec<pdfcraft_engine::FormField>>) {
        let mut session = pdfcraft_engine::Session::new();
        let id = session.open("form.pdf", None, Arc::new(bytes), None).expect("opens");
        let doc = session.get(id).expect("open");
        (doc.info.fields.clone(), doc.form.clone())
    }

    /// Rows go by page, the unplaced first, each page's in tab order (here its annotation order,
    /// not the `/Fields` order), and a row selects the first form field of its name.
    #[test]
    fn rows_follow_pages_and_tab_order() {
        let widget = |name: &str, page: Option<usize>| match page {
            Some(p) => format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /Rect [10 10 90 30] /P {} 0 R >>", 3 + p),
            None => format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /Rect [10 10 90 30] >>"),
        };
        let bytes = pdf(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 7 0 R 8 0 R 9 0 R 10 0 R 6 0 R] >> >>".into(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [6 0 R 5 0 R] >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [7 0 R 8 0 R 9 0 R] >>".into(),
            widget("a1", Some(0)),
            widget("a2", Some(0)),
            widget("b", Some(1)),
            widget("twin", Some(1)),
            widget("twin", Some(1)),
            widget("loose", None),
        ]);
        let (fields, form) = open(bytes);
        // A page's rows as (field name, the form field and widget a click selects).
        type Named<'a> = Vec<(&'a str, Option<(String, usize)>)>;
        let rows: Vec<(Option<usize>, Named)> = field_rows(&fields, &form)
            .into_iter()
            .map(|(page, rows)| (page, rows.into_iter().map(|(f, key)| (f.name.as_str(), key)).collect()))
            .collect();
        let key = |name: &str| Some((name.to_string(), 0));
        assert_eq!(
            rows,
            [
                (None, vec![("loose", key("loose"))]),
                (Some(0), vec![("a2", key("a2")), ("a1", key("a1"))]),
                (Some(1), vec![("b", key("b")), ("twin", key("twin")), ("twin", key("twin"))]),
            ]
        );
    }

    /// The panel's rows for a large form take milliseconds: looking each row's field up in the
    /// form, there and inside the sort, was O(fields²) every frame.
    #[test]
    fn rows_for_a_large_form_are_quick() {
        const PAGES: usize = 150;
        const PER_PAGE: usize = 100;
        let mut objects = vec![String::new(), String::new()];
        let page = |p: usize| 3 + p;
        let field = |p: usize, k: usize| 3 + PAGES + p * PER_PAGE + k;
        let mut all = Vec::new();
        for p in 0..PAGES {
            let annots: Vec<String> = (0..PER_PAGE).map(|k| format!("{} 0 R", field(p, k))).collect();
            all.extend(annots.iter().cloned());
            objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [{}] >>", annots.join(" ")));
        }
        for p in 0..PAGES {
            for k in 0..PER_PAGE {
                objects.push(format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (field{p}_{k}) /Rect [10 10 90 30] /P {} 0 R >>", page(p)));
            }
        }
        // `/Fields` in no particular order, so the rows must be sorted.
        let mut seed = 12345u64;
        for i in (1..all.len()).rev() {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            all.swap(i, (seed >> 33) as usize % (i + 1));
        }
        objects[0] = format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [{}] >> >>", all.join(" "));
        let kids: Vec<String> = (0..PAGES).map(|p| format!("{} 0 R", page(p))).collect();
        objects[1] = format!("<< /Type /Pages /Kids [{}] /Count {PAGES} >>", kids.join(" "));
        let (fields, form) = open(pdf(&objects));
        assert_eq!(fields.len(), PAGES * PER_PAGE);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rows = field_rows(&fields, &form);
            let _ = tx.send((rows.len(), rows.iter().map(|(_, r)| r.len()).sum::<usize>()));
        });
        let counted = rx.recv_timeout(std::time::Duration::from_secs(3)).expect("the Fields panel's rows must not take seconds a frame");
        assert_eq!(counted, (PAGES, PAGES * PER_PAGE));
    }
}
