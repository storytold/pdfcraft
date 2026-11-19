//! The Digitally sign and Certify (visible signature) page tool: drag a rectangle on a page.

use craft::behavior::{CancelReason, Cursor, CursorIcon};
use craft::pages::ui::PlacedPage;
use craft::ui::canvas::{CanvasTool, ToolAction, ToolContext, ToolPath, ToolPointer};
use craft::ui::primitives::Draw;
use egui::Pos2;
use pdfcraft_render::DocInfo;

use crate::canvas::{PageBoxDrag, PageXform};

/// The least side, in screen points, of a drawn signature area.
const LEAST_SIDE: f32 = 8.0;

/// Drags a signature rectangle on one page. The rectangle of a finished drag is taken with
/// [`SignAreaTool::finished`], in user space of its page.
pub(crate) struct SignAreaTool<'a> {
    /// The session's drag, kept from the press to the release.
    drag: &'a mut PageBoxDrag,
    /// Certify (DocMDP) rather than sign: names the command that takes the same numbers.
    certify: bool,
    placed: &'a [PlacedPage],
    /// The view's rotation, in degrees clockwise.
    rotation: u16,
    info: &'a DocInfo,
    drawn: Option<(usize, [f64; 4])>,
}

impl<'a> SignAreaTool<'a> {
    /// A tool over the placed pages that draws the area for a certification when `certify`, for a
    /// signature otherwise, with its rectangle in `drag`.
    pub(crate) fn new(certify: bool, drag: &'a mut PageBoxDrag, placed: &'a [PlacedPage], rotation: u16, info: &'a DocInfo) -> Self {
        Self { drag, certify, placed, rotation, info, drawn: None }
    }

    /// The (page, user-space rectangle) a drag finished with since the last call, once.
    pub(crate) fn finished(&mut self) -> Option<(usize, [f64; 4])> {
        self.drawn.take()
    }

    fn placed_at(&self, p: Pos2) -> Option<&'a PlacedPage> {
        self.placed.iter().find(|placed| placed.rect.contains(p))
    }

    /// A screen point of `placed` in the page's user space.
    fn to_user(&self, placed: &PlacedPage, p: Pos2) -> Option<[f64; 2]> {
        let page = self.info.pages.get(placed.page)?;
        let (vx, vy) = PageXform::of(placed, self.rotation, (page.width, page.height)).screen_to_view(p);
        let u = page.view_to_user(vx, vy);
        Some([f64::from(u[0]), f64::from(u[1])])
    }
}

impl CanvasTool for SignAreaTool<'_> {
    fn press(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) -> ToolAction {
        match self.placed_at(pointer.pos) {
            Some(placed) if self.drag.press(placed, pointer.pos) => ToolAction::Claimed,
            _ => ToolAction::Ignored,
        }
    }

    fn drag(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {
        self.drag.drag(pointer.pos);
    }

    fn release(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {
        self.drag.drag(pointer.pos);
        // A click with no drag leaves nothing drawn: `release` gives none under the least side.
        let Some((page, rect)) = self.drag.release(LEAST_SIDE) else { return };
        let Some(placed) = self.placed.iter().find(|p| p.page == page) else { return };
        let (Some(a), Some(b)) = (self.to_user(placed, rect.left_top()), self.to_user(placed, rect.right_bottom())) else { return };
        self.drawn = Some((page, [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]));
    }

    fn cancel(&mut self, _reason: CancelReason, _cx: &mut ToolContext<'_>) {
        self.drag.cancel();
    }

    fn paint(&self, _draw: &Draw<'_>, cx: &ToolContext<'_>) {
        self.drag.paint(cx);
    }

    fn cursor(&self, cx: &ToolContext<'_>) -> Cursor {
        match cx.pointer {
            Some(p) if self.placed_at(p.pos).is_some() => Cursor::System(CursorIcon::Crosshair),
            _ => Cursor::System(CursorIcon::Default),
        }
    }

    fn command_form(&self) -> ToolPath {
        ToolPath::Command(if self.certify { "the Certify command" } else { "the Digitally Sign command" })
    }
}
