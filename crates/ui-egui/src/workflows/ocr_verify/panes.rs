//! The OCR Verify screen's side panes: the page list, the recognized text, and the word
//! inspector with its zoom crop.

use std::borrow::Cow;

use craft::behavior::{DisplayOrder, ItemSource, Selection, SelectionChange};
use craft::theme::TypeRole;
use craft::ui::buttons::Button;
use craft::ui::choice::Checkbox;
use craft::ui::contract::{Bind, CollectionEvent, Props as _, SelectMode, Show as _, ShowCollection as _, ShowContainer as _};
use craft::ui::display::{Picture, Shade, Text};
use craft::ui::layout_components::{Row, Stack};
use craft::ui::list::{List, ListRow, ThumbAspect};
use craft::ui::patterns::panels::Inspector;
use craft::ui::primitives::textures::TextureState;
use craft::ui::runtime::focus::FocusRequest;
use craft::ui::text::{RunWord, TextField, WordRun, WordState};
use egui::Rect;
use pdfcraft_engine::ocr::{OcrPage, is_suspect};

use super::screen::Intents;
use super::{source, suggestions};
use crate::PdfCraftApp;

/// The page list's rows: one per read page, keyed by the page's number so a row keeps its key
/// while the list changes under it.
struct PageRows<'a>(&'a [OcrPage]);

impl DisplayOrder<usize> for PageRows<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn key(&self, at: usize) -> usize {
        self.0.get(at).map_or(0, |page| page.page)
    }

    fn index_of(&self, key: &usize) -> Option<usize> {
        self.0.iter().position(|page| page.page == *key)
    }

    fn text(&self, at: usize) -> Option<Cow<'_, str>> {
        self.0.get(at).map(|page| Cow::Owned(format!("{} {}", tl!("Page"), page.page + 1)))
    }
}

impl ItemSource<usize> for PageRows<'_> {}

/// The page list: one row per read page, with its live word count and a flag for the words that
/// still ask for review. Choosing a row shows the page.
pub(super) fn pages(app: &PdfCraftApp, ui: &mut egui::Ui, n: &mut Intents) {
    let v = &app.ocr_verify;
    let rows = PageRows(&v.pages);
    // The pane's panel scrolls its body, so the list takes its natural height and the panel carries the scroll.
    // The screen's keys (F8) reach it while focus is inside it: the list takes the focus nothing else holds, so
    // they work from the moment the screen opens, as they always did.
    let out = List::<usize>::new(tl!("Pages"))
        .request_focus(FocusRequest::WhenNone)
        .select(SelectMode::Single)
        .selection(Bind::given(Selection::single(v.current)))
        .natural_height(true)
        .show_items(ui, &rows, &mut |key, _| {
            let Some(pg) = v.pages.iter().find(|pg| pg.page == *key) else { return ListRow::spacer() };
            let live = pg.words.iter().filter(|w| !w.text.trim().is_empty()).count();
            let suspects = pg.words.iter().filter(|w| !w.text.trim().is_empty() && is_suspect(w.confidence)).count();
            let marked = if pg.skipped.is_some() {
                format!(" · {}", tl!("skipped"))
            } else if v.review.verified_pages.contains(&pg.page) {
                format!(" · {}", tl!("verified"))
            } else {
                String::new()
            };
            let row = ListRow::new(format!("{} {}{} · {live}", tl!("Page"), pg.page + 1, marked));
            if suspects > 0 { row.detail(crate::i18n::fmt(tl!("{n} word(s) to review"), &[("n", &suspects.to_string())])) } else { row }
        });
    for event in out.events {
        if let CollectionEvent::Selection(change) = event {
            match change {
                SelectionChange::Set(keys) => n.select_page = keys.last().copied().or(n.select_page),
                SelectionChange::Toggle(key) => n.select_page = Some(key),
                _ => {}
            }
        }
    }
}

/// The RECOGNIZED pane: the page's words as the engine read them — a preview, not authoritative
/// (the source pane's boxes and the inspector are where corrections happen). Removed words show
/// struck out, corrected ones underlined; picking a word selects it in the inspector.
pub(super) fn text(app: &PdfCraftApp, ui: &mut egui::Ui, n: &mut Intents) {
    let v = &app.ocr_verify;
    let Some(pg) = v.page(v.current) else { return };
    if let Some(why) = &pg.skipped {
        Text::new(why.clone()).shade(Shade::Muted).role(TypeRole::Caption).show(ui);
        return;
    }
    if pg.words.is_empty() {
        Text::new(tl!("OCR complete — no text recognized on this page.")).shade(Shade::Muted).role(TypeRole::Caption).show(ui);
        return;
    }
    let words: Vec<RunWord<(usize, usize)>> = pg
        .words
        .iter()
        .enumerate()
        .map(|(wi, w)| {
            let key = (pg.page, wi);
            let state = if v.is_removed(key) {
                WordState::Struck
            } else if v.is_corrected(key) {
                WordState::Underlined
            } else {
                WordState::Normal
            };
            RunWord { key, text: w.text.clone(), state }
        })
        .collect();
    // A long page scrolls inside the pane, as production capped it, so the inspector under it stays in view.
    Stack::new().scroll(craft::ui::contract::Scroll::vertical()).room_height(TEXT_ROOM).show(ui, |ui, _| {
        if let Some(key) = WordRun::new(tl!("Recognized text"), &words, Bind::given(v.selected)).show(ui).picked {
            n.select_word = Some(key);
        }
    });
}

/// The most the recognized text takes of the column, in points.
const TEXT_ROOM: f32 = 180.0;

/// The ZOOM + word inspector: a magnified crop of the selected word, its facts (id, confidence,
/// engine, review state), the correction field (Enter commits; clearing removes the word; the
/// original box never moves), Remove/Restore, + Dictionary, the per-word Verified toggle, and
/// suggestions within edit distance 2.
pub(super) fn inspector(app: &mut PdfCraftApp, ui: &mut egui::Ui, n: &mut Intents) {
    let key = app.ocr_verify.selected.filter(|key| app.ocr_verify.word(*key).is_some());
    let gone = app.ocr_verify.selected.is_some() && key.is_none();
    let empty = if gone { tl!("The selected word is gone; choose another") } else { tl!("Select a word on the page to inspect it") };
    let Some(key) = key else {
        Inspector::new(tl!("Word")).empty(empty).show(ui, |_, _| {});
        return;
    };

    let ocr = &app.ocr_verify;
    let (id, conf, source_name) = {
        let w = ocr.word(key);
        (
            key.1 + 1,
            w.and_then(|w| w.confidence).map_or_else(|| "—".to_owned(), |c| format!("{c:.0}%")),
            w.map(|w| w.source.clone()).unwrap_or_default(),
        )
    };
    let state = if ocr.is_removed(key) {
        tl!("removed")
    } else if ocr.is_corrected(key) {
        tl!("corrected")
    } else if ocr.is_verified(key) {
        tl!("verified")
    } else {
        tl!("unreviewed")
    };
    let readout = format!("{id} · {conf} · {source_name} · {state}");
    let mut verified = ocr.is_verified(key);
    let verified_before = verified;
    let crop = zoom_crop(app, ui.ctx(), key);
    let suggested = {
        let word_text = app.ocr_verify.text_of(key).unwrap_or_default().to_owned();
        let page_words = app.ocr_verify.page(key.0).map(|p| p.words.clone()).unwrap_or_default();
        suggestions(&word_text, &app.ocr_verify.review.dictionary, &page_words)
    };
    let mut submitted = false;
    let correction = &mut app.ocr_verify.correction;

    Inspector::new(tl!("Word")).target(key).header(format!("{} {id}", tl!("Word"))).show(ui, |ui, _| {
        if let Some(crop) = &crop {
            // While the page picture is loading the crop paints nothing and the readout below stays.
            Picture::painter(tl!("Zoom"), crop.aspect(), |cx, rect| {
                if let Some(texture) = crop.texture {
                    cx.picture(texture, rect, crop.uv, 0.0);
                }
            })
            .show(ui);
        }
        Text::new(readout.clone()).role(TypeRole::Caption).shade(Shade::Muted).show(ui);
        // Enter commits; an empty commit removes the word, and the box never moves.
        let field =
            TextField::new(tl!("Corrected text"), Bind::bound(correction)).placeholder(tl!("Corrected text (Enter commits)")).submits().show(ui);
        submitted |= field.submitted;
        Row::new().wrap(true).show(ui, |ui, _| {
            if Button::new(tl!("Remove")).show(ui).clicked() {
                n.set_removed = Some(true);
            }
            if Button::new(tl!("Restore")).show(ui).clicked() {
                n.set_removed = Some(false);
            }
            if Button::new(tl!("+ Dictionary")).show(ui).clicked() {
                n.dictionary_add = true;
            }
            Checkbox::new(Bind::bound(&mut verified)).label(tl!("Verified")).show(ui);
            if Button::new(tl!("Undo")).tooltip(tl!("Undo the last review change (Ctrl+Z)")).show(ui).clicked() {
                n.undo = true;
            }
        });
        if !suggested.is_empty() {
            Row::new().wrap(true).show(ui, |ui, _| {
                Text::new(tl!("Suggestions")).role(TypeRole::Caption).shade(Shade::Muted).show(ui);
                for s in &suggested {
                    if Button::new(s.clone()).show(ui).clicked() {
                        n.correct = Some(s.clone());
                    }
                }
            });
        }
    });
    if submitted {
        n.correct = Some(app.ocr_verify.correction.clone());
    }
    if verified != verified_before {
        n.set_word_verified = Some(verified);
    }
}

/// The magnified crop of the selected word: a part of the source pane's page picture, so it is
/// as sharp as that picture and needs no texture of its own.
struct Crop {
    /// The page picture's texture, once it is ready.
    texture: Option<egui::TextureId>,
    /// The crop as a part of the page's picture (0 to 1 on both axes).
    uv: Rect,
    /// The crop's size in view-space points.
    size: egui::Vec2,
}

impl Crop {
    /// The crop's width to height, as the picture's aspect.
    fn aspect(&self) -> ThumbAspect {
        // The longer side counts 1000 parts, so a narrow word keeps its ratio in whole numbers.
        let scale = 1000.0 / self.size.x.max(self.size.y).max(1.0);
        ThumbAspect::new((self.size.x * scale).max(1.0) as u16, (self.size.y * scale).max(1.0) as u16)
    }
}

/// The crop of `key`'s word in the page picture (user → view through the page's rotation, plus
/// a margin), or `None` when the word or its page is not there or the crop is empty.
fn zoom_crop(app: &PdfCraftApp, ctx: &egui::Context, key: (usize, usize)) -> Option<Crop> {
    let w = app.ocr_verify.word(key)?;
    let doc = app.ocr_verify.doc?;
    let info = app.session.get(doc)?.info.pages.get(key.0)?.clone();
    let a = info.user_to_view(w.origin[0] as f32, w.origin[1] as f32);
    let b = info.user_to_view((w.origin[0] + w.across[0] + w.up[0]) as f32, (w.origin[1] + w.across[1] + w.up[1]) as f32);
    let (x0, y0, x1, y1) = (a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1]));
    let (mw, mh) = ((x1 - x0).max(8.0) * 0.5, (y1 - y0).max(8.0) * 0.6);
    let page = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(info.width.max(1.0), info.height.max(1.0)));
    let crop = Rect::from_min_max(egui::pos2(x0 - mw, y0 - mh), egui::pos2(x1 + mw, y1 + mh)).intersect(page);
    if crop.width() < 1.0 || crop.height() < 1.0 {
        return None;
    }
    let uv = Rect::from_min_max(
        egui::pos2(crop.left() / page.width(), crop.top() / page.height()),
        egui::pos2(crop.right() / page.width(), crop.bottom() / page.height()),
    );
    let texture = match source::published_side(ctx).map(|side| source::page_state(app, ctx, doc, key.0, side)) {
        Some(TextureState::Ready(texture)) => Some(texture.id),
        _ => None,
    };
    Some(Crop { texture, uv, size: crop.size() })
}
