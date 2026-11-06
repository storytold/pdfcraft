//! A movable, persistent set of shortcuts to the shared command registry and core document tools.
use egui::{Color32, CornerRadius, Rect, Sense, Stroke, vec2};
use pdfcraft_engine::Edit;
use pdfcraft_engine::commands::{COMMANDS, command};
use serde::{Deserialize, Serialize};

use crate::{PdfCraftApp, QuickTool, comments, icons, theme, theme::Tokens};

/// Persistent state and UI state for the customizable document hotbar.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Hotbar {
    pub tools: Vec<String>,
    /// Position as a fraction of the available travel, so resizing keeps the bar visible.
    pub position: [f32; 2],
    pub vertical: bool,
    pub button_size: f32,
    pub dock: Option<String>,
    pub full_side: bool,
    #[serde(default)]
    pub placed: bool,
    #[serde(skip)]
    search: String,
    #[serde(skip)]
    picker_open: bool,
}

impl Default for Hotbar {
    /// Create a default vertical floating hotbar positioned in the document margin.
    fn default() -> Self {
        Self {
            tools: Vec::new(),
            position: [0.0, 0.0],
            vertical: true,
            button_size: 32.0,
            dock: None,
            full_side: false,
            placed: false,
            search: String::new(),
            picker_open: false,
        }
    }
}

impl Hotbar {
    /// Clamp button size and normalized position coordinates, drop unknown docked sides,
    /// and deduplicate/filter tool command identifiers against the command registry.
    pub fn normalize(&mut self) {
        self.button_size = if self.button_size.is_finite() { self.button_size.clamp(24.0, 64.0) } else { 32.0 };
        if self.dock.as_deref().is_some_and(|side| !matches!(side, "left" | "right" | "top" | "bottom")) {
            self.dock = None;
        }
        let mut seen = std::collections::HashSet::new();
        self.tools.retain(|id| command(id).is_some() && seen.insert(id.clone()));
        for coordinate in &mut self.position {
            *coordinate = if coordinate.is_finite() { coordinate.clamp(0.0, 1.0) } else { 0.5 };
        }
        if self.dock.is_some() || self.position != [0.0, 0.0] {
            self.placed = true;
        }
    }

    /// Adopt preferences saved before the hotbar absorbed the quick-action bar (#5890), which have
    /// no `placed` key. A bar left at that release's default (floating, horizontal, top centre)
    /// moves to the new default in the left margin; any other spot is one the user chose, so it
    /// is kept, including the top-left corner.
    pub fn adopt_legacy(&mut self) {
        if self.dock.is_none() && !self.vertical && self.position == [0.5, 0.0] {
            self.position = [0.0, 0.0];
            self.vertical = true;
            self.full_side = false;
            self.placed = false;
        } else {
            self.placed = true;
        }
    }

    /// Parse and validate a comma-separated list of command identifiers for the hotbar.
    pub fn set_tools(&mut self, value: &str) -> Result<(), String> {
        let tools: Vec<String> = value.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect();
        if let Some(id) = tools.iter().find(|id| command(id).is_none()) {
            return Err(format!("Unknown hotbar command: {id}"));
        }
        self.tools = tools;
        self.normalize();
        Ok(())
    }

    /// Dock the hotbar to `"left"`, `"right"`, `"top"`, or `"bottom"`, or undock with `"float"`.
    pub fn place(&mut self, side: &str) -> Result<(), String> {
        if side == "float" {
            self.dock = None;
            self.placed = true;
            return Ok(());
        }
        let (position, vertical) = match side {
            "left" => ([0.0, 0.5], true),
            "right" => ([1.0, 0.5], true),
            "top" => ([0.5, 0.0], false),
            "bottom" => ([0.5, 1.0], false),
            _ => return Err("hotbar-side must be float, left, right, top or bottom".into()),
        };
        self.dock = Some(side.to_owned());
        self.position = position;
        self.vertical = vertical;
        self.placed = true;
        Ok(())
    }
}

/// Render the hotbar settings gear button and open the tool picker when clicked.
fn settings_button(ui: &mut egui::Ui, cell: f32, picker_open: &mut bool) -> bool {
    let settings = ui
        .push_id("hotbar-settings", |ui| {
            icons::button(ui, "settings", cell, false, tl!("Hotbar settings")).on_hover_text(tl!("Hotbar settings / add tools"))
        })
        .inner;
    if settings.clicked() {
        *picker_open = true;
        true
    } else {
        false
    }
}

/// Determine where flyout popup menus should open from a toolbar button based on docking and layout.
fn popup_align_for(dock: Option<&str>, vertical: bool) -> egui::RectAlign {
    match dock {
        Some("right") => egui::RectAlign::LEFT_START,
        Some("bottom") => egui::RectAlign::TOP_START,
        Some("top") => egui::RectAlign::BOTTOM_START,
        _ => {
            if vertical {
                egui::RectAlign::RIGHT_START
            } else {
                egui::RectAlign::BOTTOM_START
            }
        }
    }
}

/// Whether `spec` matches the hotbar picker search query across the translated label, canonical
/// English label, or command identifier (case-insensitively).
fn matches_search(spec: &pdfcraft_engine::commands::CommandSpec, query: &str) -> bool {
    let search = query.to_lowercase();
    tl!(spec.label).to_lowercase().contains(&search) || spec.label.to_lowercase().contains(&search) || spec.id.to_lowercase().contains(&search)
}

/// Render the configured custom tool shortcut buttons and their removal context menus.
fn tool_buttons(ui: &mut egui::Ui, app: &PdfCraftApp, tools: &[String], cell: f32, run: &mut Option<&'static str>, remove: &mut Option<usize>) {
    for (index, id) in tools.iter().enumerate() {
        let Some(spec) = command(id) else { continue };
        let active = crate::comments::CommentTool::from_command(id).is_some_and(|tool| app.quick_tool == crate::QuickTool::Comment(tool));
        let button = ui.add_enabled_ui(app.command_enabled(spec), |ui| icons::button(ui, spec.icon, cell, active, tl!(spec.label))).inner;
        if button.clicked() {
            *run = Some(spec.id);
        }
        button.context_menu(|ui| {
            if ui.button(tl!("Remove from hotbar")).clicked() {
                *remove = Some(index);
                ui.close();
            }
        });
    }
}

/// Render core quick tools: Select, Hand, Comment groups, Edit text, and Fill & Sign.
fn core_tool_buttons(ui: &mut egui::Ui, app: &mut PdfCraftApp, cell: f32, popup_align: egui::RectAlign, t: &Tokens) {
    if icons::button(ui, "mouse-pointer-2", cell, app.quick_tool == QuickTool::Select, tl!("Select (V)")).clicked() {
        app.quick_tool = QuickTool::Select;
    }
    if icons::button(ui, "hand", cell, app.quick_tool == QuickTool::Hand, tl!("Hand (H)")).clicked() {
        app.quick_tool = QuickTool::Hand;
    }
    // Comment ▸, Highlight ▸, Draw ▸ (Acrobat's comment toolbar groups). Clicking a
    // group selects its last-used tool; clicking it again opens the flyout.
    for g in 0..comments::GROUPS.len() {
        let current = app.comment_prefs.group_tool[g];
        let active = matches!(app.quick_tool, QuickTool::Comment(t) if t.group() == g);
        let resp = icons::button(ui, current.icon(), cell, active, tl!(current.label()));
        if comments::GROUPS[g].len() == 1 {
            if resp.clicked() {
                app.execute(current.command());
            }
            continue;
        }
        // A small corner triangle marks the flyout.
        let r = resp.rect;
        let tri = [r.right_bottom() + vec2(-4.0, -4.0), r.right_bottom() + vec2(-9.0, -4.0), r.right_bottom() + vec2(-4.0, -9.0)];
        ui.painter().add(egui::Shape::convex_polygon(tri.to_vec(), if active { t.accent } else { t.text_faint }, Stroke::NONE));
        let open = (resp.clicked() && active) || resp.secondary_clicked();
        if resp.clicked() && !active {
            app.execute(current.command());
        }
        egui::Popup::menu(&resp).open_memory(open.then_some(egui::SetOpenCommand::Toggle)).align(popup_align).gap(6.0).show(|ui| {
            for tool in comments::GROUPS[g] {
                let on = app.quick_tool == QuickTool::Comment(*tool);
                let (row, click) = ui.allocate_exact_size(vec2(180.0, 28.0), Sense::click());
                let press = theme::Press::track(ui, &click);
                press.wash(ui, row, 4, on);
                let body = row.translate(press.offset());
                icons::paint(ui, Rect::from_min_size(body.min + vec2(8.0, 6.0), vec2(16.0, 16.0)), tool.icon(), 16.0, t.text);
                let reserve = if on { 28.0 } else { 8.0 };
                crate::widgets::paint_left(
                    ui,
                    body.left_center() + vec2(34.0, 0.0),
                    tl!(tool.label()),
                    theme::regular(13.0),
                    t.text,
                    (row.width() - 34.0 - reserve).max(0.0),
                );
                if on {
                    icons::paint(ui, Rect::from_min_size(body.right_top() + vec2(-24.0, 7.0), vec2(14.0, 14.0)), "check", 14.0, t.accent);
                }
                let click = theme::hand(click);
                let info = tl!(tool.label()).to_string();
                click.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, info.clone()));
                if click.clicked() {
                    app.execute(tool.command());
                    ui.close();
                }
            }
        });
    }
    // Edit text & images, while the Edit workspace is open (Acrobat's Edit toolbar)
    // or the tool is in use.
    if app.mode == crate::Mode::Edit || app.quick_tool == QuickTool::EditText {
        let on = app.quick_tool == QuickTool::EditText;
        if icons::button(ui, "text-cursor-input", cell, on, tl!("Edit text & images")).clicked() {
            if on {
                app.put_down_edit_text();
            } else {
                app.quick_tool = QuickTool::EditText;
            }
        }
    }
    // Fill & Sign ▸ (text, marks, date, signature).
    let current_fill = match app.quick_tool {
        QuickTool::Fill(f) => Some(f),
        _ => None,
    };
    let shown = current_fill.unwrap_or(app.fill_tool);
    let tip = match shown {
        crate::fill_sign::FillTool::Signature | crate::fill_sign::FillTool::Initials if current_fill.is_none() => {
            let initials = shown == crate::fill_sign::FillTool::Initials;
            let saved = if initials { app.initials.is_some() } else { app.signature.is_some() };
            crate::i18n::fmt(
                tl!(if saved { "Place saved {what}" } else { "Add {what}" }),
                &[("what", tl!(if initials { "initials" } else { "signature" }))],
            )
        }
        _ => tl!(shown.label()).to_string(),
    };
    let tip = format!("{}: {tip}", tl!("Fill & Sign"));
    let resp = icons::button(ui, shown.icon(), cell, current_fill.is_some(), &tip);
    let r = resp.rect;
    let tri = [r.right_bottom() + vec2(-4.0, -4.0), r.right_bottom() + vec2(-9.0, -4.0), r.right_bottom() + vec2(-4.0, -9.0)];
    ui.painter().add(egui::Shape::convex_polygon(tri.to_vec(), if current_fill.is_some() { t.accent } else { t.text_faint }, Stroke::NONE));
    if resp.clicked() && current_fill.is_none() {
        app.execute(shown.command());
    }
    let open = (resp.clicked() && current_fill.is_some()) || resp.secondary_clicked();
    egui::Popup::menu(&resp).open_memory(open.then_some(egui::SetOpenCommand::Toggle)).align(popup_align).gap(6.0).show(|ui| {
        for tool in crate::fill_sign::FILL_TOOLS {
            if matches!(tool, crate::fill_sign::FillTool::Signature | crate::fill_sign::FillTool::Initials) {
                continue;
            }
            let on = current_fill == Some(tool);
            let (row, click) = ui.allocate_exact_size(vec2(180.0, 28.0), Sense::click());
            let press = theme::Press::track(ui, &click);
            press.wash(ui, row, 4, on);
            let body = row.translate(press.offset());
            icons::paint(ui, Rect::from_min_size(body.min + vec2(8.0, 6.0), vec2(16.0, 16.0)), tool.icon(), 16.0, t.text);
            let reserve = if on { 28.0 } else { 8.0 };
            crate::widgets::paint_left(
                ui,
                body.left_center() + vec2(34.0, 0.0),
                tl!(tool.label()),
                theme::regular(13.0),
                t.text,
                (row.width() - 34.0 - reserve).max(0.0),
            );
            if on {
                icons::paint(ui, Rect::from_min_size(body.right_top() + vec2(-24.0, 7.0), vec2(14.0, 14.0)), "check", 14.0, t.accent);
            }
            let click = theme::hand(click);
            let info = tl!(tool.label()).to_string();
            click.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, info.clone()));
            if click.clicked() {
                app.execute(tool.command());
                ui.close();
            }
        }
        ui.separator();
        if let Some(command) = crate::fill_sign::signature_entries(ui, app, t) {
            app.execute(command);
            ui.close();
        }
    });
}

fn controls_divider(ui: &mut egui::Ui, t: &Tokens, vertical: bool) {
    let (r, _) = if vertical {
        ui.allocate_exact_size(vec2(crate::canvas::bars::QUICK_BAR_WIDTH, 9.0), Sense::hover())
    } else {
        ui.allocate_exact_size(vec2(9.0, crate::canvas::bars::QUICK_BAR_WIDTH), Sense::hover())
    };
    if vertical {
        ui.painter().hline(r.x_range().shrink(6.0), r.center().y, Stroke::new(1.0, t.divider));
    } else {
        ui.painter().vline(r.center().x, r.y_range().shrink(6.0), Stroke::new(1.0, t.divider));
    }
}

fn selected_fill_text(app: &PdfCraftApp) -> Option<(usize, usize)> {
    let (index, id) = app.active_ids()?;
    let (page, annotation) = app.views.get(index)?.comments.selected?;
    let doc = app.session.get(id).filter(|doc| doc.allows_annotation())?;
    doc.info
        .annotations
        .iter()
        .any(|a| a.page == page && a.index == annotation && a.fill_sign && a.subtype == "FreeText" && !a.locked)
        .then_some((page, annotation))
}

/// Fill & Sign's Border controls for typed text: its width, colour and shape, for the selected
/// text (`picked`), or for new text when none is selected. The label shows in a vertical bar, as the
/// font size's does.
fn fill_text_border_controls(ui: &mut egui::Ui, app: &mut PdfCraftApp, picked: Option<(usize, usize)>, vertical: bool) {
    // The selected text's own border (Comment Properties may have changed it
    // since): the controls below show it and change only it. Without one they
    // set what new text gets until PdfCraft quits; Make Current Properties
    // Default on typed text keeps it.
    let selected = picked.and_then(|(page, annotation)| {
        let p = app.active_ids().and_then(|(_, id)| app.session.get(id))?.comment_props(page, annotation)?;
        let own = crate::fill_sign::TextBorder { width: p.width.unwrap_or(0.0), color: p.border_color, shape: p.border_shape.unwrap_or_default() };
        Some((page, annotation, own))
    });
    let was = selected.map_or(
        crate::fill_sign::TextBorder { width: app.fill_text_border, color: app.fill_text_border_color, shape: app.fill_text_border_shape },
        |(_, _, own)| own,
    );
    let mut border = was;
    // A border around the text, in its colour; the text keeps its place.
    let border_label = |w: f64| if w > 0.0 { format!("{w} pt") } else { tl!("None").to_string() };
    let label = vertical.then(|| ui.label(tl!("Border:")));
    let combo = craft_ui::ComboBox::from_id_salt("fill-text-border")
        .selected_text(border_label(was.width))
        .width(72.0)
        .show_ui(ui, |ui| {
            for w in crate::fill_sign::TEXT_BORDERS {
                ui.selectable_value(&mut border.width, w, border_label(w));
            }
        })
        .response;
    match label {
        Some(label) => {
            combo.labelled_by(label.id);
        }
        None => {
            combo.on_hover_text(tl!("Border:"));
        }
    }
    // The border's own colour (the text's unless picked).
    if was.width > 0.0
        && let Some(color) = comments::border_colour_button(ui, was.color, [0.0; 3])
    {
        border.color = color;
    }
    // Its shape: an oval or a circle goes around the text, which keeps its place.
    if was.width > 0.0 {
        craft_ui::ComboBox::from_id_salt("fill-text-border-shape")
            .selected_text(tl!(was.shape.name()))
            .width(96.0)
            .show_ui(ui, |ui| {
                for shape in pdfcraft_engine::BorderShape::ALL {
                    ui.selectable_value(&mut border.shape, shape, tl!(shape.name()));
                }
            })
            .response
            .on_hover_text(tl!("Border shape"));
    }
    // No border, no shape: an oval or a circle left without its line would
    // keep its box grown around a shape that isn't drawn.
    if border.width <= 0.0 {
        border.shape = pdfcraft_engine::BorderShape::Rectangle;
    }
    match selected {
        Some((page, index, _)) => {
            // One undo step for what one choice changed (Border ▸ None takes a
            // circle's line and its shape).
            let mut edits = Vec::new();
            if border.width != was.width {
                edits.push(Edit::StyleAnnotation { page, index, color: None, opacity: None, width: Some(border.width), endings: None });
            }
            if border.color != was.color {
                edits.push(Edit::BorderColorAnnotation { page, index, color: border.color });
            }
            if border.shape != was.shape {
                edits.push(Edit::BorderShapeAnnotation { page, index, shape: border.shape });
            }
            if edits.len() > 1 {
                app.apply_edit(Edit::Batch { label: "Change comment properties".into(), edits });
            } else if let Some(edit) = edits.pop() {
                app.apply_edit(edit);
            }
        }
        None => {
            app.fill_text_border = border.width;
            app.fill_text_border_color = border.color;
            app.fill_text_border_shape = border.shape;
        }
    }
}

/// Extra controls rendered below/beside the buttons when a comment or fill tool is active.
fn active_tool_controls(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens, vertical: bool) {
    let current_fill = match app.quick_tool {
        QuickTool::Fill(f) => Some(f),
        _ => None,
    };
    if matches!(current_fill, Some(crate::fill_sign::FillTool::Text | crate::fill_sign::FillTool::Date)) {
        controls_divider(ui, t, vertical);
        if let Some(color) = comments::quick_bar_colour(ui, app.fill_text_color) {
            app.fill_text_color = color;
            if let Some((page, index)) = selected_fill_text(app) {
                app.apply_edit(Edit::StyleAnnotation { page, index, color: Some(color), opacity: None, width: None, endings: None });
            }
        }
        let resp = icons::button(ui, "a-large-small", 32.0, false, tl!("Font size"));
        let resized = egui::Popup::menu(&resp)
            .align(egui::RectAlign::RIGHT_START)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_min_width(180.0);
                ui.label(egui::RichText::new(tl!("Font size")).font(theme::semibold(12.0)));
                let size = ui.add(
                    egui::Slider::new(&mut app.fill_text_size, crate::fill_sign::MIN_TEXT_SIZE..=crate::fill_sign::MAX_TEXT_SIZE)
                        .logarithmic(true)
                        .suffix(" pt"),
                );
                // #1274: the size also applies to the Fill & Sign text that is selected,
                // once per change (a typed value, or the end of a drag), so a drag adds
                // one undo step and not one per frame.
                (size.changed() && !size.dragged()) || size.drag_stopped()
            })
            .is_some_and(|popup| popup.inner);
        let size = app.fill_text_size;
        if resized
            && let Some((page, index)) = selected_fill_text(app)
            && app
                .active_ids()
                .and_then(|(_, id)| app.session.get(id)?.comment_props(page, index)?.font_size)
                .is_none_or(|stored| format!("{stored:.3}") != format!("{size:.3}"))
        {
            app.apply_edit(Edit::FontSizeAnnotation { page, index, size });
        }
        // After the text's own controls, so they keep their place in a bar that scrolls.
        fill_text_border_controls(ui, app, selected_fill_text(app), vertical);
    }
    if let QuickTool::Comment(tool) = app.quick_tool {
        controls_divider(ui, t, vertical);
        // Text boxes and callouts: the border's own colour, for new ones and the selected one
        // this tool made (not a locked one). With one selected the button shows its colours
        // (Comment Properties may have changed them), else the tool's.
        if tool.has_border_color() {
            let style = app.comment_prefs.style(tool);
            let selected = app.active_ids().and_then(|(index, id)| {
                let (page, annotation) = app.views.get(index)?.comments.selected?;
                let doc = app.session.get(id)?;
                let made = doc.allows_annotation()
                    && doc.info.annotations.iter().any(|a| a.page == page && a.index == annotation && !a.locked && comments::made_by(a, tool));
                made.then(|| (page, annotation, doc.comment_props(page, annotation)))
            });
            let (shown, text) = match selected.as_ref().and_then(|(.., p)| p.as_ref()) {
                Some(p) => (p.border_color, p.color.unwrap_or(style.color)),
                None => (style.border_color, style.color),
            };
            if let Some(color) = comments::border_colour_button(ui, shown, text) {
                app.comment_prefs.set_border_color(tool, color);
                if let Some((page, annotation, _)) = selected {
                    app.apply_edit(Edit::BorderColorAnnotation { page, index: annotation, color });
                }
            }
        }
        // The selected comments this tool made take the colour too (locked ones keep theirs).
        if let Some(color) = comments::quick_bar_controls(ui, tool, &mut app.comment_prefs) {
            comments::apply_quick_bar_color(app, tool, color);
        }
    }
}

/// Render a modern iOS-style pill toggle switch.
fn toggle_switch(ui: &mut egui::Ui, on: &mut bool, label: &str, t: &Tokens) -> egui::Response {
    let width = 34.0;
    let height = 18.0;
    let (rect, mut response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, ui.is_enabled(), *on, label));

    if ui.is_rect_visible(rect) {
        let radius = height / 2.0;
        let bg_color = if *on { t.accent } else { t.border };
        ui.painter().rect(rect, CornerRadius::same(radius as u8), bg_color, Stroke::NONE, egui::StrokeKind::Inside);
        let circle_radius = radius - 2.5;
        let circle_x = if *on { rect.max.x - radius } else { rect.min.x + radius };
        ui.painter().circle_filled(egui::pos2(circle_x, rect.center().y), circle_radius, Color32::WHITE);
    }
    response
}

/// Render the dock placement choice: [ Float | Left | Right | Top | Bottom ], scrolling sideways
/// when the picker is narrower than the row.
fn segmented_dock_picker(ui: &mut egui::Ui, bar: &mut Hotbar, t: &Tokens) {
    let options = [
        (None, tl!("Float")),
        (Some("left"), tl!("Left")),
        (Some("right"), tl!("Right")),
        (Some("top"), tl!("Top")),
        (Some("bottom"), tl!("Bottom")),
    ];
    let mut side = options.iter().map(|(side, _)| *side).find(|side| *side == bar.dock.as_deref()).flatten();
    let changed = egui::ScrollArea::horizontal()
        .id_salt("hotbar-dock")
        .auto_shrink([false, true])
        .show(ui, |ui| crate::dialogs::preferences::segmented_pill_choice(ui, t, &mut side, &options))
        .inner;
    if changed {
        // `place` accepts every side listed above.
        let _ = bar.place(side.unwrap_or("float"));
    }
}

/// Draw the unified floating or side-docked toolbar, combining quick actions, core tools, and custom shortcuts.
pub(crate) fn show(app: &mut PdfCraftApp, area: Rect, page_left: Option<f32>, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let mut bar = std::mem::take(&mut app.hotbar);
    bar.normalize();
    let t = Tokens::get(&ctx);
    let id = egui::Id::unique("quick-bar");
    let full_side = bar.full_side && bar.dock.is_some();
    let bounds = if full_side { area } else { area.shrink(10.0) };
    if let Some(side) = bar.dock.clone() {
        let _ = bar.place(&side);
    }
    let cell = bar.button_size;
    let popup_align = popup_align_for(bar.dock.as_deref(), bar.vertical);
    let mut size = ctx.data_mut(|data| data.get_temp::<egui::Vec2>(id)).unwrap_or(vec2(crate::canvas::bars::QUICK_BAR_WIDTH, 220.0));
    if full_side {
        size = if bar.vertical { vec2(cell + 12.0, bounds.height()) } else { vec2(bounds.width(), cell + 12.0) };
    }
    let travel = (bounds.size() - size).max(vec2(0.0, 0.0));
    let default_x = crate::canvas::bars::quick_bar_offset(page_left.map_or(0.0, |left| left - area.left()), size.x);
    let pos = if full_side {
        bounds.min + vec2(travel.x * bar.position[0], travel.y * bar.position[1])
    } else if bar.dock.is_none() && !bar.placed {
        area.left_top() + vec2(default_x, crate::canvas::bars::QUICK_BAR_EDGE)
    } else {
        bounds.min + vec2(travel.x * bar.position[0], travel.y * bar.position[1])
    };
    let mut run = None;
    let was_picker_open = bar.picker_open;
    let mut picker_open = bar.picker_open;
    let mut settings_clicked = false;
    let shown = egui::Area::new(id).order(egui::Order::Middle).fixed_pos(pos).constrain_to(bounds).show(&ctx, |ui| {
        let frame = if full_side {
            egui::Frame::NONE.fill(t.chrome).corner_radius(egui::CornerRadius::ZERO).inner_margin(6)
        } else {
            egui::Frame::new()
                .fill(t.card)
                .stroke(egui::Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(12))
                .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if t.dark() { 100 } else { 28 }) })
                .inner_margin(6)
        };
        let frame_resp = frame.show(ui, |ui| {
            if full_side {
                ui.set_min_size((size - vec2(12.0, 12.0)).max(vec2(0.0, 0.0)));
            }
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            if bar.vertical {
                ui.set_width(cell);
            } else {
                ui.set_height(cell);
            }
            let mut remove = None;
            if full_side {
                if bar.vertical {
                    ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                        settings_clicked |= settings_button(ui, cell, &mut picker_open);
                        ui.add_space(4.0);
                        let remaining = ui.available_height().max(cell);
                        egui::ScrollArea::vertical().max_height(remaining).show(ui, |ui| {
                            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                                core_tool_buttons(ui, app, cell, popup_align, &t);
                                active_tool_controls(ui, app, &t, true);
                                if !bar.tools.is_empty() {
                                    ui.separator();
                                    tool_buttons(ui, app, &bar.tools, cell, &mut run, &mut remove);
                                }
                            });
                        });
                    });
                } else {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        settings_clicked |= settings_button(ui, cell, &mut picker_open);
                        ui.add_space(4.0);
                        let remaining = ui.available_width().max(cell);
                        egui::ScrollArea::horizontal().max_height(cell).max_width(remaining).show(ui, |ui| {
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                core_tool_buttons(ui, app, cell, popup_align, &t);
                                active_tool_controls(ui, app, &t, false);
                                if !bar.tools.is_empty() {
                                    ui.separator();
                                    tool_buttons(ui, app, &bar.tools, cell, &mut run, &mut remove);
                                }
                            });
                        });
                    });
                }
            } else {
                let layout =
                    if bar.vertical { egui::Layout::top_down(egui::Align::Center) } else { egui::Layout::left_to_right(egui::Align::Center) };
                ui.with_layout(layout, |ui| {
                    let grip_size = if bar.vertical { vec2(cell, 18.0) } else { vec2(18.0, cell) };
                    let (grip_rect, grip) = ui.allocate_exact_size(grip_size, Sense::drag());
                    for x in [-3.0, 3.0] {
                        for y in [-6.0, 0.0, 6.0] {
                            let offset = if bar.vertical { vec2(y, x) } else { vec2(x, y) };
                            ui.painter().circle_filled(grip_rect.center() + offset, 1.5, t.text_faint);
                        }
                    }
                    let drag_label = tl!("Drag hotbar");
                    let grip = grip.on_hover_text(drag_label).on_hover_cursor(egui::CursorIcon::Grab);
                    grip.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, drag_label));
                    if grip.dragged() && bar.dock.is_none() {
                        let delta = grip.drag_delta();
                        if !bar.placed {
                            let cur_x = (pos.x - bounds.min.x).max(0.0);
                            let cur_y = (pos.y - bounds.min.y).max(0.0);
                            bar.position[0] = (cur_x / travel.x.max(1.0)).clamp(0.0, 1.0);
                            bar.position[1] = (cur_y / travel.y.max(1.0)).clamp(0.0, 1.0);
                        }
                        bar.position[0] = (bar.position[0] + delta.x / travel.x.max(1.0)).clamp(0.0, 1.0);
                        bar.position[1] = (bar.position[1] + delta.y / travel.y.max(1.0)).clamp(0.0, 1.0);
                        bar.placed = true;
                    }
                    if bar.vertical {
                        let fixed_overhead = grip_size.y + cell + 24.0;
                        let max_tools_length = (bounds.height() - fixed_overhead).max(cell);
                        egui::ScrollArea::vertical().max_height(max_tools_length).show(ui, |ui| {
                            ui.with_layout(layout, |ui| {
                                core_tool_buttons(ui, app, cell, popup_align, &t);
                                active_tool_controls(ui, app, &t, true);
                                if !bar.tools.is_empty() {
                                    ui.separator();
                                    tool_buttons(ui, app, &bar.tools, cell, &mut run, &mut remove);
                                }
                            });
                        });
                        settings_clicked |= settings_button(ui, cell, &mut picker_open);
                    } else {
                        core_tool_buttons(ui, app, cell, popup_align, &t);
                        active_tool_controls(ui, app, &t, false);
                        if !bar.tools.is_empty() {
                            ui.separator();
                            let scroll_max = (bounds.width() - cell - 50.0).max(cell);
                            egui::ScrollArea::horizontal().max_height(cell).max_width(scroll_max).show(ui, |ui| {
                                ui.with_layout(layout, |ui| {
                                    tool_buttons(ui, app, &bar.tools, cell, &mut run, &mut remove);
                                });
                            });
                        }
                        settings_clicked |= settings_button(ui, cell, &mut picker_open);
                    }
                });
            }
            if let Some(index) = remove {
                bar.tools.remove(index);
            }
        });
        if full_side {
            let r = frame_resp.response.rect;
            let divider = egui::Stroke::new(1.0, t.divider);
            match bar.dock.as_deref() {
                Some("left") => {
                    ui.painter().vline(r.right() - 0.5, r.top()..=r.bottom(), divider);
                }
                Some("right") => {
                    ui.painter().vline(r.left() + 0.5, r.top()..=r.bottom(), divider);
                }
                Some("top") => {
                    ui.painter().hline(r.left()..=r.right(), r.bottom() - 0.5, divider);
                }
                Some("bottom") => {
                    ui.painter().hline(r.left()..=r.right(), r.top() + 0.5, divider);
                }
                _ => {}
            }
        }
    });
    let picker_id = egui::Id::unique("hotbar-picker");
    // A remembered collapsed state can survive disabling the collapse affordance.
    let mut expanded = egui::collapsing_header::CollapsingState::load_with_default_open(&ctx, picker_id.with("collapsing"), true);
    expanded.set_open(true);
    expanded.store(&ctx);
    let screen = ctx.content_rect().shrink(12.0);
    let margin_x = 14.0;
    let card_margin_total = margin_x * 2.0;
    let card_width = 280.0f32.min((screen.width() - card_margin_total).max(180.0));
    let outer_width = card_width + card_margin_total;
    let bar_rect = shown.response.rect;
    let picker_x =
        if bar_rect.right() + outer_width + 10.0 < screen.right() { bar_rect.right() + 14.0 } else { bar_rect.left() - outer_width - 14.0 };
    let picker_y = if bar.dock.as_deref() == Some("top") {
        (bar_rect.bottom() + 10.0).clamp(screen.top(), (screen.bottom() - 480.0).max(screen.top()))
    } else if bar.dock.as_deref() == Some("bottom") {
        (bar_rect.top() - 500.0).clamp(screen.top(), (screen.bottom() - 480.0).max(screen.top()))
    } else {
        bar_rect.top().clamp(screen.top(), (screen.bottom() - 480.0).max(screen.top()))
    };
    let picker_pos = egui::pos2(picker_x.clamp(screen.left(), (screen.right() - outer_width).max(screen.left())), picker_y);
    let mut close_dialog = false;
    let picker_resp = egui::Window::new(tl!("Hotbar tools"))
        .id(picker_id)
        .order(egui::Order::Foreground)
        .open(&mut picker_open)
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .fixed_pos(picker_pos)
        .frame(
            egui::Frame::NONE
                .fill(t.card)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(10))
                .shadow(egui::Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(if t.dark() { 120 } else { 35 }) })
                .inner_margin(egui::Margin::symmetric(14, 12)),
        )
        .show(&ctx, |ui| {
            ui.set_width(card_width);

            // Title bar: "Hotbar Tools" on the left, "X" close button on the right
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tl!("Hotbar tools")).size(14.0).strong().color(t.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::button(ui, "x", 16.0, false, tl!("Close menu")).clicked() {
                        close_dialog = true;
                    }
                });
            });

            ui.add_space(10.0);

            // Section 1: Layout & Sizing
            ui.label(egui::RichText::new(tl!("Layout & Sizing")).size(13.0).strong().color(t.text));
            ui.add_space(6.0);

            // Toolbar size slider row
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tl!("Toolbar size")).size(12.5).color(t.text));
                let slider_width = (ui.available_width() - 55.0).clamp(50.0, 140.0);
                ui.spacing_mut().slider_width = slider_width;
                ui.add(egui::Slider::new(&mut bar.button_size, 24.0..=64.0).show_value(false));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format!("{:.0} px", bar.button_size)).size(12.0).color(t.text_muted));
                });
            });

            ui.add_space(8.0);

            // Vertical layout toggle switch row
            ui.horizontal(|ui| {
                ui.add_enabled_ui(bar.dock.is_none(), |ui| {
                    let label = tl!("Vertical layout");
                    toggle_switch(ui, &mut bar.vertical, label, &t);
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(label).size(12.5).color(t.text));
                });
            });

            ui.add_space(10.0);

            // Stick to a side section with segmented control
            ui.label(egui::RichText::new(tl!("Stick to a side:")).size(12.5).color(t.text));
            ui.add_space(6.0);
            segmented_dock_picker(ui, &mut bar, &t);

            ui.add_space(8.0);

            // Full-side strip checkbox
            let strip_hint = tl!("Extend the docked toolbar along the entire side. Choose a side above to enable.");
            ui.add_enabled(bar.dock.is_some(), crate::widgets::checkbox(&mut bar.full_side, tl!("Full-side strip")))
                .on_hover_text(strip_hint)
                .on_disabled_hover_text(strip_hint);

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(10.0);

            // Section 2: Add Tools to Hotbar
            ui.label(egui::RichText::new(tl!("Add Tools to Hotbar")).size(13.0).strong().color(t.text));
            ui.add_space(8.0);

            // Search bar pill
            let search_frame = egui::Frame::NONE
                .fill(t.field)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(10, 6));
            search_frame.show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.add(icons::image("search", 14.0, t.text_faint));
                    ui.add(
                        egui::TextEdit::singleline(&mut bar.search)
                            .hint_text(tl!("Search all tools…"))
                            .frame(egui::Frame::NONE)
                            .desired_width(ui.available_width()),
                    );
                });
            });

            ui.add_space(8.0);

            // Scrollable tools list with checkboxes
            let max_list_height = (screen.height() - 340.0).clamp(120.0, 220.0);
            egui::ScrollArea::vertical().max_height(max_list_height).auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 5.0;
                for spec in COMMANDS {
                    if !matches_search(spec, &bar.search) {
                        continue;
                    }
                    let mut selected = bar.tools.iter().any(|id| id == spec.id);
                    if ui.add(crate::widgets::checkbox(&mut selected, tl!(spec.label))).changed() {
                        if selected {
                            bar.tools.push(spec.id.to_owned());
                        } else {
                            bar.tools.retain(|id| id != spec.id);
                        }
                    }
                }
            });
        });
    let clicked_outside = was_picker_open
        && !settings_clicked
        && picker_resp.as_ref().is_some_and(|wr| {
            wr.response.clicked_elsewhere()
                || ctx.input(|i| i.pointer.any_click() && !i.pointer.interact_pos().is_some_and(|p| wr.response.rect.contains(p)))
        });
    if clicked_outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close_dialog = true;
    }
    if close_dialog {
        picker_open = false;
    }
    ctx.data_mut(|data| {
        data.insert_temp(id, shown.response.rect.size());
        data.insert_temp(egui::Id::unique("custom-hotbar"), shown.response.rect.size());
    });
    bar.picker_open = picker_open;
    app.hotbar = bar;
    if let Some(id) = run {
        app.execute(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotbar_validation_and_placement() {
        let mut bar = Hotbar::default();
        assert!(bar.tools.is_empty());
        bar.set_tools("comment.ink,comment.freehand_highlight,comment.ink").unwrap();
        assert_eq!(bar.tools.len(), 2);
        assert!(bar.set_tools("does.not.exist").is_err());
        assert_eq!(bar.tools.len(), 2);
        bar.place("right").unwrap();
        assert!(bar.vertical);
        assert_eq!(bar.position, [1.0, 0.5]);
        bar.place("bottom").unwrap();
        assert!(!bar.vertical);
        assert_eq!(bar.position, [0.5, 1.0]);
        assert_eq!(bar.dock.as_deref(), Some("bottom"));
        bar.place("float").unwrap();
        assert!(bar.dock.is_none());
    }

    #[test]
    fn hotbar_preferences_round_trip() {
        let mut app = PdfCraftApp::new();
        app.set_option("hotbar-tools", "comment.freehand_highlight,comment.ink").unwrap();
        app.set_option("hotbar-side", "left").unwrap();
        app.set_option("hotbar-size", "48").unwrap();
        app.set_option("hotbar-full-side", "true").unwrap();
        assert!(app.set_option("hotbar-size", "NaN").is_err());
        let mut restored = PdfCraftApp::new();
        restored.restore(&app.persist());
        assert_eq!(restored.hotbar.tools, app.hotbar.tools);
        assert_eq!(restored.hotbar.position, [0.0, 0.5]);
        assert!(restored.hotbar.vertical);
        assert_eq!(restored.hotbar.button_size, 48.0);
        assert!(restored.hotbar.full_side);
        assert_eq!(restored.hotbar.dock.as_deref(), Some("left"));
    }

    #[test]
    fn hotbar_restore_filters_unknown_tools_and_offscreen_positions() {
        let mut app = PdfCraftApp::new();
        app.restore(r#"{"hotbar":{"tools":["invalid","comment.ink","comment.ink"],"position":[-10,999]}}"#);
        assert_eq!(app.hotbar.tools, ["comment.ink"]);
        assert_eq!(app.hotbar.position, [0.0, 1.0]);
    }

    #[test]
    fn hotbar_restore_moves_legacy_default_into_margin_and_keeps_chosen_spots() {
        let mut app = PdfCraftApp::new();
        app.restore(r#"{"hotbar":{"tools":["comment.ink"],"position":[0.5,0.0],"vertical":false,"button_size":40.0,"dock":null}}"#);
        assert_eq!(app.hotbar.position, [0.0, 0.0]);
        assert!(app.hotbar.vertical);
        assert!(!app.hotbar.placed);
        assert_eq!(app.hotbar.tools, ["comment.ink"]);
        assert_eq!(app.hotbar.button_size, 40.0);
        app.restore(r#"{"hotbar":{"position":[0.0,0.0],"vertical":false,"dock":null}}"#);
        assert_eq!(app.hotbar.position, [0.0, 0.0]);
        assert!(!app.hotbar.vertical);
        assert!(app.hotbar.placed, "a legacy bar dragged to the corner stays there");
        app.restore(r#"{"hotbar":{"position":[0.5,0.0],"vertical":false,"dock":null,"placed":true}}"#);
        assert_eq!(app.hotbar.position, [0.5, 0.0], "current preferences are not migrated");
        assert!(!app.hotbar.vertical);
    }

    #[test]
    fn hotbar_search_matches_translated_and_canonical_labels() {
        if !crate::i18n::needs_catalogs() {
            return;
        }
        let ink = command("comment.ink").unwrap();
        crate::i18n::set_current(crate::i18n::lang_from_tag("fr").unwrap());
        assert_eq!(tl!(ink.label), "Dessiner à main levée");
        assert!(matches_search(ink, "dessiner"), "translated French label must match");
        assert!(matches_search(ink, "freehand"), "canonical English label must still match");
        assert!(matches_search(ink, "comment.ink"), "command ID must still match");
        assert!(!matches_search(ink, "introuvable"));
        crate::i18n::set_current(crate::i18n::Lang::EN);
    }
}
