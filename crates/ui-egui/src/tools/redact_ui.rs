//! Redact a PDF (execution plan M8.5–M8.6): mark text and areas with the Redact tool (drag across
//! text to mark it, drag elsewhere to mark a box), mark whole pages, find text or patterns and
//! mark every match, set the default box colour and overlay text, and apply all marks (with a
//! confirmation, as Acrobat asks) or clear them.

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke};
use pdfcraft_engine::{Edit, NewAnnotation, REDACT_PATTERNS, REDACTION_CODE_SETS, RedactPattern, RedactionCodeSet, Rgb, Shape, Style, rect_quad};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageToolContext, PageXform};
use crate::page_cursor::Layer;
use crate::theme::Tokens;
use crate::{PdfCraftApp, widgets};

const MARK_RED: Color32 = Color32::from_rgb(0xE3, 0x22, 0x22);

/// Redaction Tool Properties: what new marks look like once applied.
#[derive(Clone, Debug, PartialEq)]
pub struct RedactPrefs {
    /// Box colour (`None` = no box, the content just disappears).
    pub fill: Option<Rgb>,
    pub use_overlay: bool,
    pub overlay: String,
    /// The overlay text is the picked redaction codes instead of `overlay`.
    pub use_code: bool,
    pub code_set: RedactionCodeSet,
    pub codes: Vec<&'static str>,
    /// Overlay text font, size (0 = auto), colour, alignment and repetition.
    pub look: pdfcraft_engine::OverlayLook,
    /// How an unapplied mark looks on the page: its outline colour (`/C`), the fill shown while
    /// the pointer is over it (a preference, not written to the file) and its opacity (`/CA`).
    pub outline: Rgb,
    pub hover_fill: Rgb,
    pub opacity: f64,
    /// "Make properties default": OK also keeps these values for the next run. Off, they last
    /// for this run only and the saved defaults stay what they were.
    pub remember: bool,
}

impl Default for RedactPrefs {
    fn default() -> Self {
        Self {
            fill: Some([0.0, 0.0, 0.0]),
            use_overlay: false,
            overlay: String::new(),
            use_code: false,
            // Infallible: a non-empty constant array.
            code_set: REDACTION_CODE_SETS[0],
            codes: Vec::new(),
            look: Default::default(),
            outline: [0.89, 0.13, 0.13],
            hover_fill: [0.89, 0.13, 0.13],
            opacity: 1.0,
            remember: true,
        }
    }
}

impl RedactPrefs {
    pub fn overlay_text(&self) -> String {
        match (self.use_overlay, self.use_code) {
            (false, _) => String::new(),
            (true, false) => self.overlay.clone(),
            // `codes` only ever holds codes of `code_set`, so this can't fail.
            (true, true) => self.code_set.overlay(&self.codes).unwrap_or_default(),
        }
    }

    pub fn mark(&self, page: usize, quads: Vec<[f64; 8]>, author: &str) -> Edit {
        let shape = Shape::Redact { quads, overlay: self.overlay_text(), look: self.look };
        let mut style = Style::default_for(&shape);
        style.fill = self.fill;
        style.color = self.outline;
        style.opacity = self.opacity.clamp(0.0, 1.0);
        Edit::AddAnnotation(NewAnnotation { page, shape, style, contents: String::new(), author: author.to_string() })
    }

    /// The settings as saved between runs (`"redact_prefs"` in the settings file): the values
    /// the dialog's last OK with "Make properties default" ticked left.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "fill": self.fill,
            "use_overlay": self.use_overlay,
            "overlay": self.overlay,
            "use_code": self.use_code,
            "code_set": self.code_set.id,
            "codes": self.codes,
            "font": self.look.font.resource(),
            "size": self.look.size,
            "color": self.look.color,
            "align": self.look.align,
            "repeat": self.look.repeat,
            "outline": self.outline,
            "hover_fill": self.hover_fill,
            "opacity": self.opacity,
        })
    }

    /// Settings are untrusted: each value is taken only when it reads as what it should be;
    /// anything else keeps the default. `None` when nothing was saved.
    pub fn from_json(v: &serde_json::Value) -> Option<Self> {
        if !v.is_object() {
            return None;
        }
        let rgb = |x: &serde_json::Value| -> Option<Rgb> {
            let a = x.as_array()?;
            if a.len() != 3 {
                return None;
            }
            let mut out = [0.0f64; 3];
            for (slot, n) in out.iter_mut().zip(a) {
                *slot = n.as_f64().filter(|n| (0.0..=1.0).contains(n))?;
            }
            Some(out)
        };
        let mut d = Self::default();
        // `"fill": null` is a saved "no colour"; a missing or malformed fill keeps the default.
        match v.get("fill") {
            Some(serde_json::Value::Null) => d.fill = None,
            Some(x) => {
                if let Some(c) = rgb(x) {
                    d.fill = Some(c);
                }
            }
            None => {}
        }
        if let Some(b) = v["use_overlay"].as_bool() {
            d.use_overlay = b;
        }
        if let Some(t) = v["overlay"].as_str() {
            d.overlay = t.chars().take(1000).collect();
        }
        if let Some(b) = v["use_code"].as_bool() {
            d.use_code = b;
        }
        if let Some(set) = v["code_set"].as_str().and_then(RedactionCodeSet::from_id) {
            d.code_set = set;
        }
        if let Some(codes) = v["codes"].as_array() {
            // Only codes of the saved set, each once, in the set's order.
            d.codes = d.code_set.codes.iter().copied().filter(|c| codes.iter().any(|x| x.as_str() == Some(c))).collect();
        }
        if let Some(f) = v["font"].as_str() {
            d.look.font = pdfcraft_engine::OverlayFont::from_resource(f);
        }
        if let Some(n) = v["size"].as_f64().filter(|n| (0.0..=400.0).contains(n)) {
            d.look.size = n;
        }
        if let Some(c) = rgb(&v["color"]) {
            d.look.color = c;
        }
        if let Some(a) = v["align"].as_u64().filter(|a| *a <= 2) {
            d.look.align = a as u8;
        }
        if let Some(b) = v["repeat"].as_bool() {
            d.look.repeat = b;
        }
        if let Some(c) = rgb(&v["outline"]) {
            d.outline = c;
        }
        if let Some(c) = rgb(&v["hover_fill"]) {
            d.hover_fill = c;
        }
        if let Some(o) = v["opacity"].as_f64().filter(|o| (0.0..=1.0).contains(o)) {
            d.opacity = o;
        }
        Some(d)
    }
}

/// Redact pages: which pages to mark.
#[derive(Clone, Debug, PartialEq)]
pub struct PagesDraft {
    pub current: bool,
    pub from: usize,
    pub to: usize,
}

impl Default for PagesDraft {
    fn default() -> Self {
        Self { current: true, from: 1, to: 1 }
    }
}

/// What Find text and redact looks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchMode {
    #[default]
    Phrase,
    /// Every word or phrase of a list, one per line.
    Words,
    Patterns,
}

/// Find text and redact.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchDraft {
    pub mode: SearchMode,
    pub text: String,
    /// The word list, one word or phrase per line.
    pub words: String,
    pub case_sensitive: bool,
    pub whole_words: bool,
    pub pattern: RedactPattern,
    /// The result of the last search (matches marked), shown in the dialog.
    pub found: Option<usize>,
    /// Whether the last search stopped at the match limit.
    pub capped: bool,
}

impl Default for SearchDraft {
    fn default() -> Self {
        Self {
            mode: SearchMode::Phrase,
            text: String::new(),
            words: String::new(),
            case_sensitive: false,
            whole_words: false,
            pattern: RedactPattern::Phone,
            found: None,
            capped: false,
        }
    }
}

/// The largest word list file Import list accepts.
pub(crate) const MAX_WORD_LIST_BYTES: usize = 1 << 20;

/// The most matches Find text and redact marks in one search.
pub const MAX_SEARCH_MATCHES: usize = 10_000;

#[cfg(test)]
thread_local! {
    static HITS_MATERIALISED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Record how many literal-search hits were built, so a test can see a capped search stop.
#[cfg(test)]
fn note_materialised_hits(n: usize) {
    HITS_MATERIALISED.with(|c| c.set(c.get().saturating_add(n)));
}

#[cfg(test)]
fn take_materialised_hits() -> usize {
    HITS_MATERIALISED.with(|c| c.replace(0))
}

/// One search hit becomes a redaction mark when it covers glyphs. Returns whether a mark was added.
///
/// A hit with no geometry does not take a slot toward the match cap.
fn mark_search_hit(
    edits: &mut Vec<Edit>,
    prefs: &RedactPrefs,
    page: usize,
    page_info: &pdfcraft_render::PageInfo,
    text: &pdfcraft_render::PageText,
    author: &str,
    hit: std::ops::Range<usize>,
) -> bool {
    let quads: Vec<[f64; 8]> = text.line_rects(hit).into_iter().map(|rect| page_info.view_rect_to_quad(rect)).collect();
    if quads.is_empty() {
        return false;
    }
    edits.push(prefs.mark(page, quads, author));
    true
}

/// Phrase and word-list hits, in needle order, stopping once `room` of them have been marked.
///
/// Each needle is searched only for the marks still needed. A hit with no geometry does not use a
/// slot; that needle is read further until it ends or the room fills. Returns whether the room filled.
fn consume_literal_hits(
    text: &pdfcraft_render::PageText,
    needles: &[String],
    case_sensitive: bool,
    whole_words: bool,
    mut room: usize,
    mut on_hit: impl FnMut(std::ops::Range<usize>) -> bool,
) -> bool {
    if room == 0 {
        return true;
    }
    for needle in needles {
        let mut skip = 0usize;
        loop {
            let ask = skip.saturating_add(room);
            let found = text.find_opts_limited(needle, case_sensitive, whole_words, ask);
            if found.len() <= skip {
                break;
            }
            let end = found.len();
            #[cfg(test)]
            note_materialised_hits(end.saturating_sub(skip));
            let exhausted = end < ask;
            let mut blanks = 0usize;
            for hit in found.into_iter().skip(skip) {
                if on_hit(hit) {
                    room = room.saturating_sub(1);
                    if room == 0 {
                        return true;
                    }
                } else {
                    blanks = blanks.saturating_add(1);
                }
            }
            skip = end;
            // Every hit in this window became a mark, or the needle has no further hits.
            // The next needle (or the next page) is next. Blanks leave slots open, so ask again.
            if exhausted || blanks == 0 {
                break;
            }
        }
    }
    false
}

/// A box being drawn with the Redact tool: (page, start).
pub type AreaDrag = Option<(usize, Pos2)>;

fn to_user(xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> [f64; 2] {
    let (vx, vy) = xf.screen_to_view(p);
    let u = info.pages[page].view_to_user(vx, vy);
    [u[0] as f64, u[1] as f64]
}

/// Redact tool input on a page. Presses on text are left to text selection (the selection is
/// marked when it ends, see [`after_text`]); presses elsewhere draw a box. Returns `true` when
/// the gesture is a box.
pub(crate) fn page_input(cx: PageToolContext<'_>, over_text: impl Fn(Pos2) -> bool, view: &mut DocView) -> bool {
    let PageToolContext { resp, xf, ptr, page, info, .. } = cx;
    let pointer = ptr.pos;
    if let Some((dp, start)) = view.redact_drag
        && dp == page
    {
        if resp.drag_stopped() || !ptr.primary_down {
            view.redact_drag = None;
            let end = pointer.unwrap_or(start);
            let r = Rect::from_two_pos(start, end).intersect(xf.rect);
            if r.width() >= 3.0 && r.height() >= 3.0 {
                let (a, b) = (to_user(xf, info, page, r.min), to_user(xf, info, page, r.max));
                view.pending_redaction = Some((page, vec![rect_quad([a[0], a[1], b[0], b[1]])]));
            }
        }
        return true;
    }
    let Some(p) = pointer.filter(|p| xf.rect.contains(*p)) else { return false };
    if !over_text(p) {
        view.cursor.claim(Layer::ToolArea, egui::CursorIcon::Crosshair);
    }
    // The tool's own press, of the primary button: nothing drawn over the page here holds it.
    if resp.drag_started_by(egui::PointerButton::Primary) && view.router.owner(page).is_none() {
        let origin = ptr.origin.unwrap_or(p);
        if !over_text(origin) {
            view.redact_drag = Some((page, origin));
            return true;
        }
    }
    false
}

/// A finished text selection with the Redact tool becomes a mark.
pub(crate) fn after_text(resp: &egui::Response, page: usize, info: &DocInfo, view: &mut DocView) {
    if !(resp.drag_stopped() || resp.double_clicked()) || view.redact_drag.is_some() {
        return;
    }
    if let Some((p, quads)) = view.selection_quads(info).filter(|(p, _)| *p == page) {
        view.clear_selection();
        view.pending_redaction = Some((p, quads));
    }
}

/// The box being drawn, to the pages' `pointer`.
pub(crate) fn paint(painter: &egui::Painter, page: usize, view: &DocView, pointer: Option<Pos2>) {
    if let (Some((dp, start)), Some(p)) = (view.redact_drag, pointer)
        && dp == page
    {
        let r = Rect::from_two_pos(start, p);
        painter.rect_filled(r, CornerRadius::ZERO, MARK_RED.gamma_multiply(0.12));
        painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.5, MARK_RED), egui::StrokeKind::Inside);
    }
}

impl PdfCraftApp {
    /// Mark every match of the search draft on every page, up to [`MAX_SEARCH_MATCHES`]; returns
    /// how many were marked.
    pub fn redact_search(&mut self) -> usize {
        self.redact_search_up_to(MAX_SEARCH_MATCHES)
    }

    /// Mark every match of the search draft across pages, stopping once `max_matches` are marked.
    ///
    /// Phrase and word-list searches ask only for the hits still needed for that cap. Pattern
    /// search still walks `find_with` and stops once the cap is filled.
    pub fn redact_search_up_to(&mut self, max_matches: usize) -> usize {
        self.redact_search.capped = false;
        let Some((_, id)) = self.active_ids() else { return 0 };
        let Some(doc) = self.session.get(id) else { return 0 };
        let d = self.redact_search.clone();
        let needles = match d.mode {
            SearchMode::Phrase if !d.text.trim().is_empty() => vec![d.text.clone()],
            SearchMode::Words => pdfcraft_engine::redact_word_list(&d.words),
            SearchMode::Phrase | SearchMode::Patterns => Vec::new(),
        };
        if (d.mode != SearchMode::Patterns && needles.is_empty()) || max_matches == 0 {
            self.redact_search.found = Some(0);
            return 0;
        }
        let config = pdfcraft_render::RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), ..Default::default() };
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
        let mut edits = Vec::new();
        let mut capped = false;
        let author = self.comment_prefs.author.clone();
        let prefs = &self.redact_prefs;
        'pages: for (page, page_info) in doc.info.pages.iter().enumerate() {
            let out = r.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
            let Some(text) = out.text else { continue };
            if d.mode == SearchMode::Patterns {
                for h in text.find_with(|c| pdfcraft_engine::find_pattern(d.pattern, c)) {
                    if mark_search_hit(&mut edits, prefs, page, page_info, &text, &author, h) && edits.len() >= max_matches {
                        capped = true;
                        break 'pages;
                    }
                }
            } else {
                let room = max_matches.saturating_sub(edits.len());
                if consume_literal_hits(&text, &needles, d.case_sensitive, d.whole_words, room, |hit| {
                    mark_search_hit(&mut edits, prefs, page, page_info, &text, &author, hit)
                }) {
                    capped = true;
                    break 'pages;
                }
            }
        }
        let n = edits.len();
        self.redact_search.found = Some(n);
        self.redact_search.capped = capped;
        if n > 0 {
            self.apply_edit(Edit::Batch { label: "Mark for redaction".into(), edits });
        }
        n
    }

    /// Mark the pages of the Redact pages draft.
    pub fn redact_pages(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let n = doc.info.pages.len();
        let d = self.redact_pages_draft.clone();
        let pages: Vec<usize> = if d.current { vec![self.views[i].current] } else { (d.from.max(1) - 1..d.to.min(n)).collect() };
        let author = self.comment_prefs.author.clone();
        let edits: Vec<Edit> = pages
            .iter()
            .map(|&p| {
                let c = doc.info.pages[p].crop;
                self.redact_prefs.mark(p, vec![rect_quad([c[0] as f64, c[1] as f64, c[2] as f64, c[3] as f64])], &author)
            })
            .collect();
        match <[Edit; 1]>::try_from(edits) {
            Ok([one]) => {
                self.apply_edit(one);
            }
            Err(edits) if edits.is_empty() => {}
            Err(edits) => {
                self.apply_edit(Edit::Batch { label: "Mark pages for redaction".into(), edits });
            }
        }
    }
}

/// Redact pages ("Mark Page Range"). Returns (apply, cancel).
pub(crate) fn pages_body(ui: &mut egui::Ui, d: &mut PagesDraft, pages: usize, _t: &Tokens) -> (bool, bool) {
    ui.label(egui::RichText::new(tl!("Mark Page Range")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    widgets::radio_value(ui, &mut d.current, true, tl!("Current page"));
    ui.horizontal(|ui| {
        widgets::radio_value(ui, &mut d.current, false, tl!("Pages from"));
        ui.add_enabled(!d.current, egui::DragValue::new(&mut d.from).range(1..=pages));
        ui.label(tl!("to"));
        ui.add_enabled(!d.current, egui::DragValue::new(&mut d.to).range(1..=pages));
        ui.label(crate::i18n::fmt(tl!("of {name}"), &[("name", &pages.to_string())]));
    });
    d.to = d.to.max(d.from);
    ui.add_space(12.0);
    buttons(ui, "OK", true)
}

/// Find text and redact. Returns (search, cancel, import a word list).
pub(crate) fn search_body(ui: &mut egui::Ui, d: &mut SearchDraft, t: &Tokens) -> (bool, bool, bool) {
    ui.set_width(420.0);
    ui.label(egui::RichText::new(tl!("Find text and redact")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    widgets::radio_value(ui, &mut d.mode, SearchMode::Phrase, tl!("Single word or phrase"));
    let mut enter = false;
    ui.add_enabled_ui(d.mode == SearchMode::Phrase, |ui| {
        let r = ui.add(crate::widgets::line(&mut d.text).hint_text(tl!("Text to find")).desired_width(380.0)).accessible_name(tl!("Text to find"));
        enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    });
    ui.add_space(6.0);
    widgets::radio_value(ui, &mut d.mode, SearchMode::Words, tl!("Multiple words or phrases"));
    let mut import = false;
    ui.add_enabled_ui(d.mode == SearchMode::Words, |ui| {
        egui::ScrollArea::vertical().id_salt("redact-words").max_height(120.0).show(ui, |ui| {
            ui.add(egui::TextEdit::multiline(&mut d.words).hint_text(tl!("One word or phrase per line")).desired_width(380.0).desired_rows(4))
                .accessible_name(tl!("Words or phrases to redact"));
        });
        ui.horizontal(|ui| {
            import = ui.button(tl!("Import list…")).clicked();
            let n = pdfcraft_engine::redact_word_list(&d.words).len();
            ui.label(egui::RichText::new(crate::i18n::fmt(tl!("{n} word(s) or phrase(s)"), &[("n", &n.to_string())])).small().color(t.text_faint));
        });
    });
    ui.add_enabled_ui(d.mode != SearchMode::Patterns, |ui| {
        ui.add(widgets::checkbox(&mut d.whole_words, tl!("Whole words only")));
        ui.add(widgets::checkbox(&mut d.case_sensitive, tl!("Case-sensitive")));
    });
    ui.add_space(6.0);
    widgets::radio_value(ui, &mut d.mode, SearchMode::Patterns, tl!("Patterns"));
    ui.add_enabled_ui(d.mode == SearchMode::Patterns, |ui| {
        craft_ui::ComboBox::from_id_salt("redact-pattern").selected_text(tl!(d.pattern.label())).width(240.0).show_ui(ui, |ui| {
            for p in REDACT_PATTERNS {
                ui.selectable_value(&mut d.pattern, p, tl!(p.label()));
            }
        });
    });
    ui.add_space(6.0);
    ui.label(egui::RichText::new(tl!("Text in images isn't found; recognise text (OCR) first.")).small().color(t.text_faint));
    if let Some(n) = d.found {
        ui.label(
            egui::RichText::new(if n == 0 {
                tl!("No matches.").to_string()
            } else if d.capped {
                crate::i18n::fmt(tl!("First {n} matches marked for redaction (limit reached)."), &[("n", &n.to_string())])
            } else {
                crate::i18n::fmt(tl!("{n} match(es) marked for redaction."), &[("n", &n.to_string())])
            })
            .color(t.text_muted),
        );
    }
    ui.add_space(12.0);
    let (go, cancel) = buttons(ui, "Mark all", true);
    (go || enter, cancel, import)
}

/// Redaction Tool Properties. Returns (apply, cancel).
pub(crate) fn props_body(ui: &mut egui::Ui, d: &mut RedactPrefs, _t: &Tokens) -> (bool, bool) {
    ui.set_width(780.0);
    ui.label(egui::RichText::new(tl!("Redaction Tool Properties")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    // Two columns, so three colour grids fit a short window: the redacted result and its
    // overlay text on the left, the mark's own look (Acrobat's third group) on
    // the right.
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(380.0);
            egui::Grid::new("redact-props").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                ui.label(tl!("Redacted area fill colour:"));
                ui.horizontal(|ui| {
                    let mut none = d.fill.is_none();
                    if ui.add(widgets::checkbox(&mut none, tl!("No colour"))).changed() {
                        d.fill = if none { None } else { Some([0.0, 0.0, 0.0]) };
                    }
                });
                ui.end_row();
                ui.label("");
                if let Some(c) = crate::comments::swatch_grid(ui, d.fill) {
                    d.fill = Some(c);
                }
                ui.end_row();
                ui.label("");
                ui.add(widgets::checkbox(&mut d.use_overlay, tl!("Use overlay text")));
                ui.end_row();
                let on = d.use_overlay;
                let l = ui.add_enabled(on, widgets::radio(!d.use_code, tl!("Custom text:")));
                if l.clicked() {
                    d.use_code = false;
                }
                ui.add_enabled(on && !d.use_code, crate::widgets::line(&mut d.overlay).desired_width(220.0)).labelled_by(l.id);
                ui.end_row();
                if ui.add_enabled(on, widgets::radio(d.use_code, tl!("Redaction code:"))).clicked() {
                    d.use_code = true;
                }
                let codes_on = on && d.use_code;
                ui.add_enabled_ui(codes_on, |ui| {
                    craft_ui::ComboBox::from_id_salt("redaction-code-set").selected_text(d.code_set.name).show_ui(ui, |ui| {
                        for set in REDACTION_CODE_SETS {
                            if ui.selectable_label(d.code_set == set, set.name).clicked() && d.code_set != set {
                                d.code_set = set;
                                d.codes.clear();
                            }
                        }
                    });
                });
                ui.end_row();
                for row in d.code_set.codes.chunks(4) {
                    ui.label("");
                    ui.add_enabled_ui(codes_on, |ui| {
                        ui.horizontal(|ui| {
                            for &code in row {
                                let mut picked = d.codes.contains(&code);
                                if ui.add_sized([62.0, 18.0], widgets::checkbox(&mut picked, code)).changed() {
                                    d.codes.retain(|c| *c != code);
                                    if picked {
                                        d.codes.push(code);
                                    }
                                }
                            }
                        });
                    });
                    ui.end_row();
                }
                let look = &mut d.look;
                ui.label(tl!("Font:"));
                ui.add_enabled_ui(on, |ui| {
                    craft_ui::ComboBox::from_id_salt("overlay-font").selected_text(look.font.name()).show_ui(ui, |ui| {
                        for f in pdfcraft_engine::OverlayFont::ALL {
                            ui.selectable_value(&mut look.font, f, f.name());
                        }
                    });
                });
                ui.end_row();
                ui.label(tl!("Font size:"));
                ui.add_enabled_ui(on, |ui| {
                    ui.horizontal(|ui| {
                        let mut auto = look.size <= 0.0;
                        if ui.add(widgets::checkbox(&mut auto, tl!("Auto-size text to fit redaction region"))).changed() {
                            look.size = if auto { 0.0 } else { 10.0 };
                        }
                        if !auto {
                            ui.add(egui::DragValue::new(&mut look.size).range(2.0..=144.0).suffix(" pt"));
                        }
                    });
                });
                ui.end_row();
                ui.label(tl!("Font colour:"));
                ui.add_enabled_ui(on, |ui| {
                    if let Some(c) = crate::comments::swatch_grid(ui, Some(look.color)) {
                        look.color = c;
                    }
                });
                ui.end_row();
                ui.label("");
                ui.add_enabled(on, widgets::checkbox(&mut look.repeat, tl!("Repeat overlay text")));
                ui.end_row();
                ui.label(tl!("Text alignment:"));
                ui.add_enabled_ui(on, |ui| {
                    ui.horizontal(|ui| {
                        for (a, label) in [(0u8, tl!("Left")), (1, tl!("Center")), (2, tl!("Right"))] {
                            widgets::radio_value(ui, &mut look.align, a, label);
                        }
                    });
                });
                ui.end_row();
            });
        });
        // No separator here: in a horizontal layout it would stretch to the window's bottom.
        ui.add_space(28.0);
        ui.vertical(|ui| {
            ui.set_width(340.0);
            ui.label(egui::RichText::new(tl!("Redaction mark appearance")).strong());
            ui.add_space(6.0);
            egui::Grid::new("redact-mark").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                ui.label(tl!("Outline colour:"));
                if let Some(c) = crate::comments::swatch_grid(ui, Some(d.outline)) {
                    d.outline = c;
                }
                ui.end_row();
                ui.label(tl!("Fill colour:"));
                ui.vertical(|ui| {
                    if let Some(c) = crate::comments::swatch_grid(ui, Some(d.hover_fill)) {
                        d.hover_fill = c;
                    }
                    ui.label(egui::RichText::new(tl!("Shown while the pointer is over a mark.")).weak().size(11.5));
                });
                ui.end_row();
                ui.label(tl!("Opacity:"));
                let mut pct = d.opacity * 100.0;
                if ui.add(egui::Slider::new(&mut pct, 10.0..=100.0).suffix("%")).changed() {
                    d.opacity = (pct / 100.0).clamp(0.1, 1.0);
                }
                ui.end_row();
                ui.label("");
                ui.add(widgets::checkbox(&mut d.remember, tl!("Make properties default")));
                ui.end_row();
            });
        });
    });
    ui.add_space(12.0);
    buttons(ui, "OK", true)
}

/// Apply redactions confirmation. Returns (apply, cancel).
pub(crate) fn apply_body(ui: &mut egui::Ui, marks: usize, t: &Tokens) -> (bool, bool) {
    ui.set_width(420.0);
    ui.label(egui::RichText::new(tl!("Apply redactions")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.label(if marks == 1 {
        tl!("You are about to apply 1 redaction mark. Text, images and drawings under the marks, and comments and form fields that overlap them, are removed permanently.").to_string()
    } else {
        crate::i18n::fmt(tl!("You are about to apply {n} redaction marks. Text, images and drawings under the marks, and comments and form fields that overlap them, are removed permanently."), &[("n", &marks.to_string())])
    });
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(tl!("Save the document afterwards: saving rewrites the whole file so no trace of the removed content stays in it."))
            .small()
            .color(t.text_muted),
    );
    ui.add_space(12.0);
    buttons(ui, "Apply", true)
}

fn buttons(ui: &mut egui::Ui, ok: &str, primary: bool) -> (bool, bool) {
    let (mut a, mut c) = (false, false);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        // `ok` stays English internally; only the display is translated.
        if widgets::pill_button(ui, tl!(ok), primary).clicked() {
            a = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            c = true;
        }
    });
    (a, c)
}

/// Remove Hidden Information: each category with what was found, checked by default when
/// something was.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HiddenDraft {
    pub found: Vec<(pdfcraft_engine::Hidden, usize, bool)>,
}

impl PdfCraftApp {
    pub fn open_remove_hidden(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        self.hidden_draft = HiddenDraft { found: doc.hidden_info().into_iter().map(|(h, n)| (h, n, n > 0)).collect() };
        self.dialog = Some(crate::Dialog::RemoveHidden);
    }
}

/// Returns (remove, cancel).
pub(crate) fn hidden_body(ui: &mut egui::Ui, d: &mut HiddenDraft, t: &Tokens) -> (bool, bool) {
    ui.set_width(440.0);
    ui.label(egui::RichText::new(tl!("Remove hidden information")).font(crate::theme::semibold(18.0)));
    ui.add_space(4.0);
    ui.label(egui::RichText::new(tl!("Select the items to remove from this document.")).color(t.text_muted));
    ui.add_space(8.0);
    let total: usize = d.found.iter().map(|f| f.1).sum();
    for (h, n, on) in d.found.iter_mut() {
        ui.add_enabled_ui(*n > 0, |ui| {
            ui.horizontal(|ui| {
                ui.add(widgets::checkbox(on, tl!(h.label())));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(if *n == 0 { tl!("None found").to_string() } else { n.to_string() }).color(t.text_muted));
                });
            });
        });
    }
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(tl!("Form fields are flattened: their values stay visible. Saving rewrites the whole file.")).small().color(t.text_faint),
    );
    ui.add_space(12.0);
    let any = d.found.iter().any(|f| f.2 && f.1 > 0);
    let (mut a, mut c) = (false, false);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.add_enabled_ui(any && total > 0, |ui| widgets::pill_button(ui, tl!("Remove"), true)).inner.clicked() {
            a = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            c = true;
        }
    });
    (a, c)
}

/// Sanitize Document confirmation. Returns (sanitize, cancel).
pub(crate) fn sanitize_body(ui: &mut egui::Ui, t: &Tokens) -> (bool, bool) {
    ui.set_width(440.0);
    ui.label(egui::RichText::new(tl!("Sanitize document")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.label(tl!("Sanitizing removes hidden information from the document: metadata, file attachments, comments, form fields (flattened), hidden text and layers, bookmarks, links, actions and scripts, and private application data."));
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(tl!("Save the document afterwards; saving rewrites the whole file so nothing removed stays in it."))
            .small()
            .color(t.text_muted),
    );
    ui.add_space(12.0);
    buttons(ui, "Sanitize", true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_text(body: &str) -> PdfCraftApp {
        let pdf = pdfcraft_engine::Session::new().create_from_text("memo", body).unwrap().to_vec();
        let mut app = PdfCraftApp::new();
        app.open_bytes("memo.pdf", None, pdf).unwrap();
        app
    }

    /// How many literal hits the open document holds, with the same options the search uses.
    fn available_hits(app: &PdfCraftApp, needles: &[String], case_sensitive: bool, whole_words: bool) -> usize {
        let Some((_, id)) = app.active_ids() else { return 0 };
        let Some(doc) = app.session.get(id) else { return 0 };
        let config = pdfcraft_render::RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), ..Default::default() };
        let mut renderer = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
        let mut n = 0usize;
        for page in 0..doc.info.pages.len() {
            let out =
                renderer.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
            let Some(text) = out.text else { continue };
            for needle in needles {
                n = n.saturating_add(text.find_opts(needle, case_sensitive, whole_words).len());
            }
        }
        n
    }

    fn redact_xs(app: &PdfCraftApp) -> Vec<f32> {
        let Some((_, id)) = app.active_ids() else { return Vec::new() };
        let Some(doc) = app.session.get(id) else { return Vec::new() };
        doc.info.annotations.iter().filter(|a| a.subtype == "Redact").map(|a| a.rect[0]).collect()
    }

    #[test]
    fn literal_phrase_search_materialises_only_the_capped_hits() {
        let mut app = app_with_text(&"Ada ".repeat(40));
        app.redact_search.text = "Ada".into();
        let available = available_hits(&app, &["Ada".into()], false, false);
        let cap = 5;
        assert!(available > cap, "fixture has {available} hits, cap is {cap}");
        take_materialised_hits();
        let marked = app.redact_search_up_to(cap);
        let produced = take_materialised_hits();
        assert_eq!((marked, app.redact_search.found, app.redact_search.capped), (cap, Some(cap), true));
        assert_eq!(produced, cap, "search materialised {produced} hits after {cap} marks were already available ({available} on the page)");
    }

    #[test]
    fn word_list_search_materialises_only_the_capped_hits() {
        let n = 40;
        let words: Vec<String> = (0..n).map(|i| format!("w{i:02}x")).collect();
        let mut app = app_with_text(&words.join(" "));
        app.redact_search.mode = SearchMode::Words;
        app.redact_search.words = words.join("\n");
        app.redact_search.whole_words = true;
        let available = available_hits(&app, &words, false, true);
        let cap = 4;
        assert!(available > cap, "fixture has {available} hits, cap is {cap}");
        take_materialised_hits();
        let marked = app.redact_search_up_to(cap);
        let produced = take_materialised_hits();
        assert_eq!((marked, app.redact_search.found, app.redact_search.capped), (cap, Some(cap), true));
        assert_eq!(produced, cap, "search materialised {produced} hits from a {available}-hit word list capped at {cap}");
    }

    #[test]
    fn search_below_the_cap_marks_every_literal_hit() {
        let mut app = app_with_text("one two three");
        app.redact_search.mode = SearchMode::Words;
        app.redact_search.words = "one\ntwo\nthree".into();
        app.redact_search.whole_words = true;
        take_materialised_hits();
        let marked = app.redact_search_up_to(10);
        let produced = take_materialised_hits();
        assert_eq!((marked, produced, app.redact_search.found, app.redact_search.capped), (3, 3, Some(3), false));
    }

    #[test]
    fn word_list_marks_follow_needle_order() {
        let mut app = app_with_text("left middle right");
        app.redact_search.mode = SearchMode::Words;
        app.redact_search.words = "right\nleft\nmiddle".into();
        app.redact_search.whole_words = true;
        let marked = app.redact_search_up_to(2);
        assert_eq!((marked, app.redact_search.capped), (2, true));
        let xs = redact_xs(&app);
        assert_eq!(xs.len(), 2);
        assert!(xs[0] > xs[1], "first mark is the first needle (right), then left; x was {xs:?}");
    }

    #[test]
    fn a_hit_without_geometry_does_not_consume_a_slot() {
        let app = app_with_text(&"Ada ".repeat(8));
        let (_, id) = app.active_ids().unwrap();
        let doc = app.session.get(id).unwrap();
        let config = pdfcraft_render::RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), ..Default::default() };
        let mut renderer = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
        let out =
            renderer.render(pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
        let text = out.text.as_ref().unwrap();
        let mut seen = 0usize;
        let mut accepted = Vec::new();
        take_materialised_hits();
        let filled = consume_literal_hits(text, &["Ada".into()], false, false, 1, |hit| {
            seen = seen.saturating_add(1);
            if seen == 1 {
                return false;
            }
            accepted.push(hit.start);
            true
        });
        let produced = take_materialised_hits();
        assert!(filled, "the second hit filled the one open slot");
        assert_eq!(accepted.len(), 1);
        assert!(accepted[0] > 0, "the first hit was not marked; accepted {accepted:?}");
        assert!(produced > 1, "a blank hit made the search ask for another, produced {produced}");
    }

    #[test]
    fn case_and_whole_words_still_bind_literal_search() {
        let mut sensitive = app_with_text("Ada ada Ada");
        sensitive.redact_search.text = "Ada".into();
        sensitive.redact_search.case_sensitive = true;
        assert_eq!(sensitive.redact_search_up_to(10), 2);

        let mut whole = app_with_text("cat catalog");
        whole.redact_search.text = "cat".into();
        whole.redact_search.whole_words = true;
        assert_eq!(whole.redact_search_up_to(10), 1);
        assert!(!whole.redact_search.capped);
    }
}
