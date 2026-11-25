use super::{Arc, DocInfo, DocView, Edit, PageGeometry, PageSession, PageText, Pos2, Range, Rect, comments};
use crate::QuickTool;
use craft::behavior::Selection;
use craft::behavior::kurbo::{self, Point};
use craft::pages::ui::{PageBody, PageTextSource, PageTexts, TextSelection, TextSpan as FwSpan};
use craft::ui::buttons::Button;
use craft::ui::canvas_controls::TaskBar;
use craft::ui::contract::{Props as _, Show as _};
use std::collections::{HashMap, HashSet};

/// The text layer of a page that could not be read: a page with nothing to select, which a selection across
/// pages passes over.
static NO_TEXT: PageText = PageText { glyphs: Vec::new(), line_of: Vec::new(), space_before: Vec::new(), truncated: false };

/// One page's extracted text as the framework's selection tool reads it. The tool works in the page as shown (the view
/// rotation applied, `PlacedPage::to_page`); the text layer is in view space, which the rotation leaves unturned, so
/// points and rectangles are mapped between the two here, along with `f32` to `f64`.
pub(crate) struct AppText<'a> {
    text: &'a PageText,
    /// The view rotation in degrees, 0 to 359.
    rotation: i32,
    /// The page's size in view space.
    size: (f64, f64),
}

impl<'a> AppText<'a> {
    /// The layer `text` of a page `size` points large in view space, shown turned by `rotation` degrees.
    pub(crate) fn new(text: &'a PageText, rotation: i32, size: (f32, f32)) -> Self {
        Self { text, rotation: rotation.rem_euclid(360), size: (f64::from(size.0), f64::from(size.1)) }
    }

    /// A point of the page as shown, in view space.
    fn to_view(&self, p: Point) -> (f32, f32) {
        let (w, h) = self.size;
        let (x, y) = match self.rotation {
            90 => (p.y, h - p.x),
            180 => (w - p.x, h - p.y),
            270 => (w - p.y, p.x),
            _ => (p.x, p.y),
        };
        (x as f32, y as f32)
    }

    /// A point of view space, in the page as shown.
    fn to_shown(&self, x: f32, y: f32) -> Point {
        let (w, h) = self.size;
        let (x, y) = (f64::from(x), f64::from(y));
        match self.rotation {
            90 => Point::new(h - y, x),
            180 => Point::new(w - x, h - y),
            270 => Point::new(y, w - x),
            _ => Point::new(x, y),
        }
    }

    fn shown_rect(&self, r: [f32; 4]) -> kurbo::Rect {
        kurbo::Rect::from_points(self.to_shown(r[0], r[1]), self.to_shown(r[2], r[3]))
    }

    /// A rectangle of the page as shown, in view space as `[x0, y0, x1, y1]`.
    pub(crate) fn view_rect(&self, r: kurbo::Rect) -> [f32; 4] {
        let (a, b) = (self.to_view(Point::new(r.x0, r.y0)), self.to_view(Point::new(r.x1, r.y1)));
        [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]
    }
}

impl PageTextSource for AppText<'_> {
    fn glyph_count(&self) -> usize {
        self.text.glyphs.len()
    }

    fn glyph_at(&self, p: Point) -> Option<usize> {
        let (x, y) = self.to_view(p);
        self.text.nearest(x, y)
    }

    fn glyph_box_at(&self, p: Point) -> Option<usize> {
        let (x, y) = self.to_view(p);
        self.text.glyphs.iter().position(|g| g.rect[0] <= x && x <= g.rect[2] && g.rect[1] <= y && y <= g.rect[3])
    }

    fn line_rects(&self, glyphs: Range<usize>) -> Vec<kurbo::Rect> {
        self.text.line_rects(glyphs).into_iter().map(|r| self.shown_rect(r)).collect()
    }

    fn glyphs_in(&self, rect: kurbo::Rect) -> Vec<usize> {
        self.text.glyphs_in(self.view_rect(rect))
    }

    fn word_at(&self, glyph: usize) -> Option<Range<usize>> {
        self.text.word_at(glyph).map(|(first, last)| first..last.saturating_add(1))
    }

    fn line_at(&self, glyph: usize) -> Option<Range<usize>> {
        self.text.line_at(glyph).map(|(first, last)| first..last.saturating_add(1))
    }

    fn text_of(&self, glyphs: Range<usize>) -> String {
        self.text.text_of(glyphs)
    }

    fn column_text(&self, glyphs: &[usize]) -> String {
        self.text.column_text(glyphs)
    }

    /// What a drag over table rows selects by rows, not reading order's detour down each column. Rows run along the
    /// text layer's x axis, which a quarter turn of the view makes the shown y axis: only an unturned or half-turned
    /// page has an `end_x` to take.
    fn rows_between(&self, anchor: usize, head: usize, end_x: f64) -> Option<Vec<usize>> {
        let x = match self.rotation {
            0 => end_x,
            180 => self.size.0 - end_x,
            _ => return None,
        };
        self.text.visual_between(anchor, head, Some(x as f32))
    }
}

/// The text layers of a document's pages for the selection tool, built per pass from what the view has read.
pub(crate) struct DocTexts<'a> {
    pages: HashMap<usize, AppText<'a>>,
}

impl<'a> DocTexts<'a> {
    /// The layers in `texts`, shown turned by `rotation`, with `sizes` the pages' sizes in view space. A page in
    /// `failed` has an empty layer: nothing is selected on it, and it does not hold a copy back.
    // PLAN: the decisions give `new(texts, rotation, sizes)`; `failed` is added so failed pages stay out of a copy.
    pub(crate) fn new(texts: &'a HashMap<usize, Arc<PageText>>, failed: &HashSet<usize>, rotation: impl Into<i32>, sizes: &[(f32, f32)]) -> Self {
        let rotation = rotation.into();
        let size = |page: usize| sizes.get(page).copied().unwrap_or_default();
        let mut pages: HashMap<usize, AppText<'a>> = texts.iter().map(|(&p, t)| (p, AppText::new(t, rotation, size(p)))).collect();
        for &p in failed {
            pages.entry(p).or_insert_with(|| AppText::new(&NO_TEXT, rotation, size(p)));
        }
        Self { pages }
    }

    /// Page `page`'s layer, with its mappings to view space.
    pub(crate) fn app(&self, page: usize) -> Option<&AppText<'a>> {
        self.pages.get(&page)
    }
}

impl PageTexts for DocTexts<'_> {
    fn page(&self, page: usize) -> Option<&dyn PageTextSource> {
        self.pages.get(&page).map(|t| t as &dyn PageTextSource)
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

/// A reading-order selection's ends in reading order; `None` for a column selection.
fn reading_span(selection: &TextSelection) -> Option<TextSpan> {
    let FwSpan::Run { anchor, head, head_page, .. } = selection.span else { return None };
    // (page, glyph) pairs compare in reading order.
    let (start, end) = if (selection.page, anchor) <= (head_page, head) {
        ((selection.page, anchor), (head_page, head))
    } else {
        ((head_page, head), (selection.page, anchor))
    };
    Some(TextSpan { start, end })
}

/// The page the selection ends on, where its drag finished: a reading-order selection's head page, a column
/// selection's own page.
fn head_page_of(selection: &TextSelection) -> usize {
    match selection.span {
        FwSpan::Run { head_page, .. } => head_page,
        _ => selection.page,
    }
}

/// The pages' sizes in view space, which the text layers are mapped by.
fn page_sizes(info: &DocInfo) -> Vec<(f32, f32)> {
    info.pages.iter().map(|p| (p.width, p.height)).collect()
}

// The selection's state lives in the page session, which the page body works on while the page view holds the view
// state; the view's methods below delegate to it.
impl PageSession {
    /// The text layers read so far, shown turned by `rotation`, with `sizes` the pages' sizes in view space.
    fn doc_texts(&self, rotation: u16, sizes: &[(f32, f32)]) -> DocTexts<'_> {
        DocTexts::new(&self.texts, &self.text_failed, rotation, sizes)
    }

    /// The text selection as markup quadrilaterals in user space, one entry per page with
    /// selected text: (page, quads). Empty when there is nothing to mark yet.
    pub(crate) fn selection_quads(&self, info: &DocInfo, rotation: u16) -> Vec<(usize, Vec<[f64; 8]>)> {
        let Some(s) = self.text.selection else { return Vec::new() };
        let sizes = page_sizes(info);
        let texts = self.doc_texts(rotation, &sizes);
        if !s.unread_pages(&texts).is_empty() {
            return Vec::new();
        }
        s.pages()
            .filter_map(|p| {
                let page = info.pages.get(p)?;
                let app = texts.app(p)?;
                let quads: Vec<[f64; 8]> = s.rects_on(p, app).into_iter().map(|r| page.view_rect_to_quad(app.view_rect(r))).collect();
                (!quads.is_empty()).then_some((p, quads))
            })
            .collect()
    }

    /// Drops the selection, with the bar and the marks that waited on it.
    pub(crate) fn clear_selection(&mut self) {
        self.text.selection = None;
        self.actions_dismissed = None;
        self.marks_waiting = None;
    }

    /// Whether a page the selection covers is still to be read, so a selection is never copied or marked short.
    fn selection_unread(&self, geo: PageGeometry<'_>) -> bool {
        self.text.selection.is_some_and(|s| !s.unread_pages(&self.doc_texts(geo.rotation, geo.sizes)).is_empty())
    }

    /// Whether the selection covers a page whose text could not be read ([`DocView::selection_has_unreadable_page`]).
    fn selection_has_unreadable_page(&self) -> bool {
        self.text.selection.is_some_and(|s| s.pages().any(|p| self.text_failed.contains(&p)))
    }

    /// What a marking tool marks for the text selection, with the pages placed by `geo`
    /// ([`DocView::tool_marks`]).
    pub(crate) fn tool_marks(
        &mut self,
        tool: QuickTool,
        finished: bool,
        page: Option<usize>,
        whole: bool,
        info: &DocInfo,
        geo: PageGeometry<'_>,
    ) -> ToolMarks {
        let Some(sel) = self.text.selection else {
            self.marks_waiting = None;
            return ToolMarks::None;
        };
        if finished {
            self.marks_waiting = Some((tool, sel));
        }
        if !self.marks_waiting.as_ref().is_some_and(|(t, s)| *t == tool && *s == sel) {
            // The selection or the tool changed: the wait is over, so a drag passing back through
            // the selection it was for doesn't mark it before it is released.
            self.marks_waiting = None;
            return ToolMarks::None;
        }
        if whole && self.selection_has_unreadable_page() {
            self.marks_waiting = None;
            return ToolMarks::Unreadable;
        }
        if self.selection_unread(geo) {
            return ToolMarks::None;
        }
        let marks = self.selection_quads(info, geo.rotation);
        if marks.is_empty() {
            // Every page is read and nothing is selected: nothing to wait for.
            self.marks_waiting = None;
            return ToolMarks::None;
        }
        if page.is_some_and(|page| !marks.iter().any(|(p, _)| *p == page)) {
            return ToolMarks::None;
        }
        self.marks_waiting = None;
        ToolMarks::Marks(marks, head_page_of(&sel))
    }

    /// The edit adding what a tool marks on a text selection ([`DocView::selection_marks_edit`]).
    pub(crate) fn selection_marks_edit(&mut self, edits: Vec<Edit>, head: usize) -> Option<Edit> {
        let edit = one_undo_step(edits);
        self.marks_across_pages = matches!(edit, Some(Edit::Batch { .. })).then_some(head);
        edit
    }
}

impl DocView {
    /// The text layers the view has read, for the selection tool and what reads its selection.
    pub(crate) fn doc_texts(&self) -> DocTexts<'_> {
        DocTexts::new(&self.session.texts, &self.session.text_failed, self.view.rotation, &self.page_sizes)
    }

    /// Whether a page the selection covers is still to be read, so a selection is never copied or marked short.
    fn selection_unread(&self) -> bool {
        self.session.selection_unread(self.geometry())
    }

    /// A reading-order selection whose pages are not all read yet: Copy reads them off the UI
    /// thread ([`crate::DocumentCopyJob::of_selection`]) rather than copying short or nothing.
    pub(crate) fn selection_waits_for_text(&self) -> Option<TextSpan> {
        let s = self.session.text.selection.filter(|_| self.selection_unread())?;
        reading_span(&s)
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
        let geo = PageGeometry { sizes: &self.page_sizes, rotation: self.view.rotation, viewport: self.viewport };
        self.session.tool_marks(tool, finished, page, whole, info, geo)
    }

    /// The edit adding what a tool marks on a text selection, one edit per page, as one undo
    /// step ([`one_undo_step`]). Across pages it is marked as such for `apply_edit`
    /// ([`Self::marks_across_pages`]), with `head`, the page the selection ends on, which it
    /// follows up as one markup: other batches of comments (Redact's page range or search) are
    /// not followed up.
    pub(crate) fn selection_marks_edit(&mut self, edits: Vec<Edit>, head: usize) -> Option<Edit> {
        self.session.selection_marks_edit(edits, head)
    }

    pub(crate) fn clear_selection(&mut self) {
        self.session.clear_selection();
    }

    /// Select text on `page` from glyph `from` to glyph `to` (tests and automation).
    pub fn select_text(&mut self, page: usize, from: usize, to: usize) {
        self.session.all_text = false;
        self.session.text.selection = Some(TextSelection::new(page, FwSpan::run(from, page, to)));
    }

    /// Screen position of the centre of glyph `glyph` on `page`, if that page is on screen and its
    /// text layer is loaded (used by UI tests and automation to aim pointer input).
    pub fn glyph_screen_pos(&self, page: usize, glyph: usize) -> Option<Pos2> {
        self.glyph_screen_rect(page, glyph).map(|r| r.center())
    }

    /// The screen rectangle of glyph `glyph` on `page`, when the page is on screen.
    pub fn glyph_screen_rect(&self, page: usize, glyph: usize) -> Option<Rect> {
        let xf = self.page_xform(page)?;
        let g = self.session.texts.get(&page)?.glyphs.get(glyph)?;
        Some(xf.view_rect(g.rect))
    }

    /// The selected text: of the pages read so far, after Select All of the document (Copy reads
    /// the others, [`DocView::pages_without_text`]). A selection across pages has each page's
    /// part on a line of its own, as Select All of the document does.
    pub fn selected_text(&self) -> Option<String> {
        if self.session.all_text {
            return Some(self.whole_text(&HashMap::new())).filter(|x| !x.is_empty());
        }
        self.session.text.selection?.text(&self.doc_texts()).filter(|x| !x.is_empty())
    }

    /// The selection's page, its text layer and the selected glyphs in copy order, for Copy with
    /// formatting. `None` after Select All of the document, for a column selection or a table's
    /// rows, which copy plain text (rows and tab-separated cells are not paragraphs), and for a
    /// selection across pages, whose formatted copy would hold one page of it: it copies plain
    /// text, every page's.
    pub(crate) fn selected_glyphs(&self) -> Option<(usize, Arc<PageText>, Vec<usize>)> {
        if self.session.all_text {
            return None;
        }
        let s = self.session.text.selection.filter(|s| s.pages().start() == s.pages().end())?;
        let t = self.session.texts.get(&s.page)?;
        let texts = self.doc_texts();
        let source = texts.page(s.page)?;
        let FwSpan::Run { anchor, head, end_x, .. } = s.span else { return None };
        // A table's rows, like a column, are tab-separated cells.
        if end_x.and_then(|x| source.rows_between(anchor, head, x)).is_some() {
            return None;
        }
        let from = anchor.min(head);
        let to = anchor.max(head).saturating_add(1).min(t.glyphs.len());
        Some((s.page, t.clone(), (from..to).collect()))
    }

    /// Select every page without navigating; shared by Organize and the Pages sidebar.
    pub fn select_all_pages(&mut self) -> bool {
        let anchor = (self.page_count > 0).then_some(self.view.current);
        self.selected = Selection::new((0..self.page_count).collect(), anchor, anchor);
        !self.selected.items.is_empty()
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
        if self.continuous() && self.page_count > 1 {
            self.session.text.selection = None;
            self.session.all_text = true;
            return self.selected_text().is_some();
        }
        self.session.all_text = false;
        let page = self.view.current;
        let Some(t) = self.session.texts.get(&page) else { return false };
        if t.glyphs.is_empty() {
            return false;
        }
        self.session.text.selection = Some(TextSelection::new(page, FwSpan::run(0, page, t.glyphs.len() - 1)));
        true
    }

    /// Pages whose text [`DocView::whole_text`] still needs: not read yet, nor failed.
    pub(crate) fn pages_without_text(&self) -> Vec<usize> {
        (0..self.page_count).filter(|p| !self.session.texts.contains_key(p) && !self.session.text_failed.contains(p)).collect()
    }

    /// The whole document's text in page order, one page after another on new lines, from the
    /// pages' text read so far and `more` (pages read for this, keyed by page).
    pub(crate) fn whole_text(&self, more: &HashMap<usize, Arc<PageText>>) -> String {
        let mut out = String::new();
        for p in 0..self.page_count {
            let Some(t) = self.session.texts.get(&p).or_else(|| more.get(&p)) else { continue };
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
/// screen, as a rectangle of the page as shown. Forward, the last rectangle on the head's page; backward across pages,
/// the first one there. Otherwise (a column, or nothing selected on the head's page) the last rectangle on the
/// last page with text selected.
pub(super) fn bar_anchor(selection: &TextSelection, texts: &dyn PageTexts) -> Option<(usize, kurbo::Rect)> {
    let at_head = || {
        let FwSpan::Run { head_page, .. } = selection.span else { return None };
        let backward = head_page < selection.page;
        let rects = selection.rects_on(head_page, texts.page(head_page)?);
        let rect = if backward { rects.first() } else { rects.last() };
        rect.map(|r| (head_page, *r))
    };
    at_head().or_else(|| {
        selection.pages().rev().find_map(|p| {
            let rects = selection.rects_on(p, texts.page(p)?);
            rects.last().map(|r| (p, *r))
        })
    })
}

/// Actions on an existing text selection, next to the selected words. Returns whether Copy was chosen; the caller
/// copies once the session borrow ends. It works on the page session, as the page body does, and takes the view's
/// `rotation` for where the words are shown.
// PLAN: pdf-canvas-w8's body calls `selection_bar(ui, body, session, info, prefs, may_mark, can_copy)`; `rotation`
// (`cx.rotation`) goes after `info`.
#[allow(clippy::too_many_arguments)]
pub(super) fn selection_bar(
    ui: &mut egui::Ui,
    body: &PageBody<'_>,
    session: &mut PageSession,
    info: &DocInfo,
    rotation: u16,
    prefs: &comments::CommentPrefs,
    can_comment: bool,
    can_copy: bool,
) -> bool {
    if body.pass.is_some_and(|p| p.press.phase.button_down()) {
        return false;
    }
    let Some(selection) = session.text.selection else {
        session.actions_dismissed = None;
        return false;
    };
    if session.actions_dismissed == Some(selection) {
        return false;
    }
    let sizes = page_sizes(info);
    let anchor = {
        let texts = session.doc_texts(rotation, &sizes);
        if selection.text(&texts).is_none_or(|t| t.is_empty()) {
            return false;
        }
        let Some((page, last)) = bar_anchor(&selection, &texts) else { return false };
        let Some(placed) = body.pages.iter().find(|p| p.page == page) else { return false };
        let corners = [Point::new(last.x0, last.y0), Point::new(last.x1, last.y0), Point::new(last.x1, last.y1), Point::new(last.x0, last.y1)];
        let screen = corners.map(|c| placed.to_screen(c));
        if !body.rect.intersects(egui::Rect::from_points(&screen)) {
            return false;
        }
        screen.map(|p| body.view.to_doc(p)).to_vec()
    };
    // A comment or a replacement sits on one page: across pages, only markup and Copy.
    let across_pages = selection.pages().start() != selection.pages().end();
    let mut chosen = None;
    TaskBar::fixed(tl!("Selection"), &anchor).show(ui, body.rect, |ui, _| {
        for (label, action) in [
            (tl!("Add comment"), SelectionAction::Note),
            (tl!("Highlight"), SelectionAction::Markup(comments::CommentTool::Highlight)),
            (tl!("Underline"), SelectionAction::Markup(comments::CommentTool::Underline)),
            (tl!("Strikethrough"), SelectionAction::Markup(comments::CommentTool::StrikeOut)),
            (tl!("Replace text"), SelectionAction::Replace),
        ] {
            let one_page_only = matches!(action, SelectionAction::Note | SelectionAction::Replace);
            if Button::new(label).enabled(can_comment && !(across_pages && one_page_only)).show(ui).clicked() {
                chosen = Some(action);
            }
        }
        if Button::new(tl!("Copy")).enabled(can_copy).show(ui).clicked() {
            chosen = Some(SelectionAction::Copy);
        }
    });
    let Some(action) = chosen else { return false };
    if let SelectionAction::Copy = action {
        // Copying closes the bar but keeps the words selected, for ⌘C or a second action.
        session.actions_dismissed = Some(selection);
        return true;
    }
    let marks = session.selection_quads(info, rotation);
    if marks.is_empty() {
        return false;
    }
    match action {
        SelectionAction::Note | SelectionAction::Replace => {
            // Disabled across pages (above).
            let Ok([(page, quads)]) = <[_; 1]>::try_from(marks) else { return false };
            let last = quads.last().copied().unwrap_or_default();
            let at = [last[2].max(last[6]), last[1].max(last[3])];
            session.clear_selection();
            let kind = if matches!(action, SelectionAction::Replace) {
                session.comments.replace = Some(quads);
                comments::ComposerKind::Replace
            } else {
                comments::ComposerKind::TextNote { quads }
            };
            session.comments.composer = Some(comments::Composer { page, at, kind, text: String::new(), focus: true });
        }
        SelectionAction::Markup(tool) => {
            // Every markup button carries a markup tool; anything else adds nothing.
            let Some(kind) = tool.markup() else { return false };
            session.clear_selection();
            let edits = marks.into_iter().map(|(page, quads)| {
                Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                    page,
                    shape: pdfcraft_engine::Shape::TextMarkup { kind, quads },
                    style: prefs.style(tool),
                    contents: String::new(),
                    author: prefs.author.clone(),
                })
            });
            session.pending_edit = session.selection_marks_edit(edits.collect(), head_page_of(&selection));
        }
        // Copy is dispatched through the app after the view borrow ends.
        SelectionAction::Copy => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A selection that covers a page whose text is not read yet gives no text: a copy waits instead of copying short.
    #[test]
    fn selected_text_is_none_while_a_covered_page_is_unread() {
        let mut texts = HashMap::new();
        texts.insert(0, Arc::new(PageText { glyphs: Vec::new(), line_of: Vec::new(), space_before: Vec::new(), truncated: false }));
        let sizes = [(300.0, 200.0); 2];
        let selection = TextSelection::new(0, FwSpan::run(0, 1, 0));
        let doc = DocTexts::new(&texts, &HashSet::new(), 0, &sizes);
        assert_eq!(selection.unread_pages(&doc), vec![1]);
        assert_eq!(selection.text(&doc), None);
        let failed = HashSet::from([1]);
        let doc = DocTexts::new(&texts, &failed, 0, &sizes);
        assert!(selection.unread_pages(&doc).is_empty(), "a page that failed is not waited for");
    }
}
