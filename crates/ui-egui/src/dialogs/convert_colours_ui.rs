//! Print production ▸ Convert colours (Acrobat's Convert Colors): the target
//! space, what to convert, the pages. Convert counts first and says what changed.

use pdfcraft_engine::{ColourTarget, ConvertColours, DocId, Edit};

use crate::pageboxes::Range;
use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

#[derive(Clone, Debug, PartialEq)]
pub struct ConvertColoursDraft {
    pub settings: ConvertColours,
    pub range: Range,
    /// The document `range` was chosen for.
    pub doc: Option<DocId>,
}

impl Default for ConvertColoursDraft {
    fn default() -> Self {
        Self { settings: ConvertColours::default(), range: Range::All, doc: None }
    }
}

impl ConvertColoursDraft {
    /// A range chosen for another document (pages 4–5 of a long one) means nothing on this one.
    pub fn for_document(&mut self, id: DocId) {
        if self.doc != Some(id) {
            self.range = Range::All;
            self.doc = Some(id);
        }
    }

    pub fn pages(&self, current: usize, count: usize) -> Vec<usize> {
        match self.range {
            Range::All => (0..count).collect(),
            Range::Current => vec![current.min(count.saturating_sub(1))],
            Range::Pages(a, b) => (a.max(1) - 1..b.min(count)).collect(),
        }
    }
}

/// The dialog: (convert, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let Some((i, id)) = app.active_ids() else { return (false, true) };
    let count = app.session.get(id).map(|d| d.info.pages.len()).unwrap_or(0);
    let current = app.views[i].current;
    let d = &mut app.convert_colours_draft;
    d.for_document(id);
    ui.label(egui::RichText::new(tl!("Convert colours")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    widgets::section_title(ui, tl!("Convert to"));
    ui.horizontal(|ui| {
        widgets::radio_value(ui, &mut d.settings.target, ColourTarget::Gray, tl!("Grayscale"));
        widgets::radio_value(ui, &mut d.settings.target, ColourTarget::Rgb, tl!("RGB"));
        widgets::radio_value(ui, &mut d.settings.target, ColourTarget::Cmyk, tl!("CMYK"));
    });
    ui.label(egui::RichText::new(tl!("By the standard device formulas; profiles are not applied.")).small().color(t.text_muted));
    widgets::section_title(ui, tl!("Options"));
    ui.horizontal(|ui| {
        ui.add(widgets::checkbox(&mut d.settings.preserve_black, tl!("Preserve black")));
        ui.label(egui::RichText::new(tl!("pure black stays 100% K or gray 0 instead of a mix")).small().color(t.text_muted));
    });
    ui.horizontal(|ui| {
        ui.add(widgets::checkbox(&mut d.settings.images, tl!("Pictures too")));
        ui.label(egui::RichText::new(tl!("re-encoded in the target space; JPEGs stay JPEG")).small().color(t.text_muted));
    });
    ui.horizontal(|ui| {
        ui.add(widgets::checkbox(&mut d.settings.annotations, tl!("Comments and form fields")));
        ui.label(egui::RichText::new(tl!("their appearances as well as the page")).small().color(t.text_muted));
    });
    widgets::section_title(ui, tl!("Pages"));
    ui.horizontal(|ui| {
        widgets::radio_value(ui, &mut d.range, Range::All, tl!("All"));
        widgets::radio_value(ui, &mut d.range, Range::Current, crate::i18n::fmt(tl!("Current page ({n})"), &[("n", &(current + 1).to_string())]));
        let (mut a, mut b) = match d.range {
            Range::Pages(a, b) => (a, b),
            _ => (1, count.max(1)),
        };
        let on = matches!(d.range, Range::Pages(..));
        let from = ui.add(widgets::radio(on, tl!("From")));
        if from.clicked() {
            d.range = Range::Pages(a, b);
        }
        let ra = ui.add(egui::DragValue::new(&mut a).range(1..=count.max(1))).labelled_by(from.id);
        let to = ui.label(tl!("to"));
        let rb = ui.add(egui::DragValue::new(&mut b).range(1..=count.max(1))).labelled_by(to.id);
        if ra.changed() || rb.changed() {
            d.range = Range::Pages(a.min(b), b.max(a));
        }
    });
    ui.add_space(12.0);
    let (mut go, mut cancel) = (false, false);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.add_enabled_ui(count > 0, |ui| widgets::pill_button(ui, tl!("Convert"), true)).inner.clicked() {
            go = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (go, cancel)
}

impl PdfCraftApp {
    /// Convert: count first (the edit refuses when nothing changes), then apply and say.
    pub(crate) fn convert_colours(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        let count = self.session.get(id).map(|d| d.info.pages.len()).unwrap_or(0);
        let pages = self.convert_colours_draft.pages(self.views[i].current, count);
        let settings = self.convert_colours_draft.settings.clone();
        let found = self.session.count_colours(id, &pages, &settings);
        if found.colours == 0 && found.images == 0 {
            self.notify_tr("Nothing on those pages needed converting");
            return;
        }
        if self.apply_edit(Edit::ConvertColours { pages, settings }) {
            self.notify_fmt(
                "Converted {c} colours and {i} pictures on {p} pages",
                &[("c", &found.colours.to_string()), ("i", &found.images.to_string()), ("p", &found.pages.to_string())],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_range_does_not_follow_the_user_to_another_document() {
        let (a, b) = (DocId(1), DocId(2));
        let mut d = ConvertColoursDraft::default();
        d.for_document(a);
        d.range = Range::Pages(4, 5);
        d.for_document(a);
        assert_eq!(d.range, Range::Pages(4, 5), "same document keeps its range");
        d.for_document(b);
        assert_eq!(d.range, Range::All);
        assert_eq!(d.pages(0, 1), vec![0]);
    }
}
