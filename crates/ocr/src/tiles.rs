//! Tiled text detection for large pages.
//!
//! The detection model reads a fixed-size image, so ocrs shrinks a larger page to that size first.
//! On a dense page (a newspaper, small print, a blurred photo) the shrunk letters become too small
//! to find. A page at least twice the model's input along an axis is therefore cut in two along it
//! (2 × 2 tiles for a full page at 300 dpi), each tile reaching a little into its neighbours so
//! that a word on a cut is whole in one of them. Words are detected tile by tile; a word is kept
//! only by the tile whose core cell holds its centre, so a word seen twice in an overlap counts
//! once. Lines are then formed and recognised on the whole page, as without tiles. Only a page
//! whose text is small once shrunk is tiled; [`crate::pitch`] decides.

/// How far a tile reaches past its core cell into each neighbour, as a fraction of the page along
/// that axis. MDPBench (de/es/fr) was measured with 4 %.
const OVERLAP: f64 = 0.04;

/// One tile of a page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Tile {
    /// The pixels the tile crops from the page, `[left, top, right, bottom]`, right and bottom
    /// exclusive. Always within the page and never empty.
    pub crop: [u32; 4],
    /// The cell the tile owns, in page pixels, `[left, top, right, bottom]`, right and bottom
    /// exclusive. The cells of the page's edge tiles reach to infinity, so the cells cover the
    /// plane without gaps or overlaps: every finite point lies in exactly one.
    pub core: [f32; 4],
}

impl Tile {
    /// Whether a word centred at `(x, y)` (page pixels) belongs to this tile. Never true for a
    /// non-finite point.
    pub fn owns(&self, x: f32, y: f32) -> bool {
        let [l, t, r, b] = self.core;
        x.is_finite() && y.is_finite() && l <= x && x < r && t <= y && y < b
    }

    pub fn width(&self) -> u32 {
        self.crop[2].saturating_sub(self.crop[0])
    }

    pub fn height(&self) -> u32 {
        self.crop[3].saturating_sub(self.crop[1])
    }
}

/// How many pieces an axis of `len` pixels is cut into for a detector that reads `input` pixels
/// along it: two once each half still fills the detector's input (the detector would shrink the
/// page by at least half), otherwise one, so a smaller page is read as before at no extra cost.
fn pieces(len: u32, input: u32) -> u32 {
    if input > 0 && len / 2 >= input { 2 } else { 1 }
}

/// The cells and crops of one axis of `len` pixels cut into `n` pieces: `(crop start, crop end,
/// core start, core end)` for each piece.
fn axis(len: u32, n: u32) -> Vec<(u32, u32, f32, f32)> {
    let total = f64::from(len);
    let margin = (OVERLAP * total).max(0.0);
    (0..n)
        .map(|i| {
            let c0 = total * f64::from(i) / f64::from(n);
            let c1 = total * f64::from(i + 1) / f64::from(n);
            // `as` saturates for floats; both values lie in 0..=len.
            let from = (c0 - margin).max(0.0).floor() as u32;
            let to = ((c1 + margin).min(total).ceil() as u32).min(len);
            let core0 = if i == 0 { f32::NEG_INFINITY } else { c0 as f32 };
            let core1 = if i + 1 == n { f32::INFINITY } else { c1 as f32 };
            (from, to, core0, core1)
        })
        .collect()
}

/// The tiles to detect text in on a `width` × `height` page, for a detector whose input is
/// `input` = (width, height) pixels. One tile covering the whole page when no axis is worth
/// cutting.
pub(crate) fn tiles(width: u32, height: u32, input: (u32, u32)) -> Vec<Tile> {
    let columns = axis(width, pieces(width, input.0));
    let rows = axis(height, pieces(height, input.1));
    rows.iter()
        .flat_map(|&(top, bottom, core_top, core_bottom)| {
            columns.iter().map(move |&(left, right, core_left, core_right)| Tile {
                crop: [left, top, right, bottom],
                core: [core_left, core_top, core_right, core_bottom],
            })
        })
        .filter(|t| t.width() > 0 && t.height() > 0)
        .collect()
}

/// The pixels of `crop` from an image `width` pixels wide with `channels` bytes per pixel, row
/// by row. `None` if the crop reaches outside `pixels`.
pub(crate) fn crop(pixels: &[u8], width: u32, channels: usize, crop: [u32; 4]) -> Option<Vec<u8>> {
    let [left, top, right, bottom] = crop.map(|v| v as usize);
    let width = width as usize;
    if right > width || left >= right || top >= bottom {
        return None;
    }
    let row_len = (right - left).checked_mul(channels)?;
    let mut out = Vec::with_capacity(row_len.checked_mul(bottom - top)?);
    for y in top..bottom {
        let start = y.checked_mul(width)?.checked_add(left)?.checked_mul(channels)?;
        out.extend_from_slice(pixels.get(start..start.checked_add(row_len)?)?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The detector of the ocrs models reads 600 × 800 pixels.
    const INPUT: (u32, u32) = (600, 800);

    #[test]
    fn small_pages_are_read_whole() {
        // US Letter at 72 and 150 dpi, a screenshot, a page twice as wide as the input but not
        // twice as tall: no axis is cut, and the one tile is the page.
        for (w, h) in [(612, 792), (1275, 1599), (1199, 3000), (1, 1)] {
            let t = tiles(w, h, INPUT);
            let expect_cols = if w >= 1200 { 2 } else { 1 };
            let expect_rows = if h >= 1600 { 2 } else { 1 };
            assert_eq!(t.len(), expect_cols * expect_rows, "{w}x{h}: {t:?}");
        }
        let one = tiles(612, 792, INPUT);
        assert_eq!(one, vec![Tile { crop: [0, 0, 612, 792], core: [f32::NEG_INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::INFINITY] }]);
    }

    #[test]
    fn a_page_at_300_dpi_is_cut_into_four_overlapping_tiles() {
        // A4 at 300 dpi.
        let (w, h) = (2480, 3508);
        let t = tiles(w, h, INPUT);
        assert_eq!(t.len(), 4, "{t:?}");
        // 4 % of 2480 is 99.2 pixels, of 3508 140.32.
        assert_eq!(t[0].crop, [0, 0, 1340, 1895]);
        assert_eq!(t[1].crop, [1140, 0, 2480, 1895]);
        assert_eq!(t[2].crop, [0, 1613, 1340, 3508]);
        assert_eq!(t[3].crop, [1140, 1613, 2480, 3508]);
        // Each tile still fills the detector's input.
        assert!(t.iter().all(|t| t.width() >= INPUT.0 && t.height() >= INPUT.1), "{t:?}");
        // The cores meet at the middle of the page.
        assert_eq!(t[0].core[2], 1240.0);
        assert_eq!(t[3].core[0], 1240.0);
        assert_eq!(t[0].core[3], 1754.0);
        // Each core lies inside its crop, wherever it does not run off the page.
        for tile in &t {
            let [l, tp, r, b] = tile.core.map(|v| v.clamp(0.0, 4000.0));
            let [cl, ct, cr, cb] = tile.crop.map(|v| v as f32);
            assert!(cl <= l && ct <= tp && r.min(w as f32) <= cr && b.min(h as f32) <= cb, "{tile:?}");
        }
    }

    /// Each axis is cut at most once, so no page costs more than four detector runs: A0 at
    /// 300 dpi, a long receipt and the largest page the types allow are cut like A4.
    #[test]
    fn no_page_is_cut_into_more_than_four_tiles() {
        for (w, h, expect) in [(9933, 14043, 4), (14043, 9933, 4), (800, 60_000, 2), (u32::MAX, u32::MAX, 4)] {
            let t = tiles(w, h, INPUT);
            assert_eq!(t.len(), expect, "{w}x{h}: {t:?}");
            assert!(t.iter().all(|t| t.width() > 0 && t.height() > 0 && t.crop[2] <= w && t.crop[3] <= h), "{w}x{h}: {t:?}");
        }
    }

    /// A word in an overlap is seen by two tiles; exactly one keeps it. Every point of the page,
    /// on the cuts and on the edges included, belongs to exactly one tile.
    #[test]
    fn every_word_centre_belongs_to_exactly_one_tile() {
        let (w, h) = (2480, 3508);
        let t = tiles(w, h, INPUT);
        let points = [
            (0.0, 0.0),
            (1239.9, 1753.9),
            (1240.0, 1754.0),
            (1240.0, 0.0),
            (1200.0, 1700.0), // in the overlap of all four crops
            (w as f32, h as f32),
            (-3.0, 5000.0), // a box that runs off the page
        ];
        for (x, y) in points {
            let owners = t.iter().filter(|tile| tile.owns(x, y)).count();
            assert_eq!(owners, 1, "({x}, {y})");
        }
        assert!(t.iter().all(|tile| !tile.owns(f32::NAN, 10.0) && !tile.owns(10.0, f32::INFINITY)));
    }

    #[test]
    fn crops_rows_of_pixels() {
        // 3 × 2 pixels, 2 bytes each.
        let px: Vec<u8> = (0..12).collect();
        assert_eq!(crop(&px, 3, 2, [1, 0, 3, 2]), Some(vec![2, 3, 4, 5, 8, 9, 10, 11]));
        assert_eq!(crop(&px, 3, 2, [0, 1, 1, 2]), Some(vec![6, 7]));
        // Outside the image, empty, or more rows than there are: no crop, no panic.
        assert_eq!(crop(&px, 3, 2, [2, 0, 4, 1]), None);
        assert_eq!(crop(&px, 3, 2, [1, 1, 1, 2]), None);
        assert_eq!(crop(&px, 3, 2, [0, 1, 3, 3]), None);
        assert_eq!(crop(&px, u32::MAX, usize::MAX, [0, 1, u32::MAX, 2]), None);
    }
}
