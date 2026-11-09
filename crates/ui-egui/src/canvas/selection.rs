use super::{Arc, DocInfo, DocView, Edit, HashMap, PageText, Pos2, Range, RecoveredText, Rect, comments, pos2};

/// A text selection on one page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Selection {
    pub(super) page: usize,
    pub(super) span: Span,
    /// A run on a recovered layer that holds only the strict glyphs `from..=to` (strict-layer
    /// indices) and the recovered glyphs between them: the layer may order strict glyphs
    /// otherwise than the strict layer, so the run between the first and the last of them can
    /// pass over others ([`DocView::select_text`]). `None`: everything in the run.
    pub(super) strict: Option<(usize, usize)>,
    /// The whole page is selected: Select All of the page, four quick clicks, or a drag by pages
    /// made it. When the page's recovered layer arrives, the selection takes the whole layer.
    pub(super) whole_page: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Span {
    /// Reading order, between two glyph indices.
    Run { anchor: usize, head: usize },
    /// Column select (#740): the glyphs inside a rectangle in page view space, drawn from the
    /// corner the drag started at (`rect[0..2]`) to the pointer (`rect[2..4]`).
    Column { rect: [f32; 4] },
}

impl Selection {
    pub(super) fn run(page: usize, anchor: usize, head: usize) -> Self {
        Self { page, span: Span::Run { anchor, head }, strict: None, whole_page: false }
    }

    pub(super) fn column(page: usize, rect: [f32; 4]) -> Self {
        Self { page, span: Span::Column { rect }, strict: None, whole_page: false }
    }

    /// The run quick clicks select, `first` to `last`: four or more clicks select the page, as
    /// Select All of it does ([`Self::whole_page`]).
    pub(super) fn clicked(page: usize, clicks: u32, first: usize, last: usize) -> Self {
        Self { whole_page: clicks >= 4, ..Self::run(page, first, last) }
    }

    /// The run from `anchor` to `head` (indices into the selection layer), as the pointer makes
    /// or extends it: everything in the run, not a strict range ([`Self::strict`]), and no
    /// longer Select All's.
    pub(super) fn set_run(&mut self, anchor: usize, head: usize) {
        self.span = Span::Run { anchor, head };
        self.strict = None;
        self.whole_page = false;
    }

    fn range(anchor: usize, head: usize) -> Range<usize> {
        anchor.min(head)..anchor.max(head).saturating_add(1)
    }

    /// One rectangle per selected piece of a line, in view space (highlighting, markup quads).
    pub(super) fn rects(&self, text: &PageText) -> Vec<[f32; 4]> {
        match self.span {
            Span::Run { anchor, head } => text.line_rects(Self::range(anchor, head)),
            Span::Column { rect } => text.glyph_rects(&text.glyphs_in(rect)),
        }
    }

    /// The selected text, as Copy puts it on the clipboard.
    pub(super) fn text(&self, text: &PageText) -> String {
        match self.span {
            Span::Run { anchor, head } => text.text_of(Self::range(anchor, head)),
            Span::Column { rect } => text.column_text(&text.glyphs_in(rect)),
        }
    }
}

/// Where glyph `strict` of the strict layer sits in its recovered `layer`: past the layer's end
/// for a glyph the strict layer lacks.
fn layer_index(layer: &RecoveredText, strict: usize) -> usize {
    layer.layer_index_of(strict).unwrap_or(layer.text.glyphs.len())
}

/// The selection on `layer` holding exactly the strict glyphs `from..=to` (and the recovered
/// glyphs between them): the run from the first to the last of their places in the layer,
/// limited to them. `None` when none of them is in the layer.
fn exact_selection(page: usize, layer: &RecoveredText, from: usize, to: usize) -> Option<Selection> {
    let (lo, hi) = (from.min(to), from.max(to));
    let places = (lo..=hi).take(layer.text.glyphs.len()).filter_map(|k| layer.layer_index_of(k));
    let (first, last) = places.fold(None, |b: Option<(usize, usize)>, c| Some(b.map_or((c, c), |(lo, hi)| (lo.min(c), hi.max(c)))))?;
    // The run keeps the selection's direction: a backward one stays anchored at its end.
    let (anchor, head) = if from <= to { (first, last) } else { (last, first) };
    Some(Selection { strict: Some((lo, hi)), ..Selection::run(page, anchor, head) })
}

/// The glyphs of `layer` a run from `anchor` to `head` holds, in copy order: all of them, or with
/// `strict` only the strict glyphs in that strict range and the recovered glyphs between.
fn run_glyphs(layer: &RecoveredText, anchor: usize, head: usize, strict: Option<(usize, usize)>) -> Vec<usize> {
    Selection::range(anchor, head)
        .take(layer.text.glyphs.len())
        .filter(|&i| match (layer.strict_index_of(i), strict) {
            (Some(k), Some((from, to))) => (from..=to).contains(&k),
            _ => true,
        })
        .collect()
}

/// The glyphs `clicks` quick clicks select around glyph `a`: two the word, three the line, more
/// the page.
pub(super) fn click_unit(text: &PageText, clicks: u32, a: usize) -> Option<(usize, usize)> {
    match clicks {
        2 => text.word_at(a),
        3 => text.line_at(a),
        _ => text.glyphs.len().checked_sub(1).map(|last| (0, last)),
    }
}

impl DocView {
    /// The text selection as markup quadrilaterals in user space: (page, quads).
    pub(crate) fn selection_quads(&self, info: &DocInfo) -> Option<(usize, Vec<[f64; 8]>)> {
        let s = self.selection?;
        let text = self.selection_layer(s.page)?;
        let page = info.pages.get(s.page)?;
        let quads: Vec<[f64; 8]> = self.selection_rects(&s, &text).into_iter().map(|r| page.view_rect_to_quad(r)).collect();
        (!quads.is_empty()).then_some((s.page, quads))
    }

    /// The view-space rectangles `s` covers on `text`, its page's selection layer (highlighting,
    /// markup quads): one per selected piece of a line, or one per glyph for a run limited to
    /// some strict glyphs ([`Selection::strict`]).
    pub(super) fn selection_rects(&self, s: &Selection, text: &PageText) -> Vec<[f32; 4]> {
        match (s.span, s.strict, self.recovered_layer(s.page)) {
            (Span::Run { anchor, head }, Some(_), Some(layer)) => text.glyph_rects(&run_glyphs(layer, anchor, head, s.strict)),
            _ => s.rects(text),
        }
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection = None;
        self.actions_dismissed = None;
    }

    /// Select text on `page` from glyph `from` to glyph `to` of the page's strict text layer, the
    /// index space of [`Self::has_page_text`], Find and `text_extract` (tests and automation).
    /// On a page with a recovered layer the selection holds exactly those strict glyphs and the
    /// recovered glyphs between them in the layer's order (see [`Selection::strict`]).
    pub fn select_text(&mut self, page: usize, from: usize, to: usize) {
        self.all_text = false;
        let exact = self.recovered_layer(page).map(|layer| exact_selection(page, layer, from, to));
        self.selection = match exact {
            Some(exact) => exact,
            None => Some(Selection::run(page, from, to)),
        };
    }

    /// Select text on `page` from glyph `from` to glyph `to` of the page's selection layer: its
    /// recovered layer where it has one, else its strict layer (tests and automation, to reach
    /// glyphs only the recovered layer reads; [`Self::layer_glyph_screen_pos`] aims at them).
    pub fn select_layer_text(&mut self, page: usize, from: usize, to: usize) {
        self.all_text = false;
        self.selection = Some(Selection::run(page, from, to));
    }

    /// Screen position of the centre of glyph `glyph` of `page`'s strict text layer (the index
    /// space of [`Self::select_text`]), if that page is on screen and its text layer is loaded
    /// (used by UI tests and automation to aim pointer input).
    pub fn glyph_screen_pos(&self, page: usize, glyph: usize) -> Option<Pos2> {
        self.glyph_screen_rect(page, glyph).map(|r| r.center())
    }

    /// The screen rectangle of glyph `glyph` on `page`, when the page is on screen.
    pub fn glyph_screen_rect(&self, page: usize, glyph: usize) -> Option<Rect> {
        let (_, xf) = self.screen_xforms.iter().find(|(p, _)| *p == page)?;
        let g = self.texts.get(&page)?.glyphs.get(glyph)?;
        Some(xf.view_rect(g.rect))
    }

    /// [`Self::glyph_screen_pos`] for glyph `glyph` of `page`'s selection layer (the index space
    /// of [`Self::select_layer_text`]), which also reaches glyphs only a recovered layer reads.
    pub fn layer_glyph_screen_pos(&self, page: usize, glyph: usize) -> Option<Pos2> {
        let (_, xf) = self.screen_xforms.iter().find(|(p, _)| *p == page)?;
        let rect = self.selection_layer(page)?.glyphs.get(glyph)?.rect;
        Some(xf.view_rect(rect).center())
    }

    /// The text layer selection works on for `page`: Copy with formatting's layer where a
    /// recovery rule read some glyph (so text only it has can be selected), else the strict one.
    /// Text markup and redaction made from a text selection (Highlight, Underline, Redact…)
    /// cover what it selects there, recovered glyphs included. Find, search, plain Copy, the
    /// caret's placement and the area tools' test for text under the pointer read the strict
    /// layer (`texts`).
    pub(crate) fn selection_layer(&self, page: usize) -> Option<Arc<PageText>> {
        match self.recovered_layer(page) {
            Some(layer) => Some(layer.text.clone()),
            None => self.texts.get(&page).cloned(),
        }
    }

    /// `page`'s recovered layer, when its strict text is loaded and a recovery rule read some
    /// glyph of it.
    pub(crate) fn recovered_layer(&self, page: usize) -> Option<&Arc<RecoveredText>> {
        self.texts.get(&page)?;
        self.recovered.get(&page)?.as_ref()
    }

    /// A run selected on `page`'s strict layer, moved onto the recovered `layer` that just
    /// arrived for the page: it keeps exactly its strict glyphs (and the recovered ones between
    /// them), and a run over the whole strict page (Select All) takes the whole layer, recovered
    /// glyphs at its ends included.
    pub(super) fn move_selection_onto(&mut self, page: usize, layer: &RecoveredText) {
        let Some(Selection { page: p, span: Span::Run { anchor, head }, whole_page: whole, .. }) = self.selection else { return };
        if p != page {
            return;
        }
        self.selection = match layer.text.glyphs.len().checked_sub(1) {
            Some(last) if whole => Some(Selection { whole_page: true, ..Selection::run(page, 0, last) }),
            _ => exact_selection(page, layer, anchor, head).or(Some(Selection::run(page, layer_index(layer, anchor), layer_index(layer, head)))),
        };
    }

    /// Selected text, if any (⌘C).
    /// The selected text: of the pages read so far, after Select All of the document (Copy reads
    /// the others, [`DocView::pages_without_text`]).
    pub fn selected_text(&self) -> Option<String> {
        if self.all_text {
            return Some(self.whole_text(&HashMap::new())).filter(|x| !x.is_empty());
        }
        let s = self.selection?;
        let t = self.texts.get(&s.page)?;
        let text = match (self.recovered_layer(s.page), s.span) {
            // A run on the recovered layer copies the strict text of exactly the strict glyphs in
            // it: laid out together with the recovered glyphs, the layer may order them otherwise
            // than the strict layer does, so they need not be one strict range.
            (Some(layer), Span::Run { anchor, head }) => t.text_of_glyphs(&layer.strict_glyphs(&run_glyphs(layer, anchor, head, s.strict))),
            // A column takes the strict glyphs inside its rectangle, whichever layer it was drawn on.
            _ => s.text(t),
        };
        Some(text).filter(|x| !x.is_empty())
    }

    /// Whether Copy has something to copy from the text selection, without building the text:
    /// strict text, or, for Copy with formatting (`with_formatting`: a clipboard that takes
    /// HTML), glyphs only the recovered layer reads. Plain Copy is strict only.
    pub fn selection_copies(&self, with_formatting: bool) -> bool {
        self.selection_has_strict_text() || (with_formatting && self.selection_has_recovered())
    }

    /// Whether the selection holds strict text (what plain Copy copies).
    fn selection_has_strict_text(&self) -> bool {
        if self.all_text {
            return self.texts.values().any(|t| t.glyphs.iter().any(|g| !g.text.is_empty()));
        }
        let Some(s) = self.selection else { return false };
        let Some(t) = self.texts.get(&s.page) else { return false };
        let nonempty = |i: usize| t.glyphs.get(i).is_some_and(|g| !g.text.is_empty());
        match (self.recovered_layer(s.page), s.span) {
            (Some(layer), Span::Run { anchor, head }) => {
                run_glyphs(layer, anchor, head, s.strict).into_iter().any(|i| layer.strict_index_of(i).is_some_and(nonempty))
            }
            (None, Span::Run { anchor, head }) => Selection::range(anchor, head).take(t.glyphs.len()).any(nonempty),
            (_, Span::Column { rect }) => t.glyphs_in(rect).into_iter().any(nonempty),
        }
    }

    /// Whether the selection holds a glyph only the recovered layer reads.
    pub(crate) fn selection_has_recovered(&self) -> bool {
        self.selected_layer_glyphs().is_some_and(|(_, layer, glyphs)| layer.any_recovered(&glyphs))
    }

    /// The selection as glyphs of its page's recovered layer: a run's in copy order, a column's
    /// inside its rectangle. `None` where the page has no recovered layer and after Select All
    /// of the document.
    pub(super) fn selected_layer_glyphs(&self) -> Option<(Selection, Arc<RecoveredText>, Vec<usize>)> {
        if self.all_text {
            return None;
        }
        let s = self.selection?;
        let layer = self.recovered_layer(s.page)?;
        let glyphs = match s.span {
            Span::Run { anchor, head } => run_glyphs(layer, anchor, head, s.strict),
            Span::Column { rect } => layer.text.glyphs_in(rect),
        };
        Some((s, layer.clone(), glyphs))
    }

    /// The selection as glyphs of its page's recovered layer (a run's in copy order), when it
    /// holds a glyph only that layer reads, for Copy with formatting, and whether it is a run
    /// (a column copies plain text only). `None` where the page has no recovered layer, after
    /// Select All of the document, and when nothing recovered is selected.
    pub(crate) fn selected_recovered(&self) -> Option<(usize, Arc<RecoveredText>, Vec<usize>, bool)> {
        let (s, layer, glyphs) = self.selected_layer_glyphs()?;
        layer.any_recovered(&glyphs).then_some((s.page, layer, glyphs, matches!(s.span, Span::Run { .. })))
    }

    /// The selection's page, its strict text layer and the selected strict glyphs in copy order,
    /// for Copy's HTML (on a page with a recovered layer, the strict glyphs the selection holds
    /// there). `None` after Select All of the document and for a column selection, which copy
    /// plain text (a column's rows and tab-separated cells are not paragraphs).
    pub(crate) fn selected_glyphs(&self) -> Option<(usize, Arc<PageText>, Vec<usize>)> {
        if self.all_text {
            return None;
        }
        let s = self.selection?;
        let t = self.texts.get(&s.page)?;
        let glyphs = match (s.span, self.recovered_layer(s.page)) {
            (Span::Run { anchor, head }, Some(layer)) => layer.strict_glyphs(&run_glyphs(layer, anchor, head, s.strict)),
            (Span::Run { anchor, head }, None) => Selection::range(anchor, head).take(t.glyphs.len()).collect(),
            (Span::Column { .. }, _) => return None,
        };
        Some((s.page, t.clone(), glyphs))
    }

    /// Select every page without navigating; shared by Organize and the Pages sidebar.
    pub fn select_all_pages(&mut self) -> bool {
        self.selected = (0..self.page_count).collect();
        self.select_anchor = (self.page_count > 0).then_some(self.current);
        !self.selected.is_empty()
    }

    /// Select all words on the current page, or every page in Organize.
    pub fn select_all(&mut self) -> bool {
        if self.organize {
            return self.select_all_pages();
        }
        // Scrolling continuously, Select All takes the whole document's text (#2639); one page at
        // a time, the page shown.
        // Like the page's, true once some text is selected: the pages whose text isn't read yet are
        // read when the selection is copied.
        if self.continuous && self.page_count > 1 {
            self.selection = None;
            self.all_text = true;
            return self.selected_text().is_some();
        }
        self.all_text = false;
        let page = self.current;
        let Some(t) = self.selection_layer(page) else { return false };
        if t.glyphs.is_empty() {
            return false;
        }
        self.selection = Some(Selection { whole_page: true, ..Selection::run(page, 0, t.glyphs.len() - 1) });
        true
    }

    /// Moves the end of the run selected on `page` to glyph `head` of its selection layer (a
    /// drag, Shift+click): `false` without a run there.
    pub(super) fn extend_selection(&mut self, page: usize, head: usize) -> bool {
        match self.selection.as_mut() {
            Some(s) if s.page == page => match s.span {
                Span::Run { anchor, .. } => {
                    s.set_run(anchor, head);
                    true
                }
                Span::Column { .. } => false,
            },
            _ => false,
        }
    }

    /// Pages whose text [`DocView::whole_text`] still needs: not read yet, nor failed.
    pub(crate) fn pages_without_text(&self) -> Vec<usize> {
        (0..self.page_count).filter(|p| !self.texts.contains_key(p) && !self.text_failed.contains(p)).collect()
    }

    /// The whole document's text in page order, one page after another on new lines, from the
    /// pages' text read so far and `more` (pages read for this, keyed by page).
    pub(crate) fn whole_text(&self, more: &HashMap<usize, Arc<PageText>>) -> String {
        let mut out = String::new();
        for p in 0..self.page_count {
            let Some(t) = self.texts.get(&p).or_else(|| more.get(&p)) else { continue };
            let text = t.text_of(0..t.glyphs.len());
            if text.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&text);
        }
        out
    }
}

#[derive(Clone, Copy)]
enum SelectionAction {
    Note,
    Markup(comments::CommentTool),
    Replace,
    Copy,
}

/// The selection bar's width before it has been laid out once (English labels).
const SELECTION_BAR_GUESS: f32 = 520.0;

/// Actions on an existing text selection, next to the selected words.
pub(super) fn selection_actions(
    ui: &mut egui::Ui,
    viewport: Rect,
    view: &mut DocView,
    info: &DocInfo,
    prefs: &comments::CommentPrefs,
    can_comment: bool,
    can_copy: bool,
) -> bool {
    if ui.input(|input| input.pointer.is_decidedly_dragging()) {
        return false;
    }
    let Some(selection) = view.selection else {
        view.actions_dismissed = None;
        return false;
    };
    if view.actions_dismissed == Some(selection) {
        return false;
    }
    let Some(text) = view.selection_layer(selection.page) else { return false };
    let Some(xf) = view.page_xform(selection.page) else { return false };
    let Some(last) = view.selection_rects(&selection, &text).last().copied() else { return false };
    let selected = selection.text(&text);
    if selected.is_empty() {
        return false;
    }
    let rect = xf.view_rect(last);
    if !viewport.intersects(rect) {
        return false;
    }
    // Copy here is Copy (edit.copy): strict text only, so a selection of recovered glyphs alone
    // has nothing for it.
    let copies = can_copy && view.selection_copies(false);
    let id = egui::Id::unique(("text-selection-actions", view.view_key()));
    // The bar stays inside the pages' area, by the width it had last frame (its labels make it
    // wider in some languages), so it never covers a side panel; a first guess before it has one.
    let width = egui::AreaState::load(ui.ctx(), id).and_then(|a| a.size).map_or(SELECTION_BAR_GUESS, |s| s.x);
    let x = rect.left().clamp(viewport.left() + 8.0, (viewport.right() - width - 8.0).max(viewport.left() + 8.0));
    let y = if rect.bottom() + 48.0 < viewport.bottom() { rect.bottom() + 6.0 } else { (rect.top() - 44.0).max(viewport.top() + 6.0) };
    let mut chosen = None;
    egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(pos2(x, y)).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                for (label, action) in [
                    (tl!("Add comment"), SelectionAction::Note),
                    (tl!("Highlight"), SelectionAction::Markup(comments::CommentTool::Highlight)),
                    (tl!("Underline"), SelectionAction::Markup(comments::CommentTool::Underline)),
                    (tl!("Strikethrough"), SelectionAction::Markup(comments::CommentTool::StrikeOut)),
                    (tl!("Replace text"), SelectionAction::Replace),
                ] {
                    if ui.add_enabled(can_comment, egui::Button::new(label)).clicked() {
                        chosen = Some(action);
                    }
                }
                if ui.add_enabled(copies, egui::Button::new(tl!("Copy"))).clicked() {
                    chosen = Some(SelectionAction::Copy);
                }
            });
        });
    });
    let Some(action) = chosen else { return false };
    if let SelectionAction::Copy = action {
        // Copying closes the bar but keeps the words selected, for ⌘C or a second action.
        view.actions_dismissed = Some(selection);
        return true;
    }
    let Some((page, quads)) = view.selection_quads(info) else { return false };
    let last = quads.last().copied().unwrap_or_default();
    let at = [last[2].max(last[6]), last[1].max(last[3])];
    view.clear_selection();
    match action {
        SelectionAction::Note | SelectionAction::Replace => {
            if matches!(action, SelectionAction::Replace) {
                view.comments.replace = Some(quads);
            }
            view.comments.composer = Some(comments::Composer {
                page,
                at,
                kind: if matches!(action, SelectionAction::Replace) { comments::ComposerKind::Replace } else { comments::ComposerKind::Note },
                text: String::new(),
                focus: true,
            });
        }
        SelectionAction::Markup(tool) => {
            // Every markup button carries a markup tool; anything else adds nothing.
            let Some(kind) = tool.markup() else { return false };
            view.pending_edit = Some(Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                page,
                shape: pdfcraft_engine::Shape::TextMarkup { kind, quads },
                style: prefs.style(tool),
                contents: String::new(),
                author: prefs.author.clone(),
            }));
        }
        // Copy is dispatched through the app after the view borrow ends.
        SelectionAction::Copy => {}
    }
    false
}
