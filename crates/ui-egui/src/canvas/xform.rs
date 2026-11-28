use craft::pages::ui::PlacedPage;
use egui::{Color32, Pos2, Rect, pos2};
use pdfcraft_render::DocInfo;

use super::{DocView, PageSession};

/// What the page body's functions read from the view besides the [`PageSession`]: every page's size, the view's
/// rotation and the document area on screen. The page view holds the view state while the body runs, so the body
/// reads these values before and hands them on.
#[derive(Clone, Copy)]
pub(crate) struct PageGeometry<'a> {
    /// Every page's size in view space, by page index, unturned by the view (`DocView::page_sizes`).
    pub(crate) sizes: &'a [(f32, f32)],
    /// The view's rotation, in degrees clockwise.
    pub(crate) rotation: u16,
    /// The document area on screen.
    pub(crate) viewport: Rect,
}

impl PageGeometry<'_> {
    /// The transform of a placed page, with the view's rotation and the page's size as the view reads it.
    pub(crate) fn xform_of(&self, placed: &PlacedPage) -> PageXform {
        // A page the list no longer has (an edit since the pass) maps as a unit page; `of` keeps the size positive.
        let size = self.sizes.get(placed.page).copied().unwrap_or((1.0, 1.0));
        PageXform::of(placed, self.rotation, size)
    }
}

impl PageSession {
    /// Where `page` is drawn in the last pass (this pass's while the page body runs), with its coordinate mapping.
    pub(crate) fn page_xform(&self, page: usize, geo: PageGeometry<'_>) -> Option<PageXform> {
        self.placed.iter().find(|p| p.page == page).map(|p| geo.xform_of(p))
    }

    /// Where the pages are on screen in the last pass, with their transforms.
    pub(crate) fn page_xforms(&self, geo: PageGeometry<'_>) -> Vec<(usize, PageXform)> {
        self.placed.iter().map(|p| (p.page, geo.xform_of(p))).collect()
    }

    /// The page drawn under the screen point `at` in the last pass, and the point in that page's user space.
    pub(crate) fn user_point_at(&self, geo: PageGeometry<'_>, info: &DocInfo, at: Pos2) -> Option<(usize, [f64; 2])> {
        let (page, xf) = self.page_xforms(geo).into_iter().find(|(_, xf)| xf.rect.contains(at))?;
        let (vx, vy) = xf.screen_to_view(at);
        let u = info.pages.get(page)?.view_to_user(vx, vy);
        Some((page, [f64::from(u[0]), f64::from(u[1])]))
    }
}

impl DocView {
    /// The page sizes, rotation and document area of the last pass, as the page body's functions read them.
    pub(crate) fn geometry(&self) -> PageGeometry<'_> {
        PageGeometry { sizes: &self.page_sizes, rotation: self.view.rotation, viewport: self.viewport }
    }

    /// Where `page` is drawn on screen in the last pass (`None` when it was not on screen).
    pub fn page_screen_rect(&self, page: usize) -> Option<Rect> {
        self.session.placed.iter().find(|p| p.page == page).map(|p| p.rect)
    }

    /// Whether the page full screen shows is wholly in view: no transition playing or waiting for
    /// its raster, so a click on it reaches what is on it. Always true outside full screen.
    pub fn presentation_settled(&self) -> bool {
        self.presentation.is_none() || self.session.presenter.settled_on(self.view.current)
    }

    /// The document area on screen in the last pass.
    pub fn viewport_rect(&self) -> Rect {
        self.viewport
    }

    /// Pages drawn in the last pass and their screen rectangles.
    pub fn visible_page_rects(&self) -> Vec<(usize, Rect)> {
        self.session.placed.iter().map(|p| (p.page, p.rect)).collect()
    }

    /// Where `page` is drawn in the last pass (this pass's while the page body runs), with its coordinate mapping.
    pub fn page_xform(&self, page: usize) -> Option<PageXform> {
        self.session.page_xform(page, self.geometry())
    }

    /// Where the pages are on screen in the last pass, with their transforms.
    pub(crate) fn page_xforms(&self) -> Vec<(usize, PageXform)> {
        self.session.page_xforms(self.geometry())
    }
}

/// Maps between a page's coordinate spaces and the screen, including view rotation.
///
/// *View space* is the page as rendered (points, y down, the document's own `/Rotate` applied);
/// normalised page coordinates `(u, v)` are view space divided by the page size. The view
/// rotation (View ▸ Rotate View) turns the page clockwise on screen by `rot` degrees.
#[derive(Clone, Copy)]
pub struct PageXform {
    /// The page's rectangle on screen (already rotated, so width/height may be swapped).
    pub rect: Rect,
    pub rot: u16,
    /// Page size in view space (unrotated by the view).
    pub pw: f32,
    pub ph: f32,
}

impl PageXform {
    /// The transform of `placed`, a page the page view placed: its screen rectangle, the view's `rotation` (degrees
    /// clockwise) and the page's view-space `size` (width, height in points, before the view's rotation). The page
    /// view places the page already turned, so a 600 × 800 page turned 90° has a rectangle 800 wide and 600 high,
    /// and [`zoom`](Self::zoom) reads its height over 600.
    pub fn of(placed: &PlacedPage, rotation: u16, size: (f32, f32)) -> PageXform {
        PageXform { rect: placed.rect, rot: rotation, pw: size.0.max(1.0), ph: size.1.max(1.0) }
    }

    /// Screen pixels per page point. The view's rotation swaps the rectangle's sides, so a page
    /// turned a quarter shows its width down the screen.
    pub(crate) fn zoom(&self) -> f32 {
        let across = if self.rot % 180 == 90 { self.rect.height() } else { self.rect.width() };
        across / self.pw.max(1.0)
    }

    /// Where the top left, top right, bottom right and bottom left of an upright picture land on screen when it is
    /// drawn in the user-space `rect` turned back by `turn`, as [`paint_user_image`](Self::paint_user_image) draws it.
    pub(crate) fn user_image_corners(&self, p: &pdfcraft_render::PageInfo, rect: [f64; 4], turn: i64) -> [Pos2; 4] {
        let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
        let (shown_w, shown_h) = if turn % 180 == 0 { (w, h) } else { (h, w) };
        let [a, b, c, d, e, f] = pdfcraft_model::view_matrix_for(turn, rect);
        [(0.0_f64, 0.0_f64), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(u, v)| {
            // The texture's (u, v), from its top-left, in the picture's upright frame, then in user space.
            let (dx, dy) = (u * shown_w, (1.0 - v) * shown_h);
            let (x, y) = (a * dx + c * dy + e, b * dx + d * dy + f);
            let q = p.user_to_view(x as f32, y as f32);
            self.norm_to_screen(q[0] / self.pw, q[1] / self.ph)
        })
    }
    pub fn norm_to_screen(&self, u: f32, v: f32) -> Pos2 {
        let (a, b) = match self.rot {
            90 => (1.0 - v, u),
            180 => (1.0 - u, 1.0 - v),
            270 => (v, 1.0 - u),
            _ => (u, v),
        };
        pos2(self.rect.left() + a * self.rect.width(), self.rect.top() + b * self.rect.height())
    }

    pub fn screen_to_norm(&self, p: Pos2) -> (f32, f32) {
        let (a, b) = ((p.x - self.rect.left()) / self.rect.width().max(1e-3), (p.y - self.rect.top()) / self.rect.height().max(1e-3));
        match self.rot {
            90 => (b, 1.0 - a),
            180 => (1.0 - a, 1.0 - b),
            270 => (1.0 - b, a),
            _ => (a, b),
        }
    }

    /// Screen point → view space (points).
    pub fn screen_to_view(&self, p: Pos2) -> (f32, f32) {
        let (u, v) = self.screen_to_norm(p);
        (u * self.pw, v * self.ph)
    }

    /// A view-space rect [x0, y0, x1, y1] → screen rect.
    pub fn view_rect(&self, g: [f32; 4]) -> Rect {
        Rect::from_two_pos(self.norm_to_screen(g[0] / self.pw, g[1] / self.ph), self.norm_to_screen(g[2] / self.pw, g[3] / self.ph))
    }

    /// A PDF user-space rect (crop box, document `/Rotate`) → screen rect.
    pub fn user_rect(&self, info: &DocInfo, page: usize, r: [f32; 4]) -> Rect {
        let p = &info.pages[page];
        let [cx0, cy0, cx1, cy1] = p.crop;
        let (cw, ch) = ((cx1 - cx0).max(1.0), (cy1 - cy0).max(1.0));
        let norm = |x: f32, y: f32| -> (f32, f32) {
            let (u, v) = ((x - cx0) / cw, (cy1 - y) / ch);
            match p.rotation {
                90 => (1.0 - v, u),
                180 => (1.0 - u, 1.0 - v),
                270 => (v, 1.0 - u),
                _ => (u, v),
            }
        };
        let (a, b) = (norm(r[0], r[1]), norm(r[2], r[3]));
        Rect::from_two_pos(self.norm_to_screen(a.0, a.1), self.norm_to_screen(b.0, b.1))
    }

    /// Whether the screen point `p` activates link `l` on `page`: inside one of its
    /// `/QuadPoints` quadrilaterals, or its rectangle when it has none (ISO 32000-2 Table 176).
    pub fn link_contains(&self, info: &DocInfo, page: usize, l: &pdfcraft_render::Link, p: Pos2) -> bool {
        if l.quads.is_empty() {
            return self.user_rect(info, page, l.rect).contains(p);
        }
        // The inverse of `user_rect`: screen → the displayed page's fraction → user space.
        let Some(pg) = info.pages.get(page) else { return false };
        let (u, v) = self.screen_to_norm(p);
        let [x, y] = pg.view_to_user(u * pg.width, v * pg.height);
        l.contains(x, y)
    }

    pub fn user_rect_f64(&self, info: &DocInfo, page: usize, r: [f64; 4]) -> Rect {
        self.user_rect(info, page, r.map(|v| v as f32))
    }

    /// Draw a texture covering the normalised page region, rotated with the view.
    pub fn paint_image(&self, painter: &egui::Painter, tex: egui::TextureId, u0: f32, v0: f32, u1: f32, v1: f32) {
        let mut mesh = egui::Mesh::with_texture(tex);
        let corners = [(u0, v0, 0.0, 0.0), (u1, v0, 1.0, 0.0), (u1, v1, 1.0, 1.0), (u0, v1, 0.0, 1.0)];
        for (u, v, tu, tv) in corners {
            mesh.vertices.push(egui::epaint::Vertex { pos: self.norm_to_screen(u, v), uv: pos2(tu, tv), color: Color32::WHITE });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(mesh));
    }
}
