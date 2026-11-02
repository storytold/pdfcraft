//! A movable, persistent set of shortcuts to the shared command registry.
use egui::{Rect, Sense, vec2};
use pdfcraft_engine::commands::{COMMANDS, command};
use serde::{Deserialize, Serialize};

use crate::{PdfCraftApp, icons, theme::Tokens};

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
    #[serde(skip)]
    search: String,
    #[serde(skip)]
    picker_open: bool,
}

impl Default for Hotbar {
    /// Create a default empty horizontal floating hotbar.
    fn default() -> Self {
        Self {
            tools: Vec::new(),
            position: [0.5, 0.0],
            vertical: false,
            button_size: 32.0,
            dock: None,
            full_side: false,
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
        Ok(())
    }
}

/// Render the hotbar settings gear button and open the tool picker when clicked.
fn settings_button(ui: &mut egui::Ui, cell: f32, picker_open: &mut bool) {
    let settings = ui
        .push_id("hotbar-settings", |ui| {
            icons::button(ui, "settings", cell, false, tl!("Hotbar settings")).on_hover_text(tl!("Hotbar settings / add tools"))
        })
        .inner;
    if settings.clicked() {
        *picker_open = true;
    }
}

/// Whether `spec` matches the hotbar picker search query across the translated label, canonical
/// English label, or command identifier (case-insensitively).
fn matches_search(spec: &pdfcraft_engine::commands::CommandSpec, query: &str) -> bool {
    let search = query.to_lowercase();
    tl!(spec.label).to_lowercase().contains(&search) || spec.label.to_lowercase().contains(&search) || spec.id.to_lowercase().contains(&search)
}

/// Render the configured tool shortcut buttons and their removal context menus.
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

/// Draw the floating or side-docked hotbar and its tool picker window inside `area`.
pub(crate) fn show(app: &mut PdfCraftApp, area: Rect, ctx: &egui::Context) {
    let mut bar = std::mem::take(&mut app.hotbar);
    let t = Tokens::get(ctx);
    let id = egui::Id::unique("custom-hotbar");
    let full_side = bar.full_side && bar.dock.is_some();
    let bounds = if full_side { area } else { area.shrink(10.0) };
    if let Some(side) = bar.dock.clone() {
        let _ = bar.place(&side);
    }
    let cell = bar.button_size;
    let mut size = ctx.data_mut(|data| data.get_temp::<egui::Vec2>(id)).unwrap_or(vec2(90.0, 44.0));
    if full_side {
        size = if bar.vertical { vec2(cell + 12.0, bounds.height()) } else { vec2(bounds.width(), cell + 12.0) };
    }
    let travel = (bounds.size() - size).max(vec2(0.0, 0.0));
    let pos = bounds.min + vec2(travel.x * bar.position[0], travel.y * bar.position[1]);
    let mut run = None;
    let mut picker_open = bar.picker_open;
    let shown = egui::Area::new(id).order(egui::Order::Middle).fixed_pos(pos).constrain_to(bounds).show(ctx, |ui| {
        let frame = if full_side {
            egui::Frame::NONE.fill(t.chrome).corner_radius(egui::CornerRadius::ZERO).inner_margin(6)
        } else {
            egui::Frame::new().fill(t.card).stroke(egui::Stroke::new(1.0, t.border)).corner_radius(10).inner_margin(6)
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
                        settings_button(ui, cell, &mut picker_open);
                        ui.add_space(4.0);
                        let remaining = ui.available_height().max(cell);
                        egui::ScrollArea::vertical().max_width(cell).max_height(remaining).show(ui, |ui| {
                            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                                tool_buttons(ui, app, &bar.tools, cell, &mut run, &mut remove);
                            });
                        });
                    });
                } else {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        settings_button(ui, cell, &mut picker_open);
                        ui.add_space(4.0);
                        let remaining = ui.available_width().max(cell);
                        egui::ScrollArea::horizontal().max_height(cell).max_width(remaining).show(ui, |ui| {
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                tool_buttons(ui, app, &bar.tools, cell, &mut run, &mut remove);
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
                        bar.position[0] = (bar.position[0] + delta.x / travel.x.max(1.0)).clamp(0.0, 1.0);
                        bar.position[1] = (bar.position[1] + delta.y / travel.y.max(1.0)).clamp(0.0, 1.0);
                    }
                    let scroll_max = if bar.vertical { (bounds.height() - cell - 50.0).max(cell) } else { (bounds.width() - cell - 50.0).max(cell) };
                    let scroll = if bar.vertical {
                        egui::ScrollArea::vertical().max_width(cell).max_height(scroll_max)
                    } else {
                        egui::ScrollArea::horizontal().max_height(cell).max_width(scroll_max)
                    };
                    scroll.auto_shrink([true, true]).show(ui, |ui| {
                        ui.with_layout(layout, |ui| {
                            tool_buttons(ui, app, &bar.tools, cell, &mut run, &mut remove);
                        });
                    });
                    settings_button(ui, cell, &mut picker_open);
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
    let mut expanded = egui::collapsing_header::CollapsingState::load_with_default_open(ctx, picker_id.with("collapsing"), true);
    expanded.set_open(true);
    expanded.store(ctx);
    let screen = ctx.content_rect().shrink(12.0);
    let picker_x = if pos.x + cell + 380.0 < screen.right() { pos.x + cell + 20.0 } else { pos.x - 360.0 };
    let picker_pos = egui::pos2(
        picker_x.clamp(screen.left(), (screen.right() - 350.0).max(screen.left())),
        pos.y.clamp(screen.top(), (screen.bottom() - 640.0).max(screen.top())),
    );
    egui::Window::new(tl!("Hotbar tools"))
        .id(picker_id)
        .order(egui::Order::Foreground)
        .open(&mut picker_open)
        .collapsible(false)
        .resizable(false)
        .fixed_pos(picker_pos)
        .max_height((screen.height() - 24.0).max(180.0))
        .vscroll(true)
        .show(ctx, |ui| {
            ui.set_min_width(260.0);
            ui.add(egui::Slider::new(&mut bar.button_size, 24.0..=64.0).text(tl!("Toolbar size")).suffix(" px"));
            ui.add_enabled(bar.dock.is_none(), crate::widgets::checkbox(&mut bar.vertical, tl!("Vertical layout")));
            if ui.selectable_label(bar.dock.is_none(), tl!("Float (drag anywhere)")).clicked() {
                let _ = bar.place("float");
            }
            ui.label(tl!("Stick to a side:"));
            ui.horizontal(|ui| {
                for (label, side) in [(tl!("Left"), "left"), (tl!("Right"), "right"), (tl!("Top"), "top"), (tl!("Bottom"), "bottom")] {
                    if ui.selectable_label(bar.dock.as_deref() == Some(side), label).clicked() {
                        let _ = bar.place(side);
                    }
                }
            });
            let strip_hint = tl!("Extend the docked toolbar along the entire side. Choose a side above to enable.");
            ui.add_enabled(bar.dock.is_some(), crate::widgets::checkbox(&mut bar.full_side, tl!("Full-side strip")))
                .on_hover_text(strip_hint)
                .on_disabled_hover_text(strip_hint);
            if ui.button(tl!("Close menu")).clicked() {
                ui.close();
            }
            ui.separator();
            ui.label(tl!("Add tools to hotbar"));
            ui.add(egui::TextEdit::singleline(&mut bar.search).hint_text(tl!("Search all tools…")));
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
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
            ui.separator();
        });
    ctx.data_mut(|data| data.insert_temp(id, shown.response.rect.size()));
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
    fn hotbar_search_matches_translated_and_canonical_labels() {
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
