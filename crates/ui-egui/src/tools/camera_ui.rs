//! Scan & OCR ▸ Enhance ▸ Camera image: four corner handles on a page, dragged onto the edges of
//! a photographed document; Enhance page straightens what they enclose
//! (`Session::enhance_camera_page`, one undoable step).

// PLAN: `craft-draw`, `craft-pages` and `craft-behavior` must be dependencies of pdfcraft-ui-egui (Cargo.toml is not in this set's files).
use craft::behavior::kurbo::{Point, Rect as DocRect};
use craft::behavior::overlay::Align;
use craft::behavior::{CancelReason, Cursor, CursorIcon, IconId, PressState};
use craft::draw::ui::PerspectiveCropQuad;
use craft::pages::ui::view::{PageBody, PlacedPage};
use craft::ui::display::Text;
use craft_ui::buttons::{Button, IconButton};
use craft_ui::canvas::{CanvasTool, CropShield, ToolAction, ToolContext, ToolPath, ToolPointer};
use craft_ui::canvas_controls::TaskBar;
use craft_ui::choice::Segmented;
use craft_ui::contract::{Bind, Entry, Props as _, Show as _, clicked};
use craft_ui::primitives::Draw;
use craft_ui::style::Variant;
use craft_ui::value_drag::Slider;
use egui::Ui;

use crate::canvas::{PageSession, ViewAction};
pub use pdfcraft_engine::camera_clean::Look;

/// How far in from the page's edges the corners start, as a share of its size.
const INSET: f32 = 0.05;
/// How strongly the area outside the quadrilateral is greyed, in percent: production's page reads mid grey there.
const SHIELD_OPACITY: f32 = 43.0;
/// The slider's top.
pub const MAX_LEVEL: u8 = pdfcraft_engine::camera_clean::MAX_LEVEL;

/// The quadrilateral being placed: its page and its corners in view space (points, y down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraQuad {
    pub page: usize,
    pub corners: [[f32; 2]; 4],
}

impl CameraQuad {
    /// The page `page` (view size `w` × `h` points) inset by [`INSET`] on every side.
    pub fn inset(page: usize, w: f32, h: f32) -> Self {
        let (dx, dy) = (w * INSET, h * INSET);
        Self { page, corners: [[dx, dy], [w - dx, dy], [w - dx, h - dy], [dx, h - dy]] }
    }
}

/// The clean-up level a page is enhanced at before the slider is touched.
pub const DEFAULT_LEVEL: u8 = 50;

/// A page just enhanced: what the bar's slider needs to enhance it again at another level
/// (from the straightened photo, keeping the turns made since).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraDone {
    pub page: usize,
    /// The corners it was straightened from, in PDF user space.
    pub corners: [[f64; 2]; 4],
    /// The look and slider level chosen, and the ones the page shows.
    pub look: Look,
    pub level: u8,
    pub applied: (Look, u8),
    /// Quarter turns made since, in degrees (clockwise positive).
    pub turns: i64,
    /// The undo steps the enhancement and the turns took.
    pub steps: usize,
}

/// The page's size in view points.
pub(crate) fn page_points(placed: &PlacedPage) -> (f32, f32) {
    let scale = placed.scale.max(1e-6) as f32;
    (placed.rect.width() / scale, placed.rect.height() / scale)
}

/// The quadrilateral on its page, shown as the framework's perspective quad over the canvas: it seeds the inset corners on
/// the current page when there are none yet, follows the corners the user drags, and turns Enter into Enhance page and
/// Esc into Cancel, which it returns for the app. `current` is the view's current page. Called only while the Camera
/// image tool is open.
pub(crate) fn show_quad(ui: &mut Ui, body: &PageBody<'_>, session: &mut PageSession, allowed: bool, current: usize) -> Option<ViewAction> {
    // Once the page is enhanced, the bar's buttons take over.
    if session.camera_done.is_some() {
        return None;
    }
    if session.camera.is_none()
        && let Some(placed) = body.pages.iter().find(|p| p.page == current)
    {
        let (w, h) = page_points(placed);
        session.camera = Some(CameraQuad::inset(placed.page, w, h));
    }
    let quad = session.camera?;
    let placed = body.pages.iter().find(|p| p.page == quad.page)?;
    let to_doc = |c: [f32; 2]| body.view.to_doc(placed.to_screen(Point::new(f64::from(c[0]), f64::from(c[1]))));
    let mut doc_quad = Some(quad.corners.map(to_doc));
    let page_bounds = DocRect::from_points(body.view.to_doc(placed.rect.min), body.view.to_doc(placed.rect.max));
    let mut shield = CropShield::PERSPECTIVE;
    shield.opacity = SHIELD_OPACITY;
    let out = PerspectiveCropQuad::new(tl!("Corners"), Bind::bound(&mut doc_quad))
        .show_grid(false)
        .shield(shield)
        .bounds(page_bounds)
        .enabled(allowed)
        .show_at(ui, body.rect);
    if let Some(moved) = doc_quad {
        let to_page = |d: Point| {
            let p = placed.to_page(body.view.to_screen(d));
            [p.x as f32, p.y as f32]
        };
        session.camera = Some(CameraQuad { page: quad.page, corners: moved.map(to_page) });
    }
    if out.discard {
        Some(ViewAction::CancelCamera)
    } else if allowed && out.apply {
        Some(ViewAction::EnhanceCamera)
    } else {
        None
    }
}

/// The task bar at the top right of the document area: Enhance page and Cancel; once the page is enhanced, rotate it either
/// way, choose its look (original, colour, greyscale, black and white) and clean-up level, Crop pages, Organize pages, or
/// Done. The instructions are a notice (`ocr.enhance_camera`). Returns the action a button asked the app for.
pub(crate) fn bar(ui: &mut Ui, body: &PageBody<'_>, session: &mut PageSession, allowed: bool) -> Option<ViewAction> {
    let mut action = None;
    TaskBar::at_edge(tl!("Camera image"), Align::End).show(ui, body.rect, |ui, _| {
        // After Enhance page: put the page the right way up, trim it, or move on.
        if let Some(mut done) = session.camera_done {
            let looks = [
                Entry::new(Look::Original, tl!("Original")),
                Entry::new(Look::Colour, tl!("Colour")),
                Entry::new(Look::Greyscale, tl!("Greyscale")),
                Entry::new(Look::BlackWhite, tl!("Black and white")),
            ];
            let mut again = false;
            let picked = Segmented::new(tl!("Look"), Bind::given(done.look), &looks).enabled(allowed).show(ui).value().copied();
            if let Some(look) = picked.filter(|look| *look != done.look) {
                done.look = look;
                again = true;
            }
            let mut level = f64::from(done.level);
            // The slider's name shows beside it, as production's bar does.
            Text::new(tl!("Clean-up")).show(ui);
            let edit = Slider::new(Bind::bound(&mut level), 0.0..=f64::from(MAX_LEVEL))
                .step(1.0)
                .label(tl!("Clean-up"))
                .enabled(allowed && done.look != Look::Original)
                .show(ui);
            done.level = level.round().clamp(0.0, f64::from(MAX_LEVEL)) as u8;
            // Enhance again once the slider is let go (or moved with the keys).
            again |= edit.events.committed().is_some();
            if again && (done.look, done.level) != done.applied {
                action = Some(ViewAction::CameraLevel);
            }
            session.camera_done = Some(done);
            if clicked(&IconButton::new(IconId::new("rotate_ccw")).label(tl!("Rotate counterclockwise")).enabled(allowed).show(ui)) {
                action = Some(ViewAction::CameraRotate { clockwise: false });
            }
            if clicked(&IconButton::new(IconId::new("rotate_cw")).label(tl!("Rotate clockwise")).enabled(allowed).show(ui)) {
                action = Some(ViewAction::CameraRotate { clockwise: true });
            }
            if clicked(&Button::new(tl!("Crop pages")).variant(Variant::GHOST).show(ui)) {
                action = Some(ViewAction::CameraNext("page.crop"));
            }
            if clicked(&Button::new(tl!("Organize pages")).variant(Variant::GHOST).show(ui)) {
                action = Some(ViewAction::CameraNext("page.organize"));
            }
            if clicked(&Button::new(tl!("Done")).variant(Variant::PRIMARY).show(ui)) {
                action = Some(ViewAction::CancelCamera);
            }
            return;
        }
        let ready = allowed && session.camera.is_some();
        if clicked(&Button::new(tl!("Enhance page")).variant(Variant::PRIMARY).enabled(ready).show(ui)) {
            action = Some(ViewAction::EnhanceCamera);
        }
        if clicked(&Button::new(tl!("Cancel")).show(ui)) {
            action = Some(ViewAction::CancelCamera);
        }
    });
    action
}

/// The press rule of the Camera image tool: a press nobody holds (the quad's corners take their own) on a page without the
/// quadrilateral moves it there, as a drag from where it began or a click where it came up. The page body reads
/// [`moved_to`](Self::moved_to) after the host ran and puts the inset quad on that page.
pub(crate) struct CameraTool<'a> {
    allowed: bool,
    pages: &'a [PlacedPage],
    /// The page the press in progress went down on, read from this pass's press: the press and its release come in
    /// different passes.
    pressed: Option<usize>,
    /// The press moved past slop: it is a drag from its page, not a click.
    dragged: bool,
    moved_to: Option<usize>,
}

impl<'a> CameraTool<'a> {
    /// The tool over the pages on screen, in a pass whose press is `press`; with `allowed` off it only refuses.
    pub(crate) fn new(allowed: bool, pages: &'a [PlacedPage], press: &PressState) -> Self {
        let pressed = press.phase.start().and_then(|start| pages.iter().find(|p| p.rect.contains(start.origin))).map(|p| p.page);
        CameraTool { allowed, pages, pressed: pressed.filter(|_| allowed), dragged: false, moved_to: None }
    }

    /// The page the quadrilateral moves to, once a press asked for it.
    pub(crate) fn moved_to(&self) -> Option<usize> {
        self.moved_to
    }

    fn page_at(&self, pointer: &ToolPointer) -> Option<usize> {
        self.pages.iter().find(|p| p.rect.contains(pointer.pos)).map(|p| p.page)
    }
}

impl CanvasTool for CameraTool<'_> {
    fn press(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) -> ToolAction {
        self.dragged = false;
        self.pressed = self.page_at(pointer).filter(|_| self.allowed);
        if self.pressed.is_some() { ToolAction::Claimed } else { ToolAction::Ignored }
    }

    fn drag(&mut self, _pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {
        if !self.dragged {
            self.dragged = true;
            self.moved_to = self.pressed;
        }
    }

    fn release(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {
        // A click is located where it came up; one released off its page moves nothing.
        if !self.dragged && self.page_at(pointer) == self.pressed {
            self.moved_to = self.pressed;
        }
        self.pressed = None;
    }

    fn cancel(&mut self, _reason: CancelReason, _cx: &mut ToolContext<'_>) {
        self.pressed = None;
        self.dragged = false;
    }

    fn paint(&self, _draw: &Draw<'_>, _cx: &ToolContext<'_>) {}

    fn command_form(&self) -> ToolPath {
        ToolPath::Command("the Enhance page command")
    }

    fn cursor(&self, cx: &ToolContext<'_>) -> Cursor {
        let over_page = cx.pointer.is_some_and(|p| self.pages.iter().any(|page| page.rect.contains(p.pos)));
        if over_page && !self.allowed { CursorIcon::NotAllowed.into() } else { CursorIcon::Default.into() }
    }
}
