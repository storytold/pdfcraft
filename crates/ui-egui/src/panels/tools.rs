use super::edit::{actions_menu, edit_panel, stamp_palette};
use super::*;

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

/// A panel's title row: a leading button (`lead`: its icon and tooltip) when given, the title
/// and Close. Returns whether the leading button and Close were clicked.
pub(crate) fn panel_header(ui: &mut egui::Ui, t: &Tokens, title: &str, lead: Option<(&str, &str)>) -> (bool, bool) {
    let mut go_back = false;
    let mut close = false;
    ui.horizontal(|ui| {
        if let Some((icon, tip)) = lead
            && icons::button(ui, icon, 26.0, false, tip).clicked()
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

/// The All tools sidebar: Expand (the full view) and Close, then the tool list, the first few
/// tools until View more shows them all.
fn all_tools(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    // Labels distinct from the Combine grid's Expand and Collapse, which can show beside them.
    let (expand, close) = panel_header(ui, t, "All tools", Some(("maximize-2", tl!("Expand all tools"))));
    if expand {
        app.expand_all_tools();
    }
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
pub(super) fn row_label(ui: &mut egui::Ui, text: &str, font: egui::FontId, color: Color32, left_center: egui::Pos2, max_width: f32) -> bool {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: max_width.max(1.0), max_rows: 1, break_anywhere: true, ..Default::default() };
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let elided = galley.elided;
    ui.painter().galley(left_center - vec2(0.0, galley.size().y / 2.0), galley, color);
    elided
}

/// Draw one tool-group row with its icon, label, badge and availability hint.
fn tool_row(ui: &mut egui::Ui, t: &Tokens, g: &ToolGroup) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!(g.label)));
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

/// Draw the detail view for one tool group, dispatching its commands and tool-specific panels.
fn tool_detail(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens, g: &'static ToolGroup) {
    if g.id == "edit" {
        edit_panel(app, ui, t, g);
        return;
    }
    let (back, close) = panel_header(ui, t, g.label, Some(("chevron-left", tl!("Back to all tools"))));
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
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!(item.label)));
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
