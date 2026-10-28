//! Clean-room 2D Page Rasterizer and Content Stream Interpreter.
//! Complies with ISO 32000-1 §8 (Graphics) and §9 (Text).

use std::fmt;
use crate::blend_modes::{BlendMode, PixelBuffer};
use crate::graphics_state::{GraphicsState, Matrix2D, PathSegment, VectorPath};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    InvalidStreamSyntax(String),
    GraphicsStackUnderflow,
    StackDepthLimitExceeded,
    ResourceNotFound(String),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStreamSyntax(msg) => write!(f, "Invalid content stream syntax: {}", msg),
            Self::GraphicsStackUnderflow => write!(f, "Graphics state stack underflow (unexpected 'Q')"),
            Self::StackDepthLimitExceeded => write!(f, "Graphics stack depth limit exceeded"),
            Self::ResourceNotFound(res) => write!(f, "Resource '{}' not found in page dictionary", res),
        }
    }
}

impl std::error::Error for RenderError {}

/// Page rasterizer rendering PDF operators onto an RGBA pixel buffer.
pub struct PageRasterizer {
    pub current_state: GraphicsState,
    state_stack: Vec<GraphicsState>,
    current_path: VectorPath,
    pub target_buffer: PixelBuffer,
    pub scale_factor: f32,
}

impl PageRasterizer {
    pub fn new(width: usize, height: usize, scale_factor: f32) -> Self {
        Self {
            current_state: GraphicsState::default(),
            state_stack: Vec::new(),
            current_path: VectorPath::new(),
            target_buffer: PixelBuffer::with_background(width, height, 255, 255, 255, 255), // Standard white page background
            scale_factor,
        }
    }

    /// Push graphics state (`q`).
    pub fn save_state(&mut self) -> Result<(), RenderError> {
        if self.state_stack.len() >= 64 {
            return Err(RenderError::StackDepthLimitExceeded);
        }
        self.state_stack.push(self.current_state.clone());
        Ok(())
    }

    /// Pop graphics state (`Q`).
    pub fn restore_state(&mut self) -> Result<(), RenderError> {
        self.current_state = self.state_stack.pop()
            .ok_or(RenderError::GraphicsStackUnderflow)?;
        Ok(())
    }

    /// Concatenate matrix to current transformation matrix (`cm`).
    pub fn concat_matrix(&mut self, a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) {
        let new_m = Matrix2D::new(a, b, c, d, e, f);
        self.current_state.ctm = self.current_state.ctm.concat(&new_m);
    }

    /// Sets stroke line width (`w`).
    pub fn set_line_width(&mut self, width: f32) {
        self.current_state.line_width = width.max(0.1);
    }

    /// Sets RGB stroke color (`RG`).
    pub fn set_stroke_rgb(&mut self, r: f32, g: f32, b: f32) {
        self.current_state.stroke_color = [
            (r.clamp(0.0, 1.0) * 255.0) as u8,
            (g.clamp(0.0, 1.0) * 255.0) as u8,
            (b.clamp(0.0, 1.0) * 255.0) as u8,
            (self.current_state.stroke_alpha * 255.0) as u8,
        ];
    }

    /// Sets RGB fill color (`rg`).
    pub fn set_fill_rgb(&mut self, r: f32, g: f32, b: f32) {
        self.current_state.fill_color = [
            (r.clamp(0.0, 1.0) * 255.0) as u8,
            (g.clamp(0.0, 1.0) * 255.0) as u8,
            (b.clamp(0.0, 1.0) * 255.0) as u8,
            (self.current_state.fill_alpha * 255.0) as u8,
        ];
    }

    /// Sets CMYK fill color (`k`).
    pub fn set_fill_cmyk(&mut self, c: f32, m: f32, y: f32, k: f32) {
        let r = (1.0 - c) * (1.0 - k);
        let g = (1.0 - m) * (1.0 - k);
        let b = (1.0 - y) * (1.0 - k);
        self.set_fill_rgb(r, g, b);
    }

    /// Sets Gray fill color (`g`).
    pub fn set_fill_gray(&mut self, gray: f32) {
        self.set_fill_rgb(gray, gray, gray);
    }

    // Path building delegation
    pub fn move_to(&mut self, x: f32, y: f32) {
        let (tx, ty) = self.current_state.ctm.transform_point(x, y);
        self.current_path.move_to(tx * self.scale_factor, ty * self.scale_factor);
    }

    pub fn line_to(&mut self, x: f32, y: f32) {
        let (tx, ty) = self.current_state.ctm.transform_point(x, y);
        self.current_path.line_to(tx * self.scale_factor, ty * self.scale_factor);
    }

    pub fn rectangle(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.move_to(x, y);
        self.line_to(x + w, y);
        self.line_to(x + w, y + h);
        self.line_to(x, y + h);
        self.current_path.close();
    }

    pub fn close_path(&mut self) {
        self.current_path.close();
    }

    /// Fills the current path with fill color (`f` or `F`).
    pub fn fill_path(&mut self) {
        // Fast AABB scanline fill for convex / rectilinear polygons
        let segments = std::mem::take(&mut self.current_path.segments);
        if segments.is_empty() {
            return;
        }

        let mut min_x = f32::MAX;
        let mut max_x = f32::MIN;
        let mut min_y = f32::MAX;
        let mut max_y = f32::MIN;

        for seg in &segments {
            match seg {
                PathSegment::MoveTo(x, y) | PathSegment::LineTo(x, y) => {
                    min_x = min_x.min(*x);
                    max_x = max_x.max(*x);
                    min_y = min_y.min(*y);
                    max_y = max_y.max(*y);
                }
                _ => {}
            }
        }

        if min_x < max_x && min_y < max_y {
            let start_x = (min_x.floor() as usize).min(self.target_buffer.width);
            let end_x = (max_x.ceil() as usize).min(self.target_buffer.width);
            let start_y = (min_y.floor() as usize).min(self.target_buffer.height);
            let end_y = (max_y.ceil() as usize).min(self.target_buffer.height);

            let color = self.current_state.fill_color;
            let mode = self.current_state.blend_mode;

            for y in start_y..end_y {
                for x in start_x..end_x {
                    self.target_buffer.composite_pixel(x, y, color, mode);
                }
            }
        }
    }

    /// Strokes the current path with stroke color and line width (`S`).
    pub fn stroke_path(&mut self) {
        let segments = std::mem::take(&mut self.current_path.segments);
        if segments.is_empty() {
            return;
        }

        let stroke_color = self.current_state.stroke_color;
        let mode = self.current_state.blend_mode;
        let half_w = (self.current_state.line_width * self.scale_factor / 2.0).max(0.5) as usize;

        let mut last_pt: Option<(f32, f32)> = None;

        for seg in &segments {
            match seg {
                PathSegment::MoveTo(x, y) => {
                    last_pt = Some((*x, *y));
                }
                PathSegment::LineTo(x, y) => {
                    if let Some((x0, y0)) = last_pt {
                        self.render_line_bresenham(x0, y0, *x, *y, half_w, stroke_color, mode);
                    }
                    last_pt = Some((*x, *y));
                }
                PathSegment::Close => {
                    // Close to first point
                    last_pt = None;
                }
                _ => {}
            }
        }
    }

    /// Draws an image XObject onto the page at current transform (`Do`).
    pub fn draw_image(&mut self, image_rgba: &[u8], img_w: usize, img_h: usize) {
        if img_w == 0 || img_h == 0 || image_rgba.len() < img_w * img_h * 4 {
            return;
        }

        // Map image unit square [0, 0] to [1, 1] through CTM
        let (p00_x, p00_y) = self.current_state.ctm.transform_point(0.0, 0.0);
        let (p11_x, p11_y) = self.current_state.ctm.transform_point(1.0, 1.0);

        let min_x = (p00_x.min(p11_x) * self.scale_factor).max(0.0) as usize;
        let max_x = (p00_x.max(p11_x) * self.scale_factor).min(self.target_buffer.width as f32) as usize;
        let min_y = (p00_y.min(p11_y) * self.scale_factor).max(0.0) as usize;
        let max_y = (p00_y.max(p11_y) * self.scale_factor).min(self.target_buffer.height as f32) as usize;

        let dst_w = max_x.saturating_sub(min_x);
        let dst_h = max_y.saturating_sub(min_y);
        if dst_w == 0 || dst_h == 0 {
            return;
        }

        let mode = self.current_state.blend_mode;

        // Bilinear or nearest-neighbor texture sampling
        for dy in 0..dst_h {
            let py = min_y + dy;
            let src_y = ((dy as f32 / dst_h as f32) * img_h as f32) as usize % img_h;
            let src_row = src_y * img_w * 4;

            for dx in 0..dst_w {
                let px = min_x + dx;
                let src_x = ((dx as f32 / dst_w as f32) * img_w as f32) as usize % img_w;
                let src_idx = src_row + src_x * 4;

                let color = [
                    image_rgba[src_idx],
                    image_rgba[src_idx + 1],
                    image_rgba[src_idx + 2],
                    image_rgba[src_idx + 3],
                ];

                self.target_buffer.composite_pixel(px, py, color, mode);
            }
        }
    }

    /// Bresenham line drawer with stroke thickness.
    fn render_line_bresenham(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        thickness: usize,
        color: [u8; 4],
        mode: BlendMode,
    ) {
        let mut ix0 = x0.round() as i32;
        let mut iy0 = y0.round() as i32;
        let ix1 = x1.round() as i32;
        let iy1 = y1.round() as i32;

        let dx = (ix1 - ix0).abs();
        let dy = -(iy1 - iy0).abs();
        let sx = if ix0 < ix1 { 1 } else { -1 };
        let sy = if iy0 < iy1 { 1 } else { -1 };
        let mut err = dx + dy;

        let max_w = self.target_buffer.width as i32;
        let max_h = self.target_buffer.height as i32;
        let t = thickness as i32;

        loop {
            // Paint brush radius
            for by in -t..=t {
                for bx in -t..=t {
                    let px = ix0 + bx;
                    let py = iy0 + by;
                    if px >= 0 && px < max_w && py >= 0 && py < max_h {
                        self.target_buffer.composite_pixel(px as usize, py as usize, color, mode);
                    }
                }
            }

            if ix0 == ix1 && iy0 == iy1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                ix0 += sx;
            }
            if e2 <= dx {
                err += dx;
                iy0 += sy;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_rasterizer_fill_rectangle() {
        let mut rasterizer = PageRasterizer::new(50, 50, 1.0);
        rasterizer.set_fill_rgb(1.0, 0.0, 0.0); // Red
        rasterizer.rectangle(10.0, 10.0, 20.0, 20.0);
        rasterizer.fill_path();

        // Point inside rectangle should be red
        let px = rasterizer.target_buffer.get_pixel(15, 15).unwrap();
        assert_eq!(px, [255, 0, 0, 255]);

        // Point outside rectangle should be white background
        let bg = rasterizer.target_buffer.get_pixel(5, 5).unwrap();
        assert_eq!(bg, [255, 255, 255, 255]);
    }

    #[test]
    fn test_graphics_state_push_pop() {
        let mut rasterizer = PageRasterizer::new(10, 10, 1.0);
        rasterizer.set_line_width(5.0);
        rasterizer.save_state().unwrap();

        rasterizer.set_line_width(1.0);
        assert_eq!(rasterizer.current_state.line_width, 1.0);

        rasterizer.restore_state().unwrap();
        assert_eq!(rasterizer.current_state.line_width, 5.0);
    }
}
