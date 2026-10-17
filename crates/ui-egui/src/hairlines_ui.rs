//! Print production ▸ Fix hairlines: lines narrower than a width on the page become a chosen
//! width; Type 3 glyphs and pattern strokes on request; the pages. Fix says how many strokes
//! changed, and refuses nothing: a document with no hairlines is told so.

use pdfcraft_engine::{Edit, Hairlines};

use crate::pageboxes::Range;
use crate::theme::{self, Tokens};
use crate::units::Unit;
use crate::{PdfCraftApp, widgets};

#[derive(Clone, Debug, PartialEq)]
pub struct HairlinesDraft {
    /// Both widths in points (shown in the preferences' unit).
    pub settings: Hairlines,
    pub range: Range,
}

impl Default for HairlinesDraft {
    fn default() -> Self {
        Self { settings: Hairlines::default(), range: Range::All }
    }
}

impl HairlinesDraft {
    pub fn pages(&self, current: usize, count: usize) -> Vec<usize> {
        match self.range {
            Range::All => (0..count).collect(),
            Range::Current => vec![current.min(count.saturating_sub(1))],
            Range::Pages(a, b) => (a.max(1) - 1..b.min(count)).collect(),
        }
    }
}

/// A width in points edited in `unit`.
fn width_field(ui: &mut egui::Ui, pts: &mut f64, unit: Unit) {
    let k = unit.per_point();
    let mut v = *pts * k;
    let step = 0.001 * k;
    if ui.add(egui::DragValue::new(&mut v).range(0.0..=10.0 * k).speed(step.max(0.0005)).fixed_decimals(unit.decimals().max(3))).changed() {
        *pts = v / k;
    }
    ui.label(unit.label());
}

/// The dialog: (fix, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let Some((i, id)) = app.active_ids() else { return (false, true) };
    let count = app.session.get(id).map(|d| d.info.pages.len()).unwrap_or(0);
    let current = app.views[i].current;
    let unit = app.units;
    let d = &mut app.hairlines_draft;
    ui.label(egui::RichText::new(tl!("Fix hairlines")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    egui::Grid::new("hairlines").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label(tl!("Narrower than"));
        ui.horizontal(|ui| width_field(ui, &mut d.settings.narrower_than, unit));
        ui.end_row();
        ui.label(tl!("New width"));
        ui.horizontal(|ui| width_field(ui, &mut d.settings.width, unit));
        ui.end_row();
    });
    ui.label(egui::RichText::new(tl!("Zero-width lines always count as narrower.")).small().color(t.text_muted));
    widgets::section_title(ui, tl!("Also"));
    ui.horizontal(|ui| {
        ui.add(widgets::checkbox(&mut d.settings.type3, tl!("Type 3 font glyphs")));
        ui.add(widgets::checkbox(&mut d.settings.patterns, tl!("Pattern strokes")));
    });
    ui.label(egui::RichText::new(tl!("Lines drawn inside Type 3 glyphs and tiling patterns; off by default.")).small().color(t.text_muted));
    widgets::section_title(ui, tl!("Pages"));
    ui.horizontal(|ui| {
        widgets::radio_value(ui, &mut d.range, Range::All, tl!("All"));
        widgets::radio_value(ui, &mut d.range, Range::Current, crate::i18n::fmt(tl!("Current page ({n})"), &[("n", &(current + 1).to_string())]));
        let (mut a, mut b) = match d.range {
            Range::Pages(a, b) => (a, b),
            _ => (1, count.max(1)),
        };
        let on = matches!(d.range, Range::Pages(..));
        if ui.add(widgets::radio(on, tl!("From"))).clicked() {
            d.range = Range::Pages(a, b);
        }
        let ra = ui.add(egui::DragValue::new(&mut a).range(1..=count.max(1)));
        ui.label(tl!("to"));
        let rb = ui.add(egui::DragValue::new(&mut b).range(1..=count.max(1)));
        if ra.changed() || rb.changed() {
            d.range = Range::Pages(a.min(b), b.max(a));
        }
    });
    ui.add_space(12.0);
    let (mut go, mut cancel) = (false, false);
    let ready = count > 0 && d.settings.width > 0.0;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.add_enabled_ui(ready, |ui| widgets::pill_button(ui, tl!("Fix"), true)).inner.clicked() {
            go = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (go, cancel)
}

impl PdfCraftApp {
    /// Fix: count first (the edit refuses when there is nothing to fix), then apply and say.
    pub(crate) fn fix_hairlines(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        let count = self.session.get(id).map(|d| d.info.pages.len()).unwrap_or(0);
        let pages = self.hairlines_draft.pages(self.views[i].current, count);
        let settings = self.hairlines_draft.settings.clone();
        let found = self.session.count_hairlines(id, &pages, &settings);
        if found.lines == 0 {
            self.notify_tr("No hairlines found on those pages");
            return;
        }
        if self.apply_edit(Edit::FixHairlines { pages, settings }) {
            self.notify_fmt("Fixed {n} hairlines on {p} pages", &[("n", &found.lines.to_string()), ("p", &found.pages.to_string())]);
        }
    }
}
