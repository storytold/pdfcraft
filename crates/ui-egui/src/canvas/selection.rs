use super::{Arc, DocInfo, DocView, Edit, HashMap, PageText, Pos2, Range, Rect, comments, pos2};

/// A text selection on one page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Selection {
    pub(super) page: usize,
    pub(super) span: Span,
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
        Self { page, span: Span::Run { anchor, head } }
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
        let text = self.texts.get(&s.page)?;
        let page = info.pages.get(s.page)?;
        let quads: Vec<[f64; 8]> = s.rects(text).into_iter().map(|r| page.view_rect_to_quad(r)).collect();
        (!quads.is_empty()).then_some((s.page, quads))
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
    /// the others, [`DocView::pages_without_text`]).
    pub fn selected_text(&self) -> Option<String> {
        if self.all_text {
            return Some(self.whole_text(&HashMap::new())).filter(|x| !x.is_empty());
        }
        let s = self.selection?;
        let t = self.texts.get(&s.page)?;
        Some(s.text(t)).filter(|x| !x.is_empty())
    }

    /// A new bookmark's title from the words selected on a page, as Acrobat names it (#5577):
    /// their text on one line, at most [`BOOKMARK_TITLE_CHARS`] characters. None after Select All
    /// of the document, or with nothing but spaces selected.
    pub fn bookmark_title(&self) -> Option<String> {
        if self.all_text {
            return None;
        }
        let s = self.selection?;
        title_of(&s.text(self.texts.get(&s.page)?))
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
    Bookmark,
}

/// The longest title [`DocView::bookmark_title`] makes, in characters.
pub const BOOKMARK_TITLE_CHARS: usize = 128;

/// What the document lets the selection bar's buttons do.
#[derive(Clone, Copy)]
pub(super) struct SelectionRights {
    pub comment: bool,
    pub copy: bool,
    /// Add a bookmark: assembly allowed and the document not read-only.
    pub bookmark: bool,
}

/// Actions on an existing text selection, next to the selected words.
pub(super) fn selection_actions(
    ui: &mut egui::Ui,
    viewport: Rect,
    view: &mut DocView,
    info: &DocInfo,
    prefs: &comments::CommentPrefs,
    can: SelectionRights,
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
    let Some(text) = view.texts.get(&selection.page) else { return false };
    let Some(xf) = view.page_xform(selection.page) else { return false };
    let Some(last) = selection.rects(text).last().copied() else { return false };
    let selected = selection.text(text);
    if selected.is_empty() {
        return false;
    }
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
                        if ui.add_enabled(can.comment, egui::Button::new(label)).clicked() {
                            chosen = Some(action);
                        }
                    }
                    // Acrobat names a bookmark made with words selected after them (#5577).
                    if ui.add_enabled(can.bookmark, egui::Button::new(tl!("Add bookmark"))).clicked() {
                        chosen = Some(SelectionAction::Bookmark);
                    }
                    if ui.add_enabled(can.copy, egui::Button::new(tl!("Copy"))).clicked() {
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
    if let SelectionAction::Bookmark = action {
        // The app adds it (`bookmark.add`), named after and pointing at the words still selected.
        view.actions_dismissed = Some(selection);
        view.bookmark_selection = true;
        return false;
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
        // Copy and Bookmark are dispatched through the app after the view borrow ends.
        SelectionAction::Copy | SelectionAction::Bookmark => {}
    }
    false
}

/// `text` on one line (every run of whitespace one space), at most [`BOOKMARK_TITLE_CHARS`]
/// characters with an ellipsis when cut; None when it is only whitespace.
fn title_of(text: &str) -> Option<String> {
    let words = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if words.is_empty() {
        return None;
    }
    Some(if words.chars().count() > BOOKMARK_TITLE_CHARS {
        words.chars().take(BOOKMARK_TITLE_CHARS - 1).collect::<String>().trim_end().to_string() + "…"
    } else {
        words
    })
}

#[cfg(test)]
mod title_tests {
    use super::{BOOKMARK_TITLE_CHARS, title_of};

    #[test]
    fn a_bookmark_title_is_the_selection_on_one_line_and_cut_short() {
        assert_eq!(title_of("  Chapter\n 3:\tThe  fox \r\n").as_deref(), Some("Chapter 3: The fox"));
        assert_eq!(title_of(" \n\t "), None);
        // Cut at a character, not a byte: "ä" is two bytes.
        let long = "ä".repeat(BOOKMARK_TITLE_CHARS + 10);
        let title = title_of(&long).unwrap();
        assert_eq!(title.chars().count(), BOOKMARK_TITLE_CHARS);
        assert!(title.ends_with('…'));
        let exact = "b".repeat(BOOKMARK_TITLE_CHARS);
        assert_eq!(title_of(&exact).as_deref(), Some(exact.as_str()), "a title that fits is kept whole");
    }
}
