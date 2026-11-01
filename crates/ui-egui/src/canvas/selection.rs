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

/// A text selection tried where there is no text: on `page`, starting at `at` in view space, at
/// input time `since` (a press after it closes the bar).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct NoText {
    pub(super) page: usize,
    pub(super) at: [f32; 2],
    pub(super) since: f64,
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

    /// Glyphs in `page`'s text layer, once it is loaded (used by UI tests and automation).
    pub fn text_glyph_count(&self, page: usize) -> Option<usize> {
        self.texts.get(&page).map(|t| t.glyphs.len())
    }

    /// `page`'s text layer as plain text, once it is loaded (used by UI tests and automation).
    pub fn page_plain_text(&self, page: usize) -> Option<String> {
        self.texts.get(&page).map(|t| t.plain_text())
    }

    /// The page whose bar offers Recognize text, after a text selection there found no text.
    pub fn no_text_page(&self) -> Option<usize> {
        self.no_text.map(|n| n.page)
    }

    /// The page being recognised in the background (1-based) and the pages to read, while that
    /// runs (used by UI tests and automation).
    pub fn auto_ocr_progress(&self) -> Option<(usize, usize)> {
        self.auto_ocr.progress()
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
                        if ui.add_enabled(can_comment, egui::Button::new(label)).clicked() {
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

/// The bar shown after a text selection is tried on a page with no text: such a page is most
/// likely a scan, whose words can be selected and found once Recognize text has added a text
/// layer. It closes on the next press outside it, on Escape, and once the page has text. Returns
/// the page to recognize when its button is clicked.
pub(super) fn no_text_bar(ui: &mut egui::Ui, viewport: Rect, view: &mut DocView, can_recognize: bool) -> Option<usize> {
    let hint = view.no_text?;
    let id = egui::Id::unique(("no-text-actions", view.id.0));
    let bar = ui.ctx().memory(|m| m.area_rect(id));
    let (pressed_elsewhere, escape) = ui.input(|i| {
        let elsewhere =
            i.pointer.any_pressed() && i.time > hint.since && !i.pointer.press_origin().is_some_and(|p| bar.is_some_and(|r| r.contains(p)));
        (elsewhere, i.key_pressed(egui::Key::Escape))
    });
    let has_text = view.texts.get(&hint.page).is_some_and(|t| !t.is_empty());
    if pressed_elsewhere || escape || has_text {
        view.no_text = None;
        return None;
    }
    let xf = view.page_xform(hint.page)?;
    let [x, y] = hint.at;
    let at = xf.view_rect([x, y, x, y]).left_top();
    if !viewport.contains(at) {
        return None;
    }
    let left = at.x.clamp(viewport.left() + 8.0, (viewport.right() - 420.0).max(viewport.left() + 8.0));
    let top = if at.y + 96.0 < viewport.bottom() { at.y + 12.0 } else { (at.y - 96.0).max(viewport.top() + 6.0) };
    let mut recognize = false;
    egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(pos2(left, top)).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_max_width(400.0);
            ui.label(tl!("There is no selectable text on this page. If it is a scan or a picture of text, recognize its text to select it."));
            recognize = ui.add_enabled(can_recognize, egui::Button::new(tl!("Recognize text…"))).clicked();
        });
    });
    if recognize {
        view.no_text = None;
    }
    recognize.then_some(hint.page)
}
