//! Organize pages ▸ Change page size (#7116): a standard paper or a custom size, portrait or
//! landscape, where the page's visible area goes on it (nine anchors and a move), and the pages.
//! Nothing on the pages is scaled; a preview shows the current page's new edges around what showed.

use egui::{Align, Color32, CornerRadius, Layout, Rect, Stroke, pos2, vec2};
use pdfcraft_engine::print::PAPERS;
use pdfcraft_engine::{Edit, MAX_PAGE_SIDE, Placement};

use crate::pageboxes::Range;
use crate::theme::{self, Tokens};
use crate::units::Unit;
use crate::{PdfCraftApp, widgets};

/// Dialog state. Lengths are in points, as the page is shown (after `/Rotate`).
#[derive(Clone, Debug, PartialEq)]
pub struct SizeDraft {
    /// An index into [`PAPERS`], or `None` for the custom size.
    pub paper: Option<usize>,
    pub landscape: bool,
    /// The custom width and height.
    pub custom: [f64; 2],
    pub unit: Unit,
    /// The anchor the visible area keeps: column and row, 0 to 2 (left/top to right/bottom).
    pub anchor: (u8, u8),
    /// Then moved right and down.
    pub offset: [f64; 2],
    pub range: Range,
    /// The draft was seeded from this document and page (re-seed when either changes).
    pub seeded: Option<(pdfcraft_engine::DocId, usize)>,
}

impl Default for SizeDraft {
    fn default() -> Self {
        Self {
            paper: None,
            landscape: false,
            custom: [612.0, 792.0],
            unit: Unit::Inches,
            anchor: (1, 1),
            offset: [0.0; 2],
            range: Range::All,
            seeded: None,
        }
    }
}

impl SizeDraft {
    /// The draft for a page shown `width` × `height` points: its standard paper when it is one
    /// (to a hundredth of a point, so OK alone changes nothing), turned as the page is, or else its
    /// own size as the custom size.
    pub fn seeded_from(width: f64, height: f64, unit: Unit) -> Self {
        let near = |a: f64, b: f64| (a - b).abs() < 0.01;
        let paper = PAPERS.iter().position(|(_, (w, h))| (near(*w, width) && near(*h, height)) || (near(*w, height) && near(*h, width)));
        Self { paper, landscape: width > height, custom: [width, height], unit, ..Default::default() }
    }

    /// The new size, width and height in points as shown.
    pub fn size(&self) -> [f64; 2] {
        match self.paper.and_then(|i| PAPERS.get(i)) {
            Some((_, (w, h))) => {
                let (short, long) = (w.min(*h), w.max(*h));
                if self.landscape { [long, short] } else { [short, long] }
            }
            None => self.custom,
        }
    }

    pub fn placement(&self) -> Placement {
        let share = |n: u8| f64::from(n.min(2)) / 2.0;
        Placement { across: share(self.anchor.0), down: share(self.anchor.1), offset: self.offset }
    }

    /// The pages (0-based) the dialog applies to.
    pub fn pages(&self, current: usize, count: usize) -> Vec<usize> {
        self.range.pages(current, count)
    }

    pub fn edit(&self, current: usize, count: usize) -> Edit {
        Edit::SetPageSize { pages: self.pages(current, count), size: self.size(), placement: self.placement() }
    }

    /// Where a page shown `shown` points wide and tall puts what showed on the new page:
    /// [left, top, right, bottom] in points from the new page's top-left corner.
    pub fn visible_area(&self, shown: [f64; 2]) -> [f64; 4] {
        let ([w, h], p) = (self.size(), self.placement());
        let x = p.across * (w - shown[0]) + p.offset[0];
        let y = p.down * (h - shown[1]) + p.offset[1];
        [x, y, x + shown[0], y + shown[1]]
    }
}

/// Every page's size as shown, in points: its visible area (f64, as the engine resizes it, not
/// the rounded sizes the viewer lays out with) times its `/UserUnit`, turned by its `/Rotate`.
fn shown_sizes(doc: &pdfcraft_engine::Document) -> Vec<[f64; 2]> {
    let units = doc.page_user_units();
    doc.page_boxes()
        .iter()
        .zip(&doc.info.pages)
        .enumerate()
        .map(|(i, (b, p))| {
            let u = units.get(i).copied().unwrap_or(1.0);
            let (w, h) = ((b[1][2] - b[1][0]) * u, (b[1][3] - b[1][1]) * u);
            if p.rotation % 180 == 90 { [h, w] } else { [w, h] }
        })
        .collect()
}

/// The sides (points) and the move (points each way, either direction) that every page with these
/// `/UserUnit`s takes: the engine allows 1 to [`MAX_PAGE_SIDE`] user-space units, so points scale
/// with each page's unit, and pages with different units take only what fits them all.
fn bounds(units: impl IntoIterator<Item = f64>) -> (std::ops::RangeInclusive<f64>, f64) {
    let (least, most) = units.into_iter().fold((f64::INFINITY, 1.0_f64), |(lo, hi), u| (lo.min(u), hi.max(u)));
    let reach = MAX_PAGE_SIDE * if least.is_finite() { least } else { 1.0 };
    (most.min(reach)..=reach, reach)
}

/// The red other dialogs warn in.
const WARNING: Color32 = Color32::from_rgb(0xD1, 0x3B, 0x3B);

const ANCHOR_NAMES: [[&str; 3]; 3] = [["Top left", "Top", "Top right"], ["Left", "Centre", "Right"], ["Bottom left", "Bottom", "Bottom right"]];

/// A length in points edited in `unit`, named by the label `label`.
fn length_field(ui: &mut egui::Ui, pts: &mut f64, unit: Unit, range: std::ops::RangeInclusive<f64>, label: egui::Id) -> bool {
    let k = unit.per_point();
    let mut v = *pts * k;
    let r = ui
        .add(
            egui::DragValue::new(&mut v)
                .speed(unit.speed())
                .range(range.start() * k..=range.end() * k)
                .max_decimals(3)
                .suffix(format!(" {}", unit.label())),
        )
        .labelled_by(label);
    if r.changed() {
        *pts = v / k;
    }
    r.changed()
}

/// Draw the dialog body; returns (apply, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let Some((i, id)) = app.active_ids() else { return (false, true) };
    let current = app.views[i].current;
    let Some(doc) = app.session.get(id) else { return (false, true) };
    let count = doc.info.pages.len().max(1);
    let sizes = shown_sizes(doc);
    let size_of = |p: usize| sizes.get(p).copied().unwrap_or([612.0, 792.0]);
    let page_units = doc.page_user_units();
    let units = app.units;
    let d = &mut app.size_draft;
    // Opening the dialog starts from the current page's size, in the unit chosen in Preferences,
    // for every page.
    if d.seeded != Some((id, current)) {
        let [w, h] = size_of(current);
        *d = SizeDraft { seeded: Some((id, current)), ..SizeDraft::seeded_from(w, h, units) };
    }
    // The preview shows the first page the dialog changes.
    let first = d.pages(current, count).first().copied().unwrap_or(current);
    let (side, reach) = bounds(d.pages(current, count).iter().map(|&p| page_units.get(p).copied().unwrap_or(1.0)));
    ui.label(egui::RichText::new(tl!("Change Page Size")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            egui::Grid::new("page-size-grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label(tl!("Size"));
                let name = d.paper.and_then(|p| PAPERS.get(p)).map_or(tl!("Custom"), |(n, _)| *n);
                craft_ui::ComboBox::from_id_salt("page-size-paper").selected_text(name).show_ui(ui, |ui| {
                    for (p, (n, _)) in PAPERS.iter().enumerate() {
                        ui.selectable_value(&mut d.paper, Some(p), *n);
                    }
                    if ui.selectable_label(d.paper.is_none(), tl!("Custom")).clicked() && d.paper.is_some() {
                        d.custom = d.size();
                        d.paper = None;
                    }
                });
                ui.end_row();
                ui.label(tl!("Orientation"));
                ui.horizontal(|ui| {
                    let was = d.landscape;
                    widgets::radio_value(ui, &mut d.landscape, false, tl!("Portrait"));
                    widgets::radio_value(ui, &mut d.landscape, true, tl!("Landscape"));
                    // A custom size turns as a paper does.
                    if d.paper.is_none() && was != d.landscape {
                        let [w, h] = d.custom;
                        d.custom = if d.landscape { [w.max(h), w.min(h)] } else { [w.min(h), w.max(h)] };
                    }
                });
                ui.end_row();
                ui.label(tl!("Units"));
                craft_ui::ComboBox::from_id_salt("page-size-unit").selected_text(d.unit.label()).show_ui(ui, |ui| {
                    for u in Unit::ALL {
                        ui.selectable_value(&mut d.unit, u, u.label());
                    }
                });
                ui.end_row();
                // Typing a width or height makes the size a custom one.
                let mut size = d.size();
                for (label, idx) in [(tl!("Width"), 0), (tl!("Height"), 1)] {
                    let l = ui.label(label).id;
                    if length_field(ui, &mut size[idx], d.unit, side.clone(), l) {
                        d.custom = size;
                        d.paper = None;
                        d.landscape = size[0] > size[1];
                    }
                    ui.end_row();
                }
                ui.label(tl!("Position"));
                egui::Grid::new("page-size-anchor").spacing([2.0, 2.0]).show(ui, |ui| {
                    for (row, names) in ANCHOR_NAMES.iter().enumerate() {
                        for (col, name) in names.iter().enumerate() {
                            let at = (col as u8, row as u8);
                            widgets::radio_value(ui, &mut d.anchor, at, "").on_hover_text(tl!(name));
                        }
                        ui.end_row();
                    }
                });
                ui.end_row();
                for (label, idx) in [(tl!("Move right"), 0), (tl!("Move down"), 1)] {
                    let l = ui.label(label).id;
                    length_field(ui, &mut d.offset[idx], d.unit, -reach..=reach, l);
                    ui.end_row();
                }
                ui.label(tl!("Pages"));
                crate::pageboxes::range_picker(ui, &mut d.range, current, count);
                ui.end_row();
            });
        });
        ui.add_space(16.0);
        ui.vertical(|ui| {
            ui.set_max_width(200.0);
            preview(ui, d, size_of(first), first, t);
        });
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

/// The new page (white) and, outlined, what page `page` (shown `shown` points) shows now, to
/// scale; a note when part of it falls outside.
fn preview(ui: &mut egui::Ui, d: &SizeDraft, shown: [f64; 2], page: usize, t: &Tokens) {
    let [w, h] = d.size();
    let area = d.visible_area(shown);
    // Fit the new page and the visible area together, so an area pushed off the page still shows.
    let (x0, y0) = (area[0].min(0.0), area[1].min(0.0));
    let (x1, y1) = (area[2].max(w), area[3].max(h));
    let s = (180.0 / (x1 - x0).max(y1 - y0).max(1.0)) as f32;
    let (r, _) = ui.allocate_exact_size(vec2(200.0, 200.0), egui::Sense::hover());
    let origin = pos2(r.center().x - ((x1 - x0) as f32 * s) / 2.0, r.top() + 10.0) - vec2(x0 as f32 * s, y0 as f32 * s);
    let at = |x: f64, y: f64| origin + vec2(x as f32 * s, y as f32 * s);
    let sheet = Rect::from_min_max(at(0.0, 0.0), at(w, h));
    ui.painter().rect_filled(sheet, CornerRadius::ZERO, Color32::WHITE);
    ui.painter().rect_stroke(sheet, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
    // As the engine counts it: a hundredth of a point over is not a cut.
    let cut = area[0] < -0.01 || area[1] < -0.01 || area[2] > w + 0.01 || area[3] > h + 0.01;
    let colour = if cut { WARNING } else { t.accent };
    ui.painter().rect_stroke(
        Rect::from_min_max(at(area[0], area[1]), at(area[2], area[3])),
        CornerRadius::ZERO,
        Stroke::new(1.5, colour),
        egui::StrokeKind::Middle,
    );
    let note = if cut {
        crate::i18n::fmt(tl!("Part of page {label} would be cut off"), &[("label", &(page + 1).to_string())])
    } else {
        crate::i18n::fmt(tl!("Page {label}"), &[("label", &(page + 1).to_string())])
    };
    // A label, not painted text: a screen reader reads the warning too.
    ui.add(egui::Label::new(egui::RichText::new(note).font(theme::regular(11.0)).color(if cut { WARNING } else { t.text_faint })).wrap());
}

impl PdfCraftApp {
    /// Change page size: apply the dialog, then say how many pages lost part of what showed.
    pub(crate) fn change_page_size(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let (count, before) = (doc.info.pages.len(), doc.page_boxes());
        let edit = self.size_draft.edit(self.views[i].current, count);
        let Edit::SetPageSize { pages, .. } = &edit else { return };
        let pages = pages.clone();
        self.size_draft.seeded = None;
        if !self.apply_edit(edit) {
            return;
        }
        let (after, units) = self.session.get(id).map(|d| (d.page_boxes(), d.page_user_units())).unwrap_or_default();
        let cut = pages
            .iter()
            .filter(|&&p| {
                let unit = units.get(p).copied().unwrap_or(1.0);
                matches!((before.get(p), after.get(p)), (Some(b), Some(a)) if pdfcraft_engine::cuts_off(b[1], a[0], unit))
            })
            .count();
        match cut {
            0 => {}
            1 => self.notify_tr("Part of 1 page is now outside the new page size"),
            n => self.notify_fmt("Part of {n} pages is now outside the new page size", &[("n", &n.to_string())]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_starts_from_the_page_and_turns_papers() {
        let a4 = PAPERS.iter().position(|(n, _)| *n == "A4").unwrap();
        let d = SizeDraft::seeded_from(841.89, 595.28, Unit::Millimetres);
        assert_eq!((d.paper, d.landscape), (Some(a4), true), "an A4 landscape page is A4, landscape");
        assert_eq!(d.size(), [841.89, 595.28]);
        let mut d = SizeDraft::seeded_from(500.0, 700.0, Unit::Points);
        assert_eq!((d.paper, d.size()), (None, [500.0, 700.0]), "an odd size is the custom size");
        d.paper = PAPERS.iter().position(|(n, _)| *n == "A3");
        d.landscape = true;
        assert_eq!(d.size(), [1190.55, 841.89]);
    }

    #[test]
    fn the_anchor_and_move_place_the_visible_area() {
        let mut d = SizeDraft { paper: None, custom: [1000.0, 800.0], ..Default::default() };
        assert_eq!(d.visible_area([600.0, 400.0]), [200.0, 200.0, 800.0, 600.0], "centred by default");
        d.anchor = (0, 2);
        d.offset = [10.0, -20.0];
        assert_eq!(d.placement(), Placement { across: 0.0, down: 1.0, offset: [10.0, -20.0] });
        assert_eq!(d.visible_area([600.0, 400.0]), [10.0, 380.0, 610.0, 780.0], "bottom left, moved right and up");
        d.range = Range::Pages(2, 9);
        assert!(matches!(d.edit(0, 3), Edit::SetPageSize { ref pages, size: [1000.0, 800.0], .. } if pages == &[1, 2]));
    }

    #[test]
    fn the_fields_take_what_every_chosen_page_takes() {
        assert_eq!(bounds([1.0]), (1.0..=MAX_PAGE_SIDE, MAX_PAGE_SIDE));
        assert_eq!(bounds([2.0]), (2.0..=2.0 * MAX_PAGE_SIDE, 2.0 * MAX_PAGE_SIDE), "points scale with /UserUnit");
        assert_eq!(bounds([1.0, 3.0]), (3.0..=MAX_PAGE_SIDE, MAX_PAGE_SIDE), "pages with other units: what fits them all");
        assert_eq!(bounds([]), (1.0..=MAX_PAGE_SIDE, MAX_PAGE_SIDE));
        assert_eq!(bounds([1.0, 20_000.0]), (MAX_PAGE_SIDE..=MAX_PAGE_SIDE, MAX_PAGE_SIDE), "never an empty range");
    }

    /// The preview's placement is the engine's: what the dialog shows is where the page ends up.
    #[test]
    fn the_dialog_places_the_page_where_the_engine_does() {
        let mut session = pdfcraft_engine::Session::new();
        let blank = session.create_blank(400.0, 300.0, 1).unwrap();
        let id = session.open_new("size.pdf", blank).unwrap();
        let d = SizeDraft { paper: None, custom: [600.0, 500.0], anchor: (2, 0), offset: [-50.0, 25.0], ..Default::default() };
        let area = d.visible_area([400.0, 300.0]);
        session.apply(id, d.edit(0, 1)).unwrap();
        let doc = session.get(id).unwrap();
        let b = doc.page_boxes()[0];
        assert_eq!([doc.info.pages[0].width, doc.info.pages[0].height], [600.0, 500.0]);
        // The old page was [0 0 400 300]: its left edge is `area[0]` points from the new page's
        // left, its top `area[1]` points down from the new page's top.
        assert_eq!([-b[0][0], b[0][3] - 300.0], [area[0], area[1]]);
        assert_eq!(doc.can_undo(), Some("Change page size"));
    }
}
