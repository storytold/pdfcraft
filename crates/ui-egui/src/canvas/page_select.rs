//! The Select tool on the page view: the framework's text selection, and the pick of a picture under a click.

use craft::behavior::{CancelReason, Cursor};
use craft::pages::ui::{PageTexts, PlacedPage, TextSelectState, TextSelectTool, TextSelection};
use craft::ui::canvas::{CanvasTool, ToolAction, ToolContext, ToolKey, ToolPath, ToolPointer};
use craft::ui::primitives::Draw;
use pdfcraft_render::DocInfo;

use crate::canvas::PageXform;
use crate::canvas::image_pick::PickedImage;

/// What a press on the page sets or lets go: the click whose picture is still being looked for, the picture found,
/// and the selection whose action bar Copy closed.
pub(crate) struct PickSlots<'a> {
    pending: &'a mut Option<(usize, [f64; 2], u64)>,
    picked: &'a mut Option<PickedImage>,
    dismissed: &'a mut Option<TextSelection>,
}

impl<'a> PickSlots<'a> {
    /// The slots over the view's `pending_pick`, `picked_image` and `actions_dismissed`.
    pub(crate) fn new(
        pending: &'a mut Option<(usize, [f64; 2], u64)>,
        picked: &'a mut Option<PickedImage>,
        dismissed: &'a mut Option<TextSelection>,
    ) -> Self {
        Self { pending, picked, dismissed }
    }

    fn let_go(&mut self) {
        *self.picked = None;
        *self.pending = None;
    }

    fn held(&self) -> bool {
        self.picked.is_some() || self.pending.is_some()
    }
}

/// Acrobat's Select tool: [`TextSelectTool`] for the text, plus the picture a click picks (a press nobody's part took
/// reaches the tool). Built per pass over the disjoint fields of the view, so the text layers can be borrowed beside it.
///
/// The pick is set by the press and let go by the first `drag` (the host calls it only past the click distance), so a
/// click keeps it and a drag, a double click and Escape drop it, with nothing carried from one pass to the next in the
/// tool.
pub(crate) struct PageSelectTool<'a> {
    text: TextSelectTool<'a>,
    info: &'a DocInfo,
    picks: PickSlots<'a>,
    xforms: &'a [(usize, PageXform)],
    generation: u64,
    /// Cap multi-click selection at a word (double-click): markup tools mark a double-clicked word
    /// without letting further rapid clicks widen to line/page selections.
    cap_clicks: bool,
    /// Whether `drag` ran in the active gesture: a plain single click without a drag clears the
    /// single-glyph press anchor on release, as production's text selection did.
    dragged: &'a mut bool,
    column: bool,
}

impl<'a> PageSelectTool<'a> {
    /// The tool over the placed `pages` and their `xforms` (the pages as the view shows them), reading text from
    /// `texts`, keeping its gesture in `text`; `generation` is the document's edit generation, which a pick carries.
    // PLAN: the decisions give no `info`; the view-to-user mapping of the pick needs it (pdf-canvas-w9's arm passes it,
    // and the two pick fields as `PickSlots::new(&mut pending_pick, &mut picked_image)`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        pages: &'a [PlacedPage],
        texts: &'a dyn PageTexts,
        text: &'a mut TextSelectState,
        info: &'a DocInfo,
        picks: PickSlots<'a>,
        xforms: &'a [(usize, PageXform)],
        generation: u64,
        dragged: &'a mut bool,
    ) -> Self {
        Self { text: TextSelectTool::new(pages, texts, text), info, picks, xforms, generation, cap_clicks: false, dragged, column: false }
    }

    /// The Column Select tool is active.
    pub(crate) fn column(mut self, on: bool) -> Self {
        self.text = self.text.column(on);
        self.column = on;
        self
    }

    /// Markup and redaction tools cap rapid multi-clicks at a word (clicks > 2 act as a single click).
    pub(crate) fn cap_clicks(mut self, on: bool) -> Self {
        self.cap_clicks = on;
        self
    }

    /// A press asks for the picture under it, unless it is a double click or more, which select text.
    fn pick(&mut self, pointer: &ToolPointer) {
        self.picks.let_go();
        if pointer.clicks >= 2 {
            return;
        }
        let Some((page, xf)) = self.xforms.iter().find(|(_, xf)| xf.rect.contains(pointer.pos)) else { return };
        let Some(info_page) = self.info.pages.get(*page) else { return };
        let (vx, vy) = xf.screen_to_view(pointer.pos);
        let u = info_page.view_to_user(vx, vy);
        *self.picks.pending = Some((*page, [f64::from(u[0]), f64::from(u[1])], self.generation));
    }
}

impl CanvasTool for PageSelectTool<'_> {
    fn cursor(&self, cx: &ToolContext<'_>) -> Cursor {
        self.text.cursor(cx)
    }

    fn press(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) -> ToolAction {
        // A drag replaces the selection without passing through none, so a re-drag over words whose bar Copy closed
        // would keep the bar hidden.
        *self.picks.dismissed = None;
        *self.dragged = false;
        let capped = if self.cap_clicks && pointer.clicks > 2 { pointer.clicks(1) } else { *pointer };
        let action = self.text.press(&capped, cx);
        self.pick(&capped);
        if action == ToolAction::Done && self.picks.pending.is_some() { ToolAction::Claimed } else { action }
    }

    fn drag(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        *self.dragged = true;
        self.picks.let_go();
        self.text.drag(pointer, cx);
    }

    fn release(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        let clicks = if self.cap_clicks && pointer.clicks > 2 { 1 } else { pointer.clicks };
        self.text.release(pointer, cx);
        if !*self.dragged && clicks < 2 && !pointer.mods.shift {
            self.text.key(&ToolKey::Cancel, cx);
        }
        *self.dragged = false;
    }

    fn cancel(&mut self, reason: CancelReason, cx: &mut ToolContext<'_>) {
        *self.dragged = false;
        self.picks.let_go();
        self.text.cancel(reason, cx);
    }

    /// Escape lets go of the picked picture, or of a click whose picture is not known yet, before it clears the text; clearing the text takes it.
    fn key(&mut self, key: &ToolKey, cx: &mut ToolContext<'_>) -> bool {
        if matches!(key, ToolKey::Cancel) && self.picks.held() {
            self.picks.let_go();
            return true;
        }
        // Esc clears a text selection (Select All too) and counts as taken, so full screen stays until the next Esc
        // (#5302, L-318): the text tool answers true only when it had a selection to clear.
        self.text.key(key, cx)
    }

    fn paint(&self, draw: &Draw<'_>, cx: &ToolContext<'_>) {
        self.text.paint(draw, cx);
    }

    fn command_form(&self) -> ToolPath {
        self.text.command_form()
    }
}
