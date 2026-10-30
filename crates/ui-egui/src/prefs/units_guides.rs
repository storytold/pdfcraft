//! Preferences ▸ Units and guides: the unit lengths are shown in (rulers, page sizes, margins),
//! the grid, how the grid and guides look, and the saved guide sets.

use super::{card, field_label, hint, radios, row, wrapped};
use crate::guides::{GridSettings, MAX_GRID_SPACING, MAX_SUBDIVISIONS, MIN_GRID_SPACING};
use crate::theme::Tokens;
use crate::{PdfCraftApp, widgets};

pub(super) fn show(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    app.ensure_guide_sets();
    card(ui, t, Some(tl!("Units")), |ui| {
        radios(ui, &mut app.units, crate::units::Unit::ALL.map(|u| (u, u.label())));
        hint(ui, t, tl!("For the rulers, the grid, page sizes and margins. Clicking the corner between the rulers steps to the next unit."));
    });
    let unit = app.units;
    let k = unit.per_point() as f32;
    let mut g = app.grid;
    // A distance in the chosen unit, stored in points.
    let length = |ui: &mut egui::Ui, label: &str, value: &mut f32, min: f32| {
        field_column(ui, |ui| {
            let name = field_label(ui, t, label);
            let mut shown = *value * k;
            let field = egui::DragValue::new(&mut shown)
                .range(min * k..=MAX_GRID_SPACING * k)
                .speed(unit.speed())
                .max_decimals(unit.decimals())
                .suffix(format!(" {}", unit.label()));
            if ui.add(field).labelled_by(name.id).changed() && shown.is_finite() && k > 0.0 {
                *value = shown / k;
            }
        });
    };
    card(ui, t, Some(tl!("Grid")), |ui| {
        fields(ui, |ui| {
            length(ui, tl!("Width between lines"), &mut g.width, MIN_GRID_SPACING);
            length(ui, tl!("Height between lines"), &mut g.height, MIN_GRID_SPACING);
            length(ui, tl!("Grid offset from left edge"), &mut g.offset_left, -MAX_GRID_SPACING);
            length(ui, tl!("Grid offset from top edge"), &mut g.offset_top, -MAX_GRID_SPACING);
            field_column(ui, |ui| {
                let name = field_label(ui, t, tl!("Subdivisions"));
                ui.add(egui::DragValue::new(&mut g.subdivisions).range(1..=MAX_SUBDIVISIONS)).labelled_by(name.id);
            });
        });
        wrapped(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            ui.add(widgets::checkbox(&mut app.show_grid, tl!("Show grid")));
            ui.add(widgets::checkbox(&mut app.snap_to_grid, tl!("Snap to grid")));
        });
    });
    card(ui, t, Some(tl!("Look")), |ui| {
        fields(ui, |ui| {
            look(ui, t, tl!("Grid lines"), &mut g.grid_color, &mut g.grid_line_width);
            look(ui, t, tl!("Guides"), &mut g.guide_color, &mut g.guide_line_width);
        });
        hint(ui, t, tl!("Guides with a colour or thickness of their own (right-click a guide, or set it in a guide-set file) keep it."));
        row(ui, |ui| {
            if ui.button(tl!("Restore defaults")).clicked() {
                g = GridSettings::default();
            }
        });
    });
    app.grid = g.sanitized();
    card(ui, t, Some(tl!("Guides")), |ui| {
        ui.add(widgets::checkbox(&mut app.show_guides, tl!("Show guides")));
        hint(
            ui,
            t,
            tl!(
                "Drag a guide out of a ruler onto a page; hold Shift as you drop it to put it on every page. Drag it back onto a ruler to remove it, or right-click it for its colour, thickness and label. Guides and the grid are never saved in the PDF."
            ),
        );
    });
    card(ui, t, Some(tl!("Guide sets")), |ui| sets(ui, app, t));
}

/// A wrapping row of [`field_column`]s, their tops in line: a name that wraps onto a second line
/// pushes its own field down, not the whole row's.
fn fields(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let layout =
        if crate::i18n::current().rtl() { egui::Layout::right_to_left(egui::Align::Min) } else { egui::Layout::left_to_right(egui::Align::Min) };
    ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 0.0), layout.with_main_wrap(true), |ui| {
        ui.spacing_mut().item_spacing.x = 24.0;
        add(ui);
    });
}

/// One setting with its name above it, in a column of fixed width, so a row of them wraps in a
/// narrow card instead of widening it.
fn field_column(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let layout = egui::Layout::top_down(if crate::i18n::current().rtl() { egui::Align::Max } else { egui::Align::Min });
    ui.allocate_ui_with_layout(egui::vec2(FIELD_WIDTH.min(ui.available_width()), 0.0), layout, |ui| {
        ui.set_width(FIELD_WIDTH.min(ui.available_width()));
        add(ui);
    });
}

const FIELD_WIDTH: f32 = 190.0;

/// A colour and a thickness, under one name.
fn look(ui: &mut egui::Ui, t: &Tokens, label: &str, color: &mut [u8; 4], width: &mut f32) {
    field_column(ui, |ui| {
        field_label(ui, t, label);
        ui.horizontal(|ui| {
            let [r, gr, b, a] = *color;
            let mut c = egui::Color32::from_rgba_unmultiplied(r, gr, b, a);
            if egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::OnlyBlend).changed() {
                *color = c.to_srgba_unmultiplied();
            }
            ui.add(
                egui::DragValue::new(width)
                    .range(crate::guide_sets::MIN_WIDTH..=crate::guide_sets::MAX_WIDTH)
                    .speed(0.1)
                    .max_decimals(1)
                    .suffix(format!(" pt {}", tl!("thick"))),
            );
        });
    });
}

/// The sets in the folder: shown or not, their shortcut, and what is wrong with a file that
/// doesn't read.
fn sets(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    let ctx = ui.ctx().clone();
    let mut toggle = None;
    let mut apply = None;
    if app.guide_sets.is_empty() {
        hint(ui, t, tl!("No saved sets yet. Right-click a ruler ▸ Save guides as a set…, or put a set file in the folder."));
    }
    for loaded in &app.guide_sets {
        match &loaded.set {
            Ok(set) => {
                row(ui, |ui| {
                    let mut on = app.shown_guide_sets.contains(&set.name);
                    if ui.add(widgets::checkbox(&mut on, set.name.as_str())).changed() {
                        toggle = Some(set.name.clone());
                    }
                    let shortcut = set.shortcut.map(|n| crate::guides::set_shortcut_label(n, &ctx)).unwrap_or_default();
                    ui.label(egui::RichText::new(format!("{shortcut}  ·  {} {}", set.guides.len(), tl!("guides"))).color(t.text_muted));
                    if app.active.is_some() && ui.small_button(tl!("Edit in document")).clicked() {
                        apply = Some(set.name.clone());
                    }
                });
            }
            Err(e) => {
                row(ui, |ui| {
                    ui.label(egui::RichText::new(format!("⚠ {}", loaded.name())).color(ui.visuals().error_fg_color));
                });
                hint(ui, t, e);
            }
        }
    }
    if let Some(name) = toggle {
        app.toggle_guide_set(&name);
    }
    if let Some(name) = apply {
        app.apply_guide_set(&name);
    }
    wrapped(ui, |ui| {
        if ui.button(tl!("Reload")).clicked() {
            app.reload_guide_sets();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(dir) = app.guide_sets_dir.clone()
            && ui.button(tl!("Open folder")).clicked()
        {
            let _ = std::fs::create_dir_all(&dir);
            if let Err(e) = crate::tab_context::reveal(&dir) {
                app.notify(e);
            }
        }
    });
    if let Some(dir) = &app.guide_sets_dir {
        hint(ui, t, tl!("Each set is a text file in"));
        // A path has nowhere to wrap: it is shortened to the card, the whole of it on hover.
        let path = dir.display().to_string();
        ui.add(egui::Label::new(egui::RichText::new(&path).small().monospace().color(t.text_muted)).truncate()).on_hover_text(path);
    }
    let keys = crate::help_guides::shortcut_args(&ctx);
    let args: Vec<(&str, &str)> = keys.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let text = crate::i18n::fmt(
        tl!(
            "{first} … {last} shows and hides the set with that shortcut. Set files can be edited in any text editor and shared with others; click Reload after changing them."
        ),
        &args,
    );
    hint(ui, t, &text);
}
