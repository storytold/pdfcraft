//! Set Page Boxes (Acrobat: Organize pages ▸ Set page boxes / Crop pages ▸ double-click), M4.4.
//!
//! Choose a box (crop, trim, bleed, art, media), margins from the media box in points, inches,
//! millimetres or centimetres, and the pages. A preview shows the current page's media box and the new box.

use egui::{Align, Color32, CornerRadius, Layout, Rect, Stroke, pos2, vec2};
use pdfcraft_engine::{BoxSpec, Edit, PageBox};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

pub use crate::units::Unit;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    All,
    Current,
    /// 1-based, inclusive.
    Pages(usize, usize),
}

/// Dialog state. Margins are kept in physical points (1/72 in): left, bottom, right, top. A page with
/// `/UserUnit` counts each user-space unit as that many points, so they are converted per page.
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

    /// The edit for the draft. `user_units` is each page's `/UserUnit` (points per user-space unit):
    /// the margins are physical, the page box is written in user space, so pages with different
    /// units get their own margins in one undo step.
    pub fn edit(&self, current: usize, count: usize, user_units: &[f64]) -> Edit {
        let pages = self.pages(current, count);
        if self.margins.iter().all(|m| *m == 0.0) && self.which != PageBox::Media {
            return Edit::SetPageBox { pages, which: self.which, spec: BoxSpec::Remove };
        }
        let unit_of = |p: usize| user_units.get(p).copied().filter(|u| *u > 0.0).unwrap_or(1.0);
        let mut groups: Vec<(f64, Vec<usize>)> = Vec::new();
        for p in pages {
            let u = unit_of(p);
            match groups.iter_mut().find(|(g, _)| *g == u) {
                Some((_, v)) => v.push(p),
                None => groups.push((u, vec![p])),
            }
        }
        let mut edits: Vec<Edit> = groups
            .into_iter()
            .map(|(u, pages)| Edit::SetPageBox { pages, which: self.which, spec: BoxSpec::Margins(self.margins.map(|m| m / u)) })
            .collect();
        if edits.len() == 1 {
            return edits.remove(0);
        }
        // Worded as the single edit is (`Edit::label`), so one undo step reads the same either way.
        let label = if self.which == PageBox::Crop { "Crop pages" } else { "Set page boxes" };
        Edit::Batch { label: label.into(), edits }
    }
}

/// The margins (left, bottom, right, top) of `which` inside the media box, in physical points: the
/// page's user-space differences times its `/UserUnit`, so the dialog shows what is written back.
fn margins_of(b: &[[f64; 4]; 5], which: PageBox, user_unit: f64) -> [f64; 4] {
    let (m, x) = (b[0], b[box_index(which)]);
    if which == PageBox::Media {
        return [0.0; 4];
    }
    [x[0] - m[0], x[1] - m[1], m[2] - x[2], m[3] - x[3]].map(|v| (v.max(0.0) * user_unit * 100.0).round() / 100.0)
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
    let page_unit = doc.page_user_unit(current);
    let count = boxes.len().max(1);
    let units = app.units;
    let d = &mut app.boxes_draft;
    // Opening the dialog starts in the unit chosen in Preferences.
    if d.seeded.is_none() {
        d.unit = units;
    }
    // Seed the margins from the current page's box when the dialog opens or the box changes.
    if d.seeded != Some((current, d.which))
        && let Some(b) = boxes.get(current)
    {
        d.margins = margins_of(b, d.which, page_unit);
        d.seeded = Some((current, d.which));
    }
    ui.label(egui::RichText::new(tl!("Set Page Boxes")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            egui::Grid::new("boxes-grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label(tl!("Box"));
                let names = [
                    (PageBox::Crop, "CropBox"),
                    (PageBox::Trim, "TrimBox"),
                    (PageBox::Bleed, "BleedBox"),
                    (PageBox::Art, "ArtBox"),
                    (PageBox::Media, "MediaBox"),
                ];
                let cur = names.iter().find(|(b, _)| *b == d.which).map_or("CropBox", |(_, l)| *l);
                craft_ui::ComboBox::from_id_salt("boxes-which").selected_text(cur).show_ui(ui, |ui| {
                    for (b, l) in names {
                        ui.selectable_value(&mut d.which, b, l);
                    }
                });
                ui.end_row();
                ui.label(tl!("Units"));
                craft_ui::ComboBox::from_id_salt("boxes-unit").selected_text(d.unit.label()).show_ui(ui, |ui| {
                    for u in Unit::ALL {
                        ui.selectable_value(&mut d.unit, u, u.label());
                    }
                });
                ui.end_row();
                let k = d.unit.per_point();
                for (label, idx) in [(tl!("Top"), 3), (tl!("Bottom"), 1), (tl!("Left"), 0), (tl!("Right"), 2)] {
                    let l = ui.label(label);
                    let mut v = d.margins[idx] * k;
                    let speed = d.unit.speed();
                    let r = ui
                        .add(egui::DragValue::new(&mut v).speed(speed).range(0.0..=10_000.0).max_decimals(3).suffix(format!(" {}", d.unit.label())))
                        .labelled_by(l.id);
                    if r.changed() {
                        d.margins[idx] = (v / k).max(0.0);
                    }
                    ui.end_row();
                }
                ui.label("");
                if ui.button(tl!("Set to zero")).clicked() {
                    d.margins = [0.0; 4];
                }
                ui.end_row();
                ui.label(tl!("Pages"));
                ui.vertical(|ui| {
                    widgets::radio_value(ui, &mut d.range, Range::All, tl!("All"));
                    widgets::radio_value(
                        ui,
                        &mut d.range,
                        Range::Current,
                        crate::i18n::fmt(tl!("Current page ({n})"), &[("n", &(current + 1).to_string())]),
                    );
                    let (mut a, mut b) = match d.range {
                        Range::Pages(a, b) => (a, b),
                        _ => (1, count),
                    };
                    ui.horizontal(|ui| {
                        let on = matches!(d.range, Range::Pages(..));
                        if ui.add(widgets::radio(on, tl!("From"))).clicked() {
                            d.range = Range::Pages(a, b);
                        }
                        let ra = ui.add(egui::DragValue::new(&mut a).range(1..=count));
                        ui.label(tl!("to"));
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
            let [l, bo, ri, to] = d.margins.map(|v| (v / page_unit) as f32 * s);
            let inner = Rect::from_min_max(pos2(page.left() + l, page.top() + to), pos2(page.right() - ri, page.bottom() - bo));
            if inner.is_positive() {
                ui.painter().rect_stroke(inner, CornerRadius::ZERO, Stroke::new(1.5, t.accent), egui::StrokeKind::Middle);
            }
            ui.painter().text(
                page.center_bottom() + vec2(0.0, 6.0),
                egui::Align2::CENTER_TOP,
                crate::i18n::fmt(tl!("Page {label}"), &[("label", &(current + 1).to_string())]),
                theme::regular(11.0),
                t.text_faint,
            );
        }
    });
    ui.add_space(12.0);
    let (mut apply, mut cancel) = (false, false);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, tl!("OK"), true).clicked() {
            apply = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
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
        assert!(matches!(d.edit(0, 5, &[]), Edit::SetPageBox { spec: BoxSpec::Remove, .. }));
        d.margins = [72.0, 0.0, 0.0, 0.0];
        assert!(matches!(d.edit(0, 5, &[]), Edit::SetPageBox { spec: BoxSpec::Margins(_), which: PageBox::Crop, .. }));
    }

    /// Two pages of 432 x 360 points, the first with `/UserUnit` 1 and the second with 2, opened in a session.
    fn session_with_user_units() -> (pdfcraft_engine::Session, pdfcraft_engine::DocId) {
        use pdfcraft_cos::{Document as Cos, Object, SaveOptions, write_full};
        let mut session = pdfcraft_engine::Session::new();
        let blank = session.create_blank(432.0, 360.0, 2).unwrap();
        let mut cos = Cos::open(blank).unwrap();
        let second = pdfcraft_model::pages(&cos)[1].obj;
        cos.update_dict(second, |d| d.set(b"UserUnit".to_vec(), Object::Int(2))).unwrap();
        let bytes = write_full(&cos, &SaveOptions::default()).unwrap();
        let id = session.open_new("units.pdf", std::sync::Arc::new(bytes)).unwrap();
        (session, id)
    }

    /// #7414: half an inch is 36 physical points, whatever the page's `/UserUnit`.
    #[test]
    fn physical_margins_are_converted_by_each_pages_user_unit() {
        let (mut session, id) = session_with_user_units();
        let units = session.get(id).unwrap().page_user_units();
        assert_eq!(units, [1.0, 2.0]);
        let mut d = BoxesDraft { which: PageBox::Crop, margins: [36.0; 4], range: Range::Current, ..Default::default() };
        session.apply(id, d.edit(1, 2, &units)).unwrap();
        let b = session.get(id).unwrap().page_boxes();
        assert_eq!(b[1][1], [18.0, 18.0, 414.0, 342.0], "UserUnit 2: 36 points are 18 user units");
        assert_eq!(b[0][1], [0.0, 0.0, 432.0, 360.0], "the page outside the range is untouched");
        d.range = Range::Pages(1, 1);
        session.apply(id, d.edit(0, 2, &units)).unwrap();
        assert_eq!(session.get(id).unwrap().page_boxes()[0][1], [36.0, 36.0, 396.0, 324.0], "UserUnit 1 keeps the margin as typed");
    }

    #[test]
    fn pages_with_different_user_units_get_their_own_margins_in_one_step() {
        let (mut session, id) = session_with_user_units();
        let units = session.get(id).unwrap().page_user_units();
        let d = BoxesDraft { which: PageBox::Crop, margins: [36.0; 4], range: Range::All, ..Default::default() };
        session.apply(id, d.edit(0, 2, &units)).unwrap();
        let doc = session.get(id).unwrap();
        let b = doc.page_boxes();
        assert_eq!(b[0][1], [36.0, 36.0, 396.0, 324.0]);
        assert_eq!(b[1][1], [18.0, 18.0, 414.0, 342.0]);
        assert_eq!(doc.can_undo(), Some("Crop pages"));
        session.undo(id).unwrap();
        let b = session.get(id).unwrap().page_boxes();
        assert_eq!(b[0][1], [0.0, 0.0, 432.0, 360.0], "one undo restores page 1");
        assert_eq!(b[1][1], [0.0, 0.0, 432.0, 360.0], "and page 2");
    }

    #[test]
    fn the_dialog_shows_the_saved_margins_in_physical_points() {
        let (session, id) = session_with_user_units();
        let doc = session.get(id).unwrap();
        let mut boxes = doc.page_boxes()[1];
        boxes[1] = [18.0, 18.0, 414.0, 342.0];
        assert_eq!(margins_of(&boxes, PageBox::Crop, 2.0), [36.0; 4], "18 user units on a UserUnit 2 page are half an inch");
        assert_eq!(margins_of(&boxes, PageBox::Media, 2.0), [0.0; 4]);
    }
}
