//! Cursor coordinates floating window (Acrobat's View ▸ Show/Hide ▸ Cursor Coordinates, `Ctrl+Y`).
//!
//! Renders an interactive, movable window showing real-time pointer coordinates on the active PDF
//! page (in user space or view space), along with the page's visible dimensions and measurement unit cycling.

use egui::{Grid, RichText, Window, pos2, vec2};

use crate::PdfCraftApp;
use crate::theme::Tokens;
use crate::units::Unit;

/// Default window position when opened for the first time.
const DEFAULT_POS: egui::Pos2 = pos2(80.0, 140.0);

/// Show the non-modal Cursor Coordinates floating window when enabled.
///
/// Tracks the pointer over pages in the active document and formats coordinates and page
/// bounds in points, inches, millimeters, or centimeters.
pub fn show(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if !app.show_cursor_coordinates {
        return;
    }

    let t = Tokens::get(ctx);
    let mut open = true;

    Window::new(tl!("Cursor Coordinates"))
        .id(egui::Id::new("cursor_coordinates_window"))
        .open(&mut open)
        .collapsible(true)
        .resizable(false)
        .default_pos(DEFAULT_POS)
        .min_width(210.0)
        .show(ctx, |ui| {
            let pointer = ctx.input(|i| i.pointer.latest_pos());

            let active = app.active_ids().and_then(|(v_idx, doc_id)| {
                let doc = app.session.get(doc_id)?;
                let view = app.views.get(v_idx)?;
                Some((view, doc))
            });

            let unit = app.units;
            let (k, d) = (unit.per_point(), unit.decimals());

            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;

                ui.horizontal(|ui| {
                    ui.label(RichText::new(tl!("Origin:")).small().color(t.text_faint));
                    if ui.selectable_label(app.cursor_coordinates_origin_bottom_left, tl!("Bottom-left")).clicked() {
                        app.cursor_coordinates_origin_bottom_left = true;
                    }
                    if ui.selectable_label(!app.cursor_coordinates_origin_bottom_left, tl!("Top-left")).clicked() {
                        app.cursor_coordinates_origin_bottom_left = false;
                    }
                });

                ui.separator();

                let mut x_str = "—".to_string();
                let mut y_str = "—".to_string();
                let mut w_str = "—".to_string();
                let mut h_str = "—".to_string();

                if let Some((view, doc)) = active {
                    let page_hit = pointer.and_then(|pos| {
                        if !view.viewport_rect().contains(pos) || ctx.layer_id_at(pos) == Some(ui.layer_id()) {
                            return None;
                        }
                        view.screen_xforms().iter().find(|(_, xf)| xf.rect.contains(pos)).map(|(p, xf)| (*p, *xf, pos))
                    });

                    let target_page = page_hit.as_ref().map(|(p, _, _)| *p).unwrap_or(view.current);

                    if let Some(p_info) = doc.info.pages.get(target_page) {
                        let [cx0, cy0, _, _] = p_info.crop;
                        let pw = f64::from(p_info.width);
                        let ph = f64::from(p_info.height);
                        w_str = format!("{:.d$} {}", pw * k, unit.label());
                        h_str = format!("{:.d$} {}", ph * k, unit.label());

                        if let Some((_, xf, pos)) = page_hit {
                            let (vx, vy) = xf.screen_to_view(pos);
                            let [ux, uy] = p_info.view_to_user(vx, vy);

                            let (x_pt, y_pt) = if app.cursor_coordinates_origin_bottom_left {
                                (f64::from(ux - cx0), f64::from(uy - cy0))
                            } else {
                                (f64::from(vx), f64::from(vy))
                            };

                            x_str = format!("{:.d$} {}", x_pt * k, unit.label());
                            y_str = format!("{:.d$} {}", y_pt * k, unit.label());
                        }
                    }
                }

                Grid::new("cursor_coords_grid").num_columns(2).spacing(vec2(16.0, 4.0)).show(ui, |ui| {
                    ui.label(RichText::new("X:").strong());
                    ui.label(RichText::new(x_str).monospace());
                    ui.end_row();

                    ui.label(RichText::new("Y:").strong());
                    ui.label(RichText::new(y_str).monospace());
                    ui.end_row();

                    ui.label(RichText::new(tl!("Width:")).strong());
                    ui.label(RichText::new(w_str).monospace());
                    ui.end_row();

                    ui.label(RichText::new(tl!("Height:")).strong());
                    ui.label(RichText::new(h_str).monospace());
                    ui.end_row();
                });

                ui.separator();

                ui.horizontal(|ui| {
                    ui.label(RichText::new(tl!("Unit:")).small().color(t.text_faint));
                    for u in Unit::ALL {
                        let selected = app.units == u;
                        if ui.selectable_label(selected, u.label()).clicked() {
                            app.units = u;
                        }
                    }
                });
            });
        });

    if !open {
        app.show_cursor_coordinates = false;
    }
}
