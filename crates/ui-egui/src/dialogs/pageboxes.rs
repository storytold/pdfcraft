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

/// Dialog state. Margins are kept in physical points (1/72 in): left, bottom, right, top. A page with
/// `/UserUnit` counts each user-space unit as that many points, so they are converted per page.
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
    /// The edit setting the box on `pages` (0-based). `user_units` is each page's `/UserUnit` (points
    /// per user-space unit): the margins are physical, the page box is written in user space, so pages
    /// with different units get their own margins in one undo step.
    pub fn edit(&self, pages: Vec<usize>, user_units: &[f64]) -> Edit {
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
fn page_margins(b: &[[f64; 4]; 5], which: PageBox, user_unit: f64) -> [f64; 4] {
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

/// The margins the rows of `f` hold, in points.
fn margins_of(f: &DialogFields) -> [f64; 4] {
    MARGIN_KEYS.map(|k| f.number(k).unwrap_or(0.0).max(0.0))
}

/// Draws the media box and the new box inside it, to scale. The media box is in user space and the
/// margins in physical points, so they are drawn divided by the page's `/UserUnit`, `page_unit`.
fn paint_preview(cx: &craft::ui::primitives::ContentPainterCx<'_>, rect: Rect, media: [f64; 4], margins: [f64; 4], page_unit: f64) {
    let (w, h) = ((media[2] - media[0]).max(1.0), (media[3] - media[1]).max(1.0));
    let s = (rect.width() / w as f32).min(rect.height() / h as f32);
    let page = Rect::from_center_size(rect.center(), vec2(w as f32 * s, h as f32 * s));
    // The page is paper: white, whatever the theme.
    cx.painter.rect_filled(page, CornerRadius::ZERO, egui::Color32::WHITE);
    cx.painter.rect_stroke(
        page,
        CornerRadius::ZERO,
        Stroke::new(cx.stroke_width(StrokeWidth::Thin), egui::Color32::from_gray(120)),
        egui::StrokeKind::Outside,
    );
    let [l, bo, ri, to] = margins.map(|v| (v / page_unit) as f32 * s);
    let inner = Rect::from_min_max(pos2(page.left() + l, page.top() + to), pos2(page.right() - ri, page.bottom() - bo));
    if inner.is_positive() {
        cx.painter.rect_stroke(
            inner,
            CornerRadius::ZERO,
            Stroke::new(cx.stroke_width(StrokeWidth::Medium), egui::Color32::from_rgb(38, 115, 230)),
            egui::StrokeKind::Middle,
        );
    }
}

/// The rows of the dialog for a document of `count` pages with `current` on screen; `media` is the current page's
/// media box and `page_unit` its `/UserUnit`.
fn params(count: usize, current: usize, media: Option<[f64; 4]>, page_unit: f64) -> Vec<Param> {
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
                Picture::painter(label.clone(), aspect, move |cx, rect| paint_preview(cx, rect, media, margins, page_unit))
                    .extent(PictureSize::Sample(ThumbSize::Large))
                    .show(ui)
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
        let page_unit = doc.page_user_unit(current);
        let d = &mut self.boxes_draft;
        // Opening the dialog starts in the unit chosen in Preferences.
        d.unit = self.units;
        if let Some(b) = page {
            d.margins = page_margins(&b, d.which, page_unit);
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
            ParamDialog::new(KEY, tl!("Set Page Boxes"), params(count, current, media, page_unit)).values(Bind::bound(fields))
        });
        crate::dialogs::host::open(self, dialog);
    }

    /// Sets the boxes when the dialog was answered with OK.
    pub(crate) fn page_boxes_pass(&mut self, ctx: &egui::Context) {
        let Some(answer) = self.dialogs.take_by_id::<HostedParamDialog>(dialog_id(KEY)) else { return };
        let Some(result) = answer.ok() else { return };
        let Some((i, id)) = self.active_ids() else { return };
        let (count, user_units) = self.session.get(id).map_or((0, Vec::new()), |d| (d.info.pages.len(), d.page_user_units()));
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
        let edit = d.edit(pages, &user_units);
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
        assert!(matches!(d.edit(vec![0, 1], &[]), Edit::SetPageBox { spec: BoxSpec::Remove, .. }));
        d.margins = [72.0, 0.0, 0.0, 0.0];
        assert!(matches!(d.edit(vec![0], &[]), Edit::SetPageBox { spec: BoxSpec::Margins(_), which: PageBox::Crop, .. }));
        d.margins = [0.0; 4];
        d.which = PageBox::Media;
        assert!(matches!(d.edit(vec![0], &[]), Edit::SetPageBox { spec: BoxSpec::Margins(_), which: PageBox::Media, .. }));
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
        let d = BoxesDraft { which: PageBox::Crop, margins: [36.0; 4], ..Default::default() };
        session.apply(id, d.edit(vec![1], &units)).unwrap();
        let b = session.get(id).unwrap().page_boxes();
        assert_eq!(b[1][1], [18.0, 18.0, 414.0, 342.0], "UserUnit 2: 36 points are 18 user units");
        assert_eq!(b[0][1], [0.0, 0.0, 432.0, 360.0], "the page outside the range is untouched");
        session.apply(id, d.edit(vec![0], &units)).unwrap();
        assert_eq!(session.get(id).unwrap().page_boxes()[0][1], [36.0, 36.0, 396.0, 324.0], "UserUnit 1 keeps the margin as typed");
    }

    #[test]
    fn pages_with_different_user_units_get_their_own_margins_in_one_step() {
        let (mut session, id) = session_with_user_units();
        let units = session.get(id).unwrap().page_user_units();
        let d = BoxesDraft { which: PageBox::Crop, margins: [36.0; 4], ..Default::default() };
        session.apply(id, d.edit(vec![0, 1], &units)).unwrap();
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
        assert_eq!(page_margins(&boxes, PageBox::Crop, 2.0), [36.0; 4], "18 user units on a UserUnit 2 page are half an inch");
        assert_eq!(page_margins(&boxes, PageBox::Media, 2.0), [0.0; 4]);
    }
}
