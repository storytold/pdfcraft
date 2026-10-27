use super::*;

impl DocView {
    /// Where `page` is drawn on screen this frame (`None` when it is not on screen).
    pub fn page_screen_rect(&self, page: usize) -> Option<Rect> {
        self.screen_xforms.iter().find(|(p, _)| *p == page).map(|(_, xf)| xf.rect)
    }

    /// The document area on screen.
    pub fn viewport_rect(&self) -> Rect {
        self.viewport_screen
    }

    /// Pages drawn last frame and their screen rectangles.
    pub fn visible_page_rects(&self) -> Vec<(usize, Rect)> {
        self.screen_xforms.iter().map(|(p, xf)| (*p, xf.rect)).collect()
    }

    /// Where `page` is drawn this frame, with its coordinate mapping.
    pub(crate) fn page_xform(&self, page: usize) -> Option<PageXform> {
        self.screen_xforms.iter().find(|(p, _)| *p == page).map(|(_, xf)| *xf)
    }
}

/// `r` moved so its corner lies on a whole physical pixel, so that a raster drawn from there
/// maps texel for texel onto the screen.
pub(super) fn snap_to_pixels(r: Rect, ppp: f32) -> Rect {
    let snap = |v: f32| (v * ppp).round() / ppp;
    Rect::from_min_size(pos2(snap(r.min.x), snap(r.min.y)), r.size())
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
    /// Screen pixels per page point. The view's rotation swaps the rectangle's sides, so a page
    /// turned a quarter shows its width down the screen.
    pub(crate) fn zoom(&self) -> f32 {
        let across = if self.rot % 180 == 90 { self.rect.height() } else { self.rect.width() };
        across / self.pw.max(1.0)
    }

    /// Draw an image in the PDF user-space `rect` as a custom stamp's appearance draws it: turned
    /// back by `turn` (the page /Rotate it was placed for), so with the page's own rotation it
    /// reads upright as displayed.
    pub(crate) fn paint_user_image(
        &self,
        painter: &egui::Painter,
        tex: egui::TextureId,
        p: &pdfcraft_render::PageInfo,
        rect: [f64; 4],
        turn: i64,
        color: Color32,
    ) {
        let mut mesh = egui::Mesh::with_texture(tex);
        let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
        let (shown_w, shown_h) = if turn % 180 == 0 { (w, h) } else { (h, w) };
        let [a, b, c, d, e, f] = pdfcraft_model::view_matrix_for(turn, rect);
        for (u, v) in [(0.0_f32, 0.0_f32), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            // The texture's (u, v), from its top-left, in the picture's upright frame, then in user space.
            let (dx, dy) = (f64::from(u) * shown_w, f64::from(1.0 - v) * shown_h);
            let (x, y) = (a * dx + c * dy + e, b * dx + d * dy + f);
            let q = p.user_to_view(x as f32, y as f32);
            mesh.vertices.push(egui::epaint::Vertex { pos: self.norm_to_screen(q[0] / self.pw, q[1] / self.ph), uv: pos2(u, v), color });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(mesh));
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

    /// This transform resized to a raster of `px` device pixels (width, height before the view
    /// rotation), so each of its texels covers one screen pixel. The rectangle's corner must
    /// already be on a whole pixel ([`snap_to_pixels`]).
    pub(super) fn texel_aligned(&self, px: [usize; 2], ppp: f32) -> PageXform {
        let (w, h) = (px[0] as f32 / ppp, px[1] as f32 / ppp);
        let size = if self.rot % 180 == 90 { vec2(h, w) } else { vec2(w, h) };
        PageXform { rect: Rect::from_min_size(self.rect.min, size), ..*self }
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
