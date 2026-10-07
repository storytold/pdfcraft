//! Set Page Boxes (Acrobat: Organize pages ▸ Set page boxes / Crop pages ▸ double-click), M4.4.
//!
//! Choose a box (crop, trim, bleed, art, media), margins from the media box in points, inches or
//! millimetres, and the pages. A preview shows the current page's media box and the new box.

use egui::{Align, Color32, CornerRadius, Layout, Rect, Stroke, pos2, vec2};
use pdfcraft_engine::{BoxSpec, Edit, PageBox};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Points,
    Inches,
    Millimetres,
}

impl Unit {
    fn per_point(self) -> f64 {
        match self {
            Unit::Points => 1.0,
            Unit::Inches => 1.0 / 72.0,
            Unit::Millimetres => 25.4 / 72.0,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Unit::Points => "pt",
            Unit::Inches => "in",
            Unit::Millimetres => "mm",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    All,
    Current,
    /// 1-based, inclusive.
    Pages(usize, usize),
}

/// Dialog state. Margins are kept in points: left, bottom, right, top.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxesDraft {
    pub which: PageBox,
    pub unit: Unit,
    pub margins: [f64; 4],
    pub range: Range,
    /// The draft was seeded from this page and box (re-seed when either changes).
    pub seeded: Option<(usize, PageBox)>,
}

impl Default for BoxesDraft {
    fn default() -> Self {
        Self { which: PageBox::Crop, unit: Unit::Inches, margins: [0.0; 4], range: Range::All, seeded: None }
    }
}

impl BoxesDraft {
    /// The pages (0-based) the dialog applies to.
    pub fn pages(&self, current: usize, count: usize) -> Vec<usize> {
        match self.range {
            Range::All => (0..count).collect(),
            Range::Current => vec![current.min(count.saturating_sub(1))],
            Range::Pages(a, b) => (a.max(1) - 1..b.min(count)).collect(),
        }
    }

    pub fn edit(&self, current: usize, count: usize) -> Edit {
        let spec =
            if self.margins.iter().all(|m| *m == 0.0) && self.which != PageBox::Media { BoxSpec::Remove } else { BoxSpec::Margins(self.margins) };
        Edit::SetPageBox { pages: self.pages(current, count), which: self.which, spec }
    }
}

fn box_index(b: PageBox) -> usize {
    match b {
        PageBox::Media => 0,
        PageBox::Crop => 1,
        PageBox::Bleed => 2,
        PageBox::Trim => 3,
        PageBox::Art => 4,
    }
}

/// Draw the dialog body; returns (apply, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let Some((i, id)) = app.active_ids() else { return (false, true) };
    let current = app.views[i].current;
    let Some(doc) = app.session.get(id) else { return (false, true) };
    let boxes = doc.page_boxes();
    let count = boxes.len().max(1);
    let d = &mut app.boxes_draft;
    // Seed the margins from the current page's box when the dialog opens or the box changes.
    if d.seeded != Some((current, d.which))
        && let Some(b) = boxes.get(current)
    {
        let (m, x) = (b[0], b[box_index(d.which)]);
        d.margins = if d.which == PageBox::Media {
            [0.0; 4]
        } else {
            [x[0] - m[0], x[1] - m[1], m[2] - x[2], m[3] - x[3]].map(|v| (v.max(0.0) * 100.0).round() / 100.0)
        };
        d.seeded = Some((current, d.which));
    }
    ui.label(egui::RichText::new("Set Page Boxes").font(theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            egui::Grid::new("boxes-grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Box");
                let names = [
                    (PageBox::Crop, "CropBox"),
                    (PageBox::Trim, "TrimBox"),
                    (PageBox::Bleed, "BleedBox"),
                    (PageBox::Art, "ArtBox"),
                    (PageBox::Media, "MediaBox"),
                ];
                let cur = names.iter().find(|(b, _)| *b == d.which).map_or("CropBox", |(_, l)| *l);
                egui::ComboBox::from_id_salt("boxes-which").selected_text(cur).show_ui(ui, |ui| {
                    for (b, l) in names {
                        ui.selectable_value(&mut d.which, b, l);
                    }
                });
                ui.end_row();
                ui.label("Units");
                egui::ComboBox::from_id_salt("boxes-unit").selected_text(d.unit.label()).show_ui(ui, |ui| {
                    for u in [Unit::Inches, Unit::Millimetres, Unit::Points] {
                        ui.selectable_value(&mut d.unit, u, u.label());
                    }
                });
                ui.end_row();
                let k = d.unit.per_point();
                for (label, idx) in [("Top", 3), ("Bottom", 1), ("Left", 0), ("Right", 2)] {
                    let l = ui.label(label);
                    let mut v = d.margins[idx] * k;
                    let speed = if d.unit == Unit::Inches { 0.01 } else { 0.5 };
                    let r = ui
                        .add(egui::DragValue::new(&mut v).speed(speed).range(0.0..=10_000.0).max_decimals(3).suffix(format!(" {}", d.unit.label())))
                        .labelled_by(l.id);
                    if r.changed() {
                        d.margins[idx] = (v / k).max(0.0);
                    }
                    ui.end_row();
                }
                ui.label("");
                if ui.button("Set to zero").clicked() {
                    d.margins = [0.0; 4];
                }
                ui.end_row();
                ui.label("Pages");
                ui.vertical(|ui| {
                    ui.radio_value(&mut d.range, Range::All, "All");
                    ui.radio_value(&mut d.range, Range::Current, format!("Current page ({})", current + 1));
                    let (mut a, mut b) = match d.range {
                        Range::Pages(a, b) => (a, b),
                        _ => (1, count),
                    };
                    ui.horizontal(|ui| {
                        let on = matches!(d.range, Range::Pages(..));
                        if ui.radio(on, "From").clicked() {
                            d.range = Range::Pages(a, b);
                        }
                        let ra = ui.add(egui::DragValue::new(&mut a).range(1..=count));
                        ui.label("to");
                        let rb = ui.add(egui::DragValue::new(&mut b).range(1..=count));
                        if ra.changed() || rb.changed() {
                            d.range = Range::Pages(a.min(b), b.max(a));
                        }
                    });
                });
                ui.end_row();
            });
        });
        ui.add_space(16.0);
        // Preview: the media box and the new box, to scale.
        if let Some(b) = boxes.get(current) {
            let m = b[0];
            let (w, h) = ((m[2] - m[0]).max(1.0), (m[3] - m[1]).max(1.0));
            let s = (180.0 / w.max(h)) as f32;
            let (r, _) = ui.allocate_exact_size(vec2(200.0, 200.0), egui::Sense::hover());
            let page = Rect::from_center_size(r.center(), vec2(w as f32 * s, h as f32 * s));
            ui.painter().rect_filled(page, CornerRadius::ZERO, Color32::WHITE);
            ui.painter().rect_stroke(page, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
            let [l, bo, ri, to] = d.margins.map(|v| v as f32 * s);
            let inner = Rect::from_min_max(pos2(page.left() + l, page.top() + to), pos2(page.right() - ri, page.bottom() - bo));
            if inner.is_positive() {
                ui.painter().rect_stroke(inner, CornerRadius::ZERO, Stroke::new(1.5, t.accent), egui::StrokeKind::Middle);
            }
            ui.painter().text(
                page.center_bottom() + vec2(0.0, 6.0),
                egui::Align2::CENTER_TOP,
                format!("Page {}", current + 1),
                theme::regular(11.0),
                t.text_faint,
            );
        }
    });
    ui.add_space(12.0);
    let (mut apply, mut cancel) = (false, false);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, "OK", true).clicked() {
            apply = true;
        }
        if widgets::pill_button(ui, "Cancel", false).clicked() {
            cancel = true;
        }
    });
    (apply, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drafts_pick_pages_and_reset_to_default_at_zero() {
        let mut d = BoxesDraft::default();
        assert_eq!(d.pages(2, 5), [0, 1, 2, 3, 4]);
        d.range = Range::Current;
        assert_eq!(d.pages(2, 5), [2]);
        d.range = Range::Pages(2, 9);
        assert_eq!(d.pages(0, 5), [1, 2, 3, 4]);
        assert!(matches!(d.edit(0, 5), Edit::SetPageBox { spec: BoxSpec::Remove, .. }));
        d.margins = [72.0, 0.0, 0.0, 0.0];
        assert!(matches!(d.edit(0, 5), Edit::SetPageBox { spec: BoxSpec::Margins(_), which: PageBox::Crop, .. }));
    }
}
