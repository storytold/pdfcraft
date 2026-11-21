//! Redact a PDF (execution plan M8.5–M8.6): mark text and areas with the Redact tool (drag across
//! text to mark it, drag elsewhere to mark a box), mark whole pages, find text or patterns and
//! mark every match, set the default box colour and overlay text, and apply all marks (with a
//! confirmation, as Acrobat asks) or clear them.

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke};
use pdfcraft_engine::redaction_codes::{CodeEdit, CodeLibrary};
use pdfcraft_engine::{Edit, NewAnnotation, REDACT_PATTERNS, REDACTION_CODE_SETS, RedactPattern, Rgb, Shape, Style, rect_quad};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageToolContext, PageXform};
use crate::page_cursor::Layer;
use crate::theme::Tokens;
use crate::{PdfCraftApp, widgets};

const MARK_RED: Color32 = Color32::from_rgb(0xE3, 0x22, 0x22);

/// Redaction Tool Properties lists a set's codes as rows of the dialog up to this many rows (the
/// built-in sets fit); a longer set scrolls in a box [`CODE_LIST_HEIGHT`] high, so the dialog's
/// OK and Cancel stay on screen in a small window.
const MAX_UNSCROLLED_CODE_ROWS: usize = 6;
const CODE_LIST_HEIGHT: f32 = 180.0;

/// One code's checkbox in Redaction Tool Properties: picking it adds it to `picked`, clearing it
/// takes it out. The overlay is drawn in the standard fonts, and Apply refuses text they can't
/// draw, so such a code can't be picked (one picked already can still be cleared) and says why.
fn code_checkbox(ui: &mut egui::Ui, picked: &mut Vec<String>, code: &str, built_in: bool) {
    let mut on = picked.iter().any(|c| c == code);
    let undrawable = pdfcraft_fonts::first_undrawable(code);
    let enabled = undrawable.is_none() || on;
    let check = widgets::checkbox(&mut on, code);
    // Built-in codes are short and line up; custom ones take their width.
    let response = if built_in { ui.add_sized([62.0, 18.0], check) } else { ui.add_enabled(enabled, check) };
    let response = match undrawable {
        Some(c) => response.on_disabled_hover_text(pdfcraft_fonts::undrawable_message("this code", c)),
        None => response,
    };
    if response.changed() {
        picked.retain(|c| c != code);
        if on {
            picked.push(code.to_string());
        }
    }
}

/// The saved code choice kept aside while the saved code sets can't be read
/// ([`PdfCraftApp::redaction_choice_unread`]).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RedactChoiceUnread {
    /// `code_set` and `codes` as the settings held them.
    pub saved: (serde_json::Value, serde_json::Value),
    /// What they were read as (the default set, no codes), which the defaults keep until changed.
    pub fallback: (String, Vec<String>),
}

/// Redaction Tool Properties: what new marks look like once applied.
#[derive(Clone, Debug, PartialEq)]
pub struct RedactPrefs {
    /// Box colour (`None` = no box, the content just disappears).
    pub fill: Option<Rgb>,
    pub use_overlay: bool,
    pub overlay: String,
    /// The overlay text is the picked redaction codes instead of `overlay`.
    pub use_code: bool,
    /// The chosen code set: a built-in set's id or a custom set's name (see [`CodeLibrary::find`]).
    pub code_set: String,
    /// The picked codes of `code_set`, in its order.
    pub codes: Vec<String>,
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
            code_set: REDACTION_CODE_SETS[0].id.to_string(),
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
    /// The overlay text new marks get, with the code sets of `codes` (built-in and custom).
    pub fn overlay_text(&self, codes: &CodeLibrary) -> String {
        match (self.use_overlay, self.use_code) {
            (false, _) => String::new(),
            (true, false) => self.overlay.clone(),
            // `codes` only ever holds codes of `code_set` (see `settle`), so this can't fail.
            (true, true) => codes.find(&self.code_set).and_then(|set| set.overlay(&self.codes).ok()).unwrap_or_default(),
        }
    }

    /// Keep the chosen set and codes in step with an Edit Codes change: a renamed set or code
    /// stays chosen under its new name, and what the change removed is no longer picked.
    pub fn follow(&mut self, edit: &CodeEdit, codes: &CodeLibrary) {
        match edit {
            CodeEdit::RenameSet { from, to } if self.code_set == *from => self.code_set = to.clone(),
            CodeEdit::RenameCode { set, from, to } if self.code_set == *set => {
                for c in &mut self.codes {
                    if c == from {
                        *c = to.clone();
                    }
                }
            }
            _ => {}
        }
        self.settle(codes);
    }

    /// A set that is gone falls back to the first built-in one; only codes the set has stay
    /// picked, each once, in the set's order.
    pub fn settle(&mut self, codes: &CodeLibrary) {
        let Some(set) = codes.find(&self.code_set) else {
            self.code_set = Self::default().code_set;
            self.codes.clear();
            return;
        };
        self.codes = set.codes().into_iter().filter(|c| self.codes.iter().any(|p| p == c)).map(str::to_string).collect();
    }

    pub fn mark(&self, codes: &CodeLibrary, page: usize, quads: Vec<[f64; 8]>, author: &str) -> Edit {
        let shape = Shape::Redact { quads, overlay: self.overlay_text(codes), look: self.look };
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
            "code_set": self.code_set,
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
    /// anything else keeps the default. `None` when nothing was saved. `codes` holds the code
    /// sets the saved choice may name.
    pub fn from_json(v: &serde_json::Value, codes: &CodeLibrary) -> Option<Self> {
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
        if let Some(set) = v["code_set"].as_str().and_then(|key| codes.find(key)) {
            d.code_set = set.key().to_string();
        }
        if let Some(picked) = v["codes"].as_array() {
            d.codes = picked.iter().filter_map(|x| x.as_str()).map(str::to_string).collect();
        }
        // Only codes of the saved set, each once, in the set's order.
        d.settle(codes);
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
        }
    }
}

/// The largest word list file Import list accepts.
pub(crate) const MAX_WORD_LIST_BYTES: usize = 1 << 20;

/// A box being drawn with the Redact tool: (page, start).
pub type AreaDrag = Option<(usize, Pos2)>;

fn to_user(xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> [f64; 2] {
    let (vx, vy) = xf.screen_to_view(p);
    let u = info.pages[page].view_to_user(vx, vy);
    [u[0] as f64, u[1] as f64]
}

/// Redact tool input on a page. Presses on text are left to text selection (the selection is
/// marked when it ends, see [`after_text`]); presses nobody holds (`free`) elsewhere draw a box.
/// Returns `true` when the gesture is a box.
pub(crate) fn page_input(cx: PageToolContext<'_>, tool: crate::QuickTool, over_text: impl Fn(Pos2) -> bool, free: bool, view: &mut DocView) -> bool {
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
                view.pending_redaction = Some((tool, vec![(page, vec![rect_quad([a[0], a[1], b[0], b[1]])])], page));
            }
        }
        return true;
    }
    // The tool's own press, of the primary button: one nobody holds, which began on this page off
    // its text. egui may see the drag only once the pointer is past the page's edge.
    if resp.drag_started_by(egui::PointerButton::Primary)
        && free
        && let Some(origin) = ptr.origin.filter(|o| xf.rect.contains(*o) && !over_text(*o))
    {
        view.redact_drag = Some((page, origin));
        return true;
    }
    if pointer.is_some_and(|p| xf.rect.contains(p) && !over_text(p)) {
        view.cursor.claim(Layer::ToolArea, egui::CursorIcon::Crosshair);
    }
    false
}

/// A finished text selection with the Redact tool becomes a mark.
/// Across pages: a mark on each page it covers, once all are read; refused when one of them could
/// not be read, rather than leave it unmarked.
pub(crate) fn after_text(resp: &egui::Response, page: usize, info: &DocInfo, view: &mut DocView) {
    if view.redact_drag.is_some() {
        return;
    }
    let finished = resp.drag_stopped_by(egui::PointerButton::Primary) || resp.double_clicked();
    match view.tool_marks(crate::QuickTool::Redact, finished, Some(page), true, info) {
        crate::canvas::ToolMarks::Marks(marks, head) => {
            view.clear_selection();
            view.pending_redaction = Some((crate::QuickTool::Redact, marks, head));
        }
        crate::canvas::ToolMarks::Unreadable => view.redaction_refused = true,
        crate::canvas::ToolMarks::None => {}
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
    /// Mark every match of the search draft on every page; returns how many were marked.
    pub fn redact_search(&mut self) -> usize {
        let Some((_, id)) = self.active_ids() else { return 0 };
        let Some(doc) = self.session.get(id) else { return 0 };
        let d = self.redact_search.clone();
        let needles = match d.mode {
            SearchMode::Phrase if !d.text.trim().is_empty() => vec![d.text.clone()],
            SearchMode::Words => pdfcraft_engine::redact_word_list(&d.words),
            SearchMode::Phrase | SearchMode::Patterns => Vec::new(),
        };
        if d.mode != SearchMode::Patterns && needles.is_empty() {
            return 0;
        }
        let config = pdfcraft_render::RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), ..Default::default() };
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
        let mut edits = Vec::new();
        let author = self.comment_prefs.author.clone();
        for (page, page_info) in doc.info.pages.iter().enumerate() {
            let out = r.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
            let Some(text) = out.text else { continue };
            let hits = match d.mode {
                SearchMode::Patterns => text.find_with(|c| pdfcraft_engine::find_pattern(d.pattern, c)),
                _ => needles.iter().flat_map(|w| text.find_opts(w, d.case_sensitive, d.whole_words)).collect(),
            };
            for h in hits {
                let quads: Vec<[f64; 8]> = text.line_rects(h).into_iter().map(|r| page_info.view_rect_to_quad(r)).collect();
                if !quads.is_empty() {
                    edits.push(self.redact_prefs.mark(&self.redaction_codes, page, quads, &author));
                }
            }
        }
        let n = edits.len();
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
                self.redact_prefs.mark(&self.redaction_codes, p, vec![rect_quad([c[0] as f64, c[1] as f64, c[2] as f64, c[3] as f64])], &author)
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
    let to_field = ui
        .horizontal(|ui| {
            let from_label = widgets::radio_value(ui, &mut d.current, false, tl!("Pages from"));
            ui.add_enabled(!d.current, egui::DragValue::new(&mut d.from).range(1..=pages)).labelled_by(from_label.id);
            let to_label = ui.label(tl!("to"));
            let to_field = ui.add_enabled(!d.current, egui::DragValue::new(&mut d.to).range(1..=pages)).labelled_by(to_label.id);
            ui.label(crate::i18n::fmt(tl!("of {name}"), &[("name", &pages.to_string())]));
            to_field
        })
        .inner;
    widgets::keep_range_ordered(d.from, &mut d.to, widgets::typing_into(&to_field));
    ui.add_space(12.0);
    let (ok, cancel) = buttons(ui, "OK", true);
    if ok {
        // OK while the end is still being typed (a screen reader can press it).
        widgets::keep_range_ordered(d.from, &mut d.to, false);
    }
    (ok, cancel)
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

/// Redaction Tool Properties, with the code sets of `codes`. Returns (apply, cancel, Edit Codes).
pub(crate) fn props_body(ui: &mut egui::Ui, d: &mut RedactPrefs, codes: &CodeLibrary, _t: &Tokens) -> (bool, bool, bool) {
    let mut edit_codes = false;
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
                let chosen = codes.find(&d.code_set);
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(codes_on, |ui| {
                        let shown = chosen.map_or(d.code_set.as_str(), |s| s.name());
                        craft_ui::ComboBox::from_id_salt("redaction-code-set").selected_text(shown).show_ui(ui, |ui| {
                            for set in codes.all() {
                                let current = d.code_set == set.key();
                                if ui.selectable_label(current, set.name()).clicked() && !current {
                                    d.code_set = set.key().to_string();
                                    d.codes.clear();
                                }
                            }
                        });
                    });
                    // Acrobat's Edit Codes: the user's own sets; the built-in ones stay as they are.
                    edit_codes = ui.button(tl!("Edit Codes…")).clicked();
                });
                ui.end_row();
                let set_codes = chosen.map(|s| s.codes()).unwrap_or_default();
                let built_in = chosen.is_none_or(|s| s.built_in());
                // Built-in codes are short: four to a row. Custom ones may be longer: two.
                let rows: Vec<&[&str]> = set_codes.chunks(if built_in { 4 } else { 2 }).collect();
                if rows.len() <= MAX_UNSCROLLED_CODE_ROWS {
                    for row in rows {
                        ui.label("");
                        ui.add_enabled_ui(codes_on, |ui| {
                            ui.horizontal(|ui| {
                                for &code in row {
                                    code_checkbox(ui, &mut d.codes, code, built_in);
                                }
                            });
                        });
                        ui.end_row();
                    }
                } else {
                    // A set holds up to 200 codes and the dialog doesn't scroll: a long list
                    // scrolls in a box of its own, so every code can be reached and OK and Cancel
                    // stay in view.
                    ui.label("");
                    egui::ScrollArea::vertical()
                        .id_salt("redaction-code-list")
                        .max_height(CODE_LIST_HEIGHT)
                        // A grid cell offers only the row's last height: the list gets the whole
                        // box, so no code is hidden under the clip.
                        .min_scrolled_height(CODE_LIST_HEIGHT)
                        .show(ui, |ui| {
                            // A grid cell lays out left to right: the rows go one under the other.
                            ui.vertical(|ui| {
                                if !codes_on {
                                    ui.disable();
                                }
                                for row in rows {
                                    ui.horizontal(|ui| {
                                        for &code in row {
                                            code_checkbox(ui, &mut d.codes, code, built_in);
                                        }
                                    });
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
    let (ok, cancel) = buttons(ui, "OK", true);
    (ok, cancel, edit_codes)
}

/// The Edit Codes dialog: the selected set and code, the text being typed for a set name and
/// a code, whether an import replaces sets of the same name, and the last outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct CodeEditor {
    /// The selected set's key (a built-in set's id or a custom set's name).
    pub set: String,
    pub code: Option<String>,
    pub set_name: String,
    pub code_text: String,
    pub replace: bool,
    /// What the last action said, and whether it was a refusal.
    pub message: Option<(String, bool)>,
}

impl Default for CodeEditor {
    fn default() -> Self {
        Self {
            set: REDACTION_CODE_SETS[0].id.to_string(),
            code: None,
            set_name: String::new(),
            code_text: String::new(),
            replace: false,
            message: None,
        }
    }
}

/// What the Edit Codes dialog asked for, acted on after it is drawn.
#[derive(Default)]
pub(crate) struct CodesOutcome {
    pub close: bool,
    pub import: bool,
    pub export: bool,
}

/// Redaction Properties ▸ Edit Codes: the code sets on the left, the selected set's codes on the
/// right. Built-in sets are listed and can be read, never changed.
pub(crate) fn codes_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> CodesOutcome {
    let mut out = CodesOutcome::default();
    let mut edit: Option<CodeEdit> = None;
    ui.set_width(660.0);
    ui.label(egui::RichText::new(tl!("Edit Codes")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    let locked = app.redaction_codes_unread.clone();
    let e = &mut app.code_editor;
    if app.redaction_codes.find(&e.set).is_none() {
        e.set = REDACTION_CODE_SETS[0].id.to_string();
        e.code = None;
    }
    let Some(set) = app.redaction_codes.find(&e.set) else { return out };
    let editable = !set.built_in() && locked.is_none();
    let can_make = locked.is_none();
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(310.0);
            ui.label(egui::RichText::new(tl!("Code sets")).strong());
            egui::ScrollArea::vertical().id_salt("redaction-code-sets").max_height(170.0).auto_shrink([false, true]).show(ui, |ui| {
                for s in app.redaction_codes.all() {
                    let label = if s.built_in() { crate::i18n::fmt(tl!("{name} (built-in)"), &[("name", s.name())]) } else { s.name().to_string() };
                    if ui.selectable_label(s.key() == e.set, label).clicked() && s.key() != e.set {
                        e.set = s.key().to_string();
                        e.code = None;
                        e.set_name = if s.built_in() { String::new() } else { s.name().to_string() };
                        e.code_text.clear();
                    }
                }
            });
            ui.add_space(6.0);
            let l = ui.label(tl!("Set name:"));
            ui.add_enabled(can_make, widgets::line(&mut e.set_name).desired_width(300.0)).labelled_by(l.id);
            let name = e.set_name.trim().to_string();
            ui.horizontal(|ui| {
                if ui.add_enabled(can_make, egui::Button::new(tl!("New set"))).clicked() {
                    edit = Some(CodeEdit::CreateSet { name: name.clone() });
                }
                if ui.add_enabled(editable, egui::Button::new(tl!("Rename set"))).clicked() {
                    edit = Some(CodeEdit::RenameSet { from: set.key().to_string(), to: name.clone() });
                }
                if ui.add_enabled(editable, egui::Button::new(tl!("Delete set"))).clicked() {
                    edit = Some(CodeEdit::DeleteSet { name: set.key().to_string() });
                }
            });
            ui.horizontal(|ui| {
                out.import = ui.add_enabled(can_make, egui::Button::new(tl!("Import…"))).clicked();
                let any = !app.redaction_codes.custom().is_empty();
                out.export = ui.add_enabled(any, egui::Button::new(tl!("Export…"))).clicked();
            });
            ui.add_enabled(can_make, widgets::checkbox(&mut e.replace, tl!("Imported sets replace sets with the same name")));
        });
        ui.add_space(20.0);
        ui.vertical(|ui| {
            ui.set_width(310.0);
            ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Codes in {set}"), &[("set", set.name())])).strong());
            egui::ScrollArea::vertical().id_salt("redaction-codes").max_height(170.0).auto_shrink([false, true]).show(ui, |ui| {
                for code in set.codes() {
                    let current = e.code.as_deref() == Some(code);
                    if ui.selectable_label(current, code).clicked() && !current {
                        e.code = Some(code.to_string());
                        e.code_text = code.to_string();
                    }
                }
            });
            ui.add_space(6.0);
            let l = ui.label(tl!("Code:"));
            ui.add_enabled(editable, widgets::line(&mut e.code_text).desired_width(300.0)).labelled_by(l.id);
            let code = e.code_text.trim().to_string();
            ui.horizontal(|ui| {
                if ui.add_enabled(editable, egui::Button::new(tl!("Add code"))).clicked() {
                    edit = Some(CodeEdit::AddCode { set: set.key().to_string(), code: code.clone() });
                }
                let picked = e.code.clone().filter(|_| editable);
                if ui.add_enabled(picked.is_some(), egui::Button::new(tl!("Rename code"))).clicked()
                    && let Some(from) = picked.clone()
                {
                    edit = Some(CodeEdit::RenameCode { set: set.key().to_string(), from, to: code.clone() });
                }
                if ui.add_enabled(picked.is_some(), egui::Button::new(tl!("Remove code"))).clicked()
                    && let Some(code) = picked
                {
                    edit = Some(CodeEdit::RemoveCode { set: set.key().to_string(), code });
                }
            });
            if set.built_in() {
                ui.label(egui::RichText::new(tl!("Built-in sets can't be changed. Make a new set for your own codes.")).small().color(t.text_muted));
            }
        });
    });
    if let Some((msg, refused)) = &e.message {
        ui.add_space(6.0);
        let color = if *refused { ui.visuals().error_fg_color } else { t.text_muted };
        ui.label(egui::RichText::new(msg.as_str()).color(color));
    }
    if let Some((_, why)) = &locked {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(crate::i18n::fmt(
                tl!("The custom code sets saved in the settings could not be read ({why}). They are kept unchanged until you discard them."),
                &[("why", why)],
            ))
            .color(ui.visuals().error_fg_color),
        );
        if ui.button(tl!("Discard saved code sets")).clicked() {
            app.discard_unread_redaction_codes();
        }
    }
    if let Some(edit) = edit {
        app.edit_redaction_codes(edit);
    }
    ui.add_space(12.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        out.close = widgets::pill_button(ui, tl!("Close"), true).clicked();
    });
    out
}

impl PdfCraftApp {
    /// Open Edit Codes on the set the Redaction Properties chose.
    pub fn open_code_editor(&mut self) {
        let set = self.redact_prefs.code_set.clone();
        let name = self.redaction_codes.find(&set).filter(|s| !s.built_in()).map(|s| s.name().to_string()).unwrap_or_default();
        self.code_editor = CodeEditor { set, set_name: name, ..Default::default() };
    }

    /// Make one Edit Codes change. The properties (this run's, the saved defaults and those
    /// Cancel goes back to) follow it, so a renamed set or code stays chosen. A refusal changes
    /// nothing and says why in the dialog.
    pub fn edit_redaction_codes(&mut self, edit: CodeEdit) -> bool {
        if self.redaction_codes_unread.is_some() {
            self.code_editor.message = Some((tl!("Discard the saved code sets that could not be read first.").to_string(), true));
            return false;
        }
        if let Err(e) = self.redaction_codes.apply(&edit) {
            self.code_editor.message = Some((e.to_string(), true));
            return false;
        }
        for prefs in [&mut self.redact_prefs, &mut self.redact_defaults].into_iter().chain(self.redact_before.as_mut()) {
            prefs.follow(&edit, &self.redaction_codes);
        }
        let e = &mut self.code_editor;
        match &edit {
            CodeEdit::CreateSet { name } => {
                e.set = name.clone();
                e.code = None;
            }
            CodeEdit::RenameSet { from, to } if e.set == *from => e.set = to.clone(),
            CodeEdit::DeleteSet { name } if e.set == *name => {
                e.set = REDACTION_CODE_SETS[0].id.to_string();
                e.set_name.clear();
                e.code = None;
            }
            CodeEdit::AddCode { code, .. } => {
                e.code = Some(code.clone());
                e.code_text.clear();
            }
            CodeEdit::RenameCode { to, .. } => e.code = Some(to.clone()),
            CodeEdit::RemoveCode { .. } => {
                e.code = None;
                e.code_text.clear();
            }
            _ => {}
        }
        e.message = None;
        true
    }

    /// Edit Codes ▸ Import: add the sets of a code set file, all of them or none. `replace` is
    /// the editor's replace choice as it was when Import was clicked.
    pub fn import_redaction_codes(&mut self, name: &str, bytes: &[u8], replace: bool) -> bool {
        if self.redaction_codes_unread.is_some() {
            self.code_editor.message = Some((tl!("Discard the saved code sets that could not be read first.").to_string(), true));
            return false;
        }
        let read = CodeLibrary::read_file(bytes).and_then(|sets| self.redaction_codes.import(sets, replace));
        match read {
            Ok(names) => {
                for prefs in [&mut self.redact_prefs, &mut self.redact_defaults].into_iter().chain(self.redact_before.as_mut()) {
                    // A set replaced under another letter case (the file's spelling wins) stays
                    // chosen under its new name, with the codes it still has.
                    if let Some(name) = names.iter().find(|n| *n != &prefs.code_set && n.to_lowercase() == prefs.code_set.to_lowercase()) {
                        prefs.code_set = name.clone();
                    }
                    prefs.settle(&self.redaction_codes);
                }
                if let Some(first) = names.first() {
                    // As choosing the set by hand does: its name to edit, and no code or code
                    // text left over from the set that was selected.
                    self.code_editor.set = first.clone();
                    self.code_editor.set_name = first.clone();
                    self.code_editor.code = None;
                    self.code_editor.code_text.clear();
                }
                let msg = match names.as_slice() {
                    [one] => crate::i18n::fmt(tl!("Imported the code set {set} from {name}."), &[("set", one), ("name", name)]),
                    _ => crate::i18n::fmt(tl!("Imported {n} code sets from {name}."), &[("n", &names.len().to_string()), ("name", name)]),
                };
                self.code_editor.message = Some((msg, false));
                true
            }
            Err(e) => {
                let msg = crate::i18n::fmt(tl!("{name} was not imported: {e}"), &[("name", name), ("e", &e.to_string())]);
                self.code_editor.message = Some((msg.clone(), true));
                self.notify(msg);
                false
            }
        }
    }

    /// Edit Codes ▸ Export: write every custom set to a code set file.
    pub fn export_redaction_codes(&mut self) {
        let bytes = match self.redaction_codes.export(None) {
            Ok(b) if !self.redaction_codes.custom().is_empty() => b,
            Ok(_) => {
                self.code_editor.message = Some((tl!("There are no custom code sets to export.").to_string(), true));
                return;
            }
            Err(e) => {
                self.code_editor.message = Some((e.to_string(), true));
                return;
            }
        };
        const NAME: &str = "redaction-codes.json";
        #[cfg(not(target_arch = "wasm32"))]
        {
            let write = move |app: &mut PdfCraftApp, path: std::path::PathBuf| {
                let said = match crate::editing::write_atomically(&path.to_string_lossy(), &bytes) {
                    Ok(()) => (tl!("Exported the custom code sets.").to_string(), false),
                    Err(e) => (e.to_string(), true),
                };
                app.code_editor.message = Some(said);
            };
            match self.export_dir_override.as_ref().map(|d| std::path::PathBuf::from(d).join(NAME)) {
                Some(path) => write(self, path),
                None => {
                    use crate::file_filter::AddFilterAnyCase as _;
                    let dialog = rfd::AsyncFileDialog::new().set_file_name(NAME).add_filter_any_case(tl!("Redaction codes"), &["json"]);
                    self.ask_one(crate::pickers::Ask::Save(dialog), None, write);
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        if let Err(e) = crate::editing::download(NAME, &bytes) {
            self.code_editor.message = Some((e, true));
        }
    }

    /// Forget saved code sets that could not be read, so new ones can be made and saved.
    pub fn discard_unread_redaction_codes(&mut self) {
        self.redaction_codes_unread = None;
        // The choice that named one of those sets goes with them.
        self.redaction_choice_unread = None;
        self.redaction_codes = CodeLibrary::default();
        self.code_editor.message = Some((tl!("The saved code sets were discarded.").to_string(), false));
    }
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
    use egui::accesskit::Role;
    use egui_kittest::kittest::Queryable;

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    /// #5574: typing "12" into the end of 5 to 5 came out as 5. The end was raised to the start
    /// after every keystroke, so the half-typed "1" was replaced before the "2" arrived.
    #[test]
    fn typing_the_last_page_keeps_what_is_typed() {
        let mut fonts = false;
        let mut h = egui_kittest::Harness::new_ui_state(
            move |ui, d: &mut PagesDraft| {
                // The app's fonts (the semibold heading) take effect from the next frame.
                if !fonts {
                    ui.ctx().set_fonts(crate::theme::font_definitions());
                    fonts = true;
                    return;
                }
                let t = Tokens::get(ui.ctx());
                pages_body(ui, d, 20, &t);
            },
            PagesDraft { current: false, from: 5, to: 5 },
        );
        h.run();
        for (typed, expected) in [("12", 12), ("3", 5)] {
            h.get_by_role_and_label(Role::SpinButton, "to").click();
            h.run();
            h.event(key(egui::Key::A, egui::Modifiers::COMMAND));
            for c in typed.chars() {
                h.event(egui::Event::Text(c.to_string()));
                h.run_steps(2);
            }
            h.event(key(egui::Key::Enter, egui::Modifiers::NONE));
            h.run_steps(2);
            assert_eq!(*h.state(), PagesDraft { current: false, from: 5, to: expected }, "typed {typed:?}");
        }
    }

    /// The dialog with its OK result kept: (draft, OK pressed).
    fn dialog() -> egui_kittest::Harness<'static, (PagesDraft, bool)> {
        let mut fonts = false;
        let mut h = egui_kittest::Harness::new_ui_state(
            move |ui, (d, ok): &mut (PagesDraft, bool)| {
                if !fonts {
                    ui.ctx().set_fonts(crate::theme::font_definitions());
                    fonts = true;
                    return;
                }
                let t = Tokens::get(ui.ctx());
                *ok |= pages_body(ui, d, 20, &t).0;
            },
            (PagesDraft { current: false, from: 5, to: 5 }, false),
        );
        h.run();
        h
    }

    fn type_last_page(h: &mut egui_kittest::Harness<'static, (PagesDraft, bool)>, typed: &str) {
        h.get_by_role_and_label(Role::SpinButton, "to").click();
        h.run();
        h.event(key(egui::Key::A, egui::Modifiers::COMMAND));
        for c in typed.chars() {
            h.event(egui::Event::Text(c.to_string()));
            h.run_steps(2);
        }
    }

    /// OK pressed while the end is still being typed (a screen reader's click leaves the field
    /// focused) orders the range, so a reversed one never reaches the edit.
    #[test]
    fn ok_while_the_last_page_is_typed_orders_the_range() {
        let mut h = dialog();
        type_last_page(&mut h, "3");
        assert_eq!(h.state().0.to, 3, "still typing");
        h.get_by_label("OK").click_accesskit();
        h.run_steps(2);
        assert_eq!(*h.state(), (PagesDraft { current: false, from: 5, to: 5 }, true));
    }

    /// Switching to another window halfway through "12" leaves the typed "1" alone: the field
    /// still has the keyboard when the window comes back.
    #[test]
    fn switching_windows_mid_number_keeps_what_is_typed() {
        let mut h = dialog();
        type_last_page(&mut h, "1");
        h.input_mut().focused = false;
        h.run_steps(2);
        assert_eq!(h.state().0.to, 1, "the typed 1 stays while the window is in the background");
        h.input_mut().focused = true;
        h.run_steps(2);
        h.event(egui::Event::Text("2".into()));
        h.run_steps(2);
        h.event(key(egui::Key::Enter, egui::Modifiers::NONE));
        h.run_steps(2);
        assert_eq!(h.state().0.to, 12);
    }
}
