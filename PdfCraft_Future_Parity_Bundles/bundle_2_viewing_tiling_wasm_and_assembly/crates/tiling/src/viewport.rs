//! Viewport Culling Calculator for Deep Zoom Tiling.

use crate::tile_coord::{Rect2D, TileCoordinate};

pub struct ViewportCuller {
    pub tile_size: u32,
}

impl ViewportCuller {
    pub fn new(tile_size: u32) -> Self {
        Self { tile_size }
    }

    /// Determines zoom level from zoom scale (e.g. 1.0 = level 0, 2.0 = level 1, 4.0 = level 2, etc.)
    pub fn calculate_zoom_level(&self, zoom_scale: f32) -> u8 {
        if zoom_scale <= 1.2 {
            0
        } else if zoom_scale <= 2.5 {
            1
        } else if zoom_scale <= 5.0 {
            2
        } else if zoom_scale <= 10.0 {
            3
        } else if zoom_scale <= 20.0 {
            4
        } else if zoom_scale <= 40.0 {
            5
        } else {
            6 // 6400%
        }
    }

    /// Computes list of tiles intersecting the viewport rect.
    pub fn visible_tiles(
        &self,
        page_index: usize,
        page_width_pts: f32,
        page_height_pts: f32,
        zoom_scale: f32,
        viewport: Rect2D,
    ) -> Vec<TileCoordinate> {
        let level = self.calculate_zoom_level(zoom_scale);
        let scale = (1 << level) as f32;

        let total_pixel_w = page_width_pts * scale;
        let total_pixel_h = page_height_pts * scale;

        let ts = self.tile_size as f32;
        let max_col = ((total_pixel_w + ts - 1.0) / ts).floor() as u32;
        let max_row = ((total_pixel_h + ts - 1.0) / ts).floor() as u32;

        let mut tiles = Vec::new();

        let start_col = (viewport.min_x / ts).floor().max(0.0) as u32;
        let end_col = ((viewport.max_x / ts).ceil() as u32).min(max_col);

        let start_row = (viewport.min_y / ts).floor().max(0.0) as u32;
        let end_row = ((viewport.max_y / ts).ceil() as u32).min(max_row);

        for row in start_row..end_row {
            for col in start_col..end_col {
                tiles.push(TileCoordinate {
                    page_index,
                    zoom_level: level,
                    col,
                    row,
                });
            }
        }

        tiles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_viewport_tile_culling() {
        let culler = ViewportCuller::new(256);
        // Page 1000x1000 at 100% (scale 1.0 -> level 0), viewport at (0, 0) to (500, 500)
        let visible = culler.visible_tiles(
            0,
            1000.0,
            1000.0,
            1.0,
            Rect2D::new(0.0, 0.0, 500.0, 500.0),
        );

        // Cols 0, 1; Rows 0, 1 -> 4 tiles
        assert_eq!(visible.len(), 4);
        assert_eq!(visible[0].col, 0);
        assert_eq!(visible[0].row, 0);
        assert_eq!(visible[3].col, 1);
        assert_eq!(visible[3].row, 1);
    }
}
