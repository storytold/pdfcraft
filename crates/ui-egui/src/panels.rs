//! Left tool panel (All tools + tool sub-panels, generated from the engine catalogue) and the
//! right-hand panels (Comments, Bookmarks, Pages, Fields, Layers, Attachments).

use egui::{Align, Align2, Color32, CornerRadius, Layout, Rect, Sense, Stroke, pos2, vec2};
use pdfcraft_engine::catalog::{self, Availability, TOOL_GROUPS, ToolGroup};
use pdfcraft_render::{DocInfo, FieldKind, OutlineItem};

use crate::theme::{self, Tokens};
use crate::{LeftPanel, PdfCraftApp, RightPanel, icons, widgets};

const COLLAPSED_TOOLS: usize = 14;

fn hue(g: &ToolGroup) -> Color32 {
    Color32::from_rgb(g.hue[0], g.hue[1], g.hue[2])
}

pub fn left_panel(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::left("tool_panel")
        .resizable(false)
        .exact_size(272.0)
        .frame(
            egui::Frame::NONE
                .fill(t.panel)
                .inner_margin(egui::Margin { left: 14, right: 10, top: 12, bottom: 10 })
                .stroke(Stroke::new(1.0, t.divider)),
        )
        .show(ui, |ui| match app.left {
            LeftPanel::AllTools => all_tools(app, ui, &t),
            LeftPanel::Tool(id) => match catalog::group(id) {
                Some(g) => tool_detail(app, ui, &t, g),
                None => app.left = LeftPanel::AllTools,
            },
        });
}

fn panel_header(ui: &mut egui::Ui, t: &Tokens, title: &str, back: bool) -> (bool, bool) {
    let mut go_back = false;
    let mut close = false;
    ui.horizontal(|ui| {
        if back && icons::button(ui, "chevron-left", 26.0, false, "Back to all tools").clicked() {
            go_back = true;
        }
        ui.label(egui::RichText::new(title).font(theme::semibold(15.5)).color(t.text));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icons::button(ui, "x", 26.0, false, "Close panel").clicked() {
                close = true;
            }
        });
    });
    ui.add_space(6.0);
    (go_back, close)
}

fn all_tools(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let (_, close) = panel_header(ui, t, "All tools", false);
    if close {
        app.left_open = false;
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let shown = if app.all_tools_expanded { TOOL_GROUPS.len() } else { COLLAPSED_TOOLS.min(TOOL_GROUPS.len()) };
        for g in &TOOL_GROUPS[..shown] {
            if tool_row(ui, t, g).clicked() {
                app.left = LeftPanel::Tool(g.id);
            }
        }
        ui.add_space(4.0);
        let more = if app.all_tools_expanded { "View less" } else { "View more" };
        if ui.add(egui::Label::new(egui::RichText::new(more).color(t.accent_text).font(theme::medium(13.0))).sense(Sense::click())).clicked() {
            app.all_tools_expanded = !app.all_tools_expanded;
        }
    });
}

fn tool_row(ui: &mut egui::Ui, t: &Tokens, g: &ToolGroup) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, g.label));
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(6), t.hover);
    }
    icons::paint(ui, Rect::from_min_size(rect.min + vec2(6.0, 7.0), vec2(20.0, 20.0)), g.icon, 19.0, hue(g));
    ui.painter().text(rect.left_center() + vec2(36.0, 0.0), Align2::LEFT_CENTER, g.label, theme::regular(13.5), t.text);
    match (g.badge, g.availability) {
        (Some(b), _) => {
            let font = theme::semibold(9.5);
            let w = ui.fonts_mut(|f| f.layout_no_wrap(b.to_string(), font.clone(), Color32::WHITE).size().x);
            let r = Rect::from_center_size(rect.right_center() - vec2(w / 2.0 + 10.0, 0.0), vec2(w + 10.0, 16.0));
            ui.painter().rect_filled(r, CornerRadius::same(4), t.badge_new);
            ui.painter().text(r.center(), Align2::CENTER_CENTER, b, font, Color32::WHITE);
        }
        // Planned tools: a quiet milestone hint instead of a chip, so the list stays calm.
        (None, Availability::Planned(m)) if resp.hovered() => {
            ui.painter().text(
                rect.right_center() - vec2(10.0, 0.0),
                Align2::RIGHT_CENTER,
                format!("Planned · {m}"),
                theme::medium(10.5),
                t.text_faint,
            );
        }
        _ => {}
    }
    let tip = match g.availability {
        Availability::Ready => "Available".to_string(),
        Availability::Planned(m) => format!("Planned for milestone {m} — open to see what it will include"),
        Availability::Provider => "Optional: needs an AI provider you configure".into(),
    };
    resp.on_hover_text(tip)
}

fn tool_detail(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens, g: &'static ToolGroup) {
    let (back, close) = panel_header(ui, t, g.label, true);
    if back {
        app.left = LeftPanel::AllTools;
        app.mode = crate::Mode::AllTools;
    }
    if close {
        app.left_open = false;
    }
    if let Availability::Planned(m) = g.availability {
        egui::Frame::NONE.fill(t.accent_soft).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                egui::RichText::new(format!("Coming in milestone {m}. Items marked Ready work today.")).color(t.text).font(theme::regular(12.0)),
            );
        });
    }
    // Add a stamp: the palette.
    if g.id == "stamp" {
        stamp_palette(app, ui, t);
        return;
    }
    // Prepare a form: Preview fills the form as a reader would; Edit goes back.
    if g.id == "form" {
        ui.horizontal(|ui| {
            let label = if app.form_preview { "Edit fields" } else { "Preview" };
            if widgets::pill_button(ui, label, app.form_preview).on_hover_text("Try the form as people filling it in will see it").clicked() {
                app.form_preview = !app.form_preview;
                if app.form_preview {
                    app.quick_tool = crate::QuickTool::Select;
                }
            }
        });
        ui.add_space(6.0);
    }
    // Edit a PDF shows Format text at the top while text is selected or being added.
    if g.id == "edit" {
        format_section(app, ui, t);
    }
    let mut run = None;
    // Redact a PDF has Acrobat's footer: Clear all / Redact all.
    let footer = g.id == "redact";
    let marks = if footer { app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(0, |d| d.redaction_marks()) } else { 0 };
    let list_h = if footer { (ui.available_height() - 52.0).max(80.0) } else { ui.available_height() };
    egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(list_h).show(ui, |ui| {
        for s in g.sections {
            widgets::section_title(ui, s.title);
            for item in s.items {
                if g.id == "fill_sign" && (item.command.starts_with("sign.fill.signature") || item.command.starts_with("sign.fill.initials")) {
                    continue;
                }
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, item.label));
                let ready = item.availability == Availability::Ready;
                if resp.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(6), t.hover);
                }
                let fg = if ready { t.text } else { t.text_muted };
                icons::paint(
                    ui,
                    Rect::from_min_size(rect.min + vec2(6.0, 8.0), vec2(18.0, 18.0)),
                    item.icon,
                    17.0,
                    if ready { hue(g) } else { t.text_faint },
                );
                ui.painter().text(rect.left_center() + vec2(34.0, 0.0), Align2::LEFT_CENTER, item.label, theme::regular(13.0), fg);
                let (chip, fill, cfg) = match item.availability {
                    Availability::Ready => ("Ready", Color32::from_rgb(0xDD, 0xF3, 0xE4), Color32::from_rgb(0x1E, 0x7B, 0x43)),
                    Availability::Planned(m) => (m, t.pressed, t.text_muted),
                    Availability::Provider => ("AI", t.pressed, t.text_muted),
                };
                let font = theme::semibold(9.5);
                let w = ui.fonts_mut(|f| f.layout_no_wrap(chip.to_string(), font.clone(), cfg).size().x);
                let r = Rect::from_center_size(rect.right_center() - vec2(w / 2.0 + 10.0, 0.0), vec2(w + 10.0, 16.0));
                ui.painter().rect_filled(r, CornerRadius::same(4), fill);
                ui.painter().text(r.center(), Align2::CENTER_CENTER, chip, font, cfg);
                if resp.on_hover_text(item.command).clicked() {
                    run = Some(item.command);
                }
            }
        }
        if g.id == "fill_sign" {
            widgets::section_title(ui, "Sign yourself");
            if let Some(cmd) = crate::fill_sign::signature_entries(ui, app, t) {
                run = Some(cmd);
            }
        }
    });
    if footer {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if marks > 0 {
                ui.label(egui::RichText::new(format!("{marks} mark{}", if marks == 1 { "" } else { "s" })).color(t.text_muted));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled_ui(marks > 0, |ui| widgets::pill_button(ui, "Redact all", true)).inner.clicked() {
                    run = Some("redact.apply");
                }
                if ui.add_enabled_ui(marks > 0, |ui| widgets::pill_button(ui, "Clear all", false)).inner.clicked() {
                    run = Some("redact.clear");
                }
            });
        });
    }
    if let Some(cmd) = run {
        app.run_command(cmd);
    }
}

/// Add a stamp: Dynamic, Sign Here and Standard Business stamps; click one, then click on the
/// page to place it.
fn stamp_palette(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    use pdfcraft_engine::{StampGroup, StampKind};
    ui.label(egui::RichText::new("Choose a stamp, then click on the page to place it.").small().color(t.text_faint));
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (group, title) in
            [(StampGroup::Dynamic, "Dynamic"), (StampGroup::SignHere, "Sign Here"), (StampGroup::StandardBusiness, "Standard Business")]
        {
            widgets::section_title(ui, title);
            for kind in StampKind::ALL.into_iter().filter(|k| k.group() == group) {
                let active = app.quick_tool == crate::QuickTool::Stamp(kind);
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, kind.label()));
                if active {
                    ui.painter().rect_filled(rect, CornerRadius::same(6), t.accent_soft);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(6), t.hover);
                }
                let [r, g, b] = kind.color().map(|v| (v * 255.0) as u8);
                let col = Color32::from_rgb(r, g, b);
                let chip = Rect::from_min_size(rect.min + vec2(8.0, 5.0), vec2((rect.width() - 16.0).min(200.0), 28.0));
                if group == StampGroup::SignHere {
                    let tip = chip.height() * 0.45;
                    let pts = vec![
                        chip.left_center(),
                        chip.left_top() + vec2(tip, 0.0),
                        chip.right_top(),
                        chip.right_bottom(),
                        chip.left_bottom() + vec2(tip, 0.0),
                    ];
                    ui.painter().add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
                    ui.painter().text(
                        chip.center() + vec2(tip / 2.0, 0.0),
                        Align2::CENTER_CENTER,
                        kind.label(),
                        theme::semibold(11.0),
                        Color32::WHITE,
                    );
                } else {
                    ui.painter().rect(chip, CornerRadius::same(5), col.gamma_multiply(0.1), Stroke::new(1.5, col), egui::StrokeKind::Inside);
                    let y = if group == StampGroup::Dynamic { chip.center().y - 4.0 } else { chip.center().y };
                    ui.painter().text(egui::pos2(chip.center().x, y), Align2::CENTER_CENTER, kind.label(), theme::semibold(11.0), col);
                    if group == StampGroup::Dynamic {
                        ui.painter().text(
                            egui::pos2(chip.center().x, chip.bottom() - 6.0),
                            Align2::CENTER_CENTER,
                            "By name at time, date",
                            theme::regular(7.5),
                            col,
                        );
                    }
                }
                if resp.clicked() {
                    app.quick_tool = crate::QuickTool::Stamp(kind);
                }
            }
        }
        crate::stamps_ui::palette_section(app, ui, t);
    });
}

/// Edit a PDF ▸ Format text: for the selected added text (one undoable change), or the style
/// new text gets.
fn format_section(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    // Editing a paragraph of existing text: its formatting, applied as it changes.
    if let Some((i, _)) = app.active_ids()
        && let Some(ed) = app.views[i].line_editor.clone()
    {
        let mut ed = ed;
        let mut changed = false;
        if let Some(look) = crate::content_ui::format_panel(ui, t, &ed.look) {
            ed.look = look;
            changed = true;
        }
        changed |= crate::edit_text_ui::extras_panel(ui, &mut ed.extras);
        if changed {
            let edit = pdfcraft_engine::Edit::EditTextBlock { page: ed.page, block: ed.block, text: ed.text.clone(), style: ed.style() };
            if app.apply_edit(edit) {
                ed.applied();
                if let Some(doc) = app.session.get(app.views[i].id)
                    && let Some(block) = doc.text_blocks(ed.page).get(ed.block)
                {
                    ed.refresh_source(block);
                }
            }
            app.views[i].line_editor = Some(ed);
        }
        ui.add_space(6.0);
        ui.separator();
        return;
    }
    let image = app.active_ids().and_then(|(i, id)| {
        let (page, index) = app.views[i].content.selected?;
        let doc = app.session.get(id)?;
        match &doc.added.iter().filter(|a| a.page == page).nth(index)?.content {
            pdfcraft_engine::AddedContent::Image(img) => Some((page, index, img.clone())),
            _ => None,
        }
    });
    if let Some((page, index, img)) = image {
        match crate::content_ui::image_panel(ui, t, &img) {
            Some(crate::content_ui::ImageAction::Update(content)) => {
                app.apply_edit(pdfcraft_engine::Edit::UpdateContent { page, index, content });
            }
            Some(crate::content_ui::ImageAction::Replace) => app.replace_image_dialog(page, index),
            None => {}
        }
        ui.add_space(6.0);
        ui.separator();
        return;
    }
    let selected = app.active_ids().and_then(|(i, id)| {
        let (page, index) = app.views[i].content.selected?;
        let doc = app.session.get(id)?;
        let a = doc.added.iter().filter(|a| a.page == page).nth(index)?;
        match &a.content {
            pdfcraft_engine::AddedContent::Text(text) => Some((page, index, text.clone())),
            _ => None,
        }
    });
    match selected {
        Some((page, index, text)) => {
            if let Some(style) = crate::content_ui::format_panel(ui, t, &text) {
                app.text_style = pdfcraft_engine::AddedText { text: String::new(), rect: [0.0; 4], ..style.clone() };
                app.apply_edit(pdfcraft_engine::Edit::UpdateContent { page, index, content: pdfcraft_engine::AddedContent::Text(style) });
            }
        }
        None if app.quick_tool == crate::QuickTool::AddText => {
            crate::content_ui::hint(ui, t);
            if let Some(style) = crate::content_ui::format_panel(ui, t, &app.text_style) {
                app.text_style = style;
            }
        }
        None => return,
    }
    ui.add_space(6.0);
    ui.separator();
}

// ───────────────────────────────────────────────────────────────────────────── right panels

pub(crate) enum Nav {
    Page(usize),
    Flash(usize, [f32; 4]),
}

pub fn right_panel(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let Some((index, id)) = app.active_ids() else { return };
    let Some(panel) = app.right else { return };
    let mut nav: Option<Nav> = None;
    let mut close = false;
    let mut toggle_layer: Option<(usize, bool)> = None;
    let mut attachment_action: Option<(usize, bool)> = None; // (index, open instead of save)
    let mut bm_action: Option<BmAction> = None;
    let mut bm_expand: Option<usize> = None;
    let mut panel_edit: Option<pdfcraft_engine::Edit> = None;
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
        let compare = &app.compare;
        let comment_allowed = doc.allows_annotation();
        egui::Panel::right("right_panel")
            .resizable(true)
            .default_size(330.0)
            .size_range(260.0..=520.0)
            .frame(
                egui::Frame::NONE
                    .fill(t.panel)
                    .inner_margin(egui::Margin { left: 14, right: 12, top: 12, bottom: 8 })
                    .stroke(Stroke::new(1.0, t.divider)),
            )
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
                    RightPanel::Search => ("Search", None),
                    RightPanel::Compare => ("Compare", compare.as_ref().filter(|c| c.new == id).map(|c| c.result.changes.len())),
                };
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(title).font(theme::semibold(15.5)));
                    if let Some(c) = count {
                        ui.label(egui::RichText::new(c.to_string()).font(theme::medium(13.0)).color(t.text_faint));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icons::button(ui, "x", 26.0, false, "Close").clicked() {
                            close = true;
                        }
                        // Right to left: close, "…", filter, search (Acrobat's order left to right).
                        if panel == RightPanel::Comments {
                            panel_command = crate::comments_panel::header_controls(ui, info, view, doc.comments_hidden());
                        }
                        if panel == RightPanel::Comments
                            && icons::button(ui, "search", 26.0, view.comments.search.is_some(), "Search comments").clicked()
                        {
                            view.comments.search = match view.comments.search {
                                Some(_) => None,
                                None => Some(String::new()),
                            };
                            view.comments.search_focus = true;
                        }
                        if panel == RightPanel::Bookmarks && !info.outline.is_empty() {
                            let more = icons::button(ui, "ellipsis", 26.0, false, "Bookmark options");
                            egui::Popup::menu(&more).show(|ui| {
                                ui.set_min_width(200.0);
                                for (levels, label) in
                                    [(usize::MAX, "Expand all bookmarks"), (1, "Expand top-level bookmarks"), (0, "Collapse all bookmarks")]
                                {
                                    if ui.button(label).clicked() {
                                        bm_expand = Some(levels);
                                        ui.close();
                                    }
                                }
                            });
                        }
                        if panel == RightPanel::Bookmarks
                            && bm_editable
                            && icons::button(ui, "bookmark-plus", 26.0, false, "New bookmark (⌘B)").clicked()
                        {
                            bm_action = Some(BmAction::New);
                        }
                    });
                });
                ui.add_space(6.0);
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match panel {
                    RightPanel::Comments => {
                        if let Some(e) = crate::comments_panel::show(ui, &t, info, view, prefs, comment_allowed, &mut nav) {
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
                        };
                        for (i, item) in info.outline.iter().enumerate() {
                            outline_item(ui, &t, info, item, &[i], info.outline.len(), &mut ctx);
                        }
                    }
                    RightPanel::Pages => pages(ui, &t, info, view, &mut nav),
                    RightPanel::Fields => fields(ui, &t, info, &doc.form, preparing, &mut nav, &mut panel_edit),
                    RightPanel::Layers => {
                        if info.layers.is_empty() {
                            empty(ui, &t, "layers", "This document has no layers.");
                        }
                        for (li, l) in info.layers.iter().enumerate() {
                            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, l.visible, &l.name));
                            if resp.hovered() {
                                ui.painter().rect_filled(rect, CornerRadius::same(6), t.hover);
                            }
                            icons::paint(
                                ui,
                                Rect::from_min_size(rect.min + vec2(4.0, 7.0), vec2(18.0, 18.0)),
                                if l.visible { "eye" } else { "eye-off" },
                                16.0,
                                t.icon,
                            );
                            let fg = if l.visible { t.text } else { t.text_faint };
                            ui.painter().text(rect.left_center() + vec2(32.0, 0.0), Align2::LEFT_CENTER, &l.name, theme::regular(13.0), fg);
                            if resp.on_hover_text(if l.visible { "Hide layer" } else { "Show layer" }).clicked() {
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
                    RightPanel::Search => crate::search_ui::panel(ui, &t, view, info.pages.len()),
                    RightPanel::Compare => compare_action = crate::compare_ui::panel(ui, &t, compare, id),
                    RightPanel::Attachments => {
                        if info.attachments.is_empty() {
                            empty(ui, &t, "paperclip", "This document has no attachments.");
                        }
                        for (ai, a) in info.attachments.iter().enumerate() {
                            egui::Frame::NONE.inner_margin(egui::Margin::symmetric(4, 6)).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.add(icons::image("paperclip", 16.0, t.icon));
                                    ui.vertical(|ui| {
                                        ui.set_width(ui.available_width() - 70.0);
                                        ui.add(egui::Label::new(egui::RichText::new(&a.name).font(theme::medium(13.0))).truncate());
                                        let mut meta = a.size.map(human_size).unwrap_or_default();
                                        if let pdfcraft_render::AttachmentSource::Annotation { page, .. } = a.source {
                                            meta = format!("{meta}  ·  on page {}", info.pages.get(page).map(|p| p.label.as_str()).unwrap_or("?"));
                                        }
                                        if let Some(d) = &a.description {
                                            meta = format!("{meta}  ·  {d}");
                                        }
                                        ui.add(egui::Label::new(egui::RichText::new(meta).color(t.text_faint).small()).truncate());
                                    });
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if icons::button(ui, "file-down", 28.0, false, "Save attachment…").clicked() {
                                            attachment_action = Some((ai, false));
                                        }
                                        if a.name.to_lowercase().ends_with(".pdf")
                                            && icons::button(ui, "file-input", 28.0, false, "Open in a new tab").clicked()
                                        {
                                            attachment_action = Some((ai, true));
                                        }
                                    });
                                });
                            });
                        }
                    }
                });
            });
    }
    if close {
        app.right = None;
    }
    if let Some(e) = panel_edit {
        app.apply_edit(e);
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
        Some(crate::sign_ui::PanelAction::ViewCertificate(chain)) => {
            app.cert_viewer = Some(crate::sign_ui::CertViewer { chain, selected: 0, tab: crate::sign_ui::CertTab::Summary });
            app.dialog = Some(crate::Dialog::CertificateViewer);
        }
        Some(crate::sign_ui::PanelAction::ViewSigned(len)) => app.view_signed_version(len),
        Some(crate::sign_ui::PanelAction::Sign(field)) => {
            let page = app.views[index].current;
            app.start_signing(page, None, Some(field), None);
        }
        Some(crate::sign_ui::PanelAction::ExportCertificate(c)) => {
            let pem = crate::sign_ui::certificate_pem(&c);
            app.write_files(&[(format!("{}.cer", c.display_name()), std::sync::Arc::new(pem.into_bytes()))], "Export certificate");
        }
        None => {}
    }
    if let Some((p, i)) = app.views.get_mut(index).and_then(|v| v.comments.default_request.take()) {
        app.make_comment_default(p, i);
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
    match nav {
        Some(Nav::Page(p)) => app.views[index].go_to_page(p),
        Some(Nav::Flash(p, r)) => {
            let v = &mut app.views[index];
            v.go_to_page(p);
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
        ui.label(egui::RichText::new(text).color(t.text_muted));
    });
}

/// What the user asked to do with a bookmark (applied after the panel is drawn).
#[derive(Clone, Debug, PartialEq)]
pub enum BmAction {
    /// New "Untitled" bookmark for the current page, after the selected one (⌘B).
    New,
    StartRename(Vec<usize>),
    Rename(Vec<usize>, String),
    SetToCurrentPage(Vec<usize>),
    Delete(Vec<usize>),
    MoveUp(Vec<usize>),
    MoveDown(Vec<usize>),
    /// Make it the last child of the bookmark above it.
    Indent(Vec<usize>),
    /// Move it out to follow its parent.
    Outdent(Vec<usize>),
}

struct OutlineCtx<'a> {
    nav: &'a mut Option<Nav>,
    action: &'a mut Option<BmAction>,
    rename: &'a mut Option<(Vec<usize>, String)>,
    editable: bool,
    current: usize,
    /// Expand all (`Some(usize::MAX)`), collapse all (`Some(0)`) or expand to a depth, this frame.
    expand: Option<usize>,
}

fn outline_item(ui: &mut egui::Ui, t: &Tokens, info: &DocInfo, item: &OutlineItem, path: &[usize], siblings: usize, cx: &mut OutlineCtx<'_>) {
    let depth = path.len() - 1;
    let indent = depth as f32 * 16.0;
    let id = ui.id().with(("outline", path));
    if let Some(levels) = cx.expand {
        ui.data_mut(|d| d.insert_temp(id, depth < levels));
    }
    let mut open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(item.open || depth == 0);
    let x_text = indent + 20.0;
    if let Some((rpath, text)) = cx.rename.as_mut()
        && rpath.as_slice() == path
    {
        // Inline rename: Enter commits, Escape cancels.
        let mut commit = None;
        let mut cancel = false;
        ui.horizontal(|ui| {
            ui.add_space(x_text);
            let edit = egui::TextEdit::singleline(text).desired_width(ui.available_width() - 8.0).id(id.with("rename"));
            let resp = ui.add(edit);
            if !resp.has_focus() && !resp.lost_focus() {
                resp.request_focus();
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            } else if resp.lost_focus() {
                commit = Some(text.trim().to_string());
            }
        });
        if cancel {
            *cx.rename = None;
        } else if let Some(title) = commit {
            *cx.rename = None;
            if !title.is_empty() && title != item.title {
                *cx.action = Some(BmAction::Rename(path.to_vec(), title));
            }
        }
    } else {
        let font = if depth == 0 { theme::medium(13.0) } else { theme::regular(13.0) };
        let wrap_w = (ui.available_width() - indent - 20.0 - 36.0).max(60.0);
        let galley = ui.fonts_mut(|f| f.layout(item.title.clone(), font, t.text, wrap_w));
        let h = (galley.size().y + 12.0).max(28.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &item.title));
        if resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(6), t.hover);
        }
        let x0 = rect.left() + indent;
        if !item.children.is_empty() {
            let tri = Rect::from_min_size(pos2(x0, rect.top() + 6.0), vec2(16.0, 16.0));
            icons::paint(ui, tri, if open { "chevron-down" } else { "chevron-right" }, 14.0, t.text_muted);
            if ui.interact(tri, id.with("tri"), Sense::click()).clicked() {
                open = !open;
                ui.data_mut(|d| d.insert_temp(id, open));
            }
        }
        ui.painter().galley(pos2(x0 + 20.0, rect.top() + 6.0), galley, t.text);
        if let Some(p) = item.page
            && let Some(page) = info.pages.get(p)
        {
            ui.painter().text(rect.right_top() + vec2(-6.0, 14.0), Align2::RIGHT_CENTER, &page.label, theme::regular(11.0), t.text_faint);
        }
        if resp.clicked()
            && let Some(p) = item.page
        {
            *cx.nav = Some(Nav::Page(p));
        }
        if resp.double_clicked() && cx.editable {
            *cx.action = Some(BmAction::StartRename(path.to_vec()));
        }
        if cx.editable
            && let Some(&i) = path.last()
        {
            let current = cx.current;
            resp.context_menu(|ui| {
                let mut pick = |ui: &mut egui::Ui, label: &str, enabled: bool, a: BmAction| {
                    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                        *cx.action = Some(a);
                        ui.close();
                    }
                };
                pick(ui, "Rename", true, BmAction::StartRename(path.to_vec()));
                pick(ui, &format!("Set to current page ({})", current + 1), true, BmAction::SetToCurrentPage(path.to_vec()));
                ui.separator();
                pick(ui, "Move up", i > 0, BmAction::MoveUp(path.to_vec()));
                pick(ui, "Move down", i + 1 < siblings, BmAction::MoveDown(path.to_vec()));
                pick(ui, "Indent", i > 0, BmAction::Indent(path.to_vec()));
                pick(ui, "Outdent", depth > 0, BmAction::Outdent(path.to_vec()));
                ui.separator();
                pick(ui, "Delete", true, BmAction::Delete(path.to_vec()));
            });
        }
    }
    if open {
        for (ci, c) in item.children.iter().enumerate() {
            let mut p = path.to_vec();
            p.push(ci);
            outline_item(ui, t, info, c, &p, item.children.len(), cx);
        }
    }
}

fn pages(ui: &mut egui::Ui, t: &Tokens, info: &DocInfo, view: &crate::DocView, nav: &mut Option<Nav>) {
    let w = (ui.available_width() - 40.0).min(150.0);
    for (i, p) in info.pages.iter().enumerate() {
        ui.vertical_centered(|ui| {
            let h = w * p.height / p.width.max(1.0);
            let (rect, resp) = ui.allocate_exact_size(vec2(w + 16.0, h + 16.0), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Page {}", p.label)));
            let selected = i == view.current;
            if selected {
                ui.painter().rect_filled(rect, CornerRadius::same(8), t.accent_soft);
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(8), t.hover);
            }
            let pr = rect.shrink(8.0);
            ui.painter().rect_filled(pr, CornerRadius::ZERO, Color32::WHITE);
            if let Some(tex) = view.thumb(i) {
                ui.painter().image(tex.id(), pr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            ui.painter().rect_stroke(
                pr,
                CornerRadius::ZERO,
                Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { t.accent } else { t.border }),
                egui::StrokeKind::Outside,
            );
            ui.label(egui::RichText::new(&p.label).font(theme::medium(12.0)).color(if selected { t.accent_text } else { t.text_muted }));
            if resp.clicked() {
                *nav = Some(Nav::Page(i));
            }
        });
        ui.add_space(4.0);
    }
}

/// The Fields panel: fields by page, in tab order. While preparing a form, each field can move
/// earlier or later in its page's tab order (Acrobat: Order Tabs Manually).
fn fields(
    ui: &mut egui::Ui,
    t: &Tokens,
    info: &DocInfo,
    form: &[pdfcraft_engine::FormField],
    preparing: bool,
    nav: &mut Option<Nav>,
    edit: &mut Option<pdfcraft_engine::Edit>,
) {
    if info.fields.is_empty() {
        empty(ui, t, "text-cursor-input", "This document has no form fields.");
        return;
    }
    let rank = |name: &str| form.iter().find(|f| f.name == name).and_then(|f| f.widgets.iter().map(|w| w.tab).min()).unwrap_or(usize::MAX);
    let mut ordered: Vec<_> = info.fields.iter().collect();
    ordered.sort_by_key(|f| (f.page, rank(&f.name)));
    if preparing {
        ui.label(egui::RichText::new("Tab order: move a field with its arrows.").small().color(t.text_muted));
    }
    let mut pages: Vec<Option<usize>> = info.fields.iter().map(|f| f.page).collect();
    pages.sort();
    pages.dedup();
    for p in pages {
        let label = p.map(|p| format!("Page {}", info.pages[p].label)).unwrap_or_else(|| "Unplaced".into());
        ui.add_space(4.0);
        ui.label(egui::RichText::new(label).font(theme::semibold(12.5)).color(t.text_muted));
        for f in ordered.iter().copied().filter(|f| f.page == p) {
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
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &f.name));
            if resp.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(6), t.hover);
            }
            icons::paint(ui, Rect::from_min_size(rect.min + vec2(8.0, 7.0), vec2(16.0, 16.0)), icon, 15.0, Color32::from_rgb(0x8E, 0x4E, 0xE6));
            ui.painter().text(rect.left_center() + vec2(32.0, 0.0), Align2::LEFT_CENTER, &f.name, theme::regular(13.0), t.text);
            if let Some(v) = &f.value {
                let v: String = if v.chars().count() > 18 { format!("{}…", v.chars().take(17).collect::<String>()) } else { v.clone() };
                ui.painter().text(rect.right_center() - vec2(8.0, 0.0), Align2::RIGHT_CENTER, v, theme::regular(11.5), t.text_faint);
            }
            let mut tip = format!("{:?} field", f.kind);
            if let Some(tt) = &f.tooltip {
                tip.push_str(&format!(" — {tt}"));
            }
            if f.has_actions {
                tip.push_str("\nHas JavaScript actions (run in M6)");
            }
            if preparing && f.page.is_some() {
                let up = Rect::from_center_size(rect.right_center() - vec2(44.0, 0.0), vec2(22.0, 22.0));
                let down = Rect::from_center_size(rect.right_center() - vec2(20.0, 0.0), vec2(22.0, 22.0));
                for (r, icon, earlier, tip) in [(up, "chevron-up", true, "Earlier in tab order"), (down, "chevron-down", false, "Later in tab order")]
                {
                    let b = ui.interact(r, ui.id().with((&f.name, earlier)), Sense::click());
                    b.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{tip}: {}", f.name)));
                    if b.hovered() {
                        ui.painter().rect_filled(r, CornerRadius::same(4), t.pressed);
                    }
                    icons::paint(ui, r.shrink(3.0), icon, 15.0, t.icon);
                    if b.on_hover_text(tip).clicked() {
                        *edit = Some(pdfcraft_engine::Edit::MoveInTabOrder { name: f.name.clone(), earlier });
                    }
                }
            }
            if resp.on_hover_text(tip).clicked()
                && let (Some(p), Some(r)) = (f.page, f.rect)
            {
                *nav = Some(Nav::Flash(p, r));
            }
        }
    }
}

pub fn human_size(n: usize) -> String {
    match n {
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KB", n as f64 / 1024.0),
        n => format!("{n} bytes"),
    }
}
