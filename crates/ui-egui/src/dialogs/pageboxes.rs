//! Set Page Boxes (Acrobat: Organize pages ▸ Set page boxes / Crop pages ▸ double-click), M4.4.
//!
//! Choose a box (crop, trim, bleed, art, media), margins from the media box in points, inches,
//! millimetres or centimetres, and the pages. A preview shows the current page's media box and the new box.

use craft::pages::ui::{PageRangeChoices, PageRangeOptions};
use craft::pages::{PageRange, PageScope};
use craft::ui::buttons::Button;
use craft::ui::contract::{Bind, Show as _};
use craft::ui::dialog::dialog_id;
use craft::ui::display::{Picture, PictureSize};
use craft::ui::list::{ThumbAspect, ThumbSize};
use craft::ui::patterns::form_dialogs::{DialogFields, HostedParamDialog, NumberUnit, Param, ParamDialog, ParamKind};
use craft::ui::primitives::StrokeWidth;
use craft::ui::runtime::env::UnitKind;
use egui::{CornerRadius, Rect, Stroke, pos2, vec2};
use pdfcraft_engine::{BoxSpec, Edit, PageBox};

use crate::PdfCraftApp;
use crate::units::Unit;

/// The dialog's key.
const KEY: &str = "page.boxes";

/// The margin rows' keys, in the draft's order: left, bottom, right, top.
const MARGIN_KEYS: [&str; 4] = ["left", "bottom", "right", "top"];

/// The boxes the dialog offers, with the id of each in the choice row and its name.
const BOXES: [(PageBox, &str, &str); 5] = [
    (PageBox::Crop, "crop", "CropBox"),
    (PageBox::Trim, "trim", "TrimBox"),
    (PageBox::Bleed, "bleed", "BleedBox"),
    (PageBox::Art, "art", "ArtBox"),
    (PageBox::Media, "media", "MediaBox"),
];

/// The framework's unit of a length unit of the app.
pub(crate) fn unit_kind(unit: Unit) -> UnitKind {
    match unit {
        Unit::Inches => UnitKind::Inches,
        Unit::Millimetres => UnitKind::Millimetres,
        Unit::Centimetres => UnitKind::Centimetres,
        Unit::Points => UnitKind::Points,
    }
}

/// The unit's id in the unit choice row.
fn unit_id(unit: Unit) -> &'static str {
    match unit {
        Unit::Inches => "in",
        Unit::Millimetres => "mm",
        Unit::Centimetres => "cm",
        Unit::Points => "pt",
    }
}

/// Dialog state. Margins are kept in points: left, bottom, right, top.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxesDraft {
    pub which: PageBox,
    pub unit: Unit,
    pub margins: [f64; 4],
    pub range: PageRange,
    /// The page and box the margins were seeded from when the dialog opened.
    pub seeded: Option<(usize, PageBox)>,
}

impl Default for BoxesDraft {
    fn default() -> Self {
        Self { which: PageBox::Crop, unit: Unit::Inches, margins: [0.0; 4], range: PageRange::default(), seeded: None }
    }
}

impl BoxesDraft {
    /// The edit setting the box on `pages` (0-based).
    pub fn edit(&self, pages: Vec<usize>) -> Edit {
        let spec =
            if self.margins.iter().all(|m| *m == 0.0) && self.which != PageBox::Media { BoxSpec::Remove } else { BoxSpec::Margins(self.margins) };
        Edit::SetPageBox { pages, which: self.which, spec }
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

/// The margins the rows of `f` hold, in points.
fn margins_of(f: &DialogFields) -> [f64; 4] {
    MARGIN_KEYS.map(|k| f.number(k).unwrap_or(0.0).max(0.0))
}

/// Draws the media box and the new box inside it, to scale.
fn paint_preview(cx: &craft::ui::primitives::ContentPainterCx<'_>, rect: Rect, media: [f64; 4], margins: [f64; 4]) {
    let (w, h) = ((media[2] - media[0]).max(1.0), (media[3] - media[1]).max(1.0));
    let s = (rect.width() / w as f32).min(rect.height() / h as f32);
    let page = Rect::from_center_size(rect.center(), vec2(w as f32 * s, h as f32 * s));
    // The page is paper: white, whatever the theme.
    cx.painter.rect_filled(page, CornerRadius::ZERO, egui::Color32::WHITE);
    cx.painter.rect_stroke(page, CornerRadius::ZERO, Stroke::new(cx.stroke_width(StrokeWidth::Thin), egui::Color32::from_gray(120)), egui::StrokeKind::Outside);
    let [l, bo, ri, to] = margins.map(|v| v as f32 * s);
    let inner = Rect::from_min_max(pos2(page.left() + l, page.top() + to), pos2(page.right() - ri, page.bottom() - bo));
    if inner.is_positive() {
        cx.painter.rect_stroke(inner, CornerRadius::ZERO, Stroke::new(cx.stroke_width(StrokeWidth::Medium), egui::Color32::from_rgb(38, 115, 230)), egui::StrokeKind::Middle);
    }
}

/// The rows of the dialog for a document of `count` pages with `current` on screen; `media` is the current page's
/// media box.
fn params(count: usize, current: usize, media: Option<[f64; 4]>) -> Vec<Param> {
    let boxes = BOXES.iter().map(|(_, id, name)| (id.to_string(), (*name).into())).collect();
    let units: Vec<NumberUnit> = Unit::ALL.iter().map(|u| NumberUnit::new(unit_id(*u), unit_kind(*u), u.label(), 1.0 / u.per_point())).collect();
    let unit_choices = Unit::ALL.iter().map(|u| (unit_id(*u).to_string(), u.label().into())).collect();
    let mut rows = vec![
        Param::new("box", tl!("Box"), ParamKind::choice(boxes, Some("crop".to_string()))),
        Param::new("unit", tl!("Units"), ParamKind::choice(unit_choices, Some(unit_id(Unit::Inches).to_string()))),
    ];
    for (label, key) in [(tl!("Top"), "top"), (tl!("Bottom"), "bottom"), (tl!("Left"), "left"), (tl!("Right"), "right")] {
        rows.push(Param::number_in_units(key, label, 0.0, 1.0, 3, "unit", units.clone()).bounds(Some(0.0), Some(10_000.0)));
    }
    rows.push(
        Param::custom("zero", "", |ui, fields, _| {
            let out = Button::new(tl!("Set to zero")).show(ui);
            if out.clicked() {
                for key in MARGIN_KEYS {
                    fields.set(key, 0.0);
                }
            }
            out.response.clone()
        })
        .full_width()
        .companions(MARGIN_KEYS),
    );
    rows.push(PageRangeOptions::param("range", tl!("Pages"), count, current, PageRangeChoices::CurrentFromTo));
    if let Some(media) = media {
        let label = crate::i18n::fmt(tl!("Page {label}"), &[("label", &(current + 1).to_string())]);
        let aspect =
            ThumbAspect::new((media[2] - media[0]).round().clamp(1.0, 65535.0) as u16, (media[3] - media[1]).round().clamp(1.0, 65535.0) as u16);
        rows.push(
            Param::custom("preview", "", move |ui, fields, _| {
                let margins = margins_of(fields);
                Picture::painter(label.clone(), aspect, move |cx, rect| paint_preview(cx, rect, media, margins)).extent(PictureSize::Sample(ThumbSize::Large)).show(ui)
            })
            .full_width(),
        );
    }
    rows
}

impl PdfCraftApp {
    /// Opens Set Page Boxes over the active document; `current_only` starts on the current page alone.
    pub(crate) fn open_page_boxes(&mut self, ctx: &egui::Context, current_only: bool) {
        let Some((i, id)) = self.active_ids() else { return };
        let current = self.views[i].current();
        let Some(doc) = self.session.get(id) else { return };
        let boxes = doc.page_boxes();
        let count = boxes.len().max(1);
        let page = boxes.get(current).copied();
        let d = &mut self.boxes_draft;
        // Opening the dialog starts in the unit chosen in Preferences.
        d.unit = self.units;
        if let Some(b) = page {
            let (m, x) = (b[0], b[box_index(d.which)]);
            d.margins = if d.which == PageBox::Media {
                [0.0; 4]
            } else {
                [x[0] - m[0], x[1] - m[1], m[2] - x[2], m[3] - x[3]].map(|v| (v.max(0.0) * 100.0).round() / 100.0)
            };
        }
        d.seeded = Some((current, d.which));
        if current_only {
            d.range.scope = PageScope::Current;
        }
        let mut fields = DialogFields::new();
        fields.set("box", BOXES.iter().find(|(b, ..)| *b == d.which).map_or("crop", |(_, id, _)| *id));
        fields.set("unit", unit_id(d.unit));
        for (key, v) in MARGIN_KEYS.iter().zip(d.margins) {
            fields.set(*key, v);
        }
        fields.set("range", PageRangeOptions::field_value(&d.range));
        let media = page.map(|b| b[0]);
        let dialog = HostedParamDialog::new(ctx, KEY, fields, move |fields| {
            ParamDialog::new(KEY, tl!("Set Page Boxes"), params(count, current, media)).values(Bind::bound(fields))
        });
        crate::dialogs::host::open(self, dialog);
    }

    /// Sets the boxes when the dialog was answered with OK.
    pub(crate) fn page_boxes_pass(&mut self, ctx: &egui::Context, _ctx: &egui::Context) {
        let Some(answer) = self.dialogs.take_by_id::<HostedParamDialog>(dialog_id(KEY)) else { return };
        let Some(result) = answer.ok() else { return };
        let Some((i, id)) = self.active_ids() else { return };
        let count = self.session.get(id).map_or(0, |d| d.info.pages.len());
        let current = self.views[i].current();
        let f = &result.fields;
        let d = &mut self.boxes_draft;
        if let Some((b, ..)) = BOXES.iter().find(|(_, id, _)| Some(*id) == f.text("box")) {
            d.which = *b;
        }
        if let Some(u) = Unit::ALL.iter().find(|u| Some(unit_id(**u)) == f.text("unit")) {
            d.unit = *u;
        }
        d.margins = margins_of(f);
        if let Some(range) = PageRangeOptions::value(f, "range") {
            d.range = range;
        }
        let pages = d.range.pages(count, current, &[]);
        let edit = d.edit(pages);
        d.seeded = None;
        self.apply_edit(ctx, edit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_margins_remove_the_box_except_for_the_media_box() {
        let mut d = BoxesDraft::default();
        assert!(matches!(d.edit(vec![0, 1]), Edit::SetPageBox { spec: BoxSpec::Remove, .. }));
        d.margins = [72.0, 0.0, 0.0, 0.0];
        assert!(matches!(d.edit(vec![0]), Edit::SetPageBox { spec: BoxSpec::Margins(_), which: PageBox::Crop, .. }));
        d.margins = [0.0; 4];
        d.which = PageBox::Media;
        assert!(matches!(d.edit(vec![0]), Edit::SetPageBox { spec: BoxSpec::Margins(_), which: PageBox::Media, .. }));
    }
}
