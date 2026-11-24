//! XFA 3.3, ch. 12 and the `barcode` element in ch. 17. PDF417 symbol encoding is
//! supplied by rxing. The XFA field determines the symbol's size in PDF points.

use rxing::common::CharacterSet;
use rxing::pdf417::encoder::{Compaction, PDF417, pdf_417_high_level_encoder::encodeHighLevel};

use crate::model::Rect;

const MAX_CODEWORDS: usize = 928;
/// The largest possible payload is 2,710 digits in numeric compaction.
const MAX_PAYLOAD_BYTES: usize = 2710;
const MIN_ROWS: u8 = 3;
const MAX_ROWS: u8 = 90;
const MIN_COLS: u8 = 1;
const MAX_COLS: u8 = 30;
const QUIET_MODULES: f64 = 2.0;
/// The PDF writer rounds page coordinates, so decline sizes that would collapse bars.
const MIN_DRAWN_MODULE: f64 = 0.01;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pdf417Options {
    pub(crate) module_width: f64,
    pub(crate) row_height: f64,
    pub(crate) error_correction: u8,
    pub(crate) dimensions: Option<(u8, u8)>,
    pub(crate) row_column_ratio: Option<f64>,
    pub(crate) truncated: bool,
}

/// A complete symbol, including its white quiet zone, kept together across page breaks.
#[derive(Clone, Debug, PartialEq)]
pub struct Pdf417Symbol {
    pub(crate) rect: Rect,
    modules_per_row: usize,
    module_width: f64,
    row_height: f64,
    modules: Vec<bool>,
}

impl Pdf417Symbol {
    pub(crate) fn module_count(&self) -> usize {
        self.modules.len()
    }

    /// Merge consecutive black modules in one row into a single vector rectangle.
    pub(crate) fn bars(&self) -> impl Iterator<Item = Rect> + '_ {
        let quiet = QUIET_MODULES * self.module_width;
        self.modules.chunks_exact(self.modules_per_row).enumerate().flat_map(move |(row, modules)| {
            let mut col = 0;
            std::iter::from_fn(move || {
                col += modules.get(col..)?.iter().position(|&black| black)?;
                let start = col;
                col += modules.get(col..)?.iter().take_while(|&&black| black).count();
                Some(Rect::new(
                    self.rect.x + quiet + start as f64 * self.module_width,
                    self.rect.y + quiet + row as f64 * self.row_height,
                    (col - start) as f64 * self.module_width,
                    self.row_height,
                ))
            })
        })
    }
}

struct Size {
    rows: u8,
    cols: u8,
    modules_per_row: usize,
    capacity: usize,
    width: f64,
    height: f64,
    scale: f64,
    ratio_error: f64,
}

impl Pdf417Options {
    fn size(self, rows: u8, cols: u8, needed: usize, field: Rect) -> Option<Size> {
        let capacity = usize::from(rows) * usize::from(cols);
        if !(MIN_ROWS..=MAX_ROWS).contains(&rows) || !(MIN_COLS..=MAX_COLS).contains(&cols) || !(needed..=MAX_CODEWORDS).contains(&capacity) {
            return None;
        }
        let overhead = if self.truncated { 35 } else { 69 };
        let modules_per_row = 17 * usize::from(cols) + overhead;
        let quiet = 2.0 * QUIET_MODULES * self.module_width;
        let width = modules_per_row as f64 * self.module_width + quiet;
        let height = f64::from(rows) * self.row_height + quiet;
        let scale = (field.w / width).min(field.h / height).min(1.0);
        if self.module_width * scale < MIN_DRAWN_MODULE {
            return None;
        }
        let ratio_error =
            self.row_column_ratio.map_or_else(|| (width / height / 3.0).ln().abs(), |ratio| (f64::from(rows) / f64::from(cols) / ratio).ln().abs());
        Some(Size { rows, cols, modules_per_row, capacity, width: width * scale, height: height * scale, scale, ratio_error })
    }

    pub(crate) fn encode(self, value: &str, field: Rect) -> Option<Pdf417Symbol> {
        if value.is_empty() || value.len() > MAX_PAYLOAD_BYTES {
            return None;
        }
        let (compaction, encoding) = if value.is_ascii() { (Compaction::AUTO, None) } else { (Compaction::BYTE, Some(CharacterSet::UTF8)) };
        let codewords = encodeHighLevel(value, compaction, encoding, false).ok()?;
        let needed = codewords.chars().count() + 1 + (1 << (self.error_correction + 1));

        let size = if let Some((rows, cols)) = self.dimensions {
            self.size(rows, cols, needed, field)?
        } else {
            (MIN_COLS..=MAX_COLS)
                .filter_map(|cols| {
                    let rows = u8::try_from(needed.div_ceil(usize::from(cols))).ok()?.max(MIN_ROWS);
                    self.size(rows, cols, needed, field)
                })
                .max_by(|a, b| {
                    a.scale.total_cmp(&b.scale).then_with(|| b.ratio_error.total_cmp(&a.ratio_error)).then_with(|| b.capacity.cmp(&a.capacity))
                })?
        };
        let mut encoder = PDF417::with_compact_hint(self.truncated);
        let cols = u32::from(size.cols);
        let rows = u32::from(size.rows);
        encoder.setDimensions(cols, cols, rows, rows);
        encoder.setCompaction(compaction);
        encoder.setEncoding(encoding);
        encoder.generateBarcodeLogic(value, u32::from(self.error_correction)).ok()?;
        // rxing's matrix lists the bottom row first. XFA's y axis increases downwards.
        let matrix = encoder.getBarcodeMatrix().as_ref()?.getMatrix();
        let modules = matrix.into_iter().rev().flatten().map(|module| module != 0).collect();

        Some(Pdf417Symbol {
            rect: Rect::new(field.x + (field.w - size.width) / 2.0, field.y + (field.h - size.height) / 2.0, size.width, size.height),
            modules_per_row: size.modules_per_row,
            module_width: self.module_width * size.scale,
            row_height: self.row_height * size.scale,
            modules,
        })
    }
}
