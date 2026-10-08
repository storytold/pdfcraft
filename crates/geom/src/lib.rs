//! pdfcraft-geom — L0 geometry primitives shared by every PdfCraft crate.
//!
//! PDF user space is y-up with the origin at the bottom-left of the page; view space is y-down.
//! `PageRect` is always in PDF user space (points, 1/72 inch).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// An axis-aligned rectangle in PDF user space (points).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PageRect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl PageRect {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self { x0: x0.min(x1), y0: y0.min(y1), x1: x0.max(x1), y1: y0.max(y1) }
    }
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }
}

/// Page rotation in quarter turns, as in the PDF `/Rotate` key (normalised to 0/90/180/270).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Rotation {
    #[default]
    R0,
    R90,
    R180,
    R270,
}

impl Rotation {
    pub fn from_degrees(deg: i64) -> Self {
        match deg.rem_euclid(360) {
            90 => Self::R90,
            180 => Self::R180,
            270 => Self::R270,
            _ => Self::R0,
        }
    }
    pub fn swaps_axes(self) -> bool {
        matches!(self, Self::R90 | Self::R270)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_normalises() {
        let r = PageRect::new(10.0, 20.0, 0.0, 5.0);
        assert_eq!((r.x0, r.y0, r.width(), r.height()), (0.0, 5.0, 10.0, 15.0));
    }

    #[test]
    fn rotation_normalises_negative_and_large_values() {
        assert_eq!(Rotation::from_degrees(-90), Rotation::R270);
        assert_eq!(Rotation::from_degrees(450), Rotation::R90);
        assert!(Rotation::R90.swaps_axes());
    }
}
