//! Which parts of the window changed since the last frame.
//!
//! egui draws the whole window every frame, and a GPU does just that. Drawn on the processor and
//! sent over Remote Desktop, a frame costs what its changed pixels cost, so only those are drawn
//! and presented: a caret that blinks is a few hundred pixels, and scrolling a page leaves the
//! toolbars and panels alone.
//!
//! The window is cut into tiles of [`TILE`] pixels. Every triangle of a frame adds its hash to the
//! tiles it may touch, in drawing order; a tile whose sum differs from the last frame's is redrawn.
//! Anything that changes a pixel changes a triangle over it (a position, a colour, a texture
//! coordinate, the texture's contents, the clip rectangle), so a tile that hashes the same looks
//! the same. The other way round a tile may be redrawn for nothing, which only costs time.

use super::raster::PixelRect;

/// Side of a tile, in pixels.
pub(crate) const TILE: i32 = 64;

/// More rectangles than this to redraw are merged into the one that holds them all: each is a
/// pass over the frame's meshes.
const MAX_RECTS: usize = 12;

#[derive(Default)]
pub(crate) struct Damage {
    width: i32,
    height: i32,
    columns: usize,
    rows: usize,
    /// The last frame's tile hashes; empty when everything must be redrawn.
    previous: Vec<u64>,
    current: Vec<u64>,
}

/// Folds `value` into `hash`. The order matters, as the order of drawing does.
#[inline]
pub(crate) fn mix(hash: u64, value: u64) -> u64 {
    (hash.rotate_left(5) ^ value).wrapping_mul(0x517c_c1b7_2722_0a95)
}

impl Damage {
    /// Starts a frame of `width` × `height` pixels. `seed` is what the whole frame depends on
    /// (the background colour, the scale).
    pub(crate) fn begin(&mut self, width: usize, height: usize, seed: u64) {
        let (width, height) = (i32::try_from(width).unwrap_or(i32::MAX), i32::try_from(height).unwrap_or(i32::MAX));
        if (width, height) != (self.width, self.height) {
            self.previous.clear();
        }
        (self.width, self.height) = (width, height);
        self.columns = usize::try_from((width.saturating_add(TILE - 1)) / TILE).unwrap_or(0);
        self.rows = usize::try_from((height.saturating_add(TILE - 1)) / TILE).unwrap_or(0);
        self.current.clear();
        self.current.resize(self.columns.saturating_mul(self.rows), seed);
    }

    /// The next frame redraws everything: the pixels kept from the last one are gone or stale.
    pub(crate) fn invalidate(&mut self) {
        self.previous.clear();
    }

    /// Something with the hash `hash` is drawn within `area`.
    pub(crate) fn add(&mut self, area: PixelRect, hash: u64) {
        let area = area.intersect(PixelRect::new(0, 0, self.width, self.height));
        if area.is_empty() {
            return;
        }
        let tile = |v: i32| usize::try_from(v / TILE).unwrap_or(0);
        let (first_column, last_column) = (tile(area.x0), tile(area.x1 - 1));
        for row in tile(area.y0)..=tile(area.y1 - 1) {
            let start = row.saturating_mul(self.columns);
            let Some(tiles) = self.current.get_mut(start.saturating_add(first_column)..=start.saturating_add(last_column)) else { continue };
            for tile in tiles {
                *tile = mix(*tile, hash);
            }
        }
    }

    /// Ends the frame: the rectangles to redraw, which don't overlap. Empty when nothing changed.
    pub(crate) fn finish(&mut self) -> Vec<PixelRect> {
        let everything = self.previous.len() != self.current.len();
        let mut rects: Vec<PixelRect> = Vec::new();
        // Rectangles that reach the row above, to grow down while the row below has the same run.
        let mut open: Vec<usize> = Vec::new();
        for row in 0..self.rows {
            let mut still_open = Vec::new();
            for (first, end) in self.runs(row, everything) {
                let (x0, x1) = (self.edge(first, self.width), self.edge(end, self.width));
                let (y0, y1) = (self.edge(row, self.height), self.edge(row + 1, self.height));
                let above = open.iter().copied().find(|at| rects.get(*at).is_some_and(|r| r.x0 == x0 && r.x1 == x1 && r.y1 == y0));
                match above.and_then(|at| rects.get_mut(at).map(|rect| (at, rect))) {
                    Some((at, rect)) => {
                        rect.y1 = y1;
                        still_open.push(at);
                    }
                    None => {
                        still_open.push(rects.len());
                        rects.push(PixelRect::new(x0, y0, x1, y1));
                    }
                }
            }
            open = still_open;
        }
        core::mem::swap(&mut self.previous, &mut self.current);
        if rects.len() > MAX_RECTS {
            let all = rects.iter().fold(PixelRect::default(), |all, rect| all.union(*rect));
            return vec![all];
        }
        rects
    }

    /// The runs of changed tiles in `row`, as `first..end` columns.
    fn runs(&self, row: usize, everything: bool) -> Vec<(usize, usize)> {
        let start = row.saturating_mul(self.columns);
        let changed = |column: usize| everything || self.previous.get(start + column) != self.current.get(start + column);
        let mut runs = Vec::new();
        let mut first = None;
        for column in 0..self.columns {
            match (changed(column), first) {
                (true, None) => first = Some(column),
                (false, Some(from)) => {
                    runs.push((from, column));
                    first = None;
                }
                _ => {}
            }
        }
        if let Some(from) = first {
            runs.push((from, self.columns));
        }
        runs
    }

    /// The pixel where tile number `tile` starts, no further than `limit`.
    fn edge(&self, tile: usize, limit: i32) -> i32 {
        i32::try_from(tile).unwrap_or(i32::MAX).saturating_mul(TILE).min(limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(damage: &mut Damage, marks: &[(PixelRect, u64)]) -> Vec<PixelRect> {
        damage.begin(640, 320, 7);
        for (area, hash) in marks {
            damage.add(*area, *hash);
        }
        damage.finish()
    }

    #[test]
    fn the_first_frame_and_a_resized_one_redraw_everything() {
        let mut damage = Damage::default();
        assert_eq!(frame(&mut damage, &[]), vec![PixelRect::new(0, 0, 640, 320)]);
        assert_eq!(frame(&mut damage, &[]), vec![], "the same frame again changes nothing");
        damage.begin(700, 320, 7);
        assert_eq!(damage.finish(), vec![PixelRect::new(0, 0, 700, 320)], "the last tile ends at the window's edge");
        damage.invalidate();
        assert_eq!(frame(&mut damage, &[]), vec![PixelRect::new(0, 0, 640, 320)]);
    }

    #[test]
    fn only_the_tiles_under_what_changed_are_redrawn() {
        let mut damage = Damage::default();
        let caret = PixelRect::new(70, 70, 72, 90);
        let page = PixelRect::new(128, 64, 512, 320);
        frame(&mut damage, &[(page, 1)]);
        assert_eq!(frame(&mut damage, &[(page, 1), (caret, 9)]), vec![PixelRect::new(64, 64, 128, 128)], "the caret's tile");
        assert_eq!(frame(&mut damage, &[(page, 1)]), vec![PixelRect::new(64, 64, 128, 128)], "and again when it goes");
        assert_eq!(frame(&mut damage, &[(page, 2)]), vec![PixelRect::new(128, 64, 512, 320)], "a scrolled page is one rectangle");
    }

    #[test]
    fn the_order_of_drawing_is_part_of_the_picture() {
        let mut damage = Damage::default();
        let area = PixelRect::new(0, 0, 10, 10);
        frame(&mut damage, &[(area, 1), (area, 2)]);
        assert_eq!(frame(&mut damage, &[(area, 2), (area, 1)]), vec![PixelRect::new(0, 0, 64, 64)]);
    }

    #[test]
    fn scattered_changes_become_one_rectangle_and_nothing_outside_the_window_counts() {
        let mut damage = Damage::default();
        frame(&mut damage, &[]);
        let dots: Vec<(PixelRect, u64)> =
            (0..5).flat_map(|row| (0..5).map(move |column| (PixelRect::new(column * 128, row * 128, column * 128 + 1, row * 128 + 1), 3))).collect();
        assert_eq!(frame(&mut damage, &dots), vec![PixelRect::new(0, 0, 576, 320)], "more than the limit: their bounding rectangle");
        frame(&mut damage, &[]);
        assert_eq!(frame(&mut damage, &[(PixelRect::new(-500, -500, -1, -1), 5), (PixelRect::new(640, 0, 900, 10), 5)]), vec![]);
        assert_eq!(frame(&mut damage, &[(PixelRect::new(i32::MIN, 0, i32::MAX, 1), 5)]), vec![PixelRect::new(0, 0, 640, 64)]);
    }
}
