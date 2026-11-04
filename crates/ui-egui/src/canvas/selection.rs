use super::{Arc, DocInfo, DocView, Edit, HashMap, PageText, Pos2, Range, Rect, comments, pos2};
use std::ops::RangeInclusive;

/// A text selection. `page` is the page it started on: a column selection stays there, a
/// reading-order one may run on to other pages.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Selection {
    pub(super) page: usize,
    pub(super) span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Span {
    /// Reading order, from glyph `anchor` on the selection's page to glyph `head` on `head_page`
    /// (either may come first). Across pages it takes the rest of the first page, every page
    /// between whole, and the last page up to its end, as Acrobat does.
    Run { anchor: usize, head: usize, head_page: usize },
    /// Column select (#740): the glyphs inside a rectangle in page view space, drawn from the
    /// corner the drag started at (`rect[0..2]`) to the pointer (`rect[2..4]`).
    Column { rect: [f32; 4] },
}

impl Selection {
    pub(super) fn run(page: usize, anchor: usize, head: usize) -> Self {
        Self { page, span: Span::Run { anchor, head, head_page: page } }
    }

    /// The pages the selection covers, first to last.
    pub(super) fn pages(&self) -> RangeInclusive<usize> {
        match self.span {
            Span::Run { head_page, .. } => self.page.min(head_page)..=self.page.max(head_page),
            Span::Column { .. } => self.page..=self.page,
        }
    }

    /// Whether the selection runs over more than one page.
    pub(super) fn spans_pages(&self) -> bool {
        self.pages().start() != self.pages().end()
    }

    /// The reading-order glyphs selected on `page`, which has `len` glyphs; `None` off the
    /// selection's pages and for a column selection.
    pub(super) fn run_on(&self, page: usize, len: usize) -> Option<Range<usize>> {
        let Span::Run { anchor, head, head_page } = self.span else { return None };
        // (page, glyph) pairs compare in reading order.
        let (start, end) = if (self.page, anchor) <= (head_page, head) {
            ((self.page, anchor), (head_page, head))
        } else {
            ((head_page, head), (self.page, anchor))
        };
        if page < start.0 || page > end.0 {
            return None;
        }
        let from = if page == start.0 { start.1 } else { 0 };
        let to = if page == end.0 { end.1.saturating_add(1) } else { len };
        Some(from..to.max(from))
    }

    /// One rectangle per selected piece of a line on `page`, in view space (highlighting,
    /// markup quads).
    pub(super) fn rects_on(&self, page: usize, text: &PageText) -> Vec<[f32; 4]> {
        match self.span {
            Span::Run { .. } => self.run_on(page, text.glyphs.len()).map(|r| text.line_rects(r)).unwrap_or_default(),
            Span::Column { rect } if page == self.page => text.glyph_rects(&text.glyphs_in(rect)),
            Span::Column { .. } => Vec::new(),
        }
    }

    /// The text selected on `page`, as Copy puts it on the clipboard.
    pub(super) fn text_on(&self, page: usize, text: &PageText) -> String {
        match self.span {
            Span::Run { .. } => self.run_on(page, text.glyphs.len()).map(|r| text.text_of(r)).unwrap_or_default(),
            Span::Column { rect } if page == self.page => text.column_text(&text.glyphs_in(rect)),
            Span::Column { .. } => String::new(),
        }
    }

    /// Extend a reading-order selection to glyph `head` on `page`; a column selection, which has
    /// no anchor glyph, stays as it is.
    pub(super) fn extend_to(&mut self, page: usize, glyph: usize) {
        if let Span::Run { head, head_page, .. } = &mut self.span {
            (*head, *head_page) = (glyph, page);
        }
    }
}

/// A drag pressed on `page` starts a text selection there: from the glyph nearest `origin`, or
/// a column rectangle from `origin` to `to` (page view space). After a multi-click the word or
/// line the clicks selected stays, for the drag to extend by that unit.
pub(super) fn start_selection_drag(view: &mut DocView, page: usize, text: &PageText, column: bool, origin: (f32, f32), to: (f32, f32)) {
    // A drag replaces the selection without passing through none, so a re-drag over text whose
    // bar Copy closed would keep the bar hidden.
    view.actions_dismissed = None;
    view.selection_drag = true;
    if view.clicks < 2 {
        let (ox, oy) = origin;
        view.selection = if column {
            Some(Selection { page, span: Span::Column { rect: [ox, oy, to.0, to.1] } })
        } else {
            text.nearest(ox, oy).map(|a| Selection::run(page, a, a))
        };
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

    /// The text selection as markup quadrilaterals in user space, one entry per page with
    /// selected text: (page, quads). Empty when there is nothing to mark yet.
    pub(crate) fn selection_quads(&self, info: &DocInfo) -> Vec<(usize, Vec<[f64; 8]>)> {
        let (Some(s), Some(texts)) = (self.selection, self.selection_texts()) else { return Vec::new() };
        texts
            .into_iter()
            .filter_map(|(p, text)| {
                let page = info.pages.get(p)?;
                let quads: Vec<[f64; 8]> = s.rects_on(p, text).into_iter().map(|r| page.view_rect_to_quad(r)).collect();
                (!quads.is_empty()).then_some((p, quads))
            })
            .collect()
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection = None;
        self.actions_dismissed = None;
    }

    /// Select text on `page` from glyph `from` to glyph `to` (tests and automation).
    pub fn select_text(&mut self, page: usize, from: usize, to: usize) {
        self.all_text = false;
        self.selection = Some(Selection::run(page, from, to));
    }

    /// Screen position of the centre of glyph `glyph` on `page`, if that page is on screen and its
    /// text layer is loaded (used by UI tests and automation to aim pointer input).
    pub fn glyph_screen_pos(&self, page: usize, glyph: usize) -> Option<Pos2> {
        let (_, xf) = self.screen_xforms.iter().find(|(p, _)| *p == page)?;
        let g = self.texts.get(&page)?.glyphs.get(glyph)?;
        Some(xf.view_rect(g.rect).center())
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
        let parts: Vec<String> = texts.into_iter().map(|(p, t)| s.text_on(p, t)).filter(|x| !x.is_empty()).collect();
        Some(parts.join("\n")).filter(|x| !x.is_empty())
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
        let Some(t) = self.texts.get(&page) else { return false };
        if t.glyphs.is_empty() {
            return false;
        }
        self.selection = Some(Selection::run(page, 0, t.glyphs.len() - 1));
        true
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
    if view.selected_text().is_none() {
        return false;
    }
    // Next to where the selection ends: the last rectangle on its last page with text selected.
    let Some(texts) = view.selection_texts() else { return false };
    let Some((page, last)) = texts.iter().rev().find_map(|(p, t)| selection.rects_on(*p, t).last().map(|r| (*p, *r))) else { return false };
    let Some(xf) = view.page_xform(page) else { return false };
    // A comment or a replacement sits on one page: across pages, only markup and Copy.
    let across_pages = selection.spans_pages();
    let rect = xf.view_rect(last);
    if !viewport.intersects(rect) {
        return false;
    }
    let x = rect.left().clamp(viewport.left() + 8.0, (viewport.right() - 520.0).max(viewport.left() + 8.0));
    let y = if rect.bottom() + 48.0 < viewport.bottom() { rect.bottom() + 6.0 } else { (rect.top() - 44.0).max(viewport.top() + 6.0) };
    let mut chosen = None;
    egui::Area::new(egui::Id::unique(("text-selection-actions", view.id.0))).order(egui::Order::Foreground).fixed_pos(pos2(x, y)).show(
        ui.ctx(),
        |ui| {
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
                    if ui.add_enabled(can_copy, egui::Button::new(tl!("Copy"))).clicked() {
                        chosen = Some(SelectionAction::Copy);
                    }
                });
            });
        },
    );
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
            view.pending_edit = one_undo_step(edits.collect());
        }
        // Copy is dispatched through the app after the view borrow ends.
        SelectionAction::Copy => {}
    }
    false
}
