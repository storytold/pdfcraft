//! Print production ▸ Add printer marks (Acrobat's Add Printer Marks): which
//! marks, the line weight, whether to widen the page to make room, and the pages. The marks are
//! a removable page mark (Remove printer marks takes them off; adding again replaces them).

use pdfcraft_engine::{Edit, PrinterMarks};

use crate::pageboxes::Range;
use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

#[derive(Clone, Debug, PartialEq)]
pub struct PrinterMarksDraft {
    pub settings: PrinterMarks,
    pub range: Range,
    /// Widen the page when the marks would fall outside it (`settings.make_room`).
    pub make_room: bool,
    pub room: f64,
}

impl Default for PrinterMarksDraft {
    fn default() -> Self {
        Self { settings: PrinterMarks::default(), range: Range::All, make_room: true, room: 36.0 }
    }
}

impl PrinterMarksDraft {
    fn all(&self) -> bool {
        let s = &self.settings;
        s.trim && s.bleed && s.registration && s.color_bars && s.page_info
    }

    fn set_all(&mut self, on: bool) {
        let s = &mut self.settings;
        s.trim = on;
        s.bleed = on;
        s.registration = on;
        s.color_bars = on;
        s.page_info = on;
    }

    fn any(&self) -> bool {
        let s = &self.settings;
        s.trim || s.bleed || s.registration || s.color_bars || s.page_info
    }

    /// The edit for `count` pages with `current` on screen, labelled with the file's name and
    /// today's date.
    pub fn edit(&self, current: usize, count: usize, label: &str, date: &str) -> Edit {
        let pages = match self.range {
            Range::All => (0..count).collect(),
            Range::Current => vec![current.min(count.saturating_sub(1))],
            Range::Pages(a, b) => (a.max(1) - 1..b.min(count)).collect(),
        };
        let settings = PrinterMarks {
            make_room: self.make_room.then_some(self.room.max(0.0)),
            label: label.to_string(),
            date: date.to_string(),
            ..self.settings.clone()
        };
        Edit::AddPrinterMarks { pages, settings, replace: true }
    }
}

/// The dialog: (add, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let Some((i, id)) = app.active_ids() else { return (false, true) };
    let count = app.session.get(id).map(|d| d.info.pages.len()).unwrap_or(0);
    let current = app.views[i].current;
    let d = &mut app.printer_marks_draft;
    ui.label(egui::RichText::new(tl!("Add printer marks")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    widgets::section_title(ui, tl!("Marks"));
    let mut all = d.all();
    if ui.add(widgets::checkbox(&mut all, tl!("All marks"))).changed() {
        d.set_all(all);
    }
    ui.horizontal(|ui| {
        ui.add(widgets::checkbox(&mut d.settings.trim, tl!("Trim marks")));
        ui.add(widgets::checkbox(&mut d.settings.bleed, tl!("Bleed marks")));
        ui.add(widgets::checkbox(&mut d.settings.registration, tl!("Registration marks")));
    });
    ui.horizontal(|ui| {
        ui.add(widgets::checkbox(&mut d.settings.color_bars, tl!("Colour bars")));
        ui.add(widgets::checkbox(&mut d.settings.page_info, tl!("Page information")));
    });
    ui.horizontal(|ui| {
        ui.label(tl!("Line weight"));
        let weights = [(0.25, "0.25 pt"), (0.5, "0.50 pt"), (1.0, "1.00 pt")];
        let shown = weights.iter().find(|(w, _)| (*w - d.settings.line_weight).abs() < 1e-6).map_or("0.25 pt", |(_, l)| l);
        egui::ComboBox::from_id_salt("marks-weight").selected_text(shown).width(100.0).show_ui(ui, |ui| {
            for (w, l) in weights {
                ui.selectable_value(&mut d.settings.line_weight, w, l);
            }
        });
    });
    widgets::section_title(ui, tl!("Page"));
    ui.horizontal(|ui| {
        ui.add(widgets::checkbox(&mut d.make_room, tl!("Widen the page to make room")));
        ui.add_enabled_ui(d.make_room, |ui| {
            ui.label(tl!("Room on each side"));
            ui.add(egui::DragValue::new(&mut d.room).range(0.0..=288.0).suffix(" pt"));
        });
    });
    ui.label(egui::RichText::new(tl!("The trim box is kept where it is, so the printed page cuts the same.")).small().color(t.text_muted));
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
    let ready = d.any() && count > 0;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.add_enabled_ui(ready, |ui| widgets::pill_button(ui, tl!("Add"), true)).inner.clicked() {
            go = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (go, cancel)
}

impl PdfCraftApp {
    /// Add: the marks on the chosen pages, labelled with the file's name and today's date.
    pub(crate) fn add_printer_marks(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let (count, label) = (doc.info.pages.len(), doc.name.clone());
        let (y, m, d) = self.session.today();
        let date = format!("{y:04}-{m:02}-{d:02}");
        let edit = self.printer_marks_draft.edit(self.views[i].current, count, &label, &date);
        if self.apply_edit(edit) {
            self.notify_tr("Printer marks added");
        }
    }
}
