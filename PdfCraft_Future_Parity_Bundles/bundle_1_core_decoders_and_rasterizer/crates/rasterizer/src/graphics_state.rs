//! Clean-room 2D Graphics State Machine and Path Construction per ISO 32000-1 §8.4.

use crate::blend_modes::BlendMode;

/// 2D Affine Transformation Matrix [a, b, c, d, e, f].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix2D {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Default for Matrix2D {
    fn default() -> Self {
        Self::identity()
    }
}

impl Matrix2D {
    pub fn identity() -> Self {
        Self { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 }
    }

    pub fn new(a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) -> Self {
        Self { a, b, c, d, e, f }
    }

    /// Multiply this matrix by another (self * other).
    pub fn concat(&self, other: &Self) -> Self {
        Self {
            a: self.a * other.a + self.b * other.c,
            b: self.a * other.b + self.b * other.d,
            c: self.c * other.a + self.d * other.c,
            d: self.c * other.b + self.d * other.d,
            e: self.e * other.a + self.f * other.c + other.e,
            f: self.e * other.b + self.f * other.d + other.f,
        }
    }

    /// Transforms a point (x, y) into device space coordinates.
    pub fn transform_point(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PathSegment {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    CubicBezier {
        ctrl1: (f32, f32),
        ctrl2: (f32, f32),
        end: (f32, f32),
    },
    Close,
}

#[derive(Debug, Clone, Default)]
pub struct VectorPath {
    pub segments: Vec<PathSegment>,
}

impl VectorPath {
    pub fn new() -> Self {
        Self { segments: Vec::new() }
    }

    pub fn move_to(&mut self, x: f32, y: f32) {
        self.segments.push(PathSegment::MoveTo(x, y));
    }

    pub fn line_to(&mut self, x: f32, y: f32) {
        self.segments.push(PathSegment::LineTo(x, y));
    }

    pub fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32) {
        self.segments.push(PathSegment::CubicBezier {
            ctrl1: (x1, y1),
            ctrl2: (x2, y2),
            end: (x3, y3),
        });
    }

    pub fn rectangle(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.move_to(x, y);
        self.line_to(x + w, y);
        self.line_to(x + w, y + h);
        self.line_to(x, y + h);
        self.close();
    }

    pub fn close(&mut self) {
        self.segments.push(PathSegment::Close);
    }

    pub fn clear(&mut self) {
        self.segments.clear();
    }
}

/// Graphics State Parameters per ISO 32000-1 Table 51.
#[derive(Debug, Clone)]
pub struct GraphicsState {
    pub ctm: Matrix2D,
    pub fill_color: [u8; 4],   // RGBA
    pub stroke_color: [u8; 4], // RGBA
    pub line_width: f32,
    pub line_cap: u8,   // 0: butt, 1: round, 2: projecting square
    pub line_join: u8,  // 0: miter, 1: round, 2: bevel
    pub miter_limit: f32,
    pub blend_mode: BlendMode,
    pub fill_alpha: f32,
    pub stroke_alpha: f32,
    pub clip_rect: Option<[f32; 4]>,
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            ctm: Matrix2D::identity(),
            fill_color: [0, 0, 0, 255],   // Default black
            stroke_color: [0, 0, 0, 255],
            line_width: 1.0,
            line_cap: 0,
            line_join: 0,
            miter_limit: 10.0,
            blend_mode: BlendMode::Normal,
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            clip_rect: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matrix_transforms() {
        let m = Matrix2D::new(2.0, 0.0, 0.0, 2.0, 10.0, 20.0);
        let (x, y) = m.transform_point(5.0, 5.0);
        assert_eq!((x, y), (20.0, 30.0));
    }

    #[test]
    fn test_vector_path_rectangle() {
        let mut path = VectorPath::new();
        path.rectangle(0.0, 0.0, 100.0, 50.0);
        assert_eq!(path.segments.len(), 5);
        assert_eq!(path.segments[0], PathSegment::MoveTo(0.0, 0.0));
        assert_eq!(path.segments[4], PathSegment::Close);
    }
}
