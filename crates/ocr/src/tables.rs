//! Table structure: the rows and cells of a page's `table` regions, and which recognised words go
//! in which cell.
//!
//! The SLANet_plus network (PaddlePaddle, Apache-2.0; the ONNX export fetched by
//! `cargo xtask models`, pinned by SHA-256) reads a picture of a table and writes its HTML
//! structure one token at a time, with a box for each cell. What it expects and what it returns
//! come from the configuration published with the model and from running it:
//!
//! - `inference.yml`: the picture is scaled so its longer side is 488 pixels, normalised with the
//!   ImageNet mean and deviation, padded to 488 × 488 and given channels first, and it is decoded
//!   as BGR. Its token dictionary lists 48 tokens (`<thead>` … ` rowspan="20"`) and asks for cells
//!   without spans to be merged (`merge_no_span_structure`).
//! - Running it (with onnxruntime, comparing variants of the input): each step scores 50 classes,
//!   at most 501 steps. The table ends with class 49; class 0 is never written before it, only
//!   after it, as padding (a start class). Classes 1 to 47 are the dictionary without `<td>`,
//!   and 48 is the merged `<td></td>`: a two-column table comes
//!   back as `<tr>` 48 48 `</tr>` for every row, and a spanning cell as `<td`, its spans, `>`,
//!   `</td>`. A cell's box is the one at its `<td></td>` or `<td` step: four corners (x, y),
//!   clockwise from the top left, as fractions of the padded 488 × 488 picture (a box ends at
//!   0.18 down a table whose picture is 87 of the 488 pixels high). Without the padding the
//!   network writes nothing but class 0. Swapping the channels made no measurable difference
//!   (MDPBench table score 0.229 as BGR, 0.232 as RGB), so the configuration's order is kept.
//!   rten runs the network (its loop included) to the same tables: 0.230 on MDPBench, as with
//!   onnxruntime.
//!
//! [`TableModel::read`] returns a table's structure and cell boxes; [`fill`] puts recognised words
//! into the cells.

use std::path::Path;

use rten::{Model, NodeId};
use rten_tensor::{AsView, Layout as _, NdTensor};

use crate::regions::axis_weights;
use crate::{Line, OcrError};

/// The size the network reads a table at.
const SIDE: usize = 488;
const INPUT_NODE: &str = "x";
/// The cell boxes, `[1, steps, 8]`, and the token scores, `[1, steps, CLASSES]`.
const BOXES_NODE: &str = "fetch_name_0";
const TOKENS_NODE: &str = "fetch_name_1";
/// The normalisation from `inference.yml`, in the order of the channels as decoded (B, G, R).
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];
/// Classes a step: start, the 47 dictionary tokens other than `<td>`, `<td></td>`, end.
const CLASSES: usize = 50;
/// Numbers a cell box: four corners, x and y each.
const BOX_VALUES: usize = 8;
/// At most this many steps are read (the network's own maximum, `max_text_length` 500 and the
/// end), which also bounds the cells of a table.
const MAX_STEPS: usize = 501;

/// At most this many tables are read on a page (each is a run of the network, a fraction of a
/// second); the rest stay paragraphs.
const MAX_TABLES: usize = 32;

/// The token classes (see the module documentation).
const START: usize = 0;
const ROW: usize = 5;
const ROW_END: usize = 6;
const CELL_OPEN: usize = 7;
const CELL_END: usize = 9;
const COLSPAN: std::ops::RangeInclusive<usize> = 10..=28;
const ROWSPAN: std::ops::RangeInclusive<usize> = 29..=47;
const CELL: usize = 48;
const END: usize = 49;

/// A recognised table: rows of cells, each spanning one or more columns and rows.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Table {
    pub rows: Vec<Vec<Cell>>,
}

/// A table cell's spans (each 1 to 20, the network's range).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub colspan: u32,
    pub rowspan: u32,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { colspan: 1, rowspan: 1 }
    }
}

impl Table {
    /// How many cells the table has.
    pub fn cells(&self) -> usize {
        self.rows.iter().map(Vec::len).sum()
    }

    /// The table as HTML, with `texts` as the cells' texts in row order, left to right (a cell
    /// beyond them is empty). The texts are escaped.
    pub fn html(&self, texts: &[String]) -> String {
        let mut texts = texts.iter();
        let mut out = String::from("<table>");
        for row in &self.rows {
            out.push_str("<tr>");
            for cell in row {
                out.push_str("<td");
                if cell.colspan > 1 {
                    out.push_str(&format!(" colspan=\"{}\"", cell.colspan));
                }
                if cell.rowspan > 1 {
                    out.push_str(&format!(" rowspan=\"{}\"", cell.rowspan));
                }
                out.push('>');
                for c in texts.next().map_or("", String::as_str).chars() {
                    match c {
                        '&' => out.push_str("&amp;"),
                        '<' => out.push_str("&lt;"),
                        '>' => out.push_str("&gt;"),
                        c => out.push(c),
                    }
                }
                out.push_str("</td>");
            }
            out.push_str("</tr>");
        }
        out.push_str("</table>");
        out
    }
}

/// A table as the network read it: its structure, and each cell's box in page picture pixels
/// `[left, top, right, bottom]` in row order (`None` where the network gave no usable box).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Found {
    pub table: Table,
    pub rects: Vec<Option<[f32; 4]>>,
}

/// The table structure model, loaded.
pub(crate) struct TableModel {
    model: Model,
    input: NodeId,
    boxes: NodeId,
    tokens: NodeId,
}

impl TableModel {
    pub fn load(path: &Path) -> Result<TableModel, OcrError> {
        let failed = |e: String| OcrError::Load(path.display().to_string(), e);
        let model = Model::load_file(path).map_err(|e| failed(e.to_string()))?;
        let node =
            |name: &str| model.find_node(name).ok_or_else(|| failed(format!("it has no node {name:?}: not the SLANet_plus model PdfCraft expects")));
        let (input, boxes, tokens) = (node(INPUT_NODE)?, node(BOXES_NODE)?, node(TOKENS_NODE)?);
        Ok(TableModel { model, input, boxes, tokens })
    }

    /// The table in `rect` (page picture pixels) of a page picture: `pixels` holds `channels`
    /// (1 or 3) bytes a pixel, row by row, `width` × `height` pixels. `None` when the rectangle
    /// holds no picture or the network finds no cell.
    pub fn read(&self, pixels: &[u8], channels: usize, width: usize, height: usize, rect: [f32; 4]) -> Result<Option<Found>, OcrError> {
        let failed = |e: String| OcrError::Recognize(format!("reading a table: {e}"));
        let Some(crop) = Crop::new(rect, width, height) else { return Ok(None) };
        let input =
            network_input(pixels, channels, width, height, &crop).ok_or_else(|| failed("the picture's size does not match its pixels".into()))?;
        let image = NdTensor::from_data([1, 3, SIDE, SIDE], input);
        let mut out = self.model.run(vec![(self.input, image.into())], &[self.boxes, self.tokens], None).map_err(|e| failed(e.to_string()))?;
        let mut take = || -> Result<NdTensor<f32, 3>, OcrError> {
            let value = out.pop().ok_or_else(|| failed("the model returned too little".into()))?;
            value.try_into().map_err(|e: rten::TryFromValueError| failed(e.to_string()))
        };
        // Popped from the back: the scores come out last.
        let (tokens, boxes) = (take()?, take()?);
        let [_, steps, classes] = tokens.shape();
        let [_, box_steps, box_values] = boxes.shape();
        let scores = tokens.to_vec();
        // Boxes of another shape are not read: the cells keep no box.
        let corners = if box_steps == steps && box_values == BOX_VALUES { boxes.to_vec() } else { Vec::new() };
        let scale = crop.scale();
        Ok(decode(&scores, &corners, steps, classes, |x, y| (x * SIDE as f32 / scale + crop.left as f32, y * SIDE as f32 / scale + crop.top as f32)))
    }
}

/// The whole pixels a table rectangle covers, inside the picture.
struct Crop {
    left: usize,
    top: usize,
    width: usize,
    height: usize,
}

impl Crop {
    fn new(rect: [f32; 4], width: usize, height: usize) -> Option<Crop> {
        if !rect.iter().all(|v| v.is_finite()) {
            return None;
        }
        // Finite and clamped to the picture, so the casts are exact.
        let x = |v: f32| v.clamp(0.0, width as f32) as usize;
        let y = |v: f32| v.clamp(0.0, height as f32) as usize;
        let (left, top, right, bottom) = (x(rect[0].floor()), y(rect[1].floor()), x(rect[2].ceil()), y(rect[3].ceil()));
        // `then`, not `then_some`: the sizes may be computed only once they are known to be positive.
        (right > left && bottom > top).then(|| Crop { left, top, width: right - left, height: bottom - top })
    }

    /// Network pixels per picture pixel: the longer side becomes [`SIDE`].
    fn scale(&self) -> f32 {
        SIDE as f32 / self.width.max(self.height).max(1) as f32
    }
}

/// The network's input: `crop` of the picture scaled so its longer side is [`SIDE`] pixels,
/// normalised, in the top left corner of a [`SIDE`] × [`SIDE`] square padded with zeros, channels
/// first as B, G, R. `None` when `pixels` is not a `width` × `height` picture of 1 or 3 channels or
/// `crop` is not inside it.
fn network_input(pixels: &[u8], channels: usize, width: usize, height: usize, crop: &Crop) -> Option<Vec<f32>> {
    if !matches!(channels, 1 | 3) || width.checked_mul(height)?.checked_mul(channels)? != pixels.len() {
        return None;
    }
    if crop.width == 0 || crop.height == 0 || crop.left.checked_add(crop.width)? > width || crop.top.checked_add(crop.height)? > height {
        return None;
    }
    let scale = crop.scale();
    // At least one pixel and at most SIDE: the longer side is SIDE exactly.
    let fit = |n: usize| ((n as f32 * scale).round() as usize).clamp(1, SIDE);
    let (out_w, out_h) = (fit(crop.width), fit(crop.height));
    let (wx, wy) = (axis_weights(crop.width, out_w), axis_weights(crop.height, out_h));
    // Inside the picture (checked above), so the index fits in `pixels`.
    let at =
        |x: usize, y: usize, c: usize| pixels.get(((crop.top + y) * width + crop.left + x) * channels + c).copied().map_or(0.0, f32::from) / 255.0;
    // Across first: `mid` is the crop's rows, each out_w wide.
    let mut mid = vec![0.0_f32; crop.height * out_w * channels];
    for y in 0..crop.height {
        for (ox, taps) in wx.iter().enumerate() {
            for c in 0..channels {
                if let Some(slot) = mid.get_mut((y * out_w + ox) * channels + c) {
                    *slot = taps.iter().map(|&(x, k)| k * at(x, y, c)).sum();
                }
            }
        }
    }
    let mut out = vec![0.0_f32; 3 * SIDE * SIDE];
    for (oy, taps) in wy.iter().enumerate() {
        for ox in 0..out_w {
            for plane in 0..3 {
                // Planes B, G, R from an RGB picture; a grey one fills all three.
                let c = if channels == 1 { 0 } else { 2 - plane };
                let v: f32 = taps.iter().map(|&(y, k)| k * mid.get((y * out_w + ox) * channels + c).copied().unwrap_or(0.0)).sum();
                if let (Some(slot), Some(mean), Some(std)) = (out.get_mut((plane * SIDE + oy) * SIDE + ox), MEAN.get(plane), STD.get(plane)) {
                    *slot = (v - mean) / std;
                }
            }
        }
    }
    Some(out)
}

/// The table in the network's output: `scores` holds `steps` rows of `classes` scores, `corners`
/// (empty when the boxes are unusable) `steps` rows of [`BOX_VALUES`] box coordinates, and
/// `to_page` maps a box corner to page picture pixels. Each step's token is its best-scored class;
/// reading stops at the end token, at a step whose scores are all NaN, after [`MAX_STEPS`] or when
/// the output runs out. Tokens out of place are read leniently: a cell outside a row starts one, a
/// span outside a cell or a stray end is ignored. `None` when the output holds no cell or has the
/// wrong number of classes.
fn decode(scores: &[f32], corners: &[f32], steps: usize, classes: usize, to_page: impl Fn(f32, f32) -> (f32, f32)) -> Option<Found> {
    if classes != CLASSES {
        return None;
    }
    let mut table = Table::default();
    let mut rects = Vec::new();
    let mut row: Vec<Cell> = Vec::new();
    // A cell opened by `<td` and not yet closed, with its box.
    let mut open: Option<(Cell, Option<[f32; 4]>)> = None;
    let rect_at = |step: usize| -> Option<[f32; 4]> {
        let at = step.checked_mul(BOX_VALUES)?;
        let v = corners.get(at..at.checked_add(BOX_VALUES)?)?;
        let (mut l, mut t, mut r, mut b) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for &[x, y] in v.as_chunks::<2>().0 {
            let (x, y) = to_page(x, y);
            if !x.is_finite() || !y.is_finite() {
                return None;
            }
            (l, t, r, b) = (l.min(x), t.min(y), r.max(x), b.max(y));
        }
        (r > l && b > t).then_some([l, t, r, b])
    };
    for step in 0..steps.min(MAX_STEPS) {
        let Some(at) = step.checked_mul(classes) else { break };
        let Some(row_scores) = scores.get(at..at.saturating_add(classes)) else { break };
        let best = row_scores.iter().enumerate().filter(|(_, v)| !v.is_nan()).max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i);
        let Some(token) = best else { break };
        let close = |open: &mut Option<(Cell, Option<[f32; 4]>)>, row: &mut Vec<Cell>, rects: &mut Vec<Option<[f32; 4]>>| {
            if let Some((cell, rect)) = open.take() {
                row.push(cell);
                rects.push(rect);
            }
        };
        match token {
            END => break,
            START => {}
            ROW | ROW_END => {
                close(&mut open, &mut row, &mut rects);
                if !row.is_empty() {
                    table.rows.push(std::mem::take(&mut row));
                }
            }
            CELL_OPEN => {
                close(&mut open, &mut row, &mut rects);
                open = Some((Cell::default(), rect_at(step)));
            }
            CELL_END => close(&mut open, &mut row, &mut rects),
            CELL => {
                close(&mut open, &mut row, &mut rects);
                row.push(Cell::default());
                rects.push(rect_at(step));
            }
            t if COLSPAN.contains(&t) => {
                if let Some((cell, _)) = open.as_mut() {
                    // 10 is colspan 2: in range, so the cast is exact.
                    cell.colspan = (t - COLSPAN.start() + 2) as u32;
                }
            }
            t if ROWSPAN.contains(&t) => {
                if let Some((cell, _)) = open.as_mut() {
                    cell.rowspan = (t - ROWSPAN.start() + 2) as u32;
                }
            }
            // `<thead>`, `</thead>`, `<tbody>`, `</tbody>` and `>` carry nothing a cell needs.
            _ => {}
        }
    }
    if let Some((cell, rect)) = open.take() {
        row.push(cell);
        rects.push(rect);
    }
    if !row.is_empty() {
        table.rows.push(row);
    }
    (table.cells() > 0).then_some(Found { table, rects })
}

/// Which of a page's paragraphs to read as tables, given the label of the region each one is: the
/// `table` regions, the first [`MAX_TABLES`] of them.
pub(crate) fn to_read<'a>(labels: impl IntoIterator<Item = Option<&'a str>>) -> Vec<bool> {
    let mut left = MAX_TABLES;
    labels
        .into_iter()
        .map(|label| {
            let read = label == Some("table") && left > 0;
            if read {
                left -= 1;
            }
            read
        })
        .collect()
}

/// The words of `lines` put into the cells whose boxes are `rects` (row order): each word into the
/// cell its box overlaps most or, overlapping none, the nearest one. One line a cell, in the same
/// order, its words in the order they came (a cell without words has an empty line). When no
/// cell has a box nothing says where a word belongs, and `lines` come back as the error.
pub(crate) fn fill(rects: &[Option<[f32; 4]>], lines: Vec<Line>) -> Result<Vec<Line>, Vec<Line>> {
    let Some(first) = rects.iter().position(Option::is_some) else { return Err(lines) };
    let mut cells: Vec<Line> = vec![Line::default(); rects.len()];
    // `max(0.0)` also turns a NaN side into 0.
    let area = |r: [f32; 4]| (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0);
    for word in lines.into_iter().flat_map(|l| l.words) {
        let w = word.rect;
        let overlap = |r: [f32; 4]| area([w[0].max(r[0]), w[1].max(r[1]), w[2].min(r[2]), w[3].min(r[3])]);
        let (cx, cy) = ((w[0] + w[2]) / 2.0, (w[1] + w[3]) / 2.0);
        let distance = |r: [f32; 4]| {
            let dx = (r[0] - cx).max(cx - r[2]).max(0.0);
            let dy = (r[1] - cy).max(cy - r[3]).max(0.0);
            dx * dx + dy * dy
        };
        let boxed = || rects.iter().enumerate().filter_map(|(i, r)| Some((i, (*r)?)));
        // The first of equally good cells wins: `max_by` would take the last.
        let most = boxed()
            .map(|(i, r)| (i, overlap(r)))
            .filter(|(_, v)| v.is_finite() && *v > 0.0)
            .fold(None, |best: Option<(usize, f32)>, (i, v)| if best.is_none_or(|(_, b)| v > b) { Some((i, v)) } else { best });
        let nearest = || {
            boxed()
                .map(|(i, r)| (i, distance(r)))
                .filter(|(_, d)| d.is_finite())
                .fold(None, |best: Option<(usize, f32)>, (i, d)| if best.is_none_or(|(_, b)| d < b) { Some((i, d)) } else { best })
        };
        // A word with a broken box goes to the first cell that has one.
        let cell = most.or_else(nearest).map_or(first, |(i, _)| i);
        if let Some(line) = cells.get_mut(cell) {
            line.words.push(word);
        }
    }
    Ok(cells)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Word;

    /// The network output for `tokens` (one step each, that class scored 1, the rest 0), with
    /// `boxes` (`[left, top, right, bottom]` fractions) as the step's box, or none.
    fn output(tokens: &[usize], boxes: &[Option<[f32; 4]>]) -> (Vec<f32>, Vec<f32>) {
        let mut scores = vec![0.0; tokens.len() * CLASSES];
        let mut corners = vec![0.0; tokens.len() * BOX_VALUES];
        for (step, &t) in tokens.iter().enumerate() {
            scores[step * CLASSES + t] = 1.0;
            if let Some(Some([x0, y0, x1, y1])) = boxes.get(step) {
                corners[step * BOX_VALUES..(step + 1) * BOX_VALUES].copy_from_slice(&[*x0, *y0, *x1, *y0, *x1, *y1, *x0, *y1]);
            }
        }
        (scores, corners)
    }

    fn identity(x: f32, y: f32) -> (f32, f32) {
        (x, y)
    }

    fn spans(table: &Table) -> Vec<Vec<(u32, u32)>> {
        table.rows.iter().map(|r| r.iter().map(|c| (c.colspan, c.rowspan)).collect()).collect()
    }

    /// The merged `<td></td>` (48) is a cell, `<tr>` … `</tr>` a row, and `<td` with spans, `>` and
    /// `</td>` a spanning cell; reading stops at the end token (49), and each cell keeps the box
    /// of the step that opened it.
    #[test]
    fn tokens_make_rows_of_cells() {
        let b = |x: f32| Some([x, 0.1, x + 0.1, 0.2]);
        // <tbody> <tr> <td colspan="2" rowspan="3"> </td> 48 </tr> <tr> 48 48 </tr> end 48
        // The class numbers as the network writes them, not the constants: they pin the dictionary.
        let tokens = [0, 3, 5, 7, 10, 30, 8, 9, 48, 6, 5, 48, 48, 6, 49, 48];
        let boxes = [None, None, None, b(0.0), b(0.5), None, None, None, b(0.3), None, None, b(0.4), b(0.6), None, None, b(0.9)];
        let (scores, corners) = output(&tokens, &boxes);
        let found = decode(&scores, &corners, tokens.len(), CLASSES, identity).unwrap();
        assert_eq!(spans(&found.table), [vec![(2, 3), (1, 1)], vec![(1, 1), (1, 1)]]);
        assert_eq!(found.rects, [b(0.0), b(0.3), b(0.4), b(0.6)]);
        // Box corners in any order make the same rectangle, mapped to the page.
        let (scores, mut corners) = output(&[CELL], &[]);
        corners.copy_from_slice(&[0.5, 0.25, 0.25, 0.75, 0.0, 0.5, 0.5, 0.5]);
        let found = decode(&scores, &corners, 1, CLASSES, |x, y| (x * 100.0 + 10.0, y * 100.0)).unwrap();
        assert_eq!(found.rects, [Some([10.0, 25.0, 60.0, 75.0])]);
    }

    /// Every span the dictionary has maps to its number: colspan 2 to 20, rowspan 2 to 20.
    #[test]
    fn span_tokens_are_the_dictionary_order() {
        for (t, colspan, rowspan) in [(10, 2, 1), (28, 20, 1), (29, 1, 2), (47, 1, 20)] {
            let (scores, corners) = output(&[CELL_OPEN, t, CELL_END], &[]);
            let found = decode(&scores, &corners, 3, CLASSES, identity).unwrap();
            assert_eq!(spans(&found.table), [vec![(colspan, rowspan)]], "token {t}");
        }
    }

    /// Output out of order or broken is read leniently and never panics: cells outside a row, a
    /// span outside a cell, a stray `</td>`, a `<td` never closed, NaN scores (a step of only NaN
    /// ends the table), boxes that are NaN or empty, and output shorter than it says.
    #[test]
    fn broken_output_is_read_leniently() {
        let (scores, corners) = output(&[CELL, 12, CELL_END, ROW_END, ROW_END, CELL_OPEN, 31], &[Some([0.0, 0.0, 0.0, 1.0])]);
        let found = decode(&scores, &corners, 7, CLASSES, identity).unwrap();
        assert_eq!(spans(&found.table), [vec![(1, 1)], vec![(1, 4)]]);
        // An empty box is no box.
        assert_eq!(found.rects, [None, None]);
        // NaN scores: the best of the rest wins; a step of only NaN ends the table.
        let (mut scores, mut corners) = output(&[CELL, CELL, CELL], &[Some([0.0, 0.0, 0.5, 0.5])]);
        assert_eq!(decode(&scores, &corners, 3, CLASSES, identity).unwrap().rects[0], Some([0.0, 0.0, 0.5, 0.5]));
        scores[0] = f32::NAN;
        scores[CLASSES..2 * CLASSES].fill(f32::NAN);
        corners[0] = f32::NAN;
        let found = decode(&scores, &corners, 3, CLASSES, identity).unwrap();
        assert_eq!((found.table.cells(), found.rects.clone()), (1, vec![None]));
        // Output shorter than its shape says, or with no box at all.
        let (scores, _) = output(&[CELL, CELL], &[]);
        assert_eq!(decode(&scores, &[], 9, CLASSES, identity).unwrap().table.cells(), 2);
        assert_eq!(decode(&scores[..CLASSES + 3], &[1.0; 3], 2, CLASSES, identity).unwrap().table.cells(), 1);
    }

    /// No cell, an empty output, the wrong number of classes or no step at all is no table.
    #[test]
    fn no_cell_is_no_table() {
        let (scores, corners) = output(&[START, ROW, ROW_END, END, CELL], &[]);
        assert_eq!(decode(&scores, &corners, 5, CLASSES, identity), None);
        assert_eq!(decode(&[], &[], 0, CLASSES, identity), None);
        assert_eq!(decode(&[], &[], usize::MAX, CLASSES, identity), None);
        let (scores, corners) = output(&[CELL], &[]);
        assert_eq!(decode(&scores, &corners, 1, CLASSES - 1, identity), None);
        assert_eq!(decode(&scores, &corners, 1, usize::MAX, identity), None);
    }

    /// The table is capped at the network's own length: a model that never ends makes at most
    /// MAX_STEPS cells.
    #[test]
    fn a_table_that_never_ends_is_capped() {
        let tokens = vec![CELL; MAX_STEPS * 3];
        let (scores, corners) = output(&tokens, &[]);
        let found = decode(&scores, &corners, tokens.len(), CLASSES, identity).unwrap();
        assert_eq!((found.table.cells(), found.rects.len()), (MAX_STEPS, MAX_STEPS));
    }

    /// Only `table` regions are read as tables, and at most MAX_TABLES of a page, the first ones.
    #[test]
    fn a_page_reads_at_most_so_many_tables() {
        assert_eq!(to_read([Some("text"), None, Some("table"), Some("image")]), [false, false, true, false]);
        let labels: Vec<Option<&str>> = (0..2 * MAX_TABLES + 9).map(|i| Some(if i % 2 == 0 { "table" } else { "text" })).collect();
        let read = to_read(labels.iter().copied());
        assert_eq!(read.iter().filter(|r| **r).count(), MAX_TABLES);
        assert!(read.iter().take(2 * MAX_TABLES - 1).step_by(2).all(|r| *r) && !read.iter().skip(2 * MAX_TABLES).any(|r| *r));
    }

    fn word(text: &str, rect: [f32; 4]) -> Word {
        Word { text: text.into(), rect }
    }

    fn texts(cells: &[Line]) -> Vec<String> {
        cells.iter().map(Line::text).collect()
    }

    /// Each word goes to the cell it overlaps most, a word in no cell to the nearest, and the
    /// words of a cell keep their order, also when one recognised line runs across two cells.
    #[test]
    fn words_go_to_the_cell_they_overlap_most() {
        let rects = [Some([0.0, 0.0, 100.0, 20.0]), Some([100.0, 0.0, 200.0, 20.0]), None, Some([0.0, 20.0, 200.0, 40.0])];
        let lines = vec![
            Line { words: vec![word("Name", [5.0, 2.0, 50.0, 18.0]), word("Berlin", [110.0, 2.0, 160.0, 18.0])] },
            // 30 % in the first cell, 70 % in the second.
            Line { words: vec![word("straddles", [70.0, 5.0, 170.0, 15.0])] },
            Line { words: vec![word("below", [10.0, 25.0, 50.0, 35.0]), word("far", [500.0, 25.0, 520.0, 35.0])] },
            // Right of the first row, nearer to it than to the second.
            Line { words: vec![word("outside", [210.0, 4.0, 240.0, 12.0])] },
        ];
        let cells = fill(&rects, lines).unwrap();
        assert_eq!(texts(&cells), ["Name", "Berlin straddles outside", "", "below far"]);
    }

    /// Words with broken boxes are kept (in the first cell with a box); with no box at all there
    /// is no table to fill, and no word is lost either way.
    #[test]
    fn broken_boxes_lose_no_word() {
        let rects = [None, Some([0.0, 0.0, 10.0, 10.0]), Some([f32::NAN, 0.0, 20.0, 10.0])];
        let lines = vec![Line { words: vec![word("nan", [f32::NAN, 0.0, 5.0, 5.0]), word("inf", [f32::NEG_INFINITY, 0.0, f32::INFINITY, 5.0])] }];
        let cells = fill(&rects, lines.clone()).unwrap();
        assert_eq!(cells.iter().map(|l| l.words.len()).sum::<usize>(), 2);
        // NaN is not equal to itself: compare the texts.
        let back = fill(&[None, None], lines).unwrap_err();
        assert_eq!(back.iter().flat_map(|l| &l.words).map(|w| w.text.as_str()).collect::<Vec<_>>(), ["nan", "inf"]);
        assert_eq!(fill(&[], Vec::new()), Err(Vec::new()));
    }

    /// HTML with the spans, the cell texts escaped, and missing texts as empty cells.
    #[test]
    fn html_carries_spans_and_escaped_text() {
        let table = Table { rows: vec![vec![Cell { colspan: 2, rowspan: 1 }], vec![Cell::default(), Cell { colspan: 1, rowspan: 3 }]] };
        let html = table.html(&["A & B".into(), "<x>".into()]);
        assert_eq!(html, "<table><tr><td colspan=\"2\">A &amp; B</td></tr><tr><td>&lt;x&gt;</td><td rowspan=\"3\"></td></tr></table>");
        assert_eq!(Table::default().html(&[]), "<table></table>");
    }

    /// The input is the crop scaled to 488 on its longer side, B, G, R planes normalised, the
    /// rest of the square zero; a grey picture fills all three planes, and a crop or a picture
    /// that does not fit is refused.
    #[test]
    fn the_input_is_the_crop_scaled_normalised_and_padded() {
        // 4 × 2 RGB picture: the right half pure red. Crop the whole picture: 488 × 244.
        let rgb: Vec<u8> = (0..8).flat_map(|i| if i % 4 < 2 { [0, 0, 0] } else { [255, 0, 0] }).collect();
        let crop = Crop::new([0.0, 0.0, 4.0, 2.0], 4, 2).unwrap();
        let input = network_input(&rgb, 3, 4, 2, &crop).unwrap();
        assert_eq!(input.len(), 3 * SIDE * SIDE);
        let at = |plane: usize, x: usize, y: usize| input[(plane * SIDE + y) * SIDE + x];
        let norm = |v: f32, plane: usize| (v - MEAN[plane]) / STD[plane];
        // Red lands in the last plane (R of B, G, R).
        assert!((at(2, SIDE - 1, 0) - norm(1.0, 2)).abs() < 1e-5 && (at(0, SIDE - 1, 0) - norm(0.0, 0)).abs() < 1e-5);
        assert!((at(2, 0, 243) - norm(0.0, 2)).abs() < 1e-5);
        // Below the picture: padding.
        assert_eq!((at(0, 0, 244), at(2, SIDE - 1, SIDE - 1)), (0.0, 0.0));
        // A crop of the right half, grey: one value, all three planes.
        let grey = [0u8, 0, 255, 255, 0, 0, 255, 255];
        let crop = Crop::new([2.0, 0.0, 4.0, 2.0], 4, 2).unwrap();
        let input = network_input(&grey, 1, 4, 2, &crop).unwrap();
        for plane in 0..3 {
            assert!((input[plane * SIDE * SIDE + SIDE * (SIDE - 1) + SIDE - 1] - norm(1.0, plane)).abs() < 1e-5);
        }
        // Rectangles that hold no pixel, are broken or reach outside are clipped or refused.
        assert!(Crop::new([5.0, 0.0, 9.0, 2.0], 4, 2).is_none());
        assert!(Crop::new([f32::NAN, 0.0, 4.0, 2.0], 4, 2).is_none());
        assert!(Crop::new([3.0, 0.0, 1.0, 2.0], 4, 2).is_none());
        let clipped = Crop::new([-9.0, 0.5, 99.0, 1.5], 4, 2).unwrap();
        assert_eq!((clipped.left, clipped.top, clipped.width, clipped.height), (0, 0, 4, 2));
        let outside = Crop { left: 3, top: 0, width: 2, height: 2 };
        assert_eq!(network_input(&rgb, 3, 4, 2, &outside), None);
        assert_eq!(network_input(&rgb[1..], 3, 4, 2, &crop), None);
        assert_eq!(network_input(&rgb, 4, 2, 2, &crop), None);
        assert_eq!(network_input(&rgb, 3, usize::MAX, 2, &crop), None);
    }
}

#[cfg(test)]
pub(crate) mod model_tests {
    use super::*;
    use crate::Word;

    /// Blocky 5 × 7 digits.
    const DIGITS: [[&str; 7]; 4] = [
        [".XXX.", "X...X", "X..XX", "X.X.X", "XX..X", "X...X", ".XXX."],
        ["..X..", ".XX..", "..X..", "..X..", "..X..", "..X..", ".XXX."],
        [".XXX.", "X...X", "....X", "...X.", "..X..", ".X...", "XXXXX"],
        ["XXXXX", "...X.", "..X..", "...X.", "....X", "X...X", ".XXX."],
    ];

    /// A ruled table of `rows` × `columns` cells, each holding a two-digit number, drawn black on
    /// white as RGB: its size, pixels, and each cell's word with its box.
    pub(crate) fn drawn_table(rows: usize, columns: usize) -> (usize, usize, Vec<u8>, Vec<Word>) {
        let (cell_w, cell_h, margin, dot) = (180, 48, 16, 3);
        let (w, h) = (2 * margin + columns * cell_w + 1, 2 * margin + rows * cell_h + 1);
        let mut px = vec![255u8; w * h * 3];
        let mut ink = |x: usize, y: usize| px[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&[0, 0, 0]);
        for r in 0..=rows {
            for x in margin..=margin + columns * cell_w {
                ink(x, margin + r * cell_h);
            }
        }
        for c in 0..=columns {
            for y in margin..=margin + rows * cell_h {
                ink(margin + c * cell_w, y);
            }
        }
        let mut words = Vec::new();
        for r in 0..rows {
            for c in 0..columns {
                let n = r * columns + c;
                let digits = [n / 4 % 4, n % 4];
                let (x0, y0) = (margin + c * cell_w + 20, margin + r * cell_h + 14);
                for (i, d) in digits.iter().enumerate() {
                    for (ry, row) in DIGITS[*d].iter().enumerate() {
                        for (rx, b) in row.bytes().enumerate() {
                            if b == b'X' {
                                for (dx, dy) in (0..dot).flat_map(|dx| (0..dot).map(move |dy| (dx, dy))) {
                                    ink(x0 + (i * 6 + rx) * dot + dx, y0 + ry * dot + dy);
                                }
                            }
                        }
                    }
                }
                let rect = [x0 as f32, y0 as f32, (x0 + 11 * dot) as f32, (y0 + 7 * dot) as f32];
                words.push(Word { text: format!("{}{}", digits[0], digits[1]), rect });
            }
        }
        (w, h, px, words)
    }

    /// End to end, when the table structure model is installed (`cargo xtask models`): a drawn
    /// table comes back with its rows and columns, and every word lands in its own cell.
    #[test]
    fn reads_a_drawn_table() {
        let Some(path) = crate::Models::find_tables() else {
            eprintln!("skipped: table structure model not installed");
            return;
        };
        let model = TableModel::load(&path).unwrap();
        let (w, h, px, words) = drawn_table(4, 3);
        let found = model.read(&px, 3, w, h, [0.0, 0.0, w as f32, h as f32]).unwrap().unwrap();
        assert_eq!(found.table.rows.iter().map(Vec::len).collect::<Vec<_>>(), [3, 3, 3, 3]);
        let lines = words.iter().map(|w| Line { words: vec![w.clone()] }).collect();
        let cells = fill(&found.rects, lines).unwrap();
        assert_eq!(cells.iter().map(Line::text).collect::<Vec<_>>(), words.iter().map(|w| w.text.clone()).collect::<Vec<_>>());
    }
}
