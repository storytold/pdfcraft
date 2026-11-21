use super::{Arc, DocInfo, DocView, Edit, HashMap, PageText, Pos2, Range, RecoveredText, Rect, comments, pos2};
use crate::QuickTool;
use std::ops::RangeInclusive;

/// A text selection. `page` is the page it started on: a column selection stays there, a
/// reading-order one may run on to other pages.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Selection {
    pub(super) page: usize,
    pub(super) span: Span,
    /// A run on one page's recovered layer that holds only the strict glyphs `from..=to`
    /// (strict-layer indices) and the recovered glyphs between them: the layer may order strict
    /// glyphs otherwise than the strict layer, so the run between the first and the last of them
    /// can pass over others ([`DocView::select_text`]). `None`: everything in the run. Extending
    /// the run, on its page or onto another, drops it.
    pub(super) strict: Option<(usize, usize)>,
    /// The whole page is selected: Select All of the page, four quick clicks, or a drag by pages
    /// on that page made it. When the page's recovered layer arrives, the selection takes the
    /// whole layer.
    pub(super) whole_page: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Span {
    /// Reading order, from glyph `anchor` on the selection's page to glyph `head` on `head_page`
    /// (either may come first), each an index into its page's selection layer
    /// ([`DocView::selection_layer`]). Across pages it takes the rest of the first page, every page
    /// between whole, and the last page up to its end, as Acrobat does. `head_x` is where a
    /// drag's pointer ended (page view space), which may lie well past the head glyph, as in the
    /// empty end of a table row; `None` when the head came from a click or a command, and when
    /// the selection runs across pages (table rows are one page).
    Run { anchor: usize, head: usize, head_page: usize, head_x: Option<f32> },
    /// Column select (#740): the glyphs inside a rectangle in page view space, drawn from the
    /// corner the drag started at (`rect[0..2]`) to the pointer (`rect[2..4]`).
    Column { rect: [f32; 4] },
}

impl Selection {
    pub(super) fn run(page: usize, anchor: usize, head: usize) -> Self {
        Self::new(page, Span::Run { anchor, head, head_page: page, head_x: None })
    }

    /// A selection of `span` started on `page`: everything in it, not a strict range
    /// ([`Self::strict`]) nor a whole page.
    pub(super) fn new(page: usize, span: Span) -> Self {
        Self { page, span, strict: None, whole_page: false }
    }

    pub(super) fn column(page: usize, rect: [f32; 4]) -> Self {
        Self::new(page, Span::Column { rect })
    }

    /// The run quick clicks select, `first` to `last`: four or more clicks select the page, as
    /// Select All of it does ([`Self::whole_page`]).
    pub(super) fn clicked(page: usize, clicks: u32, first: usize, last: usize) -> Self {
        Self { whole_page: clicks >= 4, ..Self::run(page, first, last) }
    }

    /// The run on the selection's own page from `anchor` to `head` (indices into its selection
    /// layer), the pointer's end at `head_x` ([`Span::Run`]): everything in the run, not a strict
    /// range ([`Self::strict`]), and no longer Select All's.
    #[cfg(test)]
    pub(super) fn set_run(&mut self, anchor: usize, head: usize, head_x: Option<f32>) {
        self.span = Span::Run { anchor, head, head_page: self.page, head_x };
        self.strict = None;
        self.whole_page = false;
    }

    fn range(anchor: usize, head: usize) -> Range<usize> {
        anchor.min(head)..anchor.max(head).saturating_add(1)
    }

    /// What a drag over table rows selects by rows, not reading order's detour down each column.
    /// Only a same-page drag (also one after a double- or triple-click) or a ⇧-click has a
    /// pointer end; a clicked word, line or page, Select all, a command's selection, and a
    /// selection across pages keep reading order.
    fn by_rows(text: &PageText, anchor: usize, head: usize, head_x: Option<f32>) -> Option<Vec<usize>> {
        head_x.and_then(|x| text.visual_between(anchor, head, Some(x)))
    }

    /// The pages the selection covers, first to last.
    pub(super) fn pages(&self) -> RangeInclusive<usize> {
        match self.span {
            Span::Run { head_page, .. } => self.page.min(head_page)..=self.page.max(head_page),
            Span::Column { .. } => self.page..=self.page,
        }
    }

    /// The page the selection ends on, where its drag finished: a reading-order selection's head
    /// page, a column selection's own page.
    pub(super) fn head_page(&self) -> usize {
        match self.span {
            Span::Run { head_page, .. } => head_page,
            Span::Column { .. } => self.page,
        }
    }

    /// Whether the selection runs over more than one page.
    pub(super) fn spans_pages(&self) -> bool {
        self.pages().start() != self.pages().end()
    }

    /// The reading-order glyphs selected on `page`, which has `len` glyphs; `None` off the
    /// selection's pages and for a column selection.
    pub(super) fn run_on(&self, page: usize, len: usize) -> Option<Range<usize>> {
        self.text_span()?.on(page, len)
    }

    /// A reading-order selection's ends in reading order; `None` for a column selection.
    pub(super) fn text_span(&self) -> Option<TextSpan> {
        let Span::Run { anchor, head, head_page, .. } = self.span else { return None };
        // (page, glyph) pairs compare in reading order.
        let (start, end) = if (self.page, anchor) <= (head_page, head) {
            ((self.page, anchor), (head_page, head))
        } else {
            ((head_page, head), (self.page, anchor))
        };
        Some(TextSpan { start, end })
    }

    /// One rectangle per selected piece of a line on `page`, in view space (highlighting,
    /// markup quads).
    pub(super) fn rects_on(&self, page: usize, text: &PageText) -> Vec<[f32; 4]> {
        match self.span {
            Span::Run { anchor, head, head_page, head_x } if page == self.page && head_page == self.page => {
                match Self::by_rows(text, anchor, head, head_x) {
                    Some(row) => text.glyph_rects(&row),
                    None => self.run_on(page, text.glyphs.len()).map(|r| text.line_rects(r)).unwrap_or_default(),
                }
            }
            Span::Run { .. } => self.run_on(page, text.glyphs.len()).map(|r| text.line_rects(r)).unwrap_or_default(),
            Span::Column { rect } if page == self.page => text.glyph_rects(&text.glyphs_in(rect)),
            Span::Column { .. } => Vec::new(),
        }
    }

    /// The text selected on `page`, as Copy puts it on the clipboard.
    pub(super) fn text_on(&self, page: usize, text: &PageText) -> String {
        match self.span {
            Span::Run { anchor, head, head_page, head_x } if page == self.page && head_page == self.page => {
                match Self::by_rows(text, anchor, head, head_x) {
                    Some(row) => text.column_text(&row),
                    None => self.run_on(page, text.glyphs.len()).map(|r| text.text_of(r)).unwrap_or_default(),
                }
            }
            Span::Run { .. } => self.run_on(page, text.glyphs.len()).map(|r| text.text_of(r)).unwrap_or_default(),
            Span::Column { rect } if page == self.page => text.column_text(&text.glyphs_in(rect)),
            Span::Column { .. } => String::new(),
        }
    }

    /// Extend a reading-order selection to glyph `head` on `page`. `head_x` is the pointer's
    /// page-view x, used to select table rows on the same page; it is dropped when the head
    /// moves to another page.
    /// The run then holds everything between its ends, not a strict range ([`Self::strict`]),
    /// and is no longer Select All's.
    pub(super) fn extend_to_at(&mut self, page: usize, glyph: usize, head_x: Option<f32>) {
        if let Span::Run { head, head_page, head_x: x, .. } = &mut self.span {
            let same_page = page == self.page;
            (*head, *head_page) = (glyph, page);
            *x = head_x.filter(|_| same_page);
            self.strict = None;
            self.whole_page = false;
        }
    }

    /// Whether the selection is a reading-order run on one page that a drag ending at its
    /// `head_x` made select a table's rows on `text`, that page's selection layer.
    fn rows_on(&self, text: &PageText) -> bool {
        match self.span {
            Span::Run { anchor, head, head_page, head_x } if head_page == self.page => Self::by_rows(text, anchor, head, head_x).is_some(),
            Span::Run { .. } | Span::Column { .. } => false,
        }
    }
}

/// What [`DocView::tool_marks`] gives a marking tool.
#[derive(Debug, PartialEq)]
pub(crate) enum ToolMarks {
    /// Nothing to mark now (perhaps later, once the selection's pages are read).
    None,
    /// (page, quads) for each page the selection covers: mark them all, in one undo step. Then
    /// the page the selection ends on, where the drag finished ([`DocView::selection_marks_edit`]).
    Marks(Vec<(usize, Vec<[f64; 8]>)>, usize),
    /// The selection covers a page whose text could not be read: not marked, say why.
    Unreadable,
}

/// Reading-order text from glyph `start.1` on page `start.0` to glyph `end.1` on page `end.0`,
/// both included, `start` first: what a reading-order selection covers, which a copy reads off
/// the UI thread too ([`crate::DocumentCopyJob::of_selection`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TextSpan {
    pub(crate) start: (usize, usize),
    pub(crate) end: (usize, usize),
}

impl TextSpan {
    /// The glyphs covered on `page`, which has `len` glyphs: the rest of the first page, every
    /// page between whole, the last page up to its end glyph. `None` off its pages.
    pub(crate) fn on(&self, page: usize, len: usize) -> Option<Range<usize>> {
        if page < self.start.0 || page > self.end.0 {
            return None;
        }
        let from = if page == self.start.0 { self.start.1 } else { 0 };
        let to = if page == self.end.0 { self.end.1.saturating_add(1) } else { len };
        Some(from..to.max(from))
    }
}

/// A drag pressed on `page` starts a text selection there: from the glyph nearest `origin`, or
/// a column rectangle from `origin` to `to` (page view space). After a multi-click the word or
/// line the clicks selected stays, for the drag to extend by that unit. `clicks` is the press's
/// effective click count (`selection_click_count`): past a double click, tools other than Select
/// start over from one.
pub(super) fn start_selection_drag(view: &mut DocView, page: usize, text: &PageText, clicks: u32, column: bool, origin: (f32, f32), to: (f32, f32)) {
    // A drag replaces the selection without passing through none, so a re-drag over text whose
    // bar Copy closed would keep the bar hidden.
    view.actions_dismissed = None;
    view.selection_drag = true;
    if clicks < 2 {
        let (ox, oy) = origin;
        view.selection =
            if column { Some(Selection::column(page, [ox, oy, to.0, to.1])) } else { text.nearest(ox, oy).map(|a| Selection::run(page, a, a)) };
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

/// The glyphs of `layer` a run from `anchor` to `head` holds, in copy order: a table's rows when a
/// drag ending at `head_x` crosses them ([`Selection::by_rows`]), else all of the run, or with
/// `strict` only the strict glyphs in that strict range and the recovered glyphs between.
fn run_glyphs(layer: &RecoveredText, anchor: usize, head: usize, head_x: Option<f32>, strict: Option<(usize, usize)>) -> Vec<usize> {
    if let Some(rows) = Selection::by_rows(&layer.text, anchor, head, head_x) {
        return rows;
    }
    Selection::range(anchor, head)
        .take(layer.text.glyphs.len())
        .filter(|&i| match (layer.strict_index_of(i), strict) {
            (Some(k), Some((from, to))) => (from..=to).contains(&k),
            _ => true,
        })
        .collect()
}

/// The glyphs of `page`'s recovered `layer` the reading-order selection `s` holds, in copy order:
/// on its one page as [`run_glyphs`] takes them, across pages the part of the run on `page`
/// ([`Selection::run_on`]). `None` for a column selection.
fn layer_run_glyphs(s: &Selection, page: usize, layer: &RecoveredText) -> Option<Vec<usize>> {
    match s.span {
        Span::Run { anchor, head, head_page, head_x } if page == s.page && head_page == s.page => {
            Some(run_glyphs(layer, anchor, head, head_x, s.strict))
        }
        Span::Run { .. } => Some(s.run_on(page, layer.text.glyphs.len()).map(Iterator::collect).unwrap_or_default()),
        Span::Column { .. } => None,
    }
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
    /// The text layers of the selection's pages, first to last: `None` while one of them is still
    /// to be read, so a selection is never copied or marked short. A page whose text could not be
    /// read has nothing selected and is left out.
    fn selection_texts(&self) -> Option<Vec<(usize, &Arc<PageText>)>> {
        let mut texts = Vec::new();
        for p in self.selection?.pages() {
            match self.texts.get(&p) {
                Some(t) => texts.push((p, t)),
                None if self.text_failed.contains(&p) => {}
                None => return None,
            }
        }
        Some(texts)
    }

    /// [`Self::selection_texts`] as the layers selection works on ([`Self::selection_layer`]):
    /// each page's recovered layer where it has one.
    fn selection_layers(&self) -> Option<Vec<(usize, Arc<PageText>)>> {
        let texts = self.selection_texts()?;
        Some(texts.into_iter().map(|(p, t)| (p, self.recovered_layer(p).map_or_else(|| t.clone(), |layer| layer.text.clone()))).collect())
    }

    /// The text selection as markup quadrilaterals in user space, one entry per page with
    /// selected text: (page, quads). Empty when there is nothing to mark yet. They cover what it
    /// selects on each page's selection layer, recovered glyphs included.
    pub(crate) fn selection_quads(&self, info: &DocInfo) -> Vec<(usize, Vec<[f64; 8]>)> {
        let (Some(s), Some(layers)) = (self.selection, self.selection_layers()) else { return Vec::new() };
        layers
            .into_iter()
            .filter_map(|(p, text)| {
                let page = info.pages.get(p)?;
                let quads: Vec<[f64; 8]> = self.selection_rects_on(&s, p, &text).into_iter().map(|r| page.view_rect_to_quad(r)).collect();
                (!quads.is_empty()).then_some((p, quads))
            })
            .collect()
    }

    /// A reading-order selection whose pages are not all read yet: Copy reads them off the UI
    /// thread ([`crate::DocumentCopyJob::of_selection`]) rather than copying short or nothing.
    pub(crate) fn selection_waits_for_text(&self) -> Option<TextSpan> {
        self.selection.filter(|_| self.selection_texts().is_none())?.text_span()
    }

    /// Whether the selection covers a page whose text could not be read. Nothing there can be
    /// selected, so a redaction of the rest would leave that page's content in place: Redact
    /// refuses such a selection rather than mark it short.
    pub(crate) fn selection_has_unreadable_page(&self) -> bool {
        self.selection.is_some_and(|s| s.pages().any(|p| self.text_failed.contains(&p)))
    }

    /// What a marking tool (a markup tool, Redact) marks for the text selection a drag, a
    /// multi-click or picking the tool just finished (`finished`): (page, quads) for every page
    /// the selection covers, as [`Self::selection_quads`], returned once: from one of its pages
    /// (`Some(page)`, called for each page drawn) or from anywhere (`None`). While a page it
    /// covers is still to be read, or none of its pages is drawn, nothing is marked yet: the
    /// selection is remembered with the tool and marked on a later frame, as long as neither has
    /// changed meanwhile. With `whole`, a selection over a page whose text could not be read is
    /// refused instead ([`Self::selection_has_unreadable_page`]).
    pub(crate) fn tool_marks(&mut self, tool: QuickTool, finished: bool, page: Option<usize>, whole: bool, info: &DocInfo) -> ToolMarks {
        let Some(sel) = self.selection else {
            self.marks_waiting = None;
            return ToolMarks::None;
        };
        if finished {
            self.marks_waiting = Some((tool, sel));
        }
        if self.marks_waiting != Some((tool, sel)) {
            // The selection or the tool changed: the wait is over, so a drag passing back through
            // the selection it was for doesn't mark it before it is released.
            self.marks_waiting = None;
            return ToolMarks::None;
        }
        if whole && self.selection_has_unreadable_page() {
            self.marks_waiting = None;
            return ToolMarks::Unreadable;
        }
        if self.selection_texts().is_none() {
            return ToolMarks::None;
        }
        let marks = self.selection_quads(info);
        if marks.is_empty() {
            // Every page is read and nothing is selected: nothing to wait for.
            self.marks_waiting = None;
            return ToolMarks::None;
        }
        if page.is_some_and(|page| !marks.iter().any(|(p, _)| *p == page)) {
            return ToolMarks::None;
        }
        self.marks_waiting = None;
        ToolMarks::Marks(marks, sel.head_page())
    }

    /// The edit adding what a tool marks on a text selection, one edit per page, as one undo
    /// step ([`one_undo_step`]). Across pages it is marked as such for `apply_edit`
    /// ([`Self::marks_across_pages`]), with `head`, the page the selection ends on, which it
    /// follows up as one markup: other batches of comments (Redact's page range or search) are
    /// not followed up.
    pub(crate) fn selection_marks_edit(&mut self, edits: Vec<Edit>, head: usize) -> Option<Edit> {
        let edit = one_undo_step(edits);
        self.marks_across_pages = matches!(edit, Some(Edit::Batch { .. })).then_some(head);
        edit
    }

    /// The view-space rectangles `s` covers on `page`, whose selection layer is `text`
    /// (highlighting, markup quads): one per selected piece of a line, or one per glyph for a
    /// run limited to some strict glyphs ([`Selection::strict`]).
    pub(super) fn selection_rects_on(&self, s: &Selection, page: usize, text: &PageText) -> Vec<[f32; 4]> {
        match (s.span, s.strict, self.recovered_layer(page)) {
            (Span::Run { anchor, head, head_page, head_x }, Some(_), Some(layer)) if page == s.page && head_page == s.page => {
                text.glyph_rects(&run_glyphs(layer, anchor, head, head_x, s.strict))
            }
            _ => s.rects_on(page, text),
        }
    }

    /// The strict text `s` selects on `page`, whose strict text layer is `t`, as Copy puts it on
    /// the clipboard.
    fn selected_text_on(&self, s: &Selection, page: usize, t: &PageText) -> String {
        // A run on the recovered layer copies the strict text of exactly the strict glyphs in it:
        // laid out together with the recovered glyphs, the layer may order them otherwise than
        // the strict layer does, so they need not be one strict range.
        if let Some(layer) = self.recovered_layer(page)
            && let Some(glyphs) = layer_run_glyphs(s, page, layer)
        {
            let strict = layer.strict_glyphs(&glyphs);
            // A table's rows, like a column, are tab-separated cells.
            return if s.rows_on(&layer.text) { t.column_text(&strict) } else { t.text_of_glyphs(&strict) };
        }
        // A column takes the strict glyphs inside its rectangle, whichever layer it was drawn on.
        s.text_on(page, t)
    }

    /// The text a reading-order selection holds on each page whose selection is on its recovered
    /// layer, for a copy read off the UI thread, which has only the strict layers and would read
    /// the layer's indices wrongly there ([`crate::DocumentCopyJob::of_selection`]).
    pub(crate) fn selected_layer_texts(&self) -> Vec<(usize, String)> {
        let Some(s) = self.selection else { return Vec::new() };
        s.pages()
            .filter_map(|p| {
                self.recovered_layer(p)?;
                let t = self.texts.get(&p)?;
                Some((p, self.selected_text_on(&s, p, t)))
            })
            .collect()
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection = None;
        self.actions_dismissed = None;
        self.marks_waiting = None;
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
        let Some(sel @ Selection { page: p, span: Span::Run { anchor, head, head_page, head_x }, whole_page: whole, .. }) = self.selection else {
            return;
        };
        if p != head_page {
            // Across pages, each end on this page moves to its glyph's place in the layer.
            let mut moved = sel;
            if let Span::Run { anchor: a, head: h, .. } = &mut moved.span {
                if p == page {
                    *a = layer_index(layer, anchor);
                }
                if head_page == page {
                    *h = layer_index(layer, head);
                }
            }
            self.selection = Some(moved);
            return;
        }
        if p != page {
            return;
        }
        self.selection = match layer.text.glyphs.len().checked_sub(1) {
            Some(last) if whole => Some(Selection { whole_page: true, ..Selection::run(page, 0, last) }),
            _ => exact_selection(page, layer, anchor, head).or(Some(Selection::run(page, layer_index(layer, anchor), layer_index(layer, head)))),
        };
        // A drag's pointer end is a place on the page, the same on either layer: rows it
        // selected stay rows there.
        if let Some(Selection { span: Span::Run { head_x: moved, .. }, whole_page: false, .. }) = self.selection.as_mut() {
            *moved = head_x;
        }
    }

    /// Selected text, if any (⌘C).
    /// The selected text: of the pages read so far, after Select All of the document (Copy reads
    /// the others, [`DocView::pages_without_text`]). A selection across pages has each page's
    /// part on a line of its own, as Select All of the document does.
    pub fn selected_text(&self) -> Option<String> {
        if self.all_text {
            return Some(self.whole_text(&HashMap::new())).filter(|x| !x.is_empty());
        }
        let (s, texts) = (self.selection?, self.selection_texts()?);
        let parts: Vec<String> = texts.into_iter().map(|(p, t)| self.selected_text_on(&s, p, t)).filter(|x| !x.is_empty()).collect();
        Some(parts.join("\n")).filter(|x| !x.is_empty())
    }

    /// Whether the text selection runs over more than one page.
    pub(crate) fn selection_spans_pages(&self) -> bool {
        self.selection.is_some_and(|s| s.spans_pages())
    }

    /// Whether Copy has something to copy from the text selection, without building the text:
    /// strict text, or, for Copy with formatting (`with_formatting`: a clipboard that takes
    /// HTML), glyphs only the recovered layer reads. Plain Copy is strict only.
    pub fn selection_copies(&self, with_formatting: bool) -> bool {
        self.selection_has_strict_text() || (with_formatting && self.selection_has_recovered())
    }

    /// Whether the selection holds strict text (what plain Copy copies).
    /// Across pages, on any page it covers whose text is read.
    fn selection_has_strict_text(&self) -> bool {
        if self.all_text {
            return self.texts.values().any(|t| t.glyphs.iter().any(|g| !g.text.is_empty()));
        }
        let Some(s) = self.selection else { return false };
        s.pages().any(|page| {
            let Some(t) = self.texts.get(&page) else { return false };
            let nonempty = |i: usize| t.glyphs.get(i).is_some_and(|g| !g.text.is_empty());
            match (self.recovered_layer(page), s.span) {
                (Some(layer), Span::Run { .. }) => {
                    layer_run_glyphs(&s, page, layer).unwrap_or_default().into_iter().any(|i| layer.strict_index_of(i).is_some_and(nonempty))
                }
                (None, Span::Run { anchor, head, head_page, head_x }) if page == s.page && head_page == s.page => {
                    match Selection::by_rows(t, anchor, head, head_x) {
                        Some(rows) => rows.into_iter().any(nonempty),
                        None => Selection::range(anchor, head).take(t.glyphs.len()).any(nonempty),
                    }
                }
                (None, Span::Run { .. }) => s.run_on(page, t.glyphs.len()).is_some_and(|mut r| r.any(nonempty)),
                (_, Span::Column { rect }) => page == s.page && t.glyphs_in(rect).into_iter().any(nonempty),
            }
        })
    }

    /// Whether the selection holds a glyph only the recovered layer reads.
    pub(crate) fn selection_has_recovered(&self) -> bool {
        self.selected_layer_glyphs().is_some_and(|(_, layer, glyphs)| layer.any_recovered(&glyphs))
    }

    /// The selection as glyphs of its page's recovered layer: a run's in copy order (a table's
    /// rows as [`PageText::visual_between`] takes them), a column's inside its rectangle. `None`
    /// where the page has no recovered layer, after Select All of the document, and for a
    /// selection across pages, which copies plain text, every page's, as [`Self::selected_glyphs`].
    pub(super) fn selected_layer_glyphs(&self) -> Option<(Selection, Arc<RecoveredText>, Vec<usize>)> {
        if self.all_text {
            return None;
        }
        let s = self.selection.filter(|s| !s.spans_pages())?;
        let layer = self.recovered_layer(s.page)?;
        let glyphs = match s.span {
            Span::Run { anchor, head, head_x, .. } => run_glyphs(layer, anchor, head, head_x, s.strict),
            Span::Column { rect } => layer.text.glyphs_in(rect),
        };
        Some((s, layer.clone(), glyphs))
    }

    /// The selection as glyphs of its page's recovered layer (a run's in copy order), when it
    /// holds a glyph only that layer reads, for Copy with formatting, and whether it is a run in
    /// reading order (a column and a table's rows copy plain text only). `None` where the page
    /// has no recovered layer, after Select All of the document, and when nothing recovered is
    /// selected.
    pub(crate) fn selected_recovered(&self) -> Option<(usize, Arc<RecoveredText>, Vec<usize>, bool)> {
        let (s, layer, glyphs) = self.selected_layer_glyphs()?;
        let in_order = matches!(s.span, Span::Run { .. }) && !s.rows_on(&layer.text);
        layer.any_recovered(&glyphs).then_some((s.page, layer, glyphs, in_order))
    }

    /// The selection's page, its strict text layer and the selected strict glyphs in copy order,
    /// for Copy's HTML (on a page with a recovered layer, the strict glyphs the selection holds
    /// there). `None` after Select All of the document, for a column selection or a table's rows,
    /// which copy plain text (rows and tab-separated cells are not paragraphs), and for a
    /// selection across pages, whose formatted copy would hold one page of it: it copies plain
    /// text, every page's.
    pub(crate) fn selected_glyphs(&self) -> Option<(usize, Arc<PageText>, Vec<usize>)> {
        if self.all_text {
            return None;
        }
        let s = self.selection.filter(|s| !s.spans_pages())?;
        let t = self.texts.get(&s.page)?;
        let glyphs = match (s.span, self.recovered_layer(s.page)) {
            (Span::Run { anchor, head, .. }, Some(layer)) if !s.rows_on(&layer.text) => {
                layer.strict_glyphs(&run_glyphs(layer, anchor, head, None, s.strict))
            }
            (Span::Run { anchor, head, .. }, None) if !s.rows_on(t) => {
                let r = Selection::range(anchor, head);
                (r.start..r.end.min(t.glyphs.len())).collect()
            }
            // A table's rows, like a column, are tab-separated cells. Across pages is excluded above.
            (Span::Run { .. } | Span::Column { .. }, _) => return None,
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

    /// Moves the end of the reading-order selection to glyph `head` of `page`'s selection layer,
    /// on its page or another (Shift+click, #527), the pointer at `head_x` ([`Span::Run`]):
    /// `false` without a reading-order selection.
    pub(super) fn extend_selection(&mut self, page: usize, head: usize, head_x: Option<f32>) -> bool {
        match self.selection.as_mut() {
            Some(s) if matches!(s.span, Span::Run { .. }) => {
                s.extend_to_at(page, head, head_x);
                true
            }
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

/// Edits that each add a page's part of something marked on a text selection, as one undo step:
/// the edit itself for one page, a batch named like it for several. For edits whose label has no
/// hole (adding a comment), so the batch's name is a wording the catalogues translate.
pub(crate) fn one_undo_step(mut edits: Vec<Edit>) -> Option<Edit> {
    if edits.len() <= 1 {
        return edits.pop();
    }
    let label = edits.first()?.label().wording.to_owned();
    Some(Edit::Batch { label, edits })
}

/// Where the selection bar goes: next to the selected text where the drag ended, which is on
/// screen. Forward, the last rectangle on the head's page; backward across pages, the first one
/// there. Otherwise (a column, or nothing selected on the head's page) the last rectangle on the
/// last page with text selected.
/// `rects` gives the rectangles the selection covers on a page and its selection layer
/// ([`DocView::selection_rects_on`]).
pub(super) fn bar_anchor(
    selection: &Selection,
    texts: &[(usize, Arc<PageText>)],
    rects: impl Fn(usize, &PageText) -> Vec<[f32; 4]>,
) -> Option<(usize, [f32; 4])> {
    let at_head = selection.text_span().and_then(|span| {
        let Span::Run { head_page, .. } = selection.span else { return None };
        let backward = span.start.0 == head_page && span.start.0 != span.end.0;
        let (_, text) = texts.iter().find(|(p, _)| *p == head_page)?;
        let rects = rects(head_page, text);
        let rect = if backward { rects.first() } else { rects.last() };
        rect.map(|r| (head_page, *r))
    });
    at_head.or_else(|| texts.iter().rev().find_map(|(p, t)| rects(*p, t).last().map(|r| (*p, *r))))
}

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
    // Text only a recovered layer reads is selected too: its markup buttons stay on.
    if view.selected_text().is_none() && !view.selection_has_recovered() {
        return false;
    }
    let Some(layers) = view.selection_layers() else { return false };
    let Some((page, last)) = bar_anchor(&selection, &layers, |p, t| view.selection_rects_on(&selection, p, t)) else { return false };
    let Some(xf) = view.page_xform(page) else { return false };
    // A comment or a replacement sits on one page: across pages, only markup and Copy.
    let across_pages = selection.spans_pages();
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
                    let one_page_only = matches!(action, SelectionAction::Note | SelectionAction::Replace);
                    if ui.add_enabled(can_comment && !(across_pages && one_page_only), egui::Button::new(label)).clicked() {
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
    let marks = view.selection_quads(info);
    if marks.is_empty() {
        return false;
    }
    match action {
        SelectionAction::Note | SelectionAction::Replace => {
            // Disabled across pages (above).
            let Ok([(page, quads)]) = <[_; 1]>::try_from(marks) else { return false };
            let last = quads.last().copied().unwrap_or_default();
            let at = [last[2].max(last[6]), last[1].max(last[3])];
            view.clear_selection();
            let kind = if matches!(action, SelectionAction::Replace) {
                view.comments.replace = Some(quads);
                comments::ComposerKind::Replace
            } else {
                comments::ComposerKind::TextNote { quads }
            };
            view.comments.composer = Some(comments::Composer { page, at, kind, text: String::new(), focus: true });
        }
        SelectionAction::Markup(tool) => {
            // Every markup button carries a markup tool; anything else adds nothing.
            let Some(kind) = tool.markup() else { return false };
            view.clear_selection();
            let edits = marks.into_iter().map(|(page, quads)| {
                Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                    page,
                    shape: pdfcraft_engine::Shape::TextMarkup { kind, quads },
                    style: prefs.style(tool),
                    contents: String::new(),
                    author: prefs.author.clone(),
                })
            });
            view.pending_edit = view.selection_marks_edit(edits.collect(), selection.head_page());
        }
        // Copy is dispatched through the app after the view borrow ends.
        SelectionAction::Copy => {}
    }
    false
}
