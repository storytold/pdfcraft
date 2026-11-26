//! The sheet preview of the Print dialog: the laid-out sheet as `PrintSheet` rows, its pager and the page pictures it
//! asks `Thumbnails<PrintPage>` for.

use std::collections::HashMap;

use craft::print::ui::{PrintSheet, SheetPage};
use craft_ui::patterns::form_dialogs::{PagerUnit, PanePager};
use craft_ui::primitives::{Priority, TextureState, ThumbRequest, ThumbSource, Thumbnails};
use craft_ui::runtime::env::Label;
use egui::{Pos2, Rect, Vec2, pos2, vec2};
use pdfcraft_engine::print::{self, Sheet};

use super::draft::{Handling, PrintDraft};
use super::preview_rasters;
use crate::canvas::pictures::PrintPage;

fn lbl(text: impl AsRef<str>) -> Label {
    Label::from(text.as_ref().to_owned())
}

/// What the layout of this pass says, for [`build`].
#[derive(Clone, Copy)]
pub(super) struct SheetInput<'a> {
    /// The draft; `d.sheet` is already clamped to `sheets`.
    pub d: &'a PrintDraft,
    /// The document's page sizes.
    pub sizes: &'a [(f64, f64)],
    /// The document's page labels.
    pub labels: &'a [String],
    /// What the draft says to print, or why it can't.
    pub settings: &'a Result<print::Settings, String>,
    /// The laid-out sheets.
    pub sheets: &'a [Sheet],
    /// Creep within its field's range was still too much for the booklet.
    pub too_much_creep: bool,
    /// The page box the preview drew last pass, in physical pixels; `None` before it is drawn.
    pub paper_px: Option<[u32; 2]>,
}

/// The preview for the sheet `d.sheet` of `sheets`, and the pager under it.
///
/// `settings` is what the draft says to print; its error, or the creep being too much, replaces the sheet with a
/// message. Pages on the shown sheet are asked for first, the neighbours' after.
pub(super) fn build(
    input: SheetInput<'_>,
    thumbs: &Thumbnails<PrintPage>,
    source: &dyn ThumbSource<PrintPage>,
    ctx: &egui::Context,
) -> (PrintSheet, Option<PanePager>) {
    let SheetInput { d, sizes: _, labels: _, settings, sheets, too_much_creep, paper_px: _ } = input;
    let count = sheets.len();
    let pager = PanePager { shown: d.sheet + 1, count, shown_label: None, back: d.sheet > 0, next: d.sheet + 1 < count, unit: PagerUnit::Sheet };
    let paper = vec2(d.paper_size().0 as f32, d.paper_size().1 as f32);
    let sheet = match (settings, sheets.get(d.sheet)) {
        (Err(e), _) => PrintSheet::message(paper, lbl(tl!(e.as_str())), Label::default()),
        (Ok(_), None) if too_much_creep => PrintSheet::message(
            paper,
            lbl(tl!("The creep moves the innermost sheet's pages too far for this many pages. Reduce Creep.")),
            Label::default(),
        ),
        (Ok(_), None) => PrintSheet::message(paper, lbl(tl!("No sheets")), Label::default()),
        (Ok(_), Some(shown)) => shown_sheet(input, shown, thumbs, source, ctx),
    };
    (sheet, Some(pager))
}

fn shown_sheet(
    input: SheetInput<'_>,
    shown: &Sheet,
    thumbs: &Thumbnails<PrintPage>,
    source: &dyn ThumbSource<PrintPage>,
    ctx: &egui::Context,
) -> PrintSheet {
    let SheetInput { d, sizes, labels, paper_px, .. } = input;
    let ppp = ctx.pixels_per_point();
    let on_sheet: Vec<usize> = shown.placed.iter().map(|p| p.page).collect();
    // The preview drew its page box in physical pixels; the rasters are sized from it in points.
    let pane = paper_px.map(|[w, h]| vec2(w as f32 / ppp, h as f32 / ppp));
    let rasters = preview_rasters(d, sizes, labels, ppp, ctx.input(|i| i.max_texture_side) as f32, pane);
    let mut states: HashMap<usize, TextureState> = HashMap::new();
    for (page, scale) in rasters {
        let Some(&size) = sizes.get(page) else { continue };
        let priority = if on_sheet.contains(&page) { Priority::VISIBLE } else { Priority::NEAR };
        let request = ThumbRequest::for_cell(size.0.max(size.1) as f32 * scale / ppp, ppp);
        let state = thumbs.state(ctx, source, &PrintPage { page, grayscale: d.grayscale }, &request, priority);
        states.entry(page).or_insert(state);
    }

    // Sheet space runs up from the bottom-left corner; paper points run down from the top-left.
    let height = shown.size.1;
    let to_paper = |x: f64, y: f64| -> Pos2 { pos2(x as f32, (height - y) as f32) };
    let pages = shown
        .placed
        .iter()
        .filter_map(|pl| {
            let &(dw, dh) = sizes.get(pl.page)?;
            if !(dw > 0.0 && dh > 0.0) {
                return None;
            }
            let [x0, y0, x1, y1] = pl.clip;
            // The picture's top-left, top-right, bottom-right and bottom-left, turned with the placement.
            let corners = [(x0, y1), (x1, y1), (x1, y0), (x0, y0)].map(|(x, y)| {
                let (sx, sy) = pl.matrix.apply(x, y);
                to_paper(sx, sy)
            });
            let mut page = SheetPage::new(corners, (pl.page + 1).to_string());
            if let Some(texture) = states.get(&pl.page).and_then(TextureState::shown) {
                // Texture y runs down from the page top.
                let uv = Rect::from_min_max(pos2((x0 / dw) as f32, (1.0 - y1 / dh) as f32), pos2((x1 / dw) as f32, (1.0 - y0 / dh) as f32));
                page = page.picture(texture.id, uv);
            }
            Some(page)
        })
        .collect();

    let paper = Vec2::new(shown.size.0 as f32, shown.size.1 as f32);
    let (w, h) = shown.size;
    let readout = lbl(d.units.size(w, h));
    let borders = shown.borders.iter().map(|b| Rect::from_two_pos(to_paper(b[0], b[1]), to_paper(b[2], b[3]))).collect();
    let cuts = shown.lines.iter().map(|l| [to_paper(l[0], l[1]), to_paper(l[2], l[3])]).collect();
    let mut notes = Vec::new();
    if d.handling == Handling::Size
        && let Some(p) = shown.placed.first()
    {
        let scale = p.matrix.0[0].hypot(p.matrix.0[1]) * 100.0;
        notes.push(lbl(crate::i18n::fmt(tl!("PDF scale: {scale}%"), &[("scale", &format!("{scale:.2}"))])));
        notes.push(lbl(tl!("Printer paper and scaling may differ. Check driver settings.")));
        if (scale - 100.0).abs() > 0.01 {
            notes.push(lbl(tl!("Drawing scale changes. Use Actual size for full-scale plans.")));
        }
    }
    PrintSheet::pages(paper, pages, readout).borders(borders).cuts(cuts).notes(notes)
}
