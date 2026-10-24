//! Left tool panel (All tools + tool sub-panels, generated from the engine catalogue) and the
//! right-hand panels (Comments, Bookmarks, Pages, Fields, Layers, Attachments).

use egui::{Align, Align2, Color32, CornerRadius, Layout, Rect, Sense, Stroke, pos2, vec2};
use pdfcraft_engine::catalog::{self, Availability, TOOL_GROUPS, ToolGroup, ToolItem};
use pdfcraft_render::{DocInfo, FieldKind, OutlineItem};

use crate::theme::{self, Tokens};
use crate::{LeftPanel, PdfCraftApp, RightPanel, icons, widgets};

const COLLAPSED_TOOLS: usize = 14;

fn hue(g: &ToolGroup) -> Color32 {
    Color32::from_rgb(g.hue[0], g.hue[1], g.hue[2])
}

pub fn left_panel(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    // Organize closed another way (the grid's Close, or a page opened from it) closes the tool too,
    // as its panel's Close does, back to the workspace's own panel.
    if app.left == LeftPanel::Tool("organize") && app.active.and_then(|i| app.views.get(i)).is_some_and(|v| !v.organize) {
        app.left = crate::organize_panel::workspace_panel(app.mode);
    }
    egui::Frame::NONE.fill(t.panel).inner_margin(egui::Margin { left: 14, right: 10, top: 12, bottom: 10 }).stroke(Stroke::new(1.0, t.divider)).show(
        ui,
        |ui| match app.left {
            // Organize pages has its own panel whenever the grid shows (with a document open).
            _ if app.active.is_some() && crate::organize_panel::showing(app) => crate::organize_panel::panel(app, ui, &t),
            LeftPanel::AllTools => all_tools(app, ui, &t),
            LeftPanel::Tool(id) => match catalog::group(id) {
                Some(g) => tool_detail(app, ui, &t, g),
                None => app.left = LeftPanel::AllTools,
            },
            LeftPanel::CalculationOrder => crate::calc_order_ui::panel(app, ui, &t),
        },
    );
}

/// A panel's title row: Back (with `back` as its tooltip) when given, the title and Close.
/// Returns whether Back and Close were clicked.
pub(crate) fn panel_header(ui: &mut egui::Ui, t: &Tokens, title: &str, back: Option<&str>) -> (bool, bool) {
    let mut go_back = false;
    let mut close = false;
    ui.horizontal(|ui| {
        if let Some(tip) = back
            && icons::button(ui, "chevron-left", 26.0, false, tip).clicked()
        {
            go_back = true;
        }
        // Reserve space for Close before laying out the translated title. Long tool names
        // must never expand the fixed-width sidebar or push the Close control off-screen.
        let title_width = (ui.available_width() - 26.0 - ui.spacing().item_spacing.x).max(1.0);
        ui.add_sized([title_width, 26.0], egui::Label::new(egui::RichText::new(tl!(title)).font(theme::semibold(15.5)).color(t.text)).truncate())
            .on_hover_text(tl!(title));
        if icons::button(ui, "x", 26.0, false, tl!("Close panel")).clicked() {
            close = true;
        }
    });
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(6.0);
    (go_back, close)
}

fn all_tools(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let (_, close) = panel_header(ui, t, "All tools", None);
    if close {
        app.left_open = false;
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(2.0);
        let shown = if app.all_tools_expanded { TOOL_GROUPS.len() } else { COLLAPSED_TOOLS.min(TOOL_GROUPS.len()) };
        for g in &TOOL_GROUPS[..shown] {
            if tool_row(ui, t, g).clicked() {
                app.select_tool(g.id);
            }
        }
        ui.add_space(4.0);
        let more = if app.all_tools_expanded { "View less" } else { "View more" };
        if ui
            .add(
                egui::Label::new(egui::RichText::new(tl!(more)).color(t.accent_text).font(theme::medium(13.0)))
                    .selectable(false)
                    .sense(Sense::click()),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            app.all_tools_expanded = !app.all_tools_expanded;
        }
    });
}

/// Draw a one-line row label without painting into the status chip or beyond the sidebar.
/// Accessibility retains the complete name through the row's WidgetInfo.
fn row_label(ui: &mut egui::Ui, text: &str, font: egui::FontId, color: Color32, left_center: egui::Pos2, max_width: f32) -> bool {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: max_width.max(1.0), max_rows: 1, break_anywhere: true, ..Default::default() };
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let elided = galley.elided;
    ui.painter().galley(left_center - vec2(0.0, galley.size().y / 2.0), galley, color);
    elided
}

fn tool_row(ui: &mut egui::Ui, t: &Tokens, g: &ToolGroup) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tl!(g.label)));
    let press = theme::Press::track(ui, &resp);
    press.wash(ui, rect, 6, false);
    let body = rect.translate(press.offset());
    icons::paint(ui, Rect::from_min_size(body.min + vec2(6.0, 7.0), vec2(20.0, 20.0)), g.icon, 19.0, hue(g));
    let right_reserved = match (g.badge, g.availability) {
        (Some(b), _) => ui.fonts_mut(|f| f.layout_no_wrap(tl!(b).to_owned(), theme::semibold(9.5), Color32::WHITE).size().x) + 26.0,
        (None, Availability::Planned(m)) if resp.hovered() => {
            let hint = format!("{} · {m}", tl!("Planned"));
            ui.fonts_mut(|f| f.layout_no_wrap(hint, theme::medium(10.5), t.text_faint).size().x) + 18.0
        }
        _ => 8.0,
    };
    let clipped =
        row_label(ui, tl!(g.label), theme::regular(13.5), t.text, body.left_center() + vec2(36.0, 0.0), rect.width() - 36.0 - right_reserved);
    match (g.badge, g.availability) {
        (Some(b), _) => {
            let font = theme::semibold(9.5);
            let badge = tl!(b);
            let w = ui.fonts_mut(|f| f.layout_no_wrap(badge.to_string(), font.clone(), Color32::WHITE).size().x);
            let r = Rect::from_center_size(body.right_center() - vec2(w / 2.0 + 10.0, 0.0), vec2(w + 10.0, 16.0));
            ui.painter().rect_filled(r, CornerRadius::same(4), t.badge_new);
            ui.painter().text(r.center(), Align2::CENTER_CENTER, badge, font, Color32::WHITE);
        }
        // Planned tools: a quiet milestone hint instead of a chip, so the list stays calm.
        (None, Availability::Planned(m)) if resp.hovered() => {
            ui.painter().text(
                body.right_center() - vec2(10.0, 0.0),
                Align2::RIGHT_CENTER,
                format!("{} · {m}", tl!("Planned")),
                theme::medium(10.5),
                t.text_faint,
            );
        }
        _ => {}
    }
    let tip = match g.availability {
        Availability::Ready => tl!("Available").to_string(),
        Availability::Planned(m) => format!("{} {m} — {}", tl!("Planned for milestone"), tl!("open to see what it will include")),
        Availability::Provider => tl!("Optional: needs an AI provider you configure").into(),
    };
    let tip = if clipped { format!("{} — {tip}", tl!(g.label)) } else { tip };
    theme::hand(resp).on_hover_text(tip)
}

fn tool_detail(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens, g: &'static ToolGroup) {
    if g.id == "edit" {
        edit_panel(app, ui, t, g);
        return;
    }
    let (back, close) = panel_header(ui, t, g.label, Some(tl!("Back to all tools")));
    if back {
        app.left = LeftPanel::AllTools;
        app.enter_mode(crate::Mode::AllTools);
    }
    if close {
        app.left_open = false;
    }
    if let Availability::Planned(m) = g.availability {
        egui::Frame::NONE.fill(t.accent_soft).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                egui::RichText::new(format!("{} {m}. {}", tl!("Coming in milestone"), tl!("Items marked Ready work today.")))
                    .color(t.text)
                    .font(theme::regular(12.0)),
            );
        });
    }
    if g.id == "measure" {
        crate::measure_ui::panel(app, ui, t);
        return;
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
            if widgets::pill_button(ui, tl!(label), app.form_preview).on_hover_text(tl!("Try the form as people filling it in will see it")).clicked()
            {
                app.form_preview = !app.form_preview;
                if app.form_preview {
                    app.quick_tool = crate::QuickTool::Select;
                }
            }
        });
        ui.add_space(6.0);
    }
    let mut run = None;
    // Redact a PDF has Acrobat's footer: Clear all / Redact all.
    let footer = g.id == "redact";
    let marks = if footer { app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(0, |d| d.redaction_marks()) } else { 0 };
    let list_h = if footer { (ui.available_height() - 52.0).max(80.0) } else { ui.available_height() };
    egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(list_h).show(ui, |ui| {
        ui.add_space(2.0);
        for s in g.sections {
            widgets::section_title(ui, tl!(s.title));
            for item in s.items {
                if g.id == "fill_sign" && (item.command.starts_with("sign.fill.signature") || item.command.starts_with("sign.fill.initials")) {
                    continue;
                }
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tl!(item.label)));
                let ready = item.availability == Availability::Ready;
                let press = theme::Press::track(ui, &resp);
                press.wash(ui, rect, 6, false);
                let body = rect.translate(press.offset());
                let fg = if ready { t.text } else { t.text_muted };
                icons::paint(
                    ui,
                    Rect::from_min_size(body.min + vec2(6.0, 8.0), vec2(18.0, 18.0)),
                    item.icon,
                    17.0,
                    if ready { hue(g) } else { t.text_faint },
                );
                let (chip, fill, cfg) = match item.availability {
                    Availability::Ready => (tl!("Ready"), Color32::from_rgb(0xDD, 0xF3, 0xE4), Color32::from_rgb(0x1E, 0x7B, 0x43)),
                    Availability::Planned(m) => (m, t.pressed, t.text_muted),
                    Availability::Provider => ("AI", t.pressed, t.text_muted),
                };
                let font = theme::semibold(9.5);
                let chip_label = tl!(chip).to_string();
                let w = crate::widgets::text_width(ui, &chip_label, &font);
                let r = Rect::from_center_size(body.right_center() - vec2(w / 2.0 + 10.0, 0.0), vec2(w + 10.0, 16.0));
                row_label(ui, tl!(item.label), theme::regular(13.0), fg, body.left_center() + vec2(34.0, 0.0), r.left() - body.left() - 42.0);
                ui.painter().rect_filled(r, CornerRadius::same(4), fill);
                ui.painter().text(r.center(), Align2::CENTER_CENTER, chip_label, font, cfg);
                let resp = theme::hand(resp).on_hover_text(format!("{} — {}", tl!(item.label), item.command));
                // An entry with further commands opens a menu; its caret goes before the chip.
                if item.actions.is_empty() {
                    if resp.clicked() {
                        run = Some(item.command);
                    }
                } else if let Some(cmd) = actions_menu(app, ui, &resp, item, r.left_center() - vec2(10.0, 0.0), fg) {
                    run = Some(cmd);
                }
            }
        }
        if g.id == "fill_sign" {
            widgets::section_title(ui, tl!("Sign yourself"));
            if let Some(cmd) = crate::fill_sign::signature_entries(ui, app, t) {
                run = Some(cmd);
            }
        }
    });
    if footer {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if marks > 0 {
                ui.label(
                    egui::RichText::new(if marks == 1 {
                        crate::i18n::fmt(tl!("1 mark"), &[])
                    } else {
                        crate::i18n::fmt(tl!("{n} marks"), &[("n", &marks.to_string())])
                    })
                    .color(t.text_muted),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled_ui(marks > 0, |ui| widgets::pill_button(ui, tl!("Redact all"), true)).inner.clicked() {
                    run = Some("redact.apply");
                }
                if ui.add_enabled_ui(marks > 0, |ui| widgets::pill_button(ui, tl!("Clear all"), false)).inner.clicked() {
                    run = Some("redact.clear");
                }
            });
        });
    }
    if let Some(cmd) = run {
        app.run_command(cmd);
    }
}

/// The page operations Edit shows as a row of icon buttons, as Acrobat's Edit panel does.
const EDIT_PAGE_BUTTONS: [&str; 4] = ["page.rotate", "page.insert", "page.delete", "page.extract"];
/// Add content items shown before More.
const EDIT_CONTENT_SHOWN: usize = 5;
/// Other options: the tools Acrobat's Edit panel links to.
const EDIT_OTHER_TOOLS: [&str; 3] = ["combine", "redact", "form"];

/// Edit a PDF, laid out as Acrobat's Edit panel: the title with Preferences and Close; Format text
/// while text is being edited or added; Modify page as a row of icon buttons and Organize pages;
/// Add content with the rarer items under More; and Other options, the neighbouring tools.
/// Edit text & images itself is on the floating toolbar, and on from the moment Edit opens.
/// Every item comes from the catalogue, so its label, icon and availability have one source.
fn edit_panel(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens, g: &'static ToolGroup) {
    let items = || g.sections.iter().flat_map(|s| s.items.iter());
    let item = |command: &str| items().find(|i| i.command == command);
    let mut run = None;
    let mut tool = None;
    // Back to All tools only when Edit was opened from there; the Edit tab is its own way back.
    let from_all_tools = app.mode != crate::Mode::Edit;
    ui.horizontal(|ui| {
        if from_all_tools && icons::button(ui, "chevron-left", 26.0, false, tl!("Back to all tools")).clicked() {
            app.left = LeftPanel::AllTools;
            app.enter_mode(crate::Mode::AllTools);
        }
        // "Edit" under the Edit tab, as Acrobat titles it; the tool's own name from All tools.
        let title = if from_all_tools { tl!(g.label) } else { tl!("Edit") };
        let title_width = (ui.available_width() - 2.0 * (26.0 + ui.spacing().item_spacing.x)).max(1.0);
        ui.add_sized([title_width, 26.0], egui::Label::new(egui::RichText::new(title).font(theme::semibold(15.5)).color(t.text)).truncate())
            .on_hover_text(title);
        if icons::button(ui, "settings", 26.0, false, tl!("Preferences")).clicked() {
            run = Some("app.preferences");
        }
        if icons::button(ui, "x", 26.0, false, tl!("Close panel")).clicked() {
            app.left_open = false;
        }
    });
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(6.0);
    // Several objects selected: how many, and how to move them. Acrobat's panel has no idle hint.
    let count = app.active_ids().filter(|_| app.quick_tool == crate::QuickTool::EditText).map_or(0, |(i, _)| app.views[i].objects.count());
    if count > 0 {
        ui.label(crate::i18n::trn(crate::i18n::current(), count as u64, "{n} object selected", "{n} objects selected"));
        ui.label(tl!("Drag the selection to move it. Esc clears the selection."));
        ui.add_space(6.0);
    }
    // Format text at the top while text is selected or being added.
    format_section(app, ui, t);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        widgets::section_title(ui, tl!("Modify page"));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for i in EDIT_PAGE_BUTTONS.iter().filter_map(|c| item(c)) {
                // Enabled as the command itself is: a document that forbids page assembly refuses these.
                let enabled = pdfcraft_engine::commands::command(i.command).is_some_and(|spec| app.command_enabled(spec));
                let resp = ui.add_enabled_ui(enabled, |ui| icons::button(ui, i.icon, 34.0, false, tl!(i.label))).inner;
                if resp.clicked() {
                    run = Some(i.command);
                }
            }
        });
        ui.add_space(2.0);
        // Organize pages is a workspace of its own: its panel replaces this one, with the grid.
        if let Some(i) = item("page.organize")
            && edit_row(ui, t, i.icon, tl!(i.label), i.availability).clicked()
        {
            // With no document there is no grid to open: the command asks for a file first.
            if app.active.is_some() {
                tool = Some("organize");
            } else {
                run = Some("page.organize");
            }
        }

        widgets::section_title(ui, tl!("Add content"));
        let content: Vec<_> =
            g.sections.iter().filter(|s| s.title == "Add content").flat_map(|s| s.items.iter()).filter(|i| i.command != "edit.edit_text").collect();
        let more_id = egui::Id::new("edit_panel_more");
        let more = ui.data(|d| d.get_temp::<bool>(more_id)).unwrap_or(false);
        let shown = if more { content.len() } else { EDIT_CONTENT_SHOWN.min(content.len()) };
        for i in content.iter().take(shown) {
            let resp = edit_row(ui, t, i.icon, tl!(i.label), i.availability);
            // Header and footer, Watermark and Background open their Add / Update / Remove menu.
            if i.actions.is_empty() {
                if resp.clicked() {
                    run = Some(i.command);
                }
            } else if let Some(cmd) = actions_menu(app, ui, &resp, i, resp.rect.right_center() - vec2(14.0, 0.0), t.text) {
                run = Some(cmd);
            }
        }
        if content.len() > EDIT_CONTENT_SHOWN {
            ui.add_space(4.0);
            let label = if more { tl!("Less") } else { tl!("More") };
            let resp = ui
                .add(
                    egui::Label::new(egui::RichText::new(label).color(t.accent_text).font(theme::medium(13.0)))
                        .selectable(false)
                        .sense(Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.clicked() {
                ui.data_mut(|d| d.insert_temp(more_id, !more));
            }
        }

        widgets::section_title(ui, tl!("Other options"));
        for other in EDIT_OTHER_TOOLS.iter().filter_map(|id| catalog::group(id)) {
            if edit_row(ui, t, other.icon, tl!(other.label), other.availability).clicked() {
                tool = Some(other.id);
            }
        }
    });
    if let Some(id) = tool {
        app.select_tool(id);
    }
    if let Some(cmd) = run {
        app.run_command(cmd);
    }
}

/// A tool entry with further commands (Add / Update / Remove): a caret at `caret` says it opens a
/// menu, and each command is enabled by its own needs. Returns the command chosen.
fn actions_menu(app: &PdfCraftApp, ui: &egui::Ui, resp: &egui::Response, item: &ToolItem, caret: egui::Pos2, color: Color32) -> Option<&'static str> {
    ui.painter().add(egui::Shape::convex_polygon(
        vec![caret + vec2(-3.5, -2.0), caret + vec2(3.5, -2.0), caret + vec2(0.0, 2.5)],
        color,
        egui::Stroke::NONE,
    ));
    let mut run = None;
    egui::Popup::menu(resp).show(|ui| {
        ui.set_min_width(180.0);
        for (n, id) in item.actions.iter().enumerate() {
            let Some(spec) = pdfcraft_engine::commands::command(id) else { continue };
            // Add stays usable with no document open: it asks for a file first.
            let enabled = (n == 0 && app.active.is_none()) || app.command_enabled(spec);
            let label = crate::i18n::menu_label(spec.id, spec.label);
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                run = Some(spec.id);
                ui.close();
            }
        }
    });
    run
}

/// One row of the Edit panel: a monochrome icon and the label, as Acrobat draws them. A planned
/// item is muted and says when it ships when hovered.
fn edit_row(ui: &mut egui::Ui, t: &Tokens, icon: &str, label: &str, availability: Availability) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let press = theme::Press::track(ui, &resp);
    press.wash(ui, rect, 6, false);
    let body = rect.translate(press.offset());
    let ready = availability == Availability::Ready;
    let fg = if ready { t.text } else { t.text_muted };
    icons::paint(ui, Rect::from_min_size(body.min + vec2(6.0, 8.0), vec2(18.0, 18.0)), icon, 17.0, if ready { t.text } else { t.text_faint });
    row_label(ui, label, theme::regular(13.5), fg, body.left_center() + vec2(34.0, 0.0), rect.width() - 42.0);
    let resp = theme::hand(resp);
    match availability {
        Availability::Ready => resp,
        Availability::Planned(m) => resp.on_hover_text(format!("{} {m}", tl!("Planned for milestone"))),
        Availability::Provider => resp.on_hover_text(tl!("Optional: needs an AI provider you configure")),
    }
}

/// Add a stamp: Dynamic, Sign Here and Standard Business stamps; click one, then click on the
/// page to place it.
fn stamp_palette(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    use pdfcraft_engine::{StampGroup, StampKind};
    ui.label(egui::RichText::new(tl!("Choose a stamp, then click on the page to place it.")).small().color(t.text_faint));
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (group, title) in [
            (StampGroup::Dynamic, tl!("Dynamic")),
            (StampGroup::SignHere, tl!("Sign Here")),
            (StampGroup::StandardBusiness, tl!("Standard Business")),
        ] {
            widgets::section_title(ui, title);
            for kind in StampKind::ALL.into_iter().filter(|k| k.group() == group) {
                let active = app.quick_tool == crate::QuickTool::Stamp(kind);
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, kind.label()));
                let press = theme::Press::track(ui, &resp);
                press.wash(ui, rect, 6, active);
                let body = rect.translate(press.offset());
                let [r, g, b] = kind.color().map(|v| (v * 255.0) as u8);
                let col = Color32::from_rgb(r, g, b);
                let chip = Rect::from_min_size(body.min + vec2(8.0, 5.0), vec2((rect.width() - 16.0).min(200.0), 28.0));
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
                    let label = kind.label();
                    let font = theme::semibold(11.0);
                    let shown = crate::widgets::fit_line(label, (chip.width() - 10.0).max(0.0), |s| crate::widgets::text_width(ui, s, &font));
                    ui.painter().with_clip_rect(chip.shrink(3.0)).text(
                        chip.center() + vec2(tip / 2.0, 0.0),
                        Align2::CENTER_CENTER,
                        shown,
                        font,
                        Color32::WHITE,
                    );
                } else {
                    ui.painter().rect(chip, CornerRadius::same(5), col.gamma_multiply(0.1), Stroke::new(1.5, col), egui::StrokeKind::Inside);
                    let y = if group == StampGroup::Dynamic { chip.center().y - 4.0 } else { chip.center().y };
                    let font = theme::semibold(11.0);
                    let shown = crate::widgets::fit_line(kind.label(), (chip.width() - 10.0).max(0.0), |s| crate::widgets::text_width(ui, s, &font));
                    ui.painter().with_clip_rect(chip.shrink(3.0)).text(egui::pos2(chip.center().x, y), Align2::CENTER_CENTER, shown, font, col);
                    if group == StampGroup::Dynamic {
                        let by = "By name at time, date";
                        let by_font = theme::regular(7.5);
                        let by_shown = crate::widgets::fit_line(by, (chip.width() - 8.0).max(0.0), |s| crate::widgets::text_width(ui, s, &by_font));
                        ui.painter().with_clip_rect(chip.shrink(2.0)).text(
                            egui::pos2(chip.center().x, chip.bottom() - 6.0),
                            Align2::CENTER_CENTER,
                            by_shown,
                            by_font,
                            col,
                        );
                    }
                }
                if theme::hand(resp).clicked() {
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
        use crate::format_text_ui::{FontChoice, Fonts};
        let mut ed = ed;
        let sources: Vec<(String, String)> = app
            .session
            .get(app.views[i].id)
            .map(|d| d.text_editor_fonts(ed.page))
            .unwrap_or_default()
            .into_iter()
            .map(|f| (f.resource, f.face))
            .collect();
        let shown = ed.font_shown(&sources);
        let out = crate::format_text_ui::panel(ui, t, &mut ed.look, Some(&mut ed.extras), Fonts { sources: Some(&sources), shown });
        let mut changed = out.changed;
        match out.font {
            Some(FontChoice::KeepSource) => {
                ed.keep_source_font();
                changed = true;
            }
            Some(FontChoice::Resource(resource)) => {
                ed.font_resource = Some(resource);
                changed = true;
            }
            Some(FontChoice::Family(family)) => {
                ed.choose_family(family);
                changed = true;
            }
            Some(FontChoice::Load) => app.pick_files(crate::files::FilePurpose::TextFont { page: ed.page }, false),
            #[cfg(not(target_arch = "wasm32"))]
            Some(FontChoice::Installed(path)) => app.use_paths(crate::files::FilePurpose::TextFont { page: ed.page }, &[path]),
            #[cfg(target_arch = "wasm32")]
            Some(FontChoice::Installed(_)) => {}
            None => {}
        }
        if changed {
            app.apply_line_format(i, ed, Some(ui.ctx()));
        }
        return;
    }
    let image = app.active_ids().and_then(|(i, id)| {
        let (page, index) = app.views[i].content.selected?;
        let doc = app.session.get(id)?;
        match &doc.added().iter().filter(|a| a.page == page).nth(index)?.content {
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
        let a = doc.added().iter().filter(|a| a.page == page).nth(index)?;
        match &a.content {
            pdfcraft_engine::AddedContent::Text(text) => Some((page, index, text.clone())),
            _ => None,
        }
    });
    match selected {
        Some((page, index, text)) => {
            let mut style = text.clone();
            added_text_format(ui, t, &mut style);
            if style != text {
                // The reusable style follows the selection only when the edit took, so a refused one adds nothing odd later.
                if app.apply_edit(pdfcraft_engine::Edit::UpdateContent { page, index, content: pdfcraft_engine::AddedContent::Text(style.clone()) }) {
                    app.text_style = pdfcraft_engine::AddedText { text: String::new(), rect: [0.0; 4], ..style };
                }
            }
        }
        None if app.quick_tool == crate::QuickTool::AddText => {
            crate::content_ui::hint(ui, t);
            let mut style = app.text_style.clone();
            added_text_format(ui, t, &mut style);
            app.text_style = style;
        }
        None => {}
    }
}

/// Format text for added text: the standard families, size, colour, style and alignment.
fn added_text_format(ui: &mut egui::Ui, t: &Tokens, style: &mut pdfcraft_engine::AddedText) {
    let shown = style.family.label().to_string();
    let out = crate::format_text_ui::panel(ui, t, style, None, crate::format_text_ui::Fonts { sources: None, shown });
    if let Some(crate::format_text_ui::FontChoice::Family(family)) = out.font {
        style.family = family;
    }
}

impl PdfCraftApp {
    /// Apply the open paragraph's formatting as one change; the paragraph stays open.
    pub(crate) fn apply_line_format(&mut self, index: usize, mut ed: crate::edit_text_ui::LineEditor, ctx: Option<&egui::Context>) {
        let edit = pdfcraft_engine::Edit::EditTextBlock { page: ed.page, block: ed.block, text: ed.text.clone(), style: ed.style() };
        if self.apply_edit(edit) {
            ed.applied();
            if let Some(doc) = self.session.get(self.views[index].id)
                && let Some(block) = doc.text_blocks(ed.page).get(ed.block)
            {
                ed.refresh_source(block, doc.text_lines(ed.page));
                if let Some(ctx) = ctx {
                    ed.fonts = crate::edit_text_ui::install_source_fonts(ctx, doc, ed.page);
                }
            }
        }
        self.views[index].line_editor = Some(ed);
    }
}

// ───────────────────────────────────────────────────────────────────────────── right panels

pub(crate) enum Nav {
    Page(usize),
    /// A bookmark's destination: its page and where on it.
    Dest(usize, pdfcraft_render::DestView),
    Flash(usize, [f32; 4]),
}

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
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(tl!(title)).font(theme::semibold(15.5)));
                    if let Some(c) = count {
                        ui.label(egui::RichText::new(c.to_string()).font(theme::medium(13.0)).color(t.text_faint));
                    }
                    if panel == RightPanel::Pages && view.selected.len() > 1 {
                        let n = view.selected.len().to_string();
                        let picked = crate::i18n::fmt(tl!("{n} pages selected"), &[("n", &n)]);
                        ui.label(egui::RichText::new(picked).font(theme::medium(12.0)).color(t.accent_text));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icons::button(ui, "x", 26.0, false, tl!("Close")).clicked() {
                            close = true;
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
                let search_id = egui::Id::new(("bookmark_search", id.0));
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
                    RightPanel::Fields => field_properties = fields(ui, &t, info, &doc.form, preparing, &mut view.prepare, &mut nav, &mut panel_edit),
                    RightPanel::Layers => {
                        if info.layers.is_empty() {
                            empty(ui, &t, "layers", "This document has no layers.");
                        }
                        for (li, l) in info.layers.iter().enumerate() {
                            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, l.visible, &l.name));
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
                            egui::Frame::NONE.inner_margin(egui::Margin::symmetric(4, 6)).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.add(icons::image("paperclip", 16.0, t.icon));
                                    ui.vertical(|ui| {
                                        ui.set_width(ui.available_width() - 70.0);
                                        ui.add(egui::Label::new(egui::RichText::new(&a.name).font(theme::medium(13.0))).truncate());
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
                                        ui.add(egui::Label::new(egui::RichText::new(meta).color(t.text_faint).small()).truncate());
                                    });
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if icons::button(ui, "file-down", 28.0, false, tl!("Save attachment…")).clicked() {
                                            attachment_action = Some((ai, false));
                                        }
                                        if a.name.to_lowercase().ends_with(".pdf")
                                            && icons::button(ui, "file-input", 28.0, false, tl!("Open in a new tab")).clicked()
                                        {
                                            attachment_action = Some((ai, true));
                                        }
                                        if let pdfcraft_render::AttachmentSource::Document { key } = &a.source
                                            && attachable
                                            && icons::button(ui, "trash-2", 28.0, false, tl!("Delete attachment")).clicked()
                                        {
                                            panel_edit = Some(pdfcraft_engine::Edit::DeleteAttachment { key: key.clone() });
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

/// What the user asked to do with a bookmark (applied after the panel is drawn).
#[derive(Clone, Debug, PartialEq)]
pub enum BmAction {
    /// New "Untitled" bookmark for the current page, after the selected one (⌘B).
    New,
    StartRename(Vec<usize>),
    Rename(Vec<usize>, String),
    SetToCurrentPage(Vec<usize>),
    /// Make the title bold or italic or not (`None` keeps that one).
    SetStyle(Vec<usize>, Option<bool>, Option<bool>),
    /// Colour the title; black is the default.
    SetColor(Vec<usize>, [f64; 3]),
    Delete(Vec<usize>),
    MoveUp(Vec<usize>),
    MoveDown(Vec<usize>),
    /// Make it the last child of the bookmark above it.
    Indent(Vec<usize>),
    /// Move it out to follow its parent.
    Outdent(Vec<usize>),
    /// Bookmarks from the tagged headings, under a new first "Untitled" bookmark.
    FromStructure,
}

/// Matching titles plus their ancestors, keeping document paths rather than filtered indexes.
/// `query` is folded with [`pdfcraft_render::text::fold_case`], as titles are here.
fn outline_matches(items: &[OutlineItem], query: &str) -> std::collections::HashSet<Vec<usize>> {
    let mut matches = std::collections::HashSet::new();
    if query.is_empty() {
        return matches;
    }
    let mut pending: Vec<_> = items.iter().enumerate().map(|(i, item)| (vec![i], item)).collect();
    while let Some((path, item)) = pending.pop() {
        if pdfcraft_render::text::fold_case(&item.title).contains(query) {
            let mut ancestor = path.clone();
            while !ancestor.is_empty() {
                matches.insert(ancestor.clone());
                ancestor.pop();
            }
        }
        for (i, child) in item.children.iter().enumerate() {
            let mut child_path = path.clone();
            child_path.push(i);
            pending.push((child_path, child));
        }
    }
    matches
}

struct OutlineCtx<'a> {
    nav: &'a mut Option<Nav>,
    action: &'a mut Option<BmAction>,
    rename: &'a mut Option<(Vec<usize>, String)>,
    editable: bool,
    current: usize,
    /// Expand all (`Some(usize::MAX)`), collapse all (`Some(0)`) or expand to a depth, this frame.
    expand: Option<usize>,
    matches: &'a std::collections::HashSet<Vec<usize>>,
    searching: bool,
}

/// The destination page label gets its own right-aligned column in a bookmark row; drawing
/// tools put whole section titles in /PageLabels, so the column is bounded (#124).
const LABEL_COLUMN_MAX: f32 = 72.0;
/// More characters than ever fit [`LABEL_COLUMN_MAX`] at the label size.
const LABEL_MEASURE_CHARS: usize = 64;

/// The longest head of `text` that `fits`, plus an ellipsis when the whole string doesn't.
/// Whole characters are dropped from the end, so the string is only ever cut at a char
/// boundary, and the loop always ends (at the ellipsis alone). A label is document text: only
/// its first [`LABEL_MEASURE_CHARS`] characters are measured, so a huge one can't make each
/// frame lay out thousands of candidates.
pub(crate) fn ellipsized_prefix(text: &str, mut fits: impl FnMut(&str) -> bool) -> String {
    let mut s: String = text.chars().take(LABEL_MEASURE_CHARS).collect();
    if s.len() == text.len() && fits(text) {
        return text.to_owned();
    }
    loop {
        s.pop();
        let candidate = format!("{s}\u{2026}");
        if s.is_empty() || fits(&candidate) {
            return candidate;
        }
    }
}

/// How a bookmark's title is drawn: in its own style and colour (ISO 32000-2 §12.3.3, Tables 151
/// and 152), bold in the heavier face and italic slanted, its colour kept readable on the panel.
/// Top-level bookmarks without a style keep the medium face.
fn title_format(item: &OutlineItem, depth: usize, t: &Tokens) -> egui::TextFormat {
    let font_id = match (item.bold, depth) {
        (true, _) => theme::semibold(13.0),
        (false, 0) => theme::medium(13.0),
        (false, _) => theme::regular(13.0),
    };
    let color = item.color.map_or(t.text, |rgb| {
        // Each component is within 0–1, so the rounded value fits a byte.
        let [r, g, b] = rgb.map(|c| (c * 255.0).round() as u8);
        theme::readable(egui::Color32::from_rgb(r, g, b), t.panel, t.text)
    });
    egui::TextFormat { font_id, color, italics: item.italic, ..Default::default() }
}

fn outline_item(ui: &mut egui::Ui, t: &Tokens, info: &DocInfo, item: &OutlineItem, path: &[usize], siblings: usize, cx: &mut OutlineCtx<'_>) {
    if cx.searching && !cx.matches.contains(path) {
        return;
    }
    let depth = path.len() - 1;
    let indent = depth as f32 * 16.0;
    let id = ui.id().with(("outline", path));
    if let Some(levels) = cx.expand {
        ui.data_mut(|d| d.insert_temp(id, depth < levels));
    }
    // A bookmark opens as the file says at every level: a positive /Count is open, a negative
    // one (or none) closed (ISO 32000-2 §12.3.3, Table 151). The reader's own toggles and a
    // search override it.
    let mut open = cx.searching || ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(item.open);
    let x_text = indent + 20.0;
    if let Some((rpath, text)) = cx.rename.as_mut()
        && rpath.as_slice() == path
    {
        // Inline rename: Enter commits, Escape cancels.
        let mut commit = None;
        let mut cancel = false;
        ui.horizontal(|ui| {
            ui.add_space(x_text);
            crate::widgets::begin_dialog(ui);
            let edit = crate::widgets::line(text).desired_width(ui.available_width() - 8.0).id(id.with("rename"));
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
        // The page label sits right-aligned in its own bounded column, and the title wraps to
        // what is really left of it — a long label painted over a long title was the overlap
        // of #124.
        let label_font = theme::regular(11.0);
        let label_galley = item.page.and_then(|p| info.pages.get(p)).map(|page| {
            ui.fonts_mut(|f| {
                let text =
                    ellipsized_prefix(&page.label, |s| f.layout_no_wrap(s.to_owned(), label_font.clone(), t.text_faint).size().x <= LABEL_COLUMN_MAX);
                f.layout_no_wrap(text, label_font, t.text_faint)
            })
        });
        let label_w = label_galley.as_ref().map_or(0.0, |g| g.size().x);
        let wrap_w = (ui.available_width() - indent - 20.0 - label_w - 12.0).max(60.0);
        let mut job = egui::text::LayoutJob::single_section(item.title.clone(), title_format(item, depth, t));
        job.wrap.max_width = wrap_w;
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        let h = (galley.size().y + 12.0).max(28.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &item.title));
        theme::Press::track(ui, &resp).wash(ui, rect, 6, false);
        let x0 = rect.left() + indent;
        if !item.children.is_empty() {
            let tri = Rect::from_min_size(pos2(x0, rect.top() + 6.0), vec2(16.0, 16.0));
            icons::paint(ui, tri, if open { "chevron-down" } else { "chevron-right" }, 14.0, t.text_muted);
            if !cx.searching && ui.interact(tri, id.with("tri"), Sense::click()).clicked() {
                open = !open;
                ui.data_mut(|d| d.insert_temp(id, open));
            }
        }
        ui.painter().galley(pos2(x0 + 20.0, rect.top() + 6.0), galley, t.text);
        if let Some(g) = label_galley {
            // Right-aligned, vertically centred at the first line, inside the column the title
            // wrapped around.
            let pos = pos2(rect.right() - 6.0 - g.size().x, rect.top() + 14.0 - g.size().y / 2.0);
            ui.painter().galley(pos, g, t.text_faint);
        }
        if theme::hand(resp.clone()).clicked()
            && let Some(p) = item.page
        {
            *cx.nav = Some(Nav::Dest(p, item.view));
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
                    if ui.add_enabled(enabled, egui::Button::new(tl!(label))).clicked() {
                        *cx.action = Some(a);
                        ui.close();
                    }
                };
                pick(ui, tl!("Rename"), true, BmAction::StartRename(path.to_vec()));
                let set_page = crate::i18n::fmt(tl!("Set to current page ({n})"), &[("n", &(current + 1).to_string())]);
                pick(ui, &set_page, true, BmAction::SetToCurrentPage(path.to_vec()));
                // The title's style and colour (ISO 32000-2 §12.3.3, Tables 151 and 152).
                let mut styled = None;
                ui.menu_button(tl!("Style"), |ui| {
                    let (mut bold, mut italic) = (item.bold, item.italic);
                    if ui.add(crate::widgets::checkbox(&mut bold, tl!("Bold"))).clicked() {
                        styled = Some(BmAction::SetStyle(path.to_vec(), Some(bold), None));
                    }
                    if ui.add(crate::widgets::checkbox(&mut italic, tl!("Italic"))).clicked() {
                        styled = Some(BmAction::SetStyle(path.to_vec(), None, Some(italic)));
                    }
                });
                ui.menu_button(tl!("Colour"), |ui| {
                    let current = item.color.map_or([0.0; 3], |c| c.map(f64::from));
                    if let Some(c) = crate::comments::swatch_grid(ui, Some(current)) {
                        styled = Some(BmAction::SetColor(path.to_vec(), c));
                    }
                });
                if styled.is_some() {
                    ui.close();
                }
                ui.separator();
                pick(ui, tl!("Move up"), i > 0, BmAction::MoveUp(path.to_vec()));
                pick(ui, tl!("Move down"), i + 1 < siblings, BmAction::MoveDown(path.to_vec()));
                pick(ui, tl!("Indent"), i > 0, BmAction::Indent(path.to_vec()));
                pick(ui, tl!("Outdent"), depth > 0, BmAction::Outdent(path.to_vec()));
                ui.separator();
                pick(ui, tl!("Delete"), true, BmAction::Delete(path.to_vec()));
                if let Some(a) = styled {
                    *cx.action = Some(a);
                }
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

/// Size controls stay above the scroll area, so they remain reachable on long documents.
fn thumbnail_size(ui: &mut egui::Ui, t: &Tokens, view: &mut crate::DocView, enabled: bool) {
    let before = view.thumbnail_zoom();
    let mut zoom = before;
    let label = ui
        .horizontal(|ui| {
            let label = ui.label(egui::RichText::new(tl!("Thumbnail size")).small().color(t.text_muted));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(egui::RichText::new(format!("{:.0}%", before * 100.0)).small().color(t.text_muted));
            });
            label
        })
        .inner;
    ui.add_enabled_ui(enabled, |ui| {
        ui.spacing_mut().slider_width = (ui.available_width() - 52.0 - 2.0 * ui.spacing().item_spacing.x).max(1.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled_ui(zoom > *crate::canvas::THUMBNAIL_ZOOM_RANGE.start(), |ui| {
                    icons::button(ui, "zoom-out", 26.0, false, tl!("Smaller thumbnails"))
                })
                .inner
                .clicked()
            {
                zoom -= 0.25;
            }
            let mut percent = zoom * 100.0;
            let range = (*crate::canvas::THUMBNAIL_ZOOM_RANGE.start() * 100.0)..=(*crate::canvas::THUMBNAIL_ZOOM_RANGE.end() * 100.0);
            ui.scope(|ui| {
                // The regular field fill blends into this panel; keep the slider's full rail visible.
                ui.visuals_mut().widgets.inactive.bg_fill = t.border;
                let slider = ui.add(egui::Slider::new(&mut percent, range).step_by(1.0).show_value(false).trailing_fill(true)).labelled_by(label.id);
                slider.widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), f64::from(percent), tl!("Thumbnail size")));
            });
            zoom = percent / 100.0;
            if ui
                .add_enabled_ui(zoom < *crate::canvas::THUMBNAIL_ZOOM_RANGE.end(), |ui| {
                    icons::button(ui, "zoom-in", 26.0, false, tl!("Larger thumbnails"))
                })
                .inner
                .clicked()
            {
                zoom += 0.25;
            }
        });
    });
    if zoom != before {
        view.set_thumbnail_zoom(zoom);
    }
    ui.add_space(6.0);
}

/// The page thumbnails. A click goes to the page; ⌘/Ctrl-click and ⇧-click pick several pages
/// (the selection page commands and Print act on) without moving the document. Right-click
/// offers Extract, Cut, Copy and Paste on the selection, or on the page clicked; dragging
/// moves the selection, or the page grabbed, to the gap it is dropped on.
fn pages(ui: &mut egui::Ui, t: &Tokens, info: &DocInfo, view: &mut crate::DocView, modal: bool, editable: bool, nav: &mut Option<Nav>) {
    // Escape drops the selection while the pointer is over the panel and nothing else wants the key.
    if !modal
        && !view.selected.is_empty()
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.rect_contains_pointer(ui.clip_rect())
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        view.clear_page_selection(Some(view.current));
    }
    // Delete (or Backspace) deletes the selection, or the current page, likewise.
    if !modal
        && editable
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.rect_contains_pointer(ui.clip_rect())
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete) || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace))
    {
        delete_pages(view, info.pages.len());
    }
    // A new current page scrolls its thumbnail into view, as little as it takes (Acrobat), but
    // not while thumbnails are dragged: then the change counts as seen. A wheel turned or a
    // scroll bar dragged by the user already wins over a scroll to a place in egui.
    let follow = view.panel_followed != Some(view.current) && view.panel_drag.is_none();
    view.panel_followed = Some(view.current);
    let available = (ui.available_width() - 16.0).max(1.0);
    let usual = (ui.available_width() - 40.0).clamp(1.0, 150.0);
    let w = (usual * view.thumbnail_zoom()).min(available);
    let count = info.pages.len();
    let mut rows: Vec<Rect> = Vec::with_capacity(info.pages.len());
    let mut dropped = false;
    for (i, p) in info.pages.iter().enumerate() {
        ui.vertical_centered(|ui| {
            let h = w * p.height / p.width.max(1.0);
            let sense = if editable { Sense::click_and_drag() } else { Sense::click() };
            let (rect, resp) = ui.allocate_exact_size(vec2(w + 16.0, h + 16.0), sense);
            rows.push(rect);
            if rect.intersects(ui.clip_rect().expand(200.0)) {
                view.need_thumbnail(i, rect.intersects(ui.clip_rect()));
            }
            // Only the primary button drags pages (the middle button scrolls, the secondary one opens the menu).
            if resp.drag_started_by(egui::PointerButton::Primary) {
                if !view.target_pages().contains(&i) {
                    view.click_page(i, egui::Modifiers::NONE, true);
                }
                view.panel_drag = Some(view.target_pages());
            }
            if resp.drag_stopped() {
                dropped = true;
            }
            let info = crate::i18n::fmt(tl!("Page {label}"), &[("label", &p.label)]);
            let picked = view.selected.contains(&i);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, picked, info.clone()));
            // The current page keeps its heavier border; picked pages share its fill.
            let selected = i == view.current;
            let press = theme::Press::track(ui, &resp);
            press.wash(ui, rect, 8, selected || picked);
            if picked {
                ui.painter().rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
            }
            let pr = rect.translate(press.offset()).shrink(8.0);
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
            let label = ui.label(egui::RichText::new(&p.label).font(theme::medium(12.0)).color(if selected || picked {
                t.accent_text
            } else {
                t.text_muted
            }));
            if follow && selected {
                ui.scroll_to_rect(rect.union(label.rect), None);
            }
            let resp = theme::hand(resp);
            if resp.clicked() {
                let m = ui.input(|i| i.modifiers);
                if m.shift || m.command {
                    view.click_page(i, m, true);
                } else {
                    view.clear_page_selection(Some(i));
                    // A page clicked here is in view already: the panel stays put.
                    view.panel_followed = Some(i);
                    *nav = Some(Nav::Page(i));
                }
            }
            resp.context_menu(|ui| {
                // A page outside the selection becomes the selection, as in the organize grid.
                if !view.target_pages().contains(&i) {
                    view.click_page(i, egui::Modifiers::NONE, true);
                }
                if ui.button(tl!("Extract pages")).clicked() {
                    view.pending_action = Some(crate::canvas::ViewAction::Extract);
                    ui.close();
                }
                if ui.button(tl!("Split document…")).clicked() {
                    if view.selected.is_empty() {
                        view.select_pages(&[i]);
                    }
                    view.pending_action = Some(crate::canvas::ViewAction::SplitBeforeSelected);
                    ui.close();
                }
                if ui.add_enabled(editable, egui::Button::new(tl!("Replace pages…"))).clicked() {
                    view.pending_action = Some(crate::canvas::ViewAction::ReplacePages);
                    ui.close();
                }
                ui.separator();
                if ui.add_enabled(editable, egui::Button::new(tl!("Cut"))).clicked() {
                    view.pending_action = Some(crate::canvas::ViewAction::CopyPages { cut: true });
                    ui.close();
                }
                if ui.button(tl!("Copy")).clicked() {
                    view.pending_action = Some(crate::canvas::ViewAction::CopyPages { cut: false });
                    ui.close();
                }
                if ui.add_enabled(editable, egui::Button::new(tl!("Paste after"))).clicked() {
                    view.pending_action = Some(crate::canvas::ViewAction::PastePages);
                    ui.close();
                }
                ui.separator();
                let some_left = view.target_pages().len() < count;
                // Never every page: a document keeps at least one.
                if ui.add_enabled(editable && some_left, egui::Button::new(tl!("Delete pages"))).clicked() {
                    delete_pages(view, count);
                    ui.close();
                }
            });
        });
        ui.add_space(4.0);
    }
    page_drag(ui, t, view, &rows, dropped);
    file_drop(ui, t, view, &rows, editable && !modal);
}

/// Files dragged in from outside go to the gap under the pointer: remember where the panel is
/// and that gap, for the app to take the drop, and show where the pages would go.
fn file_drop(ui: &mut egui::Ui, t: &Tokens, view: &mut crate::DocView, rows: &[Rect], editable: bool) {
    if !editable {
        return;
    }
    let clip = ui.clip_rect();
    view.pages_panel = Some(clip);
    let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|p| clip.contains(*p));
    view.panel_gap = pointer.filter(|_| view.panel_drag.is_none()).and_then(|p| panel_gap(rows, p.y));
    if pointer.is_none() || ui.input(|i| i.raw.hovered_files.is_empty()) {
        return;
    }
    let painter = ui.painter().with_clip_rect(clip);
    painter.rect_stroke(clip.shrink(2.0), CornerRadius::same(6), Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
    if let (Some(gap), Some(first), Some(last)) = (view.panel_gap, rows.first(), rows.last()) {
        let y = rows.get(gap).map_or(last.bottom() + 2.0, |r| r.top() - 3.0);
        painter.line_segment([pos2(first.left(), y), pos2(first.right(), y)], Stroke::new(3.0, t.accent));
    }
}

/// Queue deleting the selection (or the current page) of a document of `n` pages, unless that
/// would leave no page. The selection goes; the current page stays on the page it showed, or on
/// the page that follows the deleted ones.
fn delete_pages(view: &mut crate::DocView, n: usize) {
    let mut pages = view.target_pages();
    pages.sort_unstable();
    pages.dedup();
    if pages.is_empty() || pages.len() >= n {
        return;
    }
    // Deleted pages before it shift it up; if it was deleted itself, the page after the deleted
    // run lands where it was (or the last page, when the deletion ran to the end).
    let before = pages.iter().filter(|p| **p < view.current).count();
    view.current = view.current.saturating_sub(before).min(n.saturating_sub(pages.len() + 1));
    view.clear_page_selection(None);
    view.pending_edit = Some(pdfcraft_engine::Edit::DeletePages { pages });
}

/// The gap (0 = before the first page, n = after the last) the pointer points at in the Pages
/// panel, whose thumbnails are `rows`, top to bottom.
fn panel_gap(rows: &[Rect], y: f32) -> Option<usize> {
    let (i, r) = rows.iter().enumerate().min_by(|(_, a), (_, b)| (a.center().y - y).abs().total_cmp(&(b.center().y - y).abs()))?;
    Some(if y < r.center().y { i } else { i + 1 })
}

/// Where page `p` goes when `moving` (sorted) move to `to` (counted without them).
fn moved_index(p: usize, moving: &[usize], to: usize) -> usize {
    match moving.iter().position(|m| *m == p) {
        Some(k) => to + k,
        None => {
            let rest = p.saturating_sub(moving.iter().filter(|m| **m < p).count());
            if rest >= to { rest + moving.len() } else { rest }
        }
    }
}

/// While thumbnails are dragged: a bar on the gap they would go to, the page count by the
/// pointer, and scrolling near the panel's edges. On release, queue the move.
fn page_drag(ui: &mut egui::Ui, t: &Tokens, view: &mut crate::DocView, rows: &[Rect], dropped: bool) {
    let Some(mut moving) = view.panel_drag.clone() else { return };
    // A drag that ended while the panel was closed (or elsewhere) leaves nothing to drop.
    if !dropped && !ui.input(|i| i.pointer.primary_down()) {
        view.panel_drag = None;
        return;
    }
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let gap = pointer.and_then(|p| panel_gap(rows, p.y));
    if let (Some(gap), Some(first), Some(last)) = (gap, rows.first(), rows.last()) {
        let y = rows.get(gap).map_or(last.bottom() + 20.0, |r| r.top() - 3.0);
        ui.painter().line_segment([pos2(first.left(), y), pos2(first.right(), y)], Stroke::new(3.0, t.accent));
    }
    if let Some(p) = pointer {
        let n = moving.len();
        let what = if n == 1 { tl!("1 page").to_string() } else { crate::i18n::fmt(tl!("{n} pages"), &[("n", &n.to_string())]) };
        ui.painter().text(p + vec2(14.0, 14.0), Align2::LEFT_TOP, what, theme::medium(12.0), t.accent_text);
        let clip = ui.clip_rect();
        let edge = 40.0;
        if p.y < clip.top() + edge {
            ui.scroll_with_delta(vec2(0.0, 10.0));
        } else if p.y > clip.bottom() - edge {
            ui.scroll_with_delta(vec2(0.0, -10.0));
        }
        ui.ctx().request_repaint();
    }
    if !dropped {
        return;
    }
    view.panel_drag = None;
    let Some(gap) = gap else { return };
    moving.sort_unstable();
    moving.dedup();
    let Some(&first) = moving.first() else { return };
    // `to` counts positions without the moving pages.
    let to = gap.saturating_sub(moving.iter().filter(|x| **x < gap).count());
    let contiguous = moving.windows(2).all(|w| w[1] == w[0] + 1);
    if contiguous && to == first {
        return;
    }
    view.current = moved_index(view.current, &moving, to);
    view.selected = (to..to + moving.len()).collect();
    view.pending_edit = Some(pdfcraft_engine::Edit::MovePages { pages: moving, to });
}

/// The Fields panel: fields by page, in tab order. While preparing a form, each field can move
/// earlier or later in its page's tab order (Acrobat: Order Tabs Manually).
#[allow(clippy::too_many_arguments)]
fn fields(
    ui: &mut egui::Ui,
    t: &Tokens,
    info: &DocInfo,
    form: &[pdfcraft_engine::FormField],
    preparing: bool,
    selection: &mut crate::prepare::PrepareView,
    nav: &mut Option<Nav>,
    edit: &mut Option<pdfcraft_engine::Edit>,
) -> Option<(String, usize)> {
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
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, &f.name));
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
                    let b = ui.interact(r, ui.id().with((&f.name, earlier)), Sense::click());
                    b.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{tip}: {}", f.name)));
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

pub fn human_size(n: usize) -> String {
    match n {
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KB", n as f64 / (1u64 << 10) as f64),
        n => byte_count(n),
    }
}

/// An exact size in bytes, with the count's own plural form.
pub fn byte_count(n: usize) -> String {
    crate::i18n::trn(crate::i18n::current(), n as u64, "{n} byte", "{n} bytes")
}

#[cfg(test)]
mod page_moves {
    use super::{moved_index, panel_gap};
    use egui::{Rect, pos2};

    /// Above a thumbnail's middle is the gap before it; below is the gap after it.
    #[test]
    fn the_gap_follows_the_nearest_thumbnail() {
        let rows: Vec<Rect> = (0..3).map(|i| Rect::from_min_max(pos2(0.0, i as f32 * 100.0), pos2(50.0, i as f32 * 100.0 + 90.0))).collect();
        assert_eq!(panel_gap(&rows, -20.0), Some(0));
        assert_eq!(panel_gap(&rows, 30.0), Some(0));
        assert_eq!(panel_gap(&rows, 60.0), Some(1));
        assert_eq!(panel_gap(&rows, 260.0), Some(3));
        assert_eq!(panel_gap(&rows, 900.0), Some(3));
        assert_eq!(panel_gap(&[], 10.0), None);
    }

    /// The current page follows a move, whether it moved or others moved around it.
    #[test]
    fn pages_keep_their_identity_across_a_move() {
        // [0 1 2 3 4], move [1 2] to the end: [0 3 4 1 2].
        let order: Vec<usize> = (0..5).map(|p| moved_index(p, &[1, 2], 3)).collect();
        assert_eq!(order, [0, 3, 4, 1, 2]);
        // Move [3] to the front: [3 0 1 2 4].
        let order: Vec<usize> = (0..5).map(|p| moved_index(p, &[3], 0)).collect();
        assert_eq!(order, [1, 2, 3, 0, 4]);
    }
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

#[cfg(test)]
mod label_column {
    use super::ellipsized_prefix;

    /// A short page label (the usual "3", "iv", "1-1") is shown unchanged.
    #[test]
    fn short_labels_are_not_touched() {
        for label in ["3", "iv", "1-1", "A-201"] {
            assert_eq!(ellipsized_prefix(label, |s| s.len() <= 10), label);
        }
    }

    /// A label wider than its column (#124: whole section titles in /PageLabels) keeps only
    /// the head that fits, with an ellipsis, and that head is a prefix of the real label.
    #[test]
    fn long_labels_are_cut_to_their_column_with_an_ellipsis() {
        let label = "Floor Plans (001 Floor Plans): GROUND FLOOR PLAN";
        let shown = ellipsized_prefix(label, |s| s.len() <= 20);
        assert!(shown.ends_with('\u{2026}'), "{shown:?}");
        assert!(shown.len() <= 20, "{shown:?}");
        assert!(label.starts_with(shown.trim_end_matches('\u{2026}')), "{shown:?}");
    }

    /// CJK labels (three bytes per character) are cut on character boundaries too.
    #[test]
    fn multibyte_labels_are_cut_on_char_boundaries() {
        let label = "図面（一階平面図）：配置図";
        let shown = ellipsized_prefix(label, |s| s.len() <= 12);
        assert!(shown.ends_with('\u{2026}'), "{shown:?}");
        assert!(shown.len() <= 12, "{shown:?}");
        assert!(label.starts_with(shown.trim_end_matches('\u{2026}')), "{shown:?}");
    }

    /// The pathological floor: nothing fits, the loop still ends with the lone ellipsis.
    #[test]
    fn the_loop_ends_when_nothing_fits() {
        assert_eq!(ellipsized_prefix("Floor Plans", |_| false), "\u{2026}");
    }

    /// A huge label from a hostile file is measured a bounded number of times per frame.
    #[test]
    fn a_huge_label_is_measured_a_bounded_number_of_times() {
        let label = "x".repeat(1_000_000);
        let mut calls = 0;
        let shown = ellipsized_prefix(&label, |s| {
            calls += 1;
            s.len() <= 12
        });
        assert!(calls <= super::LABEL_MEASURE_CHARS + 1, "{calls} measurements");
        assert!(shown.ends_with('\u{2026}') && label.starts_with(shown.trim_end_matches('\u{2026}')), "{shown:?}");
    }
}

#[cfg(test)]
mod bookmark_filter {
    use super::{outline_matches, title_format};
    use crate::theme::{self, Tokens};
    use pdfcraft_render::OutlineItem;

    /// #3193: a bookmark's title is drawn in its own style and colour.
    #[test]
    fn bookmark_titles_take_their_style_and_colour() {
        let t = Tokens::for_kind(crate::theme::ThemeKind::Light);
        let plain = item("Plain", Vec::new());
        assert_eq!(title_format(&plain, 0, &t).font_id, theme::medium(13.0));
        assert_eq!(title_format(&plain, 1, &t).font_id, theme::regular(13.0));
        assert_eq!(title_format(&plain, 1, &t).color, t.text);
        assert!(!title_format(&plain, 1, &t).italics);
        let styled = OutlineItem { color: Some([1.0, 0.0, 0.0]), italic: true, bold: true, ..item("Styled", Vec::new()) };
        let f = title_format(&styled, 1, &t);
        assert_eq!(f.font_id, theme::semibold(13.0));
        assert!(f.italics);
        assert_eq!(f.color, egui::Color32::from_rgb(255, 0, 0));
        let half = OutlineItem { color: Some([0.0, 0.5, 0.0]), ..item("Green", Vec::new()) };
        assert_eq!(title_format(&half, 1, &t).color, egui::Color32::from_rgb(0, 128, 0));
    }

    fn item(title: &str, children: Vec<OutlineItem>) -> OutlineItem {
        OutlineItem {
            title: title.into(),
            page: Some(0),
            view: pdfcraft_render::DestView::Top,
            children,
            open: true,
            color: None,
            italic: false,
            bold: false,
        }
    }

    /// Turkish dotted and dotless i match in any case, as in Find (#892).
    #[test]
    fn titles_match_in_any_case_with_turkish_i() {
        let outline = [item("İSTANBUL ŞUBESİ", vec![item("Kış dönemi", vec![])]), item("Ankara", vec![])];
        let fold = pdfcraft_render::text::fold_case;
        assert_eq!(outline_matches(&outline, &fold("istanbul")).len(), 1);
        assert_eq!(outline_matches(&outline, &fold("şubesi")).len(), 1);
        // A child match keeps its parent: [0] and [0, 0].
        assert_eq!(outline_matches(&outline, &fold("KIŞ")).len(), 2);
        assert!(outline_matches(&outline, &fold("izmir")).is_empty());
    }
}
