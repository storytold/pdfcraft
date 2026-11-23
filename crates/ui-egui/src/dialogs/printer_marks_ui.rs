//! Print production ▸ Add printer marks (Acrobat's Add Printer Marks): which
//! marks, the line weight, whether to widen the page to make room, and the pages. The marks are
//! a removable page mark (Remove printer marks takes them off; adding again replaces them).

use craft::pages::PageRange;
use craft::pages::ui::{PageRangeChoices, PageRangeOptions};
use craft::ui::choice::Checkbox;
use craft::ui::contract::{Bind, Props as _, Show as _};
use craft::ui::dialog::dialog_id;
use craft::ui::patterns::form_dialogs::{DialogFields, HostedParamDialog, Param, ParamDialog, ParamKind};
use craft::ui::runtime::env::Label;
use pdfcraft_engine::{Edit, PrinterMarks};

use crate::PdfCraftApp;
use crate::pageboxes::unit_kind;
use crate::units::Unit;

/// The dialog's key.
const KEY: &str = "prepress.marks";

/// The keys of the five mark rows, which "All marks" sets together.
const MARKS: [&str; 5] = ["trim", "bleed", "registration", "color_bars", "page_info"];

#[derive(Clone, Debug, PartialEq)]
pub struct PrinterMarksDraft {
    pub settings: PrinterMarks,
    pub range: PageRange,
    /// Widen the page when the marks would fall outside it (`settings.make_room`).
    pub make_room: bool,
    pub room: f64,
}

impl Default for PrinterMarksDraft {
    fn default() -> Self {
        Self { settings: PrinterMarks::default(), range: PageRange::default(), make_room: true, room: 36.0 }
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

    /// The edit for `pages`, labelled with the file's name and today's date.
    pub fn edit(&self, pages: Vec<usize>, label: &str, date: &str) -> Edit {
        let settings = PrinterMarks {
            make_room: self.make_room.then_some(self.room.max(0.0)),
            label: label.to_string(),
            date: date.to_string(),
            ..self.settings.clone()
        };
        Edit::AddPrinterMarks { pages, settings, replace: true }
    }

    /// Takes the dialog's values; the room is shown in a unit of `per_point` units per point.
    fn read(&mut self, f: &DialogFields, per_point: f64) {
        let s = &mut self.settings;
        s.trim = f.bool("trim").unwrap_or(s.trim);
        s.bleed = f.bool("bleed").unwrap_or(s.bleed);
        s.registration = f.bool("registration").unwrap_or(s.registration);
        s.color_bars = f.bool("color_bars").unwrap_or(s.color_bars);
        s.page_info = f.bool("page_info").unwrap_or(s.page_info);
        s.line_weight = f.number("line_weight").unwrap_or(s.line_weight);
        self.make_room = f.bool("make_room").unwrap_or(self.make_room);
        if let Some(room) = f.number("room") {
            self.room = room / per_point;
        }
        if let Some(range) = PageRangeOptions::value(f, "range") {
            self.range = range;
        }
    }

    /// Puts the values into the dialog's rows; the inverse of [`Self::read`].
    fn write(&self, f: &mut DialogFields, per_point: f64) {
        let s = &self.settings;
        f.set("trim", s.trim);
        f.set("bleed", s.bleed);
        f.set("registration", s.registration);
        f.set("color_bars", s.color_bars);
        f.set("page_info", s.page_info);
        f.set("line_weight", s.line_weight);
        f.set("make_room", self.make_room);
        f.set("room", self.room * per_point);
        f.set("range", PageRangeOptions::field_value(&self.range));
    }
}

/// The rows of the dialog for a document of `count` pages with `current` on screen.
fn params(count: usize, current: usize, unit: Unit) -> Vec<Param> {
    let k = unit.per_point();
    let marks = [
        ("trim", tl!("Trim marks")),
        ("bleed", tl!("Bleed marks")),
        ("registration", tl!("Registration marks")),
        ("color_bars", tl!("Colour bars")),
        ("page_info", tl!("Page information")),
    ];
    let mut rows = vec![
        Param::custom("all", "", move |ui, fields, _| {
            let mut d = PrinterMarksDraft::default();
            d.read(fields, k);
            let out = Checkbox::new(Bind::given(d.all())).label(tl!("All marks")).show(ui);
            if let Some(&on) = out.events.landed() {
                d.set_all(on);
                d.write(fields, k);
            }
            out.response
        })
        .companions(MARKS)
        .full_width(),
    ];
    // Production sets the marks three to a line, then two: Trim, Bleed and Registration, then Colour bars and Page
    // information.
    rows.extend(marks.iter().enumerate().map(|(at, (key, label))| {
        let row = Param::new(*key, *label, ParamKind::bool(true));
        if matches!(at, 1 | 2 | 4) { row.beside_previous() } else { row }
    }));
    rows.push(Param::number("line_weight", tl!("Line weight"), 0.25, 0.25, 2).bounds(Some(0.1), Some(4.0)).unit(unit_kind(Unit::Points)));
    rows.push(Param::new("make_room", tl!("Widen the page to make room"), ParamKind::bool(true)));
    rows.push(
        Param::number("room", tl!("Room on each side"), 36.0 * k, 1.0 * k, 2)
            .bounds(Some(0.0), Some(PrinterMarks::MAX_ROOM * k))
            .unit(unit_kind(unit))
            .enabled_when("make_room", true)
            .beside_previous(),
    );
    rows.push(Param::read_only_text("trim_note", "", tl!("The trim box is kept where it is, so the printed page cuts the same.")).full_width());
    rows.push(PageRangeOptions::param("range", tl!("Pages"), count, current, PageRangeChoices::CurrentFromTo));
    rows
}

impl PdfCraftApp {
    /// Opens Add Printer Marks over the active document.
    pub(crate) fn open_printer_marks(&mut self, ctx: &egui::Context) {
        let Some((i, id)) = self.active_ids() else { return };
        let count = self.session.get(id).map_or(0, |d| d.info.pages.len());
        let current = self.views[i].current();
        let unit = self.units;
        let mut fields = DialogFields::new();
        self.printer_marks_draft.write(&mut fields, unit.per_point());
        let dialog = HostedParamDialog::new(ctx, KEY, fields, move |fields| {
            let mut d = PrinterMarksDraft::default();
            d.read(fields, unit.per_point());
            let ready = d.any() && !d.range.pages(count, current, &[]).is_empty();
            ParamDialog::new(KEY, tl!("Add printer marks"), params(count, current, unit))
                .values(Bind::bound(fields))
                .ok_label(Label::from(tl!("Add")))
                .ok_enabled(ready)
        });
        crate::dialogs::host::open(self, dialog);
    }

    /// Adds the marks when the dialog was answered with OK.
    pub(crate) fn printer_marks_pass(&mut self, ctx: &egui::Context) {
        let Some(answer) = self.dialogs.take_by_id::<HostedParamDialog>(dialog_id(KEY)) else { return };
        let Some(result) = answer.ok() else { return };
        let per_point = self.units.per_point();
        self.printer_marks_draft.read(&result.fields, per_point);
        self.add_printer_marks(ctx);
    }

    /// Add: the marks on the chosen pages, labelled with the file's name and today's date.
    pub(crate) fn add_printer_marks(&mut self, ctx: &egui::Context) {
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let (count, label) = (doc.info.pages.len(), doc.name.clone());
        let (y, m, d) = self.session.today();
        let date = format!("{y:04}-{m:02}-{d:02}");
        let pages = self.printer_marks_draft.range.pages(count, self.views[i].current(), &[]);
        let edit = self.printer_marks_draft.edit(pages, &label, &date);
        if self.apply_edit(ctx, edit) {
            self.notify_tr("Printer marks added");
        }
    }
}
