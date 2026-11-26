//! The Crop tool (Acrobat: Edit ▸ Crop pages): drag a rectangle on a page to crop it to that
//! rectangle; double-click a page to open Set Page Boxes for it.

use craft::behavior::{CancelReason, Cursor, CursorIcon};
use craft::pages::ui::PlacedPage;
use craft::ui::canvas::{CanvasTool, CropShield, ToolAction, ToolContext, ToolPath, ToolPointer, paint_outside};
use craft::ui::primitives::Draw;
use craft::ui::style::DataColor;
use pdfcraft_engine::{BoxSpec, Edit, PageBox};
use pdfcraft_render::DocInfo;

use super::page_box_drag::{PageBoxDrag, page_under};
use crate::canvas::PageXform;

/// The smallest crop rectangle, in screen points on either side (the gesture constant of L-159).
const MIN_SIDE: f32 = 4.0;

/// A finished crop drag: the page and the rectangle in page view points [x0, y0, x1, y1].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CropResult {
    pub(crate) page: usize,
    pub(crate) view_rect: [f32; 4],
}

/// The edit that crops `done.page` to its rectangle, in user space.
pub(crate) fn crop_edit(info: &DocInfo, done: &CropResult) -> Option<Edit> {
    let page = info.pages.get(done.page)?;
    let [x0, y0, x1, y1] = done.view_rect;
    let (a, b) = (to_user(page, x0, y0), to_user(page, x1, y1));
    let rect = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
    Some(Edit::SetPageBox { pages: vec![done.page], which: PageBox::Crop, spec: BoxSpec::Rect(rect) })
}

fn to_user(page: &pdfcraft_render::PageInfo, vx: f32, vy: f32) -> [f64; 2] {
    let u = page.view_to_user(vx, vy);
    [f64::from(u[0]), f64::from(u[1])]
}

/// The Crop tool, built per pass over the placed pages.
pub(crate) struct CropTool<'a> {
    allowed: bool,
    placed: Vec<PlacedPage>,
    /// Per placed page: its unrotated view-space size.
    sizes: Vec<(usize, (f32, f32))>,
    rotation: u16,
    /// The session's drag, kept from the press to the release.
    drag: &'a mut PageBoxDrag,
    done: Option<CropResult>,
    open_boxes: Option<usize>,
}

impl<'a> CropTool<'a> {
    // PLAN: pdf-canvas-w9 asked for `CropTool::new(allowed)`; the tool needs the placed pages to know which page a press is on.
    /// A tool over `placed`, with its rectangle in `drag`; `allowed` is false when page editing is forbidden.
    pub(crate) fn new(
        allowed: bool,
        drag: &'a mut PageBoxDrag,
        placed: &[PlacedPage],
        rotation: u16,
        page_size_of: impl Fn(usize) -> (f32, f32),
    ) -> Self {
        let sizes = placed.iter().map(|p| (p.page, page_size_of(p.page))).collect();
        Self { allowed, placed: placed.to_vec(), sizes, rotation, drag, done: None, open_boxes: None }
    }

    /// The crop the last release made, once.
    pub(crate) fn take_result(&mut self) -> Option<CropResult> {
        self.done.take()
    }

    /// The page a double click asked Set Page Boxes for, once.
    pub(crate) fn take_open_boxes(&mut self) -> Option<usize> {
        self.open_boxes.take()
    }

    fn xform(&self, placed: &PlacedPage) -> Option<PageXform> {
        let size = self.sizes.iter().find(|(p, _)| *p == placed.page).map(|(_, s)| *s)?;
        Some(PageXform::of(placed, self.rotation, size))
    }
}

impl CanvasTool for CropTool<'_> {
    fn press(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) -> ToolAction {
        if !self.allowed {
            return ToolAction::Ignored;
        }
        let Some(placed) = page_under(&self.placed, pointer.pos) else { return ToolAction::Ignored };
        if pointer.clicks == 2 {
            self.open_boxes = Some(placed.page);
            return ToolAction::Done;
        }
        if self.drag.press(placed, pointer.pos) { ToolAction::Claimed } else { ToolAction::Ignored }
    }

    fn drag(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {
        self.drag.drag(pointer.pos);
    }

    fn release(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {
        self.drag.drag(pointer.pos);
        let Some((page, rect)) = self.drag.release(MIN_SIDE) else { return };
        let Some(placed) = self.placed.iter().find(|p| p.page == page) else { return };
        let Some(xf) = self.xform(placed) else { return };
        let (a, b) = (xf.screen_to_view(rect.left_top()), xf.screen_to_view(rect.right_bottom()));
        self.done = Some(CropResult { page, view_rect: [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)] });
    }

    fn cancel(&mut self, _reason: CancelReason, _cx: &mut ToolContext<'_>) {
        self.drag.cancel();
    }

    fn paint(&self, draw: &Draw<'_>, cx: &ToolContext<'_>) {
        let Some((page, rect)) = self.drag.current() else { return };
        if let (Some(space), Some(style)) = (cx.space(), cx.style()) {
            let canvas = style.color(style.tokens.color.surface.canvas).egui();
            if let Some(fill) = CropShield::default().fill_on(&space, canvas, true) {
                let hole = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
                let quad = [page.left_top(), page.right_top(), page.right_bottom(), page.left_bottom()];
                paint_outside(draw, quad, &hole, DataColor::new(fill));
            }
        }
        self.drag.paint(cx);
    }

    fn cursor(&self, cx: &ToolContext<'_>) -> Cursor {
        let over = cx.pointer.is_some_and(|p| page_under(&self.placed, p.pos).is_some());
        match (over, self.allowed) {
            (true, true) => Cursor::System(CursorIcon::Crosshair),
            (true, false) => Cursor::System(CursorIcon::NotAllowed),
            _ => Cursor::System(CursorIcon::Default),
        }
    }

    fn command_form(&self) -> ToolPath {
        ToolPath::Command("the Set Page Boxes dialog")
    }
}
