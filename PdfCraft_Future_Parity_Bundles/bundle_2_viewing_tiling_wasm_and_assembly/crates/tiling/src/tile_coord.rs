//! Deep Zoom Tile Coordinate and Bounding Box Geometry.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileCoordinate {
    pub page_index: usize,
    pub zoom_level: u8, // 0 = 100%, 1 = 200%, 2 = 400%, 3 = 800%, 4 = 1600%, 5 = 3200%, 6 = 6400%
    pub col: u32,
    pub row: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect2D {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

impl Rect2D {
    pub fn new(min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> Self {
        Self { min_x, min_y, max_x, max_y }
    }

    pub fn intersects(&self, other: &Self) -> bool {
        self.min_x < other.max_x && self.max_x > other.min_x &&
        self.min_y < other.max_y && self.max_y > other.min_y
    }
}
