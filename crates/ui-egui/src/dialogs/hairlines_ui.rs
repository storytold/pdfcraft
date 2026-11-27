//! Print production ▸ Fix hairlines: lines narrower than a width on the page become a chosen
//! width; Type 3 glyphs and pattern strokes on request; the pages. Fix says how many strokes
//! changed, and refuses nothing: a document with no hairlines is told so.

use pdfcraft_engine::{Edit, Hairlines};

use craft::pages::PageRange;
use craft::pages::ui::{PageRangeChoices, PageRangeOptions};
use craft::ui::contract::Bind;
use craft::ui::dialog::dialog_id;
use craft::ui::patterns::form_dialogs::{DialogFields, HostedParamDialog, Param, ParamDialog, ParamKind};
use craft::ui::runtime::env::Label;

use crate::PdfCraftApp;
use crate::pageboxes::unit_kind;
use crate::units::Unit;

/// The dialog's key.
const KEY: &str = "prepress.hairlines";

#[derive(Clone, Debug, PartialEq, Default)]
pub struct HairlinesDraft {
    /// Both widths in points (shown in the preferences' unit).
    pub settings: Hairlines,
    pub range: PageRange,
}

impl HairlinesDraft {
    /// Puts the values into the dialog's rows, the widths in the unit of `per_point` units per point.
    fn write(&self, f: &mut DialogFields, per_point: f64) {
        let s = &self.settings;
        f.set("narrower", s.narrower_than * per_point);
        f.set("width", s.width * per_point);
        f.set("type3", s.type3);
        f.set("patterns", s.patterns);
        f.set("range", PageRangeOptions::field_value(&self.range));
    }

    /// Takes the dialog's values.
    fn read(&mut self, f: &DialogFields, per_point: f64) {
        let s = &mut self.settings;
        if let Some(v) = f.number("narrower") {
            s.narrower_than = v / per_point;
        }
        if let Some(v) = f.number("width") {
            s.width = v / per_point;
        }
        s.type3 = f.bool("type3").unwrap_or(s.type3);
        s.patterns = f.bool("patterns").unwrap_or(s.patterns);
        if let Some(range) = PageRangeOptions::value(f, "range") {
            self.range = range;
        }
    }
}

/// The rows of the dialog for a document of `count` pages with `current` on screen, the widths in `unit`.
fn params(count: usize, current: usize, unit: Unit) -> Vec<Param> {
    let k = unit.per_point();
    let decimals = unit.decimals().max(3) as u32;
    let width = |key: &str, label: &'static str| {
        Param::number(key, label, 0.0, 0.001 * k, decimals).bounds(Some(0.0), Some(Hairlines::MAX_POINTS * k)).unit(unit_kind(unit))
    };
    vec![
        width("narrower", tl!("Narrower than")),
        width("width", tl!("New width")),
        Param::read_only_text("narrower_note", "", tl!("Zero-width lines always count as narrower.")).full_width(),
        Param::new("type3", tl!("Type 3 font glyphs"), ParamKind::bool(false)),
        Param::new("patterns", tl!("Pattern strokes"), ParamKind::bool(false)),
        Param::read_only_text("also_note", "", tl!("Lines drawn inside Type 3 glyphs and tiling patterns; off by default.")).full_width(),
        PageRangeOptions::param("range", tl!("Pages"), count, current, PageRangeChoices::CurrentFromTo),
    ]
}

impl PdfCraftApp {
    /// Opens Fix Hairlines over the active document.
    pub(crate) fn open_fix_hairlines(&mut self, ctx: &egui::Context) {
        let Some((i, id)) = self.active_ids() else { return };
        let count = self.session.get(id).map_or(0, |d| d.info.pages.len());
        let current = self.views[i].current();
        let unit = self.units;
        let mut fields = DialogFields::new();
        self.hairlines_draft.write(&mut fields, unit.per_point());
        let dialog = HostedParamDialog::new(ctx, KEY, fields, move |fields| {
            let pages = PageRangeOptions::value(fields, "range").unwrap_or_default().pages(count, current, &[]);
            let ready = !pages.is_empty() && fields.number("width").is_some_and(|w| w > 0.0);
            ParamDialog::new(KEY, tl!("Fix hairlines"), params(count, current, unit))
                .values(Bind::bound(fields))
                .ok_label(Label::from(tl!("Fix")))
                .ok_enabled(ready)
        });
        crate::dialogs::host::open(self, dialog);
    }

    /// Fixes the hairlines when the dialog was answered with OK.
    pub(crate) fn fix_hairlines_pass(&mut self, ctx: &egui::Context) {
        let Some(answer) = self.dialogs.take_by_id::<HostedParamDialog>(dialog_id(KEY)) else { return };
        let Some(result) = answer.ok() else { return };
        let per_point = self.units.per_point();
        self.hairlines_draft.read(&result.fields, per_point);
        self.fix_hairlines(ctx);
    }

    /// Fix: count first (the edit refuses when there is nothing to fix), then apply and say.
    pub(crate) fn fix_hairlines(&mut self, ctx: &egui::Context) {
        let Some((i, id)) = self.active_ids() else { return };
        let count = self.session.get(id).map(|d| d.info.pages.len()).unwrap_or(0);
        let pages = self.hairlines_draft.range.pages(count, self.views[i].current(), &[]);
        let settings = self.hairlines_draft.settings.clone();
        let found = self.session.count_hairlines(id, &pages, &settings);
        if found.lines == 0 {
            self.notify_tr("No hairlines found on those pages");
            return;
        }
        if self.apply_edit(ctx, Edit::FixHairlines { pages, settings }) {
            let (n, p) = (found.lines.to_string(), found.pages.to_string());
            match (found.lines, found.pages) {
                // One stroke can only be on one page.
                (1, _) => self.notify_tr("Fixed 1 hairline on 1 page"),
                (_, 1) => self.notify_fmt("Fixed {n} hairlines on 1 page", &[("n", &n)]),
                _ => self.notify_fmt("Fixed {n} hairlines on {p} pages", &[("n", &n), ("p", &p)]),
            }
        }
    }
}
