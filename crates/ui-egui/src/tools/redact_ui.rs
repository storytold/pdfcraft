//! Redact a PDF (execution plan M8.5–M8.6): mark text and areas with the Redact tool (drag across
//! text to mark it, drag elsewhere to mark a box), mark whole pages, find text or patterns and
//! mark every match, set the default box colour and overlay text, and apply all marks (with a
//! confirmation, as Acrobat asks) or clear them.

use pdfcraft_engine::{Edit, NewAnnotation, REDACTION_CODE_SETS, RedactPattern, RedactionCodeSet, Rgb, Shape, Style, rect_quad};

use crate::PdfCraftApp;

pub(crate) mod dialogs;
pub(crate) mod tool;

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
fn mark_search_match(
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
    mut on_match: impl FnMut(std::ops::Range<usize>) -> bool,
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
                if on_match(hit) {
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

impl PdfCraftApp {
    /// Mark every match of the search draft on every page, up to [`MAX_SEARCH_MATCHES`]; returns
    /// how many were marked.
    pub fn redact_search(&mut self, ctx: &egui::Context) -> usize {
        self.redact_search_up_to(ctx, MAX_SEARCH_MATCHES)
    }

    /// Mark every match of the search draft across pages, stopping once `max_matches` are marked.
    ///
    /// Phrase and word-list searches ask only for the hits still needed for that cap. Pattern
    /// search still walks `find_with` and stops once the cap is filled.
    pub fn redact_search_up_to(&mut self, ctx: &egui::Context, max_matches: usize) -> usize {
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
                    if mark_search_match(&mut edits, prefs, page, page_info, &text, &author, h) && edits.len() >= max_matches {
                        capped = true;
                        break 'pages;
                    }
                }
            } else {
                let room = max_matches.saturating_sub(edits.len());
                if consume_literal_hits(&text, &needles, d.case_sensitive, d.whole_words, room, |hit| {
                    mark_search_match(&mut edits, prefs, page, page_info, &text, &author, hit)
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
            self.apply_edit(ctx, Edit::Batch { label: "Mark for redaction".into(), edits });
        }
        n
    }

    /// Mark whole `pages` (0-based) with the current redaction properties.
    pub fn redact_pages(&mut self, ctx: &egui::Context, pages: &[usize]) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let author = self.comment_prefs.author.clone();
        let edits: Vec<Edit> = pages
            .iter()
            .filter_map(|&p| {
                let c = doc.info.pages.get(p)?.crop;
                Some(self.redact_prefs.mark(p, vec![rect_quad([c[0] as f64, c[1] as f64, c[2] as f64, c[3] as f64])], &author))
            })
            .collect();
        match <[Edit; 1]>::try_from(edits) {
            Ok([one]) => {
                self.apply_edit(ctx, one);
            }
            Err(edits) if edits.is_empty() => {}
            Err(edits) => {
                self.apply_edit(ctx, Edit::Batch { label: "Mark pages for redaction".into(), edits });
            }
        }
    }
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
        self.hidden_draft = Some(HiddenDraft { found: doc.hidden_info().into_iter().map(|(h, n)| (h, n, n > 0)).collect() });
    }
}

/// The Apply redactions prompt was confirmed: apply every mark, count the step for the
/// `_Redacted` save rules, and say what the removed text left behind.
pub(crate) fn finish_apply(ctx: &egui::Context, app: &mut PdfCraftApp) {
    let marks = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(0, |d| d.redaction_marks());
    if !app.apply_edit(ctx, Edit::ApplyRedactions { pages: None }) {
        return;
    }
    if let Some(view) = app.active.and_then(|i| app.views.get_mut(i)) {
        let delivered_undone = view.redactions_applied < view.redactions_downloaded;
        view.redactions_applied = view.redactions_applied.saturating_add(1);
        // A new step clears the redo history, so the undone saved steps can't come back.
        if view.redactions_undone_after_save > 0 || delivered_undone {
            view.redactions_undone_after_save = 0;
            view.saved_redactions_cut_off = true;
        }
    }
    if let Some((_, id)) = app.active_ids() {
        app.renew_recovery(id);
    }
    let applied = if marks == 1 {
        tl!("Applied 1 redaction mark. Save to remove the content from the file.").to_string()
    } else {
        crate::i18n::fmt(tl!("Applied {n} redaction marks. Save to remove the content from the file."), &[("n", &marks.to_string())])
    };
    // Where the removed text is still held that applying leaves as it is, and what
    // became of a PDF/A or PDF/UA claim in the document XMP it removed: said in the
    // same notification, so neither can replace the first sentence.
    let doc = app.active_ids().and_then(|(_, id)| app.session.get(id));
    let left = doc.and_then(|d| d.redaction_elsewhere()).and_then(|e| still_holding_note(&e));
    let id = doc.and_then(|d| d.redaction_identification()).and_then(|id| identification_note(&id));
    let message = [Some(applied), left, id].into_iter().flatten().collect::<Vec<_>>().join(" ");
    app.notify(message);
}

/// The Sanitize prompt was confirmed: remove every kind of hidden information.
pub(crate) fn finish_sanitize(ctx: &egui::Context, app: &mut PdfCraftApp) {
    if app.apply_edit(ctx, Edit::Sanitize) {
        if let Some((_, id)) = app.active_ids() {
            app.renew_recovery(id);
        }
        app.notify_tr("Document sanitized. Save to finish: saving rewrites the whole file.");
    }
}

/// The sentence telling the user where the removed text is still held that applying redactions
/// leaves as it is, or `None` when it is nowhere else. When the text couldn't be searched for,
/// the places are those holding any text, and the sentence says they may hold it.
pub(crate) fn still_holding_note(e: &pdfcraft_engine::RedactionElsewhere) -> Option<String> {
    let count = |n: usize, one: &str, many: &str| match n {
        0 => None,
        1 => Some(tl!(one).to_string()),
        n => Some(crate::i18n::fmt(tl!(many), &[("n", &n.to_string())])),
    };
    let places: Vec<String> = [
        count(e.bookmarks, "1 bookmark title", "{n} bookmark titles"),
        count(e.attachments, "1 attachment", "{n} attachments"),
        count(e.comments, "1 comment", "{n} comments"),
        count(e.xmp, "1 other metadata stream", "{n} other metadata streams"),
    ]
    .into_iter()
    .flatten()
    .collect();
    let (last, rest) = places.split_last()?;
    let list = if rest.is_empty() { last.clone() } else { crate::i18n::fmt(tl!("{list} and {last}"), &[("list", &rest.join(", ")), ("last", last)]) };
    let template = if e.unsearched {
        "The redacted text couldn't be searched for, so {places} may still hold it. Remove hidden information removes them."
    } else {
        "The redacted text also appears in {places}, which redaction doesn't change. Remove hidden information removes them."
    };
    Some(crate::i18n::fmt(tl!(template), &[("places", &list)]))
}

/// The sentence telling the user what became of the document's PDF/A or PDF/UA identification
/// when applying redactions removed the XMP packet that held it (and the redacted text), or
/// `None` when no such packet was removed.
pub(crate) fn identification_note(id: &pdfcraft_engine::RedactionIdentification) -> Option<String> {
    use pdfcraft_engine::RedactionClaim as Claim;
    let named = |claim: Claim| -> Option<String> {
        match (id.pdfa == claim, id.pdfua == claim) {
            (true, true) => Some(tl!("PDF/A and PDF/UA").to_string()),
            (true, false) => Some("PDF/A".to_string()),
            (false, true) => Some("PDF/UA".to_string()),
            (false, false) => None,
        }
    };
    let kept = named(Claim::Kept).map(|s| crate::i18n::fmt(tl!("Its {standard} identification was written back on its own."), &[("standard", &s)]));
    let lost = named(Claim::Lost).map(|s| {
        crate::i18n::fmt(
            tl!("Its {standard} identification couldn't be kept, so the file no longer says it conforms to {standard}."),
            &[("standard", &s)],
        )
    });
    if kept.is_none() && lost.is_none() {
        return None;
    }
    let head = tl!("The document's XMP metadata held the redacted text and was removed.").to_string();
    Some([Some(head), kept, lost].into_iter().flatten().collect::<Vec<_>>().join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_text(body: &str) -> (egui::Context, PdfCraftApp) {
        let ctx = egui::Context::default();
        let pdf = pdfcraft_engine::Session::new().create_from_text("memo", body).unwrap().to_vec();
        let mut app = PdfCraftApp::new(&ctx);
        app.open_bytes(&ctx, "memo.pdf", None, pdf).unwrap();
        (ctx, app)
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
        let (ctx, mut app) = app_with_text(&"Ada ".repeat(40));
        app.redact_search.text = "Ada".into();
        let available = available_hits(&app, &["Ada".into()], false, false);
        let cap = 5;
        assert!(available > cap, "fixture has {available} hits, cap is {cap}");
        take_materialised_hits();
        let marked = app.redact_search_up_to(&ctx, cap);
        let produced = take_materialised_hits();
        assert_eq!((marked, app.redact_search.found, app.redact_search.capped), (cap, Some(cap), true));
        assert_eq!(produced, cap, "search materialised {produced} hits after {cap} marks were already available ({available} on the page)");
    }

    #[test]
    fn word_list_search_materialises_only_the_capped_hits() {
        let n = 40;
        let words: Vec<String> = (0..n).map(|i| format!("w{i:02}x")).collect();
        let (ctx, mut app) = app_with_text(&words.join(" "));
        app.redact_search.mode = SearchMode::Words;
        app.redact_search.words = words.join("\n");
        app.redact_search.whole_words = true;
        let available = available_hits(&app, &words, false, true);
        let cap = 4;
        assert!(available > cap, "fixture has {available} hits, cap is {cap}");
        take_materialised_hits();
        let marked = app.redact_search_up_to(&ctx, cap);
        let produced = take_materialised_hits();
        assert_eq!((marked, app.redact_search.found, app.redact_search.capped), (cap, Some(cap), true));
        assert_eq!(produced, cap, "search materialised {produced} hits from a {available}-hit word list capped at {cap}");
    }

    #[test]
    fn search_below_the_cap_marks_every_literal_hit() {
        let (ctx, mut app) = app_with_text("one two three");
        app.redact_search.mode = SearchMode::Words;
        app.redact_search.words = "one\ntwo\nthree".into();
        app.redact_search.whole_words = true;
        take_materialised_hits();
        let marked = app.redact_search_up_to(&ctx, 10);
        let produced = take_materialised_hits();
        assert_eq!((marked, produced, app.redact_search.found, app.redact_search.capped), (3, 3, Some(3), false));
    }

    #[test]
    fn word_list_marks_follow_needle_order() {
        let (ctx, mut app) = app_with_text("left middle right");
        app.redact_search.mode = SearchMode::Words;
        app.redact_search.words = "right\nleft\nmiddle".into();
        app.redact_search.whole_words = true;
        let marked = app.redact_search_up_to(&ctx, 2);
        assert_eq!((marked, app.redact_search.capped), (2, true));
        let xs = redact_xs(&app);
        assert_eq!(xs.len(), 2);
        assert!(xs[0] > xs[1], "first mark is the first needle (right), then left; x was {xs:?}");
    }

    #[test]
    fn a_hit_without_geometry_does_not_consume_a_slot() {
        let (_ctx, app) = app_with_text(&"Ada ".repeat(8));
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
        let (ctx, mut sensitive) = app_with_text("Ada ada Ada");
        sensitive.redact_search.text = "Ada".into();
        sensitive.redact_search.case_sensitive = true;
        assert_eq!(sensitive.redact_search_up_to(&ctx, 10), 2);

        let (ctx, mut whole) = app_with_text("cat catalog");
        whole.redact_search.text = "cat".into();
        whole.redact_search.whole_words = true;
        assert_eq!(whole.redact_search_up_to(&ctx, 10), 1);
        assert!(!whole.redact_search.capped);
    }
}
