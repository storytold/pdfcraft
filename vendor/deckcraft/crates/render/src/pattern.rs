//! Preset pattern fills (`a:pattFill`): an 8×8 two-colour tile, each cell one 96-dpi pixel
//! (0.75 pt) of the slide, as PowerPoint draws them.
//!
//! The tiles were read from PowerPoint's own slide export of every preset (black on white, at
//! exactly two output pixels per pattern pixel), so the marks, their density and their phase
//! match it. Bit 7 of each row is the leftmost cell; a set bit is the foreground colour.

use std::sync::Arc;

use deckcraft_color::Rgba;
use kurbo::{Affine, BezPath};
use vello_cpu::{Pixmap, RenderContext, peniko};

/// Side of one pattern cell, in points.
pub const CELL: f64 = 0.75;

const TILES: &[(&str, [u8; 8])] = &[
    ("pct5", [0x80, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00]),
    ("pct10", [0x80, 0x00, 0x08, 0x00, 0x80, 0x00, 0x08, 0x00]),
    ("pct20", [0x88, 0x00, 0x22, 0x00, 0x88, 0x00, 0x22, 0x00]),
    ("pct25", [0x88, 0x22, 0x88, 0x22, 0x88, 0x22, 0x88, 0x22]),
    ("pct30", [0xAA, 0x44, 0xAA, 0x11, 0xAA, 0x44, 0xAA, 0x11]),
    ("pct40", [0xAA, 0x55, 0xAA, 0x51, 0xAA, 0x55, 0xAA, 0x15]),
    ("pct50", [0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55]),
    ("pct60", [0xEE, 0x55, 0xBB, 0x55, 0xEE, 0x55, 0xBB, 0x55]),
    ("pct70", [0x77, 0xDD, 0x77, 0xDD, 0x77, 0xDD, 0x77, 0xDD]),
    ("pct75", [0x77, 0xFF, 0xDD, 0xFF, 0x77, 0xFF, 0xDD, 0xFF]),
    ("pct80", [0xEF, 0xFF, 0xFE, 0xFF, 0xEF, 0xFF, 0xFE, 0xFF]),
    ("pct90", [0xFF, 0xFF, 0xFF, 0xF7, 0xFF, 0xFF, 0xFF, 0x7F]),
    ("horz", [0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
    ("vert", [0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80]),
    ("ltHorz", [0xFF, 0x00, 0x00, 0x00, 0xFF, 0x00, 0x00, 0x00]),
    ("ltVert", [0x88, 0x88, 0x88, 0x88, 0x88, 0x88, 0x88, 0x88]),
    ("dkHorz", [0xFF, 0xFF, 0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00]),
    ("dkVert", [0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC]),
    ("narHorz", [0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00]),
    ("narVert", [0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55]),
    ("dashHorz", [0xF0, 0x00, 0x00, 0x00, 0x0F, 0x00, 0x00, 0x00]),
    ("dashVert", [0x80, 0x80, 0x80, 0x80, 0x08, 0x08, 0x08, 0x08]),
    ("cross", [0xFF, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80]),
    ("dnDiag", [0x80, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01]),
    ("upDiag", [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80]),
    ("ltDnDiag", [0x88, 0x44, 0x22, 0x11, 0x88, 0x44, 0x22, 0x11]),
    ("ltUpDiag", [0x11, 0x22, 0x44, 0x88, 0x11, 0x22, 0x44, 0x88]),
    ("dkDnDiag", [0xCC, 0x66, 0x33, 0x99, 0xCC, 0x66, 0x33, 0x99]),
    ("dkUpDiag", [0x33, 0x66, 0xCC, 0x99, 0x33, 0x66, 0xCC, 0x99]),
    ("wdDnDiag", [0xC1, 0xE0, 0x70, 0x38, 0x1C, 0x0E, 0x07, 0x83]),
    ("wdUpDiag", [0x83, 0x07, 0x0E, 0x1C, 0x38, 0x70, 0xE0, 0xC1]),
    ("dashDnDiag", [0x00, 0x00, 0x88, 0x44, 0x22, 0x11, 0x00, 0x00]),
    ("dashUpDiag", [0x00, 0x00, 0x11, 0x22, 0x44, 0x88, 0x00, 0x00]),
    ("diagCross", [0x81, 0x42, 0x24, 0x18, 0x18, 0x24, 0x42, 0x81]),
    ("smCheck", [0x99, 0x66, 0x66, 0x99, 0x99, 0x66, 0x66, 0x99]),
    ("lgCheck", [0xF0, 0xF0, 0xF0, 0xF0, 0x0F, 0x0F, 0x0F, 0x0F]),
    ("smGrid", [0xFF, 0x88, 0x88, 0x88, 0xFF, 0x88, 0x88, 0x88]),
    ("lgGrid", [0xFF, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80]),
    ("dotGrid", [0xAA, 0x00, 0x80, 0x00, 0x80, 0x00, 0x80, 0x00]),
    ("smConfetti", [0x80, 0x08, 0x40, 0x02, 0x10, 0x01, 0x20, 0x04]),
    ("lgConfetti", [0xB1, 0x30, 0x03, 0x1B, 0xD8, 0xC0, 0x0C, 0x8D]),
    ("horzBrick", [0xFF, 0x80, 0x80, 0x80, 0xFF, 0x08, 0x08, 0x08]),
    ("diagBrick", [0x01, 0x02, 0x04, 0x08, 0x18, 0x24, 0x42, 0x81]),
    ("solidDmnd", [0x10, 0x38, 0x7C, 0xFE, 0x7C, 0x38, 0x10, 0x00]),
    ("openDmnd", [0x82, 0x44, 0x28, 0x10, 0x28, 0x44, 0x82, 0x01]),
    ("dotDmnd", [0x80, 0x00, 0x22, 0x00, 0x08, 0x00, 0x22, 0x00]),
    ("plaid", [0xAA, 0x55, 0xAA, 0x55, 0xF0, 0xF0, 0xF0, 0xF0]),
    ("sphere", [0x77, 0x89, 0x8F, 0x8F, 0x77, 0x98, 0xF8, 0xF8]),
    ("weave", [0x88, 0x54, 0x22, 0x45, 0x88, 0x14, 0x22, 0x51]),
    ("divot", [0x00, 0x10, 0x08, 0x10, 0x00, 0x80, 0x01, 0x80]),
    ("shingle", [0x03, 0x84, 0x48, 0x30, 0x0C, 0x02, 0x01, 0x01]),
    ("wave", [0x00, 0x18, 0x25, 0xC0, 0x00, 0x18, 0x25, 0xC0]),
    ("trellis", [0xFF, 0x66, 0xFF, 0x99, 0xFF, 0x66, 0xFF, 0x99]),
    ("zigZag", [0x81, 0x42, 0x24, 0x18, 0x81, 0x42, 0x24, 0x18]),
];

/// Every preset name, in the order of the tiles.
pub fn presets() -> impl Iterator<Item = &'static str> {
    TILES.iter().map(|(n, _)| *n)
}

/// The 8×8 tile of preset `name` (`pct50` for an unknown name).
pub fn tile(name: &str) -> [u8; 8] {
    TILES
        .iter()
        .find(|(n, _)| *n == name)
        .or_else(|| TILES.iter().find(|(n, _)| *n == "pct50"))
        .map(|(_, t)| *t)
        .unwrap_or([0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55])
}

/// Is cell (`x`, `y`) of the tile the foreground?
pub fn is_fg(tile: &[u8; 8], x: usize, y: usize) -> bool {
    tile.get(y % 8).is_some_and(|row| row & (0x80 >> (x % 8)) != 0)
}

fn premul(c: Rgba, alpha: f64) -> vello_cpu::color::PremulRgba8 {
    let a = (c.a as f64 * alpha.clamp(0.0, 1.0)).round().clamp(0.0, 255.0);
    let m = |v: u8| (v as f64 * a / 255.0).round().clamp(0.0, 255.0) as u8;
    vello_cpu::color::PremulRgba8 { r: m(c.r), g: m(c.g), b: m(c.b), a: a as u8 }
}

/// Fill `path` (in the space `m` maps to the output) with preset `name` in `fg` on `bg`, faded by
/// `alpha`.
pub fn fill(ctx: &mut RenderContext, name: &str, fg: Rgba, bg: Rgba, alpha: f64, path: &BezPath, m: Affine) {
    let det = m.determinant();
    if !det.is_finite() || det.abs() <= 1e-12 {
        return;
    }
    let t = tile(name);
    let (f, b) = (premul(fg, alpha), premul(bg, alpha));
    let data: Vec<vello_cpu::color::PremulRgba8> = (0..64).map(|i| if is_fg(&t, i % 8, i / 8) { f } else { b }).collect();
    let pm = Arc::new(Pixmap::from_parts(data, 8, 8));
    ctx.set_transform(m);
    ctx.set_paint(vello_cpu::Image {
        image: vello_cpu::ImageSource::Pixmap(pm),
        sampler: peniko::ImageSampler {
            x_extend: peniko::Extend::Repeat,
            y_extend: peniko::Extend::Repeat,
            // Bilinear, as PowerPoint samples the tile: exact at a whole number of output pixels
            // per cell; between, a pixel straddling two cells takes a blend of both (fitted to its
            // export at 0.75 px per cell: mean error 0.067 against 0.082 for the nearest cell).
            quality: peniko::ImageQuality::Medium,
            alpha: 1.0,
        },
    });
    // Cells are laid out on the output from its origin (the slide's top-left corner), so
    // neighbouring shapes' patterns line up, as in PowerPoint.
    let k = crate::scale_of(m);
    let k = if k.is_finite() && k > 0.0 { k } else { 1.0 };
    ctx.set_paint_transform(m.inverse() * Affine::scale(CELL * k));
    ctx.fill_path(path);
    ctx.reset_paint_transform();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_drawingml_preset_has_a_tile() {
        // ECMA-376 ST_PresetPatternVal.
        let all = [
            "pct5",
            "pct10",
            "pct20",
            "pct25",
            "pct30",
            "pct40",
            "pct50",
            "pct60",
            "pct70",
            "pct75",
            "pct80",
            "pct90",
            "horz",
            "vert",
            "ltHorz",
            "ltVert",
            "dkHorz",
            "dkVert",
            "narHorz",
            "narVert",
            "dashHorz",
            "dashVert",
            "cross",
            "dnDiag",
            "upDiag",
            "ltDnDiag",
            "ltUpDiag",
            "dkDnDiag",
            "dkUpDiag",
            "wdDnDiag",
            "wdUpDiag",
            "dashDnDiag",
            "dashUpDiag",
            "diagCross",
            "smCheck",
            "lgCheck",
            "smGrid",
            "lgGrid",
            "dotGrid",
            "smConfetti",
            "lgConfetti",
            "horzBrick",
            "diagBrick",
            "solidDmnd",
            "openDmnd",
            "dotDmnd",
            "plaid",
            "sphere",
            "weave",
            "divot",
            "shingle",
            "wave",
            "trellis",
            "zigZag",
        ];
        assert_eq!(all.len(), 54);
        for n in all {
            assert!(TILES.iter().any(|(t, _)| *t == n), "{n}");
        }
        assert_eq!(TILES.len(), 54);
    }

    #[test]
    fn percent_presets_get_denser() {
        // The pctN tiles set more cells as N grows (PowerPoint's own: pct5 sets 2 of 64, pct50
        // half, pct90 all but 2).
        let share = |n: &str| {
            let t = tile(n);
            (0..64).filter(|i| is_fg(&t, i % 8, i / 8)).count()
        };
        let names = ["pct5", "pct10", "pct20", "pct25", "pct30", "pct40", "pct50", "pct60", "pct70", "pct75", "pct80", "pct90"];
        assert!(names.windows(2).all(|w| share(w[0]) <= share(w[1])), "{:?}", names.map(share));
        assert_eq!((share("pct5"), share("pct50"), share("pct90")), (2, 32, 62));
        assert_eq!(tile("nonsense"), tile("pct50"));
    }
}
