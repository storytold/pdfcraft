//! Shared text-or-area marking tool (Redact and Highlight, L-262): a press over text selects it,
//! a press on free page space draws a box that becomes one mark.

use craft::behavior::{CancelReason, Cursor, CursorIcon};
use craft::pages::ui::PlacedPage;
use craft::ui::canvas::{CanvasTool, ToolAction, ToolContext, ToolPath, ToolPointer};
use craft::ui::primitives::Draw;
use egui::Pos2;
use pdfcraft_engine::rect_quad;
use pdfcraft_render::DocInfo;

use crate::QuickTool;
use crate::canvas::PageXform;
use crate::canvas::page_select::PageSelectTool;
use crate::canvas::{PageBoxDrag, PageGeometry, PageSession, ToolMarks};

/// The smallest box, in screen points on each side, that counts as a mark.
const LEAST_BOX: f32 = 3.0;

/// What the pointer's press is doing. The tool is built each pass, so the session holds this from the press to the
/// release.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum AreaMarkGesture {
    #[default]
    Idle,
    /// A text selection the select tool runs, begun on this page.
    Text { page: usize },
    /// A box being drawn off text.
    Box,
}

/// What a finished gesture asks the document for.
pub(crate) enum AreaMarkResult {
    /// A box on `page`, as a user-space quad, made by `made_by`.
    Area { made_by: QuickTool, page: usize, quad: [f64; 8] },
    /// A text selection that ended for `made_by`; the selection holds what to mark.
    Text { made_by: QuickTool, page: usize },
}

impl AreaMarkResult {
    /// Hands the finished gesture to the session as pending marks, or as a refusal when a
    /// selected page's text could not be read; `geo` places the pages of the pass.
    pub(crate) fn land(self, session: &mut PageSession, info: &DocInfo, geo: PageGeometry<'_>) {
        match self {
            Self::Area { made_by, page, quad } => {
                session.pending_marks = Some((made_by, vec![(page, vec![quad])], page));
            }
            // Across pages: a mark on each page it covers, once all are read; refused when one of
            // them could not be read, rather than leave it unmarked.
            Self::Text { made_by, page } => match session.tool_marks(made_by, true, Some(page), made_by == QuickTool::Redact, info, geo) {
                ToolMarks::Marks(marks, head) => {
                    session.clear_selection();
                    session.pending_marks = Some((made_by, marks, head));
                }
                ToolMarks::Unreadable => session.redaction_refused = true,
                ToolMarks::None => {}
            },
        }
    }
}

/// The shared text-or-box marking tool for one pass (Redact and Highlight off text).
pub(crate) struct AreaMarkTool<'a> {
    made_by: QuickTool,
    select: PageSelectTool<'a>,
    drag: &'a mut PageBoxDrag,
    gesture: &'a mut AreaMarkGesture,
    placed: &'a [PlacedPage],
    /// The view's rotation, in degrees clockwise.
    rotation: u16,
    info: &'a DocInfo,
    over_text: Box<dyn Fn(Pos2) -> bool + 'a>,
    finished: &'a mut Option<AreaMarkResult>,
}

impl<'a> AreaMarkTool<'a> {
    /// The tool over the placed pages, with the select tool that runs text selections, the session's box drag and
    /// gesture, the test of whether a screen point is over text (the same one the I-beam uses), and the slot where
    /// a finished gesture lands.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        made_by: QuickTool,
        select: PageSelectTool<'a>,
        drag: &'a mut PageBoxDrag,
        gesture: &'a mut AreaMarkGesture,
        placed: &'a [PlacedPage],
        rotation: u16,
        info: &'a DocInfo,
        over_text: impl Fn(Pos2) -> bool + 'a,
        finished: &'a mut Option<AreaMarkResult>,
    ) -> Self {
        Self { made_by, select, drag, gesture, placed, rotation, info, over_text: Box::new(over_text), finished }
    }

    fn placed_at(&self, p: Pos2) -> Option<&'a PlacedPage> {
        self.placed.iter().find(|placed| placed.rect.contains(p))
    }

    /// A screen point of `placed` in the page's user space.
    fn to_user(&self, placed: &PlacedPage, p: Pos2) -> Option<[f64; 2]> {
        let page = self.info.pages.get(placed.page)?;
        let xf = PageXform::of(placed, self.rotation, (page.width, page.height));
        let (vx, vy) = xf.screen_to_view(p);
        let u = page.view_to_user(vx, vy);
        Some([u[0] as f64, u[1] as f64])
    }
}

impl CanvasTool for AreaMarkTool<'_> {
    fn each_pass(&mut self, cx: &mut ToolContext<'_>) {
        self.select.each_pass(cx);
    }

    fn press(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) -> ToolAction {
        let Some(placed) = self.placed_at(pointer.pos) else { return ToolAction::Ignored };
        if (self.over_text)(pointer.pos) {
            let action = self.select.press(pointer, cx);
            match action {
                ToolAction::Claimed => *self.gesture = AreaMarkGesture::Text { page: placed.page },
                // A double click selects a word whole: Redact ends the selection at once; Highlight follows
                // up through `page_after_text`.
                ToolAction::Done if self.made_by == QuickTool::Redact => {
                    *self.finished = Some(AreaMarkResult::Text { made_by: self.made_by, page: placed.page });
                }
                _ => {}
            }
            return action;
        }
        self.drag.press(placed, pointer.pos);
        *self.gesture = AreaMarkGesture::Box;
        ToolAction::Claimed
    }

    fn drag(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        match *self.gesture {
            AreaMarkGesture::Text { .. } => self.select.drag(pointer, cx),
            AreaMarkGesture::Box => self.drag.drag(pointer.pos),
            AreaMarkGesture::Idle => {}
        }
    }

    fn release(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        match std::mem::take(self.gesture) {
            AreaMarkGesture::Text { page } => {
                self.select.release(pointer, cx);
                if self.made_by == QuickTool::Redact {
                    *self.finished = Some(AreaMarkResult::Text { made_by: self.made_by, page });
                }
            }
            AreaMarkGesture::Box => {
                self.drag.drag(pointer.pos);
                // A click with no drag marks nothing: `release` gives no box below the least size.
                if let Some((page, rect)) = self.drag.release(LEAST_BOX)
                    && let Some(placed) = self.placed.iter().find(|p| p.page == page)
                    && let (Some(a), Some(b)) = (self.to_user(placed, rect.min), self.to_user(placed, rect.max))
                {
                    *self.finished = Some(AreaMarkResult::Area { made_by: self.made_by, page, quad: rect_quad([a[0], a[1], b[0], b[1]]) });
                }
            }
            AreaMarkGesture::Idle => {}
        }
    }

    fn hover(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        self.select.hover(pointer, cx);
    }

    fn cancel(&mut self, reason: CancelReason, cx: &mut ToolContext<'_>) {
        if matches!(std::mem::take(self.gesture), AreaMarkGesture::Text { .. }) {
            self.select.cancel(reason, cx);
        }
        *self.drag = PageBoxDrag::default();
    }

    fn paint(&self, draw: &Draw<'_>, cx: &ToolContext<'_>) {
        self.select.paint(draw, cx);
        self.drag.paint(cx);
    }

    fn command_form(&self) -> ToolPath {
        ToolPath::Command("the Mark Page Range command")
    }

    fn cursor(&self, cx: &ToolContext<'_>) -> Cursor {
        match cx.pointer {
            Some(p) if self.placed_at(p.pos).is_some() && !(self.over_text)(p.pos) => Cursor::System(CursorIcon::Crosshair),
            _ => self.select.cursor(cx),
        }
    }
}
