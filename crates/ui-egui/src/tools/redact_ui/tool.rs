//! The Redact tool on the pages: a press over text selects it (the selection becomes marks when
//! it ends), a press on free page space draws a box that becomes one mark.

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
use crate::canvas::{DocView, PageBoxDrag, ToolMarks};

/// The smallest box, in screen points on each side, that counts as a mark.
const LEAST_BOX: f32 = 3.0;

/// What the pointer's press is doing.
enum Gesture {
    Idle,
    /// A text selection the select tool runs.
    Text,
    /// A box being drawn.
    Box,
}

/// What a finished gesture asks the document for.
pub(crate) enum RedactResult {
    /// A box on `page`, as a user-space quad.
    Area { page: usize, quad: [f64; 8] },
    /// A text selection that ended; the selection holds what to mark.
    Text { page: usize },
}

/// The Redact tool for one pass.
pub(crate) struct RedactTool<'a> {
    select: PageSelectTool<'a>,
    drag: PageBoxDrag,
    placed: &'a [PlacedPage],
    info: &'a DocInfo,
    over_text: Box<dyn Fn(Pos2) -> bool + 'a>,
    gesture: Gesture,
    text_page: Option<usize>,
    finished: Option<RedactResult>,
}

impl<'a> RedactTool<'a> {
    /// The tool over the placed pages, with the select tool that runs text selections and the
    /// test of whether a screen point is over text (the same one the I-beam uses).
    pub(crate) fn new(select: PageSelectTool<'a>, placed: &'a [PlacedPage], info: &'a DocInfo, over_text: impl Fn(Pos2) -> bool + 'a) -> Self {
        Self {
            select,
            drag: PageBoxDrag::default(),
            placed,
            info,
            over_text: Box::new(over_text),
            gesture: Gesture::Idle,
            text_page: None,
            finished: None,
        }
    }

    /// After the host ran: hands a finished gesture to the view as a pending redaction, or as a
    /// refusal when a selected page's text could not be read.
    pub(crate) fn finished(self, view: &mut DocView) {
        match self.finished {
            Some(RedactResult::Area { page, quad }) => {
                view.pending_redaction = Some((QuickTool::Redact, vec![(page, vec![quad])], page));
            }
            // Across pages: a mark on each page it covers, once all are read; refused when one of
            // them could not be read, rather than leave it unmarked.
            Some(RedactResult::Text { page }) => match view.tool_marks(QuickTool::Redact, true, Some(page), true, self.info) {
                ToolMarks::Marks(marks, head) => {
                    view.clear_selection();
                    view.pending_redaction = Some((QuickTool::Redact, marks, head));
                }
                ToolMarks::Unreadable => view.redaction_refused = true,
                ToolMarks::None => {}
            },
            None => {}
        }
    }

    fn placed_at(&self, p: Pos2) -> Option<&'a PlacedPage> {
        self.placed.iter().find(|placed| placed.rect.contains(p))
    }

    /// A screen point of `placed` in the page's user space.
    fn to_user(&self, placed: &PlacedPage, p: Pos2) -> Option<[f64; 2]> {
        let xf = PageXform::of(placed, self.info);
        let (vx, vy) = xf.screen_to_view(p);
        let u = self.info.pages.get(placed.page)?.view_to_user(vx, vy);
        Some([u[0] as f64, u[1] as f64])
    }
}

impl CanvasTool for RedactTool<'_> {
    fn each_pass(&mut self, cx: &mut ToolContext<'_>) {
        self.select.each_pass(cx);
    }

    fn press(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) -> ToolAction {
        let Some(placed) = self.placed_at(pointer.pos) else { return ToolAction::Ignored };
        if (self.over_text)(pointer.pos) {
            let action = self.select.press(pointer, cx);
            match action {
                ToolAction::Claimed => {
                    self.gesture = Gesture::Text;
                    self.text_page = Some(placed.page);
                }
                // A double click selects a word whole: it ends the selection at once.
                ToolAction::Done => self.finished = Some(RedactResult::Text { page: placed.page }),
                _ => {}
            }
            return action;
        }
        self.drag.press(placed, pointer.pos);
        self.gesture = Gesture::Box;
        ToolAction::Claimed
    }

    fn drag(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        match self.gesture {
            Gesture::Text => self.select.drag(pointer, cx),
            Gesture::Box => self.drag.drag(pointer.pos),
            Gesture::Idle => {}
        }
    }

    fn release(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        match std::mem::replace(&mut self.gesture, Gesture::Idle) {
            Gesture::Text => {
                self.select.release(pointer, cx);
                if let Some(page) = self.text_page.take() {
                    self.finished = Some(RedactResult::Text { page });
                }
            }
            Gesture::Box => {
                self.drag.drag(pointer.pos);
                // A click with no drag marks nothing: `release` gives no box below the least size.
                if let Some((page, rect)) = self.drag.release(LEAST_BOX)
                    && let Some(placed) = self.placed.iter().find(|p| p.page == page)
                    && let (Some(a), Some(b)) = (self.to_user(placed, rect.min), self.to_user(placed, rect.max))
                {
                    self.finished = Some(RedactResult::Area { page, quad: rect_quad([a[0], a[1], b[0], b[1]]) });
                }
            }
            Gesture::Idle => {}
        }
    }

    fn hover(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        self.select.hover(pointer, cx);
    }

    fn cancel(&mut self, reason: CancelReason, cx: &mut ToolContext<'_>) {
        if matches!(std::mem::replace(&mut self.gesture, Gesture::Idle), Gesture::Text) {
            self.select.cancel(reason, cx);
        }
        self.text_page = None;
        self.drag = PageBoxDrag::default();
    }

    fn paint(&self, draw: &Draw<'_>, cx: &ToolContext<'_>) {
        self.select.paint(draw, cx);
        // PLAN: assumed `PageBoxDrag::paint(&self, &ToolContext)` draws the box with marching ants (pdf-canvas-w18)
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
