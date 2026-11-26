//! Advanced QR Code & Barcode Scanner for PDF Craft (L6 Engine).
//!
//! Provides automated detection, pattern analysis, and payload decoding for QR codes
//! and 1D barcodes (Code 128, Code 39, EAN-13) present on PDF pages (both embedded rasters
//! and vector-rendered page buffers), plus pixel-perfect replacement generation matching exact original dimensions.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use crate::Document;
#[cfg(test)]
use crate::qr::QrCode;

/// The recognized barcode or 2D matrix symbology.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CodeKind {
    #[default]
    QrCode,
    Code128,
    Code39,
    Ean13,
    GenericBarcode,
}

impl CodeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::QrCode => "QR Code",
            Self::Code128 => "Code 128 Barcode",
            Self::Code39 => "Code 39 Barcode",
            Self::Ean13 => "EAN-13 Barcode",
            Self::GenericBarcode => "Barcode",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::QrCode => "qr-code",
            _ => "scan",
        }
    }
}

/// An identified and decoded barcode or QR code on a PDF page.
#[derive(Clone, Debug, PartialEq)]
pub struct ScannedCode {
    /// Unique 0-based identifier among codes detected on this page.
    pub id: usize,
    /// 0-based page index.
    pub page: usize,
    /// Symbology detected.
    pub kind: CodeKind,
    /// Decoded text payload.
    pub text: String,
    /// Bounding rectangle in PDF user space: `[min_x, min_y, max_x, max_y]`.
    pub rect: [f64; 4],
    /// Exact pixel dimensions of the source code image: `[width, height]`.
    pub pixel_size: [u32; 2],
    /// Index in `Document::page_images` if matched to an existing page image.
    pub image_index: Option<usize>,
}

/// Gray-scale image buffer with width and height.
pub struct GrayImage {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

impl GrayImage {
    pub fn from_rgba(width: usize, height: usize, rgba: &[u8]) -> Option<Self> {
        if width == 0 || height == 0 || rgba.len() < width.saturating_mul(height).saturating_mul(4) {
            return None;
        }
        let mut pixels = Vec::with_capacity(width.saturating_mul(height));
        for &[r, g, b, a] in rgba.as_chunks::<4>().0.iter().take(width.saturating_mul(height)) {
            if a < 32 {
                // Transparent pixel treated as white background
                pixels.push(255);
            } else {
                let luma = (u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000;
                pixels.push(luma.min(255) as u8);
            }
        }
        Some(Self { width, height, pixels })
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> u8 {
        if x < self.width && y < self.height {
            let idx = y.saturating_mul(self.width).saturating_add(x);
            self.pixels.get(idx).copied().unwrap_or(255)
        } else {
            255
        }
    }
}

/// Binary (black/white) representation of an image.
pub struct BinaryImage {
    pub width: usize,
    pub height: usize,
    pub dark: Vec<bool>,
}

impl BinaryImage {
    pub fn from_gray(gray: &GrayImage) -> Self {
        // Calculate Otsu threshold
        let mut histogram = [0u64; 256];
        for &p in &gray.pixels {
            if let Some(slot) = histogram.get_mut(p as usize) {
                *slot = slot.saturating_add(1);
            }
        }
        let total = gray.pixels.len() as f64;
        let mut sum = 0.0;
        for (i, &count) in histogram.iter().enumerate() {
            sum += (i as f64) * (count as f64);
        }

        let mut sum_b = 0.0;
        let mut w_b = 0.0;
        let mut max_var = 0.0;
        let mut threshold = 128u8;

        for (i, &count) in histogram.iter().enumerate() {
            w_b += count as f64;
            if w_b == 0.0 {
                continue;
            }
            let w_f = total - w_b;
            if w_f == 0.0 {
                break;
            }
            sum_b += (i as f64) * (count as f64);
            let m_b = sum_b / w_b;
            let m_f = (sum - sum_b) / w_f;
            let var = w_b * w_f * (m_b - m_f) * (m_b - m_f);
            if var > max_var {
                max_var = var;
                threshold = i as u8;
            }
        }

        // Clamp threshold to reasonable range [60, 200] so mostly-white shipping labels don't get washed out
        let threshold = threshold.clamp(60, 190);
        let dark = gray.pixels.iter().map(|&p| p <= threshold).collect();
        Self { width: gray.width, height: gray.height, dark }
    }

    #[inline]
    pub fn is_dark(&self, x: usize, y: usize) -> bool {
        if x < self.width && y < self.height {
            let idx = y.saturating_mul(self.width).saturating_add(x);
            self.dark.get(idx).copied().unwrap_or(false)
        } else {
            false
        }
    }
}

// -------------------------------------------------------------------------------------------------
// Code 128 Specification & Decoder
// -------------------------------------------------------------------------------------------------

/// Standard Code 128 symbol patterns (107 total). Each character consists of 6 element widths
/// (3 bars, 3 spaces) summing to 11 modules. Stop (index 106) has 7 elements summing to 13 modules.
const CODE128_PATTERNS: [[u8; 6]; 107] = [
    [2, 1, 2, 2, 2, 2], // 0: SP
    [2, 2, 2, 1, 2, 2], // 1: !
    [2, 2, 2, 2, 2, 1], // 2: "
    [1, 2, 1, 2, 2, 3], // 3: #
    [1, 2, 1, 3, 2, 2], // 4: $
    [1, 3, 1, 2, 2, 2], // 5: %
    [1, 2, 2, 2, 1, 3], // 6: &
    [1, 2, 2, 3, 1, 2], // 7: '
    [1, 3, 2, 2, 1, 2], // 8: (
    [2, 2, 1, 2, 1, 3], // 9: )
    [2, 2, 1, 3, 1, 2], // 10: *
    [2, 3, 1, 2, 1, 2], // 11: +
    [1, 1, 2, 2, 3, 2], // 12: ,
    [1, 2, 2, 1, 3, 2], // 13: -
    [1, 2, 2, 2, 3, 1], // 14: .
    [1, 1, 3, 2, 2, 2], // 15: /
    [1, 2, 3, 1, 2, 2], // 16: 0
    [1, 2, 3, 2, 2, 1], // 17: 1
    [2, 2, 3, 2, 1, 1], // 18: 2
    [2, 2, 1, 1, 3, 2], // 19: 3
    [2, 2, 1, 2, 3, 1], // 20: 4
    [2, 1, 3, 2, 1, 2], // 21: 5
    [2, 2, 3, 1, 1, 2], // 22: 6
    [3, 1, 2, 1, 3, 1], // 23: 7
    [3, 1, 1, 2, 2, 2], // 24: 8
    [3, 2, 1, 1, 2, 2], // 25: 9
    [3, 2, 1, 2, 2, 1], // 26: :
    [3, 1, 2, 2, 1, 2], // 27: ;
    [3, 2, 2, 1, 1, 2], // 28: <
    [3, 2, 2, 2, 1, 1], // 29: =
    [2, 1, 2, 1, 2, 3], // 30: >
    [2, 1, 2, 3, 2, 1], // 31: ?
    [2, 3, 2, 1, 2, 1], // 32: @
    [1, 1, 1, 3, 2, 3], // 33: A
    [1, 3, 1, 1, 2, 3], // 34: B
    [1, 3, 1, 3, 2, 1], // 35: C
    [1, 1, 2, 3, 1, 3], // 36: D
    [1, 3, 2, 1, 1, 3], // 37: E
    [1, 3, 2, 3, 1, 1], // 38: F
    [2, 1, 1, 3, 1, 3], // 39: G
    [2, 3, 1, 1, 1, 3], // 40: H
    [2, 3, 1, 3, 1, 1], // 41: I
    [1, 1, 2, 1, 3, 3], // 42: J
    [1, 1, 2, 3, 3, 1], // 43: K
    [1, 3, 2, 1, 3, 1], // 44: L
    [1, 1, 3, 1, 2, 3], // 45: M
    [1, 1, 3, 3, 2, 1], // 46: N
    [1, 3, 3, 1, 2, 1], // 47: O
    [3, 1, 3, 1, 2, 1], // 48: P
    [2, 1, 1, 3, 3, 1], // 49: Q
    [2, 3, 1, 1, 3, 1], // 50: R
    [2, 1, 3, 1, 1, 3], // 51: S
    [2, 1, 3, 3, 1, 1], // 52: T
    [2, 1, 3, 1, 3, 1], // 53: U
    [3, 1, 1, 1, 2, 3], // 54: V
    [3, 1, 1, 3, 2, 1], // 55: W
    [3, 3, 1, 1, 2, 1], // 56: X
    [3, 1, 2, 1, 1, 3], // 57: Y
    [3, 1, 2, 3, 1, 1], // 58: Z
    [3, 3, 2, 1, 1, 1], // 59: [
    [3, 1, 4, 1, 1, 1], // 60: \
    [2, 2, 1, 4, 1, 1], // 61: ]
    [4, 3, 1, 1, 1, 1], // 62: ^
    [1, 1, 1, 2, 2, 4], // 63: _
    [1, 1, 1, 4, 2, 2], // 64: `
    [1, 2, 1, 1, 2, 4], // 65: a
    [1, 2, 1, 4, 2, 1], // 66: b
    [1, 4, 1, 1, 2, 2], // 67: c
    [1, 4, 1, 2, 2, 1], // 68: d
    [1, 1, 2, 2, 1, 4], // 69: e
    [1, 1, 2, 4, 1, 2], // 70: f
    [1, 2, 2, 1, 1, 4], // 71: g
    [1, 2, 2, 4, 1, 1], // 72: h
    [1, 4, 2, 1, 1, 2], // 73: i
    [1, 4, 2, 2, 1, 1], // 74: j
    [2, 4, 1, 2, 1, 1], // 75: k
    [2, 2, 1, 1, 1, 4], // 76: l
    [4, 1, 3, 1, 1, 1], // 77: m
    [2, 4, 1, 1, 1, 2], // 78: n
    [1, 3, 4, 1, 1, 1], // 79: o
    [1, 1, 1, 2, 4, 2], // 80: p
    [1, 2, 1, 1, 4, 2], // 81: q
    [1, 2, 1, 2, 4, 1], // 82: r
    [1, 1, 4, 2, 1, 2], // 83: s
    [1, 2, 4, 1, 1, 2], // 84: t
    [1, 2, 4, 2, 1, 1], // 85: u
    [4, 1, 1, 2, 1, 2], // 86: v
    [4, 2, 1, 1, 1, 2], // 87: w
    [4, 2, 1, 2, 1, 1], // 88: x
    [2, 1, 2, 1, 4, 1], // 89: y
    [2, 1, 4, 1, 2, 1], // 90: z
    [4, 1, 2, 1, 2, 1], // 91: {
    [1, 1, 1, 1, 4, 3], // 92: |
    [1, 1, 1, 3, 4, 1], // 93: }
    [1, 3, 1, 1, 4, 1], // 94: ~
    [1, 1, 4, 1, 1, 3], // 95: DEL
    [1, 1, 4, 3, 1, 1], // 96: FNC3
    [4, 1, 1, 1, 1, 3], // 97: FNC2
    [4, 1, 1, 3, 1, 1], // 98: SHIFT
    [1, 1, 3, 1, 4, 1], // 99: CODE_C
    [1, 1, 4, 1, 3, 1], // 100: CODE_B
    [3, 1, 1, 1, 4, 1], // 101: CODE_A
    [4, 1, 1, 1, 3, 1], // 102: FNC1
    [2, 1, 1, 4, 1, 2], // 103: START_A
    [2, 1, 1, 2, 1, 4], // 104: START_B
    [2, 1, 1, 2, 3, 2], // 105: START_C
    [2, 3, 3, 1, 1, 1], // 106: STOP (first 6 bars)
];
const CODE128_STOP: [u8; 7] = [2, 3, 3, 1, 1, 1, 2];

fn match_code128_pattern(runs: &[usize]) -> Option<usize> {
    if runs.len() < 6 {
        return None;
    }
    let total: usize = runs.iter().take(6).sum();
    if total < 6 {
        return None;
    }
    let total_f = total as f32;
    let mut best_idx = None;
    let mut best_dist = f32::MAX;

    for (idx, pat) in CODE128_PATTERNS.iter().take(106).enumerate() {
        let mut dist = 0.0f32;
        for i in 0..6 {
            let actual_frac = runs.get(i).copied().unwrap_or(0) as f32 / total_f;
            let ideal_frac = pat.get(i).copied().unwrap_or(0) as f32 / 11.0;
            dist += (actual_frac - ideal_frac).abs();
        }
        if dist < best_dist {
            best_dist = dist;
            best_idx = Some(idx);
        }
    }

    if best_dist < 0.32 { best_idx } else { None }
}

fn is_code128_stop(runs: &[usize]) -> bool {
    if runs.len() < 7 {
        return false;
    }
    let total: usize = runs.iter().take(7).sum();
    if total < 7 {
        return false;
    }
    let total_f = total as f32;
    let mut dist = 0.0f32;
    for i in 0..7 {
        let actual_frac = runs.get(i).copied().unwrap_or(0) as f32 / total_f;
        let ideal_frac = CODE128_STOP.get(i).copied().unwrap_or(0) as f32 / 13.0;
        dist += (actual_frac - ideal_frac).abs();
    }
    dist < 0.35
}

fn has_code128_end_boundary(runs: &[(bool, usize)], stop_start_idx: usize, stop_runs: &[usize]) -> bool {
    let after_idx = stop_start_idx + 7;
    let Some(&(is_dark, trailing_len)) = runs.get(after_idx) else {
        // Scanline ends right at the terminating dark bar of STOP (e.g., tightly cropped raster).
        return true;
    };
    if is_dark {
        return false;
    }
    let stop_total: usize = stop_runs.iter().sum();
    let module_px = (stop_total as f32) / 13.0;
    // Every space inside a Code 128 data/checksum symbol is at most 4 modules wide.
    // Require a trailing quiet zone wider than any internal space (>= 5.5 modules) before committing STOP.
    (trailing_len as f32) >= module_px * 5.5
}

/// Attempt to decode a full Code 128 barcode starting from `start_run_idx` in `runs`.
fn try_decode_code128(runs: &[(bool, usize)], start_run_idx: usize) -> Option<(String, usize, usize)> {
    if start_run_idx + 18 >= runs.len() {
        return None;
    }

    // Must start with a dark bar
    if !runs.get(start_run_idx).is_some_and(|r| r.0) {
        return None;
    }

    let mut raw_runs = Vec::with_capacity(6);
    for i in 0..6 {
        raw_runs.push(runs.get(start_run_idx + i)?.1);
    }

    let start_code = match_code128_pattern(&raw_runs)?;
    if !(103..=105).contains(&start_code) {
        return None;
    }

    let mut current_idx = start_run_idx + 6;
    let mut symbols = vec![start_code];

    while current_idx < runs.len() {
        // Check for 7-element Stop pattern, only committing if end boundary + checksum validate;
        // a tentative stop match whose trailing boundary or checksum fails must still try decoding as a data symbol.
        if current_idx + 7 <= runs.len() {
            let mut stop_runs = [0usize; 7];
            for (i, slot) in stop_runs.iter_mut().enumerate() {
                *slot = runs.get(current_idx + i)?.1;
            }
            if is_code128_stop(&stop_runs)
                && has_code128_end_boundary(runs, current_idx, &stop_runs)
                && let Some(text) = try_finalize_code128_stop(start_code, &symbols)
            {
                return Some((text, start_run_idx, current_idx + 7));
            }
        }

        if current_idx + 6 > runs.len() {
            break;
        }

        let mut sym_runs = Vec::with_capacity(6);
        for i in 0..6 {
            sym_runs.push(runs.get(current_idx + i)?.1);
        }
        let sym = match_code128_pattern(&sym_runs)?;
        symbols.push(sym);
        current_idx += 6;
    }

    None
}

fn try_finalize_code128_stop(start_code: usize, symbols: &[usize]) -> Option<String> {
    if symbols.len() < 2 {
        return None;
    }
    let (&checksum, data_symbols) = symbols.split_last()?;
    let mut calc_sum = start_code;
    for (weight, &sym) in data_symbols.iter().skip(1).enumerate() {
        calc_sum = calc_sum.saturating_add((weight + 1).saturating_mul(sym));
    }
    if calc_sum % 103 != checksum {
        return None;
    }

    let mut text = String::new();
    let mut mode = start_code;
    for &sym in data_symbols.iter().skip(1) {
        match mode {
            105 => {
                // Code C: numeric pairs 00-99
                if sym <= 99 {
                    text.push_str(&format!("{:02}", sym));
                } else if sym == 100 {
                    mode = 104; // Switch to B
                } else if sym == 101 {
                    mode = 103; // Switch to A
                } else {
                    return None;
                }
            }
            104 => {
                // Code B: ASCII 32..127
                if sym <= 95 {
                    text.push((sym as u8).saturating_add(32) as char);
                } else if sym == 99 {
                    mode = 105; // Switch to C
                } else if sym == 100 || sym == 101 {
                    mode = 103; // Switch to A
                } else {
                    return None;
                }
            }
            103 => {
                // Code A: ASCII 0..95
                if sym <= 63 {
                    text.push((sym as u8).saturating_add(32) as char);
                } else if sym <= 95 {
                    text.push((sym as u8).saturating_sub(64) as char);
                } else if sym == 99 {
                    mode = 105; // Switch to C
                } else if sym == 100 || sym == 101 {
                    mode = 104; // Switch to B
                } else {
                    return None;
                }
            }
            _ => return None,
        }
    }

    (!text.is_empty()).then_some(text)
}

// -------------------------------------------------------------------------------------------------
// Code 39 Specification & Decoder
// -------------------------------------------------------------------------------------------------

const CODE39_CHARS: [(u16, char); 44] = [
    (0b000110100, '0'),
    (0b100100001, '1'),
    (0b001100001, '2'),
    (0b101100000, '3'),
    (0b000110001, '4'),
    (0b100110000, '5'),
    (0b001110000, '6'),
    (0b000100101, '7'),
    (0b100100100, '8'),
    (0b001100100, '9'),
    (0b100001001, 'A'),
    (0b001001001, 'B'),
    (0b101001000, 'C'),
    (0b000011001, 'D'),
    (0b100011000, 'E'),
    (0b001011000, 'F'),
    (0b000001101, 'G'),
    (0b100001100, 'H'),
    (0b001001100, 'I'),
    (0b000011100, 'J'),
    (0b100000011, 'K'),
    (0b001000011, 'L'),
    (0b101000010, 'M'),
    (0b000010011, 'N'),
    (0b100010010, 'O'),
    (0b001010010, 'P'),
    (0b000000111, 'Q'),
    (0b100000110, 'R'),
    (0b001000110, 'S'),
    (0b000010110, 'T'),
    (0b110000001, 'U'),
    (0b011000001, 'V'),
    (0b111000000, 'W'),
    (0b010010001, 'X'),
    (0b110010000, 'Y'),
    (0b011010000, 'Z'),
    (0b010000101, '-'),
    (0b110000100, '.'),
    (0b011000100, ' '),
    (0b010010100, '*'),
    (0b010101000, '$'),
    (0b010100010, '/'),
    (0b010001010, '+'),
    (0b000101010, '%'),
];

fn match_code39_symbol(runs: &[usize]) -> Option<char> {
    if runs.len() < 9 {
        return None;
    }
    let mut sorted = [0usize; 9];
    sorted.copy_from_slice(&runs[..9]);
    sorted.sort_unstable();
    let threshold = (sorted[2] + sorted[6]) / 2;

    let mut pattern = 0u16;
    for &r in &runs[..9] {
        pattern <<= 1;
        if r > threshold {
            pattern |= 1;
        }
    }

    CODE39_CHARS.iter().find(|(p, _)| *p == pattern).map(|(_, c)| *c)
}

fn try_decode_code39(runs: &[(bool, usize)], start_run_idx: usize) -> Option<(String, usize, usize)> {
    if start_run_idx + 19 >= runs.len() {
        return None;
    }
    if !runs.get(start_run_idx).is_some_and(|r| r.0) {
        return None;
    }

    let mut first_runs = [0usize; 9];
    for (i, slot) in first_runs.iter_mut().enumerate() {
        *slot = runs.get(start_run_idx + i)?.1;
    }
    if match_code39_symbol(&first_runs)? != '*' {
        return None;
    }

    let mut text = String::new();
    let mut curr = start_run_idx + 10; // 9 elements + 1 inter-character gap

    while curr + 9 <= runs.len() {
        let mut char_runs = [0usize; 9];
        for (i, slot) in char_runs.iter_mut().enumerate() {
            *slot = runs.get(curr + i)?.1;
        }
        if let Some(ch) = match_code39_symbol(&char_runs) {
            if ch == '*' {
                // Stop delimiter reached
                if !text.is_empty() {
                    return Some((text, start_run_idx, curr + 9));
                }
                return None;
            }
            text.push(ch);
            curr += 10;
        } else {
            break;
        }
    }

    None
}

// -------------------------------------------------------------------------------------------------
// 1D Barcode Line Scanning & Extent Detection
// -------------------------------------------------------------------------------------------------

fn collect_scanline_runs(binary: &BinaryImage, y: usize) -> Vec<(bool, usize)> {
    let mut runs = Vec::with_capacity(128);
    let mut current_dark = binary.is_dark(0, y);
    let mut current_len = 0usize;
    for x in 0..binary.width {
        let dark = binary.is_dark(x, y);
        if dark == current_dark {
            current_len = current_len.saturating_add(1);
        } else {
            runs.push((current_dark, current_len));
            current_dark = dark;
            current_len = 1;
        }
    }
    runs.push((current_dark, current_len));
    runs
}

fn detect_scanline_1d(runs: &[(bool, usize)]) -> Option<(CodeKind, String, usize, usize)> {
    if runs.len() < 25 {
        return None;
    }
    let limit = runs.len().saturating_sub(20);
    for r_idx in 0..limit {
        if let Some((text, start_idx, end_idx)) = try_decode_code128(runs, r_idx) {
            return Some((CodeKind::Code128, text, start_idx, end_idx));
        }
    }
    for r_idx in 0..limit {
        if let Some((text, start_idx, end_idx)) = try_decode_code39(runs, r_idx) {
            return Some((CodeKind::Code39, text, start_idx, end_idx));
        }
    }
    None
}

fn trace_1d_vertical_bounds(binary: &BinaryImage, y: usize, start_px: usize, bar_width: usize) -> (usize, usize) {
    let h = binary.height;
    let mid_x = start_px + bar_width / 2;
    let mid_dark = binary.is_dark(mid_x, y);
    let mut top_y = y;
    while top_y > 0 {
        if binary.is_dark(mid_x, top_y) != mid_dark && !binary.is_dark(start_px + 2, top_y) {
            break;
        }
        top_y = top_y.saturating_sub(1);
    }

    let mut bot_y = y;
    while bot_y < h.saturating_sub(1) {
        if binary.is_dark(mid_x, bot_y) != mid_dark && !binary.is_dark(start_px + 2, bot_y) {
            break;
        }
        bot_y = bot_y.saturating_add(1);
    }

    let bar_h = (bot_y - top_y).max(12);
    let top_y = top_y.saturating_sub(2);
    let bot_y = (top_y + bar_h + 4).min(h);
    (top_y, bot_y)
}

fn scan_1d_barcodes(binary: &BinaryImage, codes: &mut Vec<ScannedCode>, page: usize) {
    let (w, h) = (binary.width, binary.height);
    if w < 50 || h < 20 {
        return;
    }

    let step = (h / 70).max(4);
    let mut y = step;

    while y < h.saturating_sub(step) {
        let runs = collect_scanline_runs(binary, y);
        let Some((kind, text, start_idx, end_idx)) = detect_scanline_1d(&runs) else {
            y = y.saturating_add(step);
            continue;
        };

        let start_px: usize = runs.iter().take(start_idx).map(|r| r.1).sum();
        let bar_width: usize = runs.iter().skip(start_idx).take(end_idx - start_idx).map(|r| r.1).sum();
        let end_px = (start_px + bar_width).min(w);
        let (top_y, bot_y) = trace_1d_vertical_bounds(binary, y, start_px, bar_width);

        // Avoid duplicate codes already found in this region
        let already_found = codes.iter().any(|c| c.text == text && (c.rect[1] as usize).abs_diff(top_y) < 30);
        if !already_found {
            codes.push(ScannedCode {
                id: codes.len(),
                page,
                kind,
                text,
                rect: [start_px as f64, top_y as f64, end_px as f64, bot_y as f64],
                pixel_size: [(end_px - start_px) as u32, (bot_y - top_y) as u32],
                image_index: None,
            });
        }

        // Advance y past this barcode's vertical footprint
        y = bot_y.saturating_add(step);
    }
}

/// Scan an arbitrary RGBA pixel buffer for embedded barcodes and QR codes.
pub fn scan_rgba_buffer(width: usize, height: usize, rgba: &[u8], page: usize) -> Vec<ScannedCode> {
    let gray = match GrayImage::from_rgba(width, height, rgba) {
        Some(g) => g,
        None => return Vec::new(),
    };

    let mut codes = Vec::new();

    let pad = 12usize;
    let pad_w = width.saturating_add(pad.saturating_mul(2));
    let pad_h = height.saturating_add(pad.saturating_mul(2));
    let mut padded_pixels = vec![255u8; pad_w.saturating_mul(pad_h)];
    for y in 0..height {
        let src_start = y.saturating_mul(width);
        let dst_start = (y.saturating_add(pad)).saturating_mul(pad_w).saturating_add(pad);
        if let (Some(src), Some(dst)) =
            (gray.pixels.get(src_start..src_start.saturating_add(width)), padded_pixels.get_mut(dst_start..dst_start.saturating_add(width)))
        {
            dst.copy_from_slice(src);
        }
    }

    // Primary Pass: Production-grade multi-format barcode & QR reader (rxing)
    if let Ok(results) = rxing::helpers::detect_multiple_in_luma(padded_pixels, pad_w as u32, pad_h as u32) {
        for res in results {
            let text = res.getText().to_string();
            if text.is_empty() {
                continue;
            }
            let (kind, text) = match *res.getBarcodeFormat() {
                rxing::BarcodeFormat::QR_CODE => (CodeKind::QrCode, text),
                rxing::BarcodeFormat::CODE_128 => (CodeKind::Code128, text),
                rxing::BarcodeFormat::CODE_39 => (CodeKind::Code39, text),
                rxing::BarcodeFormat::EAN_13 => (CodeKind::Ean13, text),
                rxing::BarcodeFormat::UPC_A => {
                    let full_ean = if text.len() == 12 { format!("0{text}") } else { text };
                    (CodeKind::Ean13, full_ean)
                }
                _ => continue,
            };
            let points = res.getPoints();
            let mut min_x = f32::MAX;
            let mut max_x = 0.0f32;
            let mut min_y = f32::MAX;
            let mut max_y = 0.0f32;

            for pt in points {
                let px = (pt.x - pad as f32).clamp(0.0, width as f32);
                let py = (pt.y - pad as f32).clamp(0.0, height as f32);
                min_x = min_x.min(px);
                max_x = max_x.max(px);
                min_y = min_y.min(py);
                max_y = max_y.max(py);
            }

            let is_1d = kind != CodeKind::QrCode;

            if is_1d {
                refine_1d_bounds(&gray, width, height, &mut min_x, &mut max_x, &mut min_y, &mut max_y);
            } else {
                let mut bounds = [min_x, min_y, max_x, max_y];
                refine_qr_bounds(&gray.pixels, [width, height], points, pad as f32, &mut bounds);
                [min_x, min_y, max_x, max_y] = bounds;
            }

            let rect = [
                (min_x as f64).clamp(0.0, width as f64),
                (min_y as f64).clamp(0.0, height as f64),
                (max_x as f64).clamp(0.0, width as f64),
                (max_y as f64).clamp(0.0, height as f64),
            ];
            let pixel_w = ((rect[2] - rect[0]).max(1.0).round()) as u32;
            let pixel_h = ((rect[3] - rect[1]).max(1.0).round()) as u32;

            // Reject false positives with tiny dimensions (e.g. single letters or table borders)
            if pixel_w < 25 || pixel_h < 12 {
                continue;
            }

            codes.push(ScannedCode { id: codes.len(), page, kind, text, rect, pixel_size: [pixel_w, pixel_h], image_index: None });
        }
    }

    // Secondary Pass: 1-D scanline / Otsu thresholding fallback ONLY if the primary pass found
    // no codes. QR codes are left to rxing: the hand-rolled grid decoder could not verify a
    // payload, so it is never reported.
    if codes.is_empty() {
        let binary = BinaryImage::from_gray(&gray);
        scan_1d_barcodes(&binary, &mut codes, page);
    }

    for (idx, c) in codes.iter_mut().enumerate() {
        c.id = idx;
    }

    codes
}

fn refine_1d_bounds(gray: &GrayImage, width: usize, height: usize, min_x: &mut f32, max_x: &mut f32, min_y: &mut f32, max_y: &mut f32) {
    if (*max_y - *min_y) > (*max_x - *min_x) {
        refine_1d_vertical(gray, width, height, min_x, max_x, min_y, max_y);
    } else {
        refine_1d_horizontal(gray, width, height, min_x, max_x, min_y, max_y);
    }
}

fn refine_1d_vertical(gray: &GrayImage, width: usize, height: usize, min_x: &mut f32, max_x: &mut f32, min_y: &mut f32, max_y: &mut f32) {
    let raw_x = (min_x.round() as usize).clamp(0, width.saturating_sub(1));
    let center_y = (((*min_y + *max_y) * 0.5).round() as usize).clamp(0, height.saturating_sub(1));

    let mut found_dark = None;
    for dx in [0isize, -1, 1, -2, 2, -3, 3] {
        let sx = (raw_x as isize + dx).clamp(0, width.saturating_sub(1) as isize) as usize;
        for dy in 0..150usize {
            let cands = [
                center_y.saturating_sub(dy),
                (center_y + dy).min(height.saturating_sub(1)),
                (min_y.round() as usize).saturating_sub(dy),
                ((min_y.round() as usize) + dy).min(height.saturating_sub(1)),
                (max_y.round() as usize).saturating_sub(dy),
                ((max_y.round() as usize) + dy).min(height.saturating_sub(1)),
            ];
            for cy in cands {
                if gray.get(sx, cy) < 180 {
                    found_dark = Some((sx, cy));
                    break;
                }
            }
            if found_dark.is_some() {
                break;
            }
        }
        if found_dark.is_some() {
            break;
        }
    }

    if let Some((dark_x, dark_y)) = found_dark {
        let mut left_x = dark_x;
        while left_x > 0 && gray.get(left_x.saturating_sub(1), dark_y) < 180 {
            left_x = left_x.saturating_sub(1);
        }
        let mut right_x = dark_x;
        while right_x + 1 < width && gray.get(right_x.saturating_add(1), dark_y) < 180 {
            right_x = right_x.saturating_add(1);
        }

        let mid_x = (left_x + right_x) / 2;

        let mut bar_top = dark_y;
        let mut gap = 0usize;
        for y in (0..=dark_y).rev() {
            if gray.get(mid_x, y) < 180 {
                bar_top = y;
                gap = 0;
            } else {
                gap = gap.saturating_add(1);
                if gap > 12 {
                    break;
                }
            }
        }

        let mut bar_bot = dark_y;
        let mut gap = 0usize;
        for y in dark_y..height {
            if gray.get(mid_x, y) < 180 {
                bar_bot = y;
                gap = 0;
            } else {
                gap = gap.saturating_add(1);
                if gap > 12 {
                    break;
                }
            }
        }

        if bar_bot > bar_top.saturating_add(15) && right_x > left_x.saturating_add(8) {
            *min_x = left_x as f32;
            *max_x = right_x as f32;
            *min_y = bar_top as f32;
            *max_y = bar_bot as f32;
        } else {
            let default_w = ((*max_y - *min_y) * 0.35).clamp(25.0, 100.0);
            *min_x = (*min_x - default_w * 0.5).max(0.0);
            *max_x = (*min_x + default_w).min(width as f32);
        }
    } else {
        let default_w = ((*max_y - *min_y) * 0.35).clamp(25.0, 100.0);
        *min_x = (*min_x - default_w * 0.5).max(0.0);
        *max_x = (*min_x + default_w).min(width as f32);
    }
}

fn refine_1d_horizontal(gray: &GrayImage, width: usize, height: usize, min_x: &mut f32, max_x: &mut f32, min_y: &mut f32, max_y: &mut f32) {
    let raw_y = (min_y.round() as usize).clamp(0, height.saturating_sub(1));
    let center_x = (((*min_x + *max_x) * 0.5).round() as usize).clamp(0, width.saturating_sub(1));

    let mut found_dark = None;
    for dy in [0isize, -1, 1, -2, 2, -3, 3] {
        let sy = (raw_y as isize + dy).clamp(0, height.saturating_sub(1) as isize) as usize;
        for dx in 0..150usize {
            let cands = [
                center_x.saturating_sub(dx),
                (center_x + dx).min(width.saturating_sub(1)),
                (min_x.round() as usize).saturating_sub(dx),
                ((min_x.round() as usize) + dx).min(width.saturating_sub(1)),
                (max_x.round() as usize).saturating_sub(dx),
                ((max_x.round() as usize) + dx).min(width.saturating_sub(1)),
            ];
            for cx in cands {
                if gray.get(cx, sy) < 180 {
                    found_dark = Some((cx, sy));
                    break;
                }
            }
            if found_dark.is_some() {
                break;
            }
        }
        if found_dark.is_some() {
            break;
        }
    }

    if let Some((dark_x, dark_y)) = found_dark {
        let mut top_y = dark_y;
        while top_y > 0 && gray.get(dark_x, top_y.saturating_sub(1)) < 180 {
            top_y = top_y.saturating_sub(1);
        }
        let mut bot_y = dark_y;
        while bot_y + 1 < height && gray.get(dark_x, bot_y.saturating_add(1)) < 180 {
            bot_y = bot_y.saturating_add(1);
        }

        let mid_y = (top_y + bot_y) / 2;

        let mut bar_left = dark_x;
        let mut gap = 0usize;
        for x in (0..=dark_x).rev() {
            if gray.get(x, mid_y) < 180 {
                bar_left = x;
                gap = 0;
            } else {
                gap = gap.saturating_add(1);
                if gap > 12 {
                    break;
                }
            }
        }

        let mut bar_right = dark_x;
        let mut gap = 0usize;
        for x in dark_x..width {
            if gray.get(x, mid_y) < 180 {
                bar_right = x;
                gap = 0;
            } else {
                gap = gap.saturating_add(1);
                if gap > 12 {
                    break;
                }
            }
        }

        if bar_right > bar_left.saturating_add(15) && bot_y > top_y.saturating_add(8) {
            *min_x = bar_left as f32;
            *max_x = bar_right as f32;
            *min_y = top_y as f32;
            *max_y = bot_y as f32;
        } else {
            let default_h = ((*max_x - *min_x) * 0.35).clamp(25.0, 100.0);
            *min_y = (*min_y - default_h * 0.5).max(0.0);
            *max_y = (*min_y + default_h).min(height as f32);
        }
    } else {
        let default_h = ((*max_x - *min_x) * 0.35).clamp(25.0, 100.0);
        *min_y = (*min_y - default_h * 0.5).max(0.0);
        *max_y = (*min_y + default_h).min(height as f32);
    }
}

// -------------------------------------------------------------------------------------------------
// Document Integration & Pixel-Perfect Replacement
// -------------------------------------------------------------------------------------------------

const MAX_EMBEDDED_SCAN_PIXELS: usize = 32_000_000;

fn fits_scan_budget(w: u32, h: u32, total_scanned_pixels: usize, max_pixels: usize) -> Option<usize> {
    if w == 0 || h == 0 {
        return None;
    }
    (w as usize).checked_mul(h as usize).and_then(|p| total_scanned_pixels.checked_add(p)).filter(|&t| t <= max_pixels)
}

fn map_embedded_code_to_page(code: &mut ScannedCode, img: &pdfcraft_edit::PageImage, size: (u32, u32), idx: usize, crop: [f32; 4]) -> bool {
    let (w, h) = size;
    let code_area = ((code.rect[2] - code.rect[0]) * (code.rect[3] - code.rect[1])).abs();
    let img_area = (w as f64) * (h as f64);
    code.image_index = if img_area > 0.0 && (code_area / img_area) > 0.9 { Some(idx) } else { None };

    let [ma, mb, mc, md, me, mf] = img.matrix;
    let apply = |u: f64, v: f64| -> (f64, f64) { (ma * u + mc * v + me, mb * u + md * v + mf) };
    let w_f = w as f64;
    let h_f = h as f64;
    let u0 = (code.rect[0] / w_f).clamp(0.0, 1.0);
    let u1 = (code.rect[2] / w_f).clamp(0.0, 1.0);
    let v0 = ((h_f - code.rect[3]) / h_f).clamp(0.0, 1.0);
    let v1 = ((h_f - code.rect[1]) / h_f).clamp(0.0, 1.0);

    let corners = [apply(u0, v0), apply(u1, v0), apply(u1, v1), apply(u0, v1)];
    let sub_min_x = corners.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let sub_min_y = corners.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let sub_max_x = corners.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let sub_max_y = corners.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);

    code.rect = [
        sub_min_x.clamp(crop[0] as f64, crop[2] as f64),
        sub_min_y.clamp(crop[1] as f64, crop[3] as f64),
        sub_max_x.clamp(crop[0] as f64, crop[2] as f64),
        sub_max_y.clamp(crop[1] as f64, crop[3] as f64),
    ];
    (code.rect[2] - code.rect[0]) > 0.5 && (code.rect[3] - code.rect[1]) > 0.5
}

fn map_rendered_code_rect(code: &mut ScannedCode, page_info: Option<&pdfcraft_render::PageInfo>, page_h: f32, effective_scale: f32, crop: [f32; 4]) {
    let vx0 = (code.rect[0] as f32) / effective_scale;
    let vy0 = (code.rect[1] as f32) / effective_scale;
    let vx1 = (code.rect[2] as f32) / effective_scale;
    let vy1 = (code.rect[3] as f32) / effective_scale;

    let (min_x, min_y, max_x, max_y) = if let Some(info) = page_info {
        let corners = [info.view_to_user(vx0, vy0), info.view_to_user(vx1, vy0), info.view_to_user(vx1, vy1), info.view_to_user(vx0, vy1)];
        (
            corners.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min),
            corners.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min),
            corners.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max),
            corners.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max),
        )
    } else {
        (vx0, page_h - vy1, vx1, page_h - vy0)
    };

    code.rect = [
        min_x.clamp(crop[0], crop[2]) as f64,
        min_y.clamp(crop[1], crop[3]) as f64,
        max_x.clamp(crop[0], crop[2]) as f64,
        max_y.clamp(crop[1], crop[3]) as f64,
    ];
}

fn is_code_visible_in_render(code: &ScannedCode, info: &pdfcraft_render::PageInfo, effective_scale: f32, rw: usize, rh: usize, rgba: &[u8]) -> bool {
    let p0 = info.user_to_view(code.rect[0] as f32, code.rect[3] as f32);
    let p1 = info.user_to_view(code.rect[2] as f32, code.rect[1] as f32);
    let rx0 = ((p0[0].min(p1[0]) * effective_scale).round() as usize).clamp(0, rw);
    let rx1 = ((p0[0].max(p1[0]) * effective_scale).round() as usize).clamp(0, rw);
    let ry0 = ((p0[1].min(p1[1]) * effective_scale).round() as usize).clamp(0, rh);
    let ry1 = ((p0[1].max(p1[1]) * effective_scale).round() as usize).clamp(0, rh);

    if rx1 <= rx0.saturating_add(5) || ry1 <= ry0.saturating_add(5) {
        return true;
    }

    let mut dark_count = 0usize;
    let total_samples = 30usize;
    for sy in 0..total_samples {
        let y = ry0 + (sy * (ry1 - ry0)) / total_samples;
        for sx in 0..total_samples {
            let x = rx0 + (sx * (rx1 - rx0)) / total_samples;
            let idx = (y * rw + x) * 4;
            if let Some(slice) = rgba.get(idx..idx + 3)
                && (slice[0] < 180 || slice[1] < 180 || slice[2] < 180)
            {
                dark_count = dark_count.saturating_add(1);
            }
        }
    }
    dark_count > (total_samples * total_samples) / 50
}

fn deduplicate_scanned_codes(scanned: Vec<ScannedCode>) -> Vec<ScannedCode> {
    let mut unique: Vec<ScannedCode> = Vec::new();
    for c in scanned {
        let is_dup = unique.iter().any(|u| {
            if u.text == c.text {
                let overlap_x = (u.rect[0].max(c.rect[0])) <= (u.rect[2].min(c.rect[2]) + 20.0);
                let overlap_y = (u.rect[1].max(c.rect[1])) <= (u.rect[3].min(c.rect[3]) + 20.0);
                overlap_x && overlap_y
            } else {
                false
            }
        });
        if !is_dup {
            unique.push(c);
        }
    }
    for (idx, c) in unique.iter_mut().enumerate() {
        c.id = idx;
    }
    unique
}

impl Document {
    /// Scan a page for all embedded QR codes and barcodes, returning their positions and text.
    ///
    /// Evaluates both embedded image objects AND the rendered page buffer to reliably detect
    /// vector-rendered shipping label barcodes, table codes, and rasterized codes.
    pub fn scan_page_codes(&self, page: usize) -> Vec<ScannedCode> {
        crate::guard(|| self.scan_page_codes_inner(page)).unwrap_or_default()
    }

    fn embedded_image_header_dims(&self, img: &pdfcraft_edit::PageImage) -> Option<(u32, u32)> {
        if !img.is_form {
            return (img.width > 0 && img.height > 0).then_some((img.width, img.height));
        }
        let obj = img.object?;
        let cos = &self.editor.as_ref()?.cos;
        let pic = crate::copy_image::picture_in_form(cos, obj)?;
        let pdfcraft_cos::Object::Stream(s) = &*cos.get(pic) else {
            return None;
        };
        let w = u32::try_from(s.dict.int(b"Width")?).ok()?;
        let h = u32::try_from(s.dict.int(b"Height")?).ok()?;
        (w > 0 && h > 0).then_some((w, h))
    }

    fn decode_embedded_image_for_scan(
        &self,
        page: usize,
        idx: usize,
        img: &pdfcraft_edit::PageImage,
        total_scanned_pixels: &mut usize,
    ) -> Option<(u32, u32, Vec<u8>)> {
        if let Some((hw, hh)) = self.embedded_image_header_dims(img) {
            fits_scan_budget(hw, hh, *total_scanned_pixels, MAX_EMBEDDED_SCAN_PIXELS)?;
        }
        let (_, bytes) = self.page_image_copy_file(page, idx).ok()?;
        // Probe image header dimensions before allocating/decoding large raster buffers.
        if let Ok((pw, ph)) = craft_image_codecs::probe_size(&bytes) {
            fits_scan_budget(pw, ph, *total_scanned_pixels, MAX_EMBEDDED_SCAN_PIXELS)?;
        }
        let remaining = MAX_EMBEDDED_SCAN_PIXELS.saturating_sub(*total_scanned_pixels) as u64;
        let opts = craft_image_codecs::DecodeOptions {
            limits: craft_image_codecs::Limits { max_pixels: remaining, max_alloc: remaining.saturating_mul(4), ..Default::default() },
            ..Default::default()
        };
        let decoded = craft_image_codecs::decode_with(&bytes, &opts).ok()?;
        let (w, h) = (decoded.width(), decoded.height());
        *total_scanned_pixels = fits_scan_budget(w, h, *total_scanned_pixels, MAX_EMBEDDED_SCAN_PIXELS)?;
        Some((w, h, decoded.to_rgba8()))
    }

    fn scan_embedded_page_images(&self, page: usize, crop: [f32; 4]) -> Vec<ScannedCode> {
        let mut scanned = Vec::new();
        let images = self.page_images(page);
        let mut cached_image_scans: std::collections::HashMap<String, (u32, u32, Vec<ScannedCode>)> = std::collections::HashMap::new();
        let mut total_scanned_pixels = 0usize;

        for (idx, img) in images.iter().enumerate() {
            let cache_key = img.object.map(|o| format!("{o:?}")).unwrap_or_else(|| img.name.clone());
            let (w, h, img_codes) = if let Some((cw, ch, codes)) = cached_image_scans.get(&cache_key) {
                (*cw, *ch, codes.clone())
            } else if total_scanned_pixels < MAX_EMBEDDED_SCAN_PIXELS {
                let Some((w, h, rgba)) = self.decode_embedded_image_for_scan(page, idx, img, &mut total_scanned_pixels) else {
                    continue;
                };
                let codes = scan_rgba_buffer(w as usize, h as usize, &rgba, page);
                cached_image_scans.insert(cache_key, (w, h, codes.clone()));
                (w, h, codes)
            } else {
                continue;
            };

            for mut code in img_codes {
                code.id = scanned.len();
                if map_embedded_code_to_page(&mut code, img, (w, h), idx, crop) {
                    scanned.push(code);
                }
            }
        }
        scanned
    }

    fn scan_rendered_page_codes(&self, page: usize, page_info: Option<&pdfcraft_render::PageInfo>, crop: [f32; 4], scanned: &mut Vec<ScannedCode>) {
        let (page_w, page_h) = page_info.map(|p| (p.width, p.height)).unwrap_or((612.0, 792.0));
        const MAX_PIXELS: f32 = 16_000_000.0;
        let max_scale = (MAX_PIXELS / (page_w * page_h).max(1.0)).sqrt();
        let mut renderer = pdfcraft_render::PageRenderer::new(self.display.clone(), self.config.clone());

        for scale in [2.0f32, 1.0f32] {
            let effective_scale = scale.min(max_scale);
            let rendered = renderer.render(pdfcraft_render::RenderRequest { page, scale: effective_scale, ..Default::default() });
            if rendered.error.is_some() || rendered.width == 0 || rendered.height == 0 || rendered.rgba.is_empty() {
                continue;
            }
            let (rw, rh) = (rendered.width as usize, rendered.height as usize);
            let buffer_codes = scan_rgba_buffer(rw, rh, &rendered.rgba, page);
            let mut found_any = false;
            for mut code in buffer_codes {
                map_rendered_code_rect(&mut code, page_info, page_h, effective_scale, crop);
                let mut matched_image_index = None;
                scanned.retain(|c| {
                    let same_symbol = c.text == code.text;
                    let overlap_x = (c.rect[0].max(code.rect[0])) <= (c.rect[2].min(code.rect[2]) + 15.0);
                    let overlap_y = (c.rect[1].max(code.rect[1])) <= (c.rect[3].min(code.rect[3]) + 15.0);
                    let matches = same_symbol && overlap_x && overlap_y;
                    if matches && c.image_index.is_some() {
                        matched_image_index = c.image_index;
                    }
                    !matches
                });
                code.id = scanned.len();
                code.image_index = matched_image_index;
                scanned.push(code);
                found_any = true;
            }

            if let Some(info) = page_info {
                scanned.retain(|c| is_code_visible_in_render(c, info, effective_scale, rw, rh, &rendered.rgba));
            }
            if found_any {
                break;
            }
        }
    }

    fn scan_page_codes_inner(&self, page: usize) -> Vec<ScannedCode> {
        let page_info = self.info.pages.get(page);
        let [cx0, cy0, cx1, cy1] = page_info.map(|p| p.crop).unwrap_or([0.0, 0.0, 612.0, 792.0]);
        let crop = [cx0.min(cx1), cy0.min(cy1), cx0.max(cx1), cy0.max(cy1)];
        let mut scanned = self.scan_embedded_page_images(page, crop);
        self.scan_rendered_page_codes(page, page_info, crop, &mut scanned);
        deduplicate_scanned_codes(scanned)
    }

    /// Generate a solid white PNG raster at the specified dimensions for whiteout edits.
    pub fn generate_whiteout(target_width: u32, target_height: u32) -> Result<Vec<u8>, String> {
        let w = target_width.clamp(8, 2048);
        let h = target_height.clamp(8, 2048);
        let rgba = vec![255u8; (w as usize).saturating_mul(h as usize).saturating_mul(4)];
        encode_png_exact(w, h, &rgba)
    }

    /// Generate an updated QR code or barcode raster at **exactly 100% of the original pixel dimensions**.
    ///
    /// This strictly enforces the pixel-perfect replacement constraint: the generated PNG byte stream
    /// matches `[width, height]` down to the exact pixel.
    pub fn generate_replacement_code(kind: CodeKind, text: &str, target_width: u32, target_height: u32) -> Result<Vec<u8>, String> {
        if target_width == 0 || target_height == 0 {
            return Err("Target dimensions must be non-zero".into());
        }
        let total_pixels = (target_width as usize)
            .checked_mul(target_height as usize)
            .filter(|&p| p <= 16_000_000)
            .ok_or_else(|| "Target dimensions exceed maximum raster budget of 16 megapixels".to_string())?;
        if total_pixels.checked_mul(4).is_none() {
            return Err("Target pixel buffer size overflow".to_string());
        }

        use rxing::Writer;
        let format = match kind {
            CodeKind::QrCode => rxing::BarcodeFormat::QR_CODE,
            CodeKind::Code128 => rxing::BarcodeFormat::CODE_128,
            CodeKind::Code39 => rxing::BarcodeFormat::CODE_39,
            CodeKind::Ean13 => rxing::BarcodeFormat::EAN_13,
            CodeKind::GenericBarcode => rxing::BarcodeFormat::CODE_128,
        };

        let writer = rxing::MultiFormatWriter;
        let matrix = writer.encode(text, &format, target_width as i32, target_height as i32).map_err(|e| e.to_string())?;

        let mw = matrix.getWidth() as usize;
        let mh = matrix.getHeight() as usize;

        // Find exact bounds of the black bars/modules in the generated matrix
        let mut min_x = usize::MAX;
        let mut max_x = 0usize;
        let mut min_y = usize::MAX;
        let mut max_y = 0usize;

        for y in 0..mh {
            for x in 0..mw {
                if matrix.get(x as u32, y as u32) {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
            }
        }

        let modules_w = if min_x <= max_x { max_x - min_x + 1 } else { 1 };
        let modules_h = if min_y <= max_y { max_y - min_y + 1 } else { 1 };
        let tw = target_width as usize;
        let th = target_height as usize;
        if modules_w > tw || modules_h > th {
            return Err(format!("Encoded barcode requires at least {modules_w}x{modules_h} pixels, which exceeds requested dimensions {tw}x{th}"));
        }
        let Some(total_out_pixels) = tw.checked_mul(th).filter(|&p| p <= 16_000_000) else {
            return Err("Target dimensions exceed maximum raster budget of 16 megapixels".into());
        };
        let Some(byte_len) = total_out_pixels.checked_mul(4) else {
            return Err("Target pixel buffer size overflow".into());
        };
        let mut rgba = vec![255u8; byte_len];

        if min_x <= max_x && min_y <= max_y {
            let cw = modules_w as f64;
            let ch = modules_h as f64;

            for y in 0..th {
                let sy = ((min_y as f64) + ((y as f64) / (th as f64)) * ch).floor() as u32;
                let sy_clamped = sy.min(max_y as u32);
                for x in 0..tw {
                    let sx = ((min_x as f64) + ((x as f64) / (tw as f64)) * cw).floor() as u32;
                    let sx_clamped = sx.min(max_x as u32);
                    if matrix.get(sx_clamped, sy_clamped) {
                        let idx = (y.saturating_mul(tw).saturating_add(x)).saturating_mul(4);
                        if let Some(slice) = rgba.get_mut(idx..idx + 4) {
                            slice[0] = 0;
                            slice[1] = 0;
                            slice[2] = 0;
                            slice[3] = 255;
                        }
                    }
                }
            }
        }

        encode_png_exact(tw as u32, th as u32, &rgba)
    }
}

fn probe_finder_radius(gray: &[u8], dims: [usize; 2], center: [isize; 2], dir: [isize; 2], thresh: u8, max_probe: isize) -> Option<f32> {
    let [width, height] = dims;
    let [cx, cy] = center;
    let [dx, dy] = dir;
    // Walk dark center (1.5m) -> light ring (1m) -> dark outer border (1m) -> light outside
    let mut stage = 0u8;
    let mut last_dark_step = 0isize;
    for step in 1..=max_probe {
        let ux = usize::try_from(cx + dx * step).ok()?;
        let uy = usize::try_from(cy + dy * step).ok()?;
        let val = if ux < width && uy < height { gray.get(uy * width + ux).copied()? } else { return None };
        let is_dark = val <= thresh;
        match stage {
            0 if !is_dark => stage = 1,
            1 if is_dark => {
                stage = 2;
                last_dark_step = step;
            }
            2 if is_dark => last_dark_step = step,
            2 if !is_dark => return (last_dark_step >= 3).then_some(last_dark_step as f32 + 0.5),
            _ => {}
        }
    }
    None
}

/// Expand QR finder-pattern center bounds (`[min_x, min_y, max_x, max_y]`) to include the full
/// outer finder band (3.5 modules).
fn refine_qr_bounds(gray: &[u8], dims: [usize; 2], points: &[rxing::Point], pad: f32, bounds: &mut [f32; 4]) {
    let [width, height] = dims;
    if points.is_empty() || width == 0 || height == 0 {
        return;
    }

    let mut finder_radii = Vec::new();
    let max_probe = (width.max(height) / 2).max(16) as isize;
    for pt in points.iter().take(3) {
        let cx = (pt.x - pad).round() as isize;
        let cy = (pt.y - pad).round() as isize;
        let (Ok(ux), Ok(uy)) = (usize::try_from(cx), usize::try_from(cy)) else { continue };
        let Some(&center_val) = (ux < width && uy < height).then(|| gray.get(uy * width + ux)).flatten() else { continue };
        let thresh = center_val.saturating_add(64).min(170);
        for &(dx, dy) in &[(1isize, 0isize), (-1, 0), (0, 1), (0, -1)] {
            if let Some(radius) = probe_finder_radius(gray, dims, [cx, cy], [dx, dy], thresh, max_probe) {
                finder_radii.push(radius);
            }
        }
    }

    let expand = if !finder_radii.is_empty() {
        finder_radii.sort_by(|a, b| a.total_cmp(b));
        finder_radii[finder_radii.len() / 2]
    } else {
        // Fallback for Version 1 (21x21): finder centers span 14 modules, outer band is 3.5 modules each side.
        let span = (bounds[2] - bounds[0]).max(bounds[3] - bounds[1]);
        (span * (3.5 / 14.0)).max(1.0)
    };
    bounds[0] = (bounds[0] - expand).clamp(0.0, width as f32);
    bounds[1] = (bounds[1] - expand).clamp(0.0, height as f32);
    bounds[2] = (bounds[2] + expand).clamp(0.0, width as f32);
    bounds[3] = (bounds[3] + expand).clamp(0.0, height as f32);
}

/// Encode RGBA buffer into a PNG byte stream.
fn encode_png_exact(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let img = craft_image_codecs::Image::from_u8(width, height, craft_image_codecs::ChannelLayout::Rgba, rgba.to_vec()).map_err(|e| e.to_string())?;
    craft_image_codecs::encode(&img, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_barcode_generation_and_scan_roundtrip() {
        let text = "ES26270317";
        let png_bytes = Document::generate_replacement_code(CodeKind::Code39, text, 300, 100).expect("generate replacement code");

        let img = craft_image_codecs::decode(&png_bytes).expect("read png");
        let rgba_img = img.convert(craft_image_codecs::ChannelLayout::Rgba, craft_image_codecs::SampleType::U8);
        let rgba = rgba_img.data();

        let codes = scan_rgba_buffer(img.width() as usize, img.height() as usize, rgba, 0);
        assert!(!codes.is_empty(), "must scan generated code");
        assert_eq!(codes[0].text, text);
        assert_eq!(codes[0].kind, CodeKind::Code39);
    }

    #[test]
    fn test_code128_generation_and_scan_roundtrip() {
        let text = "TEST128";
        let png_bytes = Document::generate_replacement_code(CodeKind::Code128, text, 300, 100).expect("generate code128");

        let img = craft_image_codecs::decode(&png_bytes).expect("read png");
        let rgba_img = img.convert(craft_image_codecs::ChannelLayout::Rgba, craft_image_codecs::SampleType::U8);
        let rgba = rgba_img.data();

        let codes = scan_rgba_buffer(img.width() as usize, img.height() as usize, rgba, 0);
        assert!(!codes.is_empty(), "must scan generated code128");
        assert_eq!(codes[0].text, text);
        assert_eq!(codes[0].kind, CodeKind::Code128);
    }

    #[test]
    fn test_qr_code_generation_and_scan_roundtrip() {
        let payload = "HELLO WORLD";
        let matrix = QrCode::encode_text(payload).expect("encode qr");
        let png_bytes = matrix.to_png_bytes(6, 4).expect("to png");
        println!("Generated PNG: {} bytes", png_bytes.len());

        let img = craft_image_codecs::decode(&png_bytes).expect("read png");
        let rgba_img = img.convert(craft_image_codecs::ChannelLayout::Rgba, craft_image_codecs::SampleType::U8);
        let rgba = rgba_img.data();
        println!("PNG frame: {}x{}", img.width(), img.height());

        let scanned = scan_rgba_buffer(img.width() as usize, img.height() as usize, rgba, 0);
        println!("Scanned codes: {:?}", scanned.iter().map(|c| (&c.text, c.kind)).collect::<Vec<_>>());
        assert!(!scanned.is_empty(), "must detect generated QR code");
        assert_eq!(scanned[0].kind, CodeKind::QrCode);
        assert_eq!(scanned[0].text, payload);
        // For module=6px and quiet=4 modules, the 21x21 QR symbol covers x=24..150, y=24..150 (126x126px),
        // including the outer 3.5-module finder bands outside the finder centers (45..129).
        assert!(scanned[0].rect[0] <= 26.0 && scanned[0].rect[1] <= 26.0, "expected outer top-left near 24, got {:?}", scanned[0].rect);
        assert!(scanned[0].rect[2] >= 148.0 && scanned[0].rect[3] >= 148.0, "expected outer bottom-right near 150, got {:?}", scanned[0].rect);
        assert!(scanned[0].pixel_size[0] >= 122 && scanned[0].pixel_size[1] >= 122, "expected full ~126px size, got {:?}", scanned[0].pixel_size);
    }

    #[test]
    fn test_code128_tentative_stop_falls_back_to_data_symbol() {
        // Sequence: Start B (104), data sym 18 ('2'), data sym 19 ('3'), checksum ((104 + 1*18 + 2*19) % 103 = 57), STOP (106).
        // The 6 runs of sym 18 followed by the first bar of sym 19 look similar to STOP; a failed tentative stop
        // check must fall back to reading sym 18 rather than aborting.
        let pattern_to_runs = |pat: [u8; 6], out: &mut Vec<(bool, usize)>| {
            for (i, w) in pat.iter().enumerate() {
                out.push((i % 2 == 0, usize::from(*w) * 2));
            }
        };
        let mut runs = Vec::new();
        pattern_to_runs(CODE128_PATTERNS[104], &mut runs);
        pattern_to_runs(CODE128_PATTERNS[18], &mut runs);
        pattern_to_runs(CODE128_PATTERNS[19], &mut runs);
        pattern_to_runs(CODE128_PATTERNS[57], &mut runs);
        for (i, w) in CODE128_STOP.iter().enumerate() {
            runs.push((i % 2 == 0, usize::from(*w) * 2));
        }
        let decoded = try_decode_code128(&runs, 0).expect("must decode Code 128 past tentative stop false match");
        assert_eq!(decoded.0, "23");
    }

    #[test]
    fn test_code128_tentative_stop_valid_prefix_checksum_requires_end_boundary() {
        // Sequence: Start B (104), 33 ('A'), 34 ('B'), 18 ('2'), 19 ('3'), 26 (checksum), STOP -> "AB23".
        // At sym 18, [18] + first bar of [19] resembles STOP AND sym 34 satisfies the prefix checksum for "A",
        // so without end-boundary validation the decoder would stop early and return "A".
        let pattern_to_runs = |pat: [u8; 6], out: &mut Vec<(bool, usize)>| {
            for (i, w) in pat.iter().enumerate() {
                out.push((i % 2 == 0, usize::from(*w) * 2));
            }
        };
        let mut runs = Vec::new();
        pattern_to_runs(CODE128_PATTERNS[104], &mut runs);
        pattern_to_runs(CODE128_PATTERNS[33], &mut runs);
        pattern_to_runs(CODE128_PATTERNS[34], &mut runs);
        pattern_to_runs(CODE128_PATTERNS[18], &mut runs);
        pattern_to_runs(CODE128_PATTERNS[19], &mut runs);
        pattern_to_runs(CODE128_PATTERNS[26], &mut runs);
        for (i, w) in CODE128_STOP.iter().enumerate() {
            runs.push((i % 2 == 0, usize::from(*w) * 2));
        }
        let decoded = try_decode_code128(&runs, 0).expect("must decode full AB23 sequence");
        assert_eq!(decoded.0, "AB23");
        assert_eq!(decoded.2, runs.len());

        // With trailing 10-module quiet zone followed by unrelated page content, full sequence still decodes.
        let mut with_quiet = runs.clone();
        with_quiet.push((false, 20));
        with_quiet.push((true, 6));
        let decoded_q = try_decode_code128(&with_quiet, 0).expect("must decode AB23 with trailing quiet zone");
        assert_eq!(decoded_q.0, "AB23");
        assert_eq!(decoded_q.2, runs.len());

        // A truncated sequence ending right after the 2-module internal space of sym 19 must not accept "A".
        let mut truncated = Vec::new();
        pattern_to_runs(CODE128_PATTERNS[104], &mut truncated);
        pattern_to_runs(CODE128_PATTERNS[33], &mut truncated);
        pattern_to_runs(CODE128_PATTERNS[34], &mut truncated);
        pattern_to_runs(CODE128_PATTERNS[18], &mut truncated);
        truncated.push((true, 4));
        truncated.push((false, 4));
        assert!(try_decode_code128(&truncated, 0).is_none());
    }

    #[test]
    fn test_qr_code_version2_covers_outer_finder_band() {
        let payload = "HTTPS://EXAMPLE.COM/VERIFICATION-V2-PAYLOAD";
        let matrix = QrCode::encode_text(payload).expect("encode v2+ qr");
        assert!(matrix.size >= 25, "expected Version 2+ QR code, got {} modules", matrix.size);
        let module_px = 5u32;
        let quiet = 4u32;
        let png_bytes = matrix.to_png_bytes(module_px, quiet).expect("to png");
        let img = craft_image_codecs::decode(&png_bytes).expect("read png");
        let rgba_img = img.convert(craft_image_codecs::ChannelLayout::Rgba, craft_image_codecs::SampleType::U8);
        let scanned = scan_rgba_buffer(img.width() as usize, img.height() as usize, rgba_img.data(), 0);
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].text, payload);
        let start = (quiet * module_px) as f64;
        let end = ((quiet as usize + matrix.size) * module_px as usize) as f64;
        assert!(scanned[0].rect[0] <= start + 2.0 && scanned[0].rect[1] <= start + 2.0, "got {:?}", scanned[0].rect);
        assert!(scanned[0].rect[2] >= end - 2.0 && scanned[0].rect[3] >= end - 2.0, "got {:?}", scanned[0].rect);
    }

    #[test]
    fn test_scan_page_codes_checks_image_budget_before_decode() {
        assert_eq!(fits_scan_budget(8000, 8000, 0, MAX_EMBEDDED_SCAN_PIXELS), None);
        assert_eq!(fits_scan_budget(4000, 4000, 20_000_000, MAX_EMBEDDED_SCAN_PIXELS), None);
        assert_eq!(fits_scan_budget(1000, 1000, 0, MAX_EMBEDDED_SCAN_PIXELS), Some(1_000_000));

        // Page with an oversized 8000x8000 JPEG header (would be 256MB RGBA if decoded) plus a small valid QR image.
        let qr = QrCode::encode_text("OK-BUDGET").expect("encode qr");
        let qr_png = qr.to_png_bytes(4, 2).expect("qr png");
        let qr_img = craft_image_codecs::decode(&qr_png).expect("decode qr png");
        let (qw, qh) = (qr_img.width(), qr_img.height());
        let qr_rgb: Vec<u8> = qr_img.to_rgba8().chunks_exact(4).flat_map(|px| [px[0], px[1], px[2]]).collect();

        // Minimal JPEG SOI + SOF0 header claiming 8000x8000 (3 components), never decoded.
        let oversized_jpeg: Vec<u8> = vec![
            0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x1F, 0x40, 0x1F, 0x40, 0x03, 0x01, 0x11, 0x00, 0x02, 0x11, 0x00, 0x03, 0x11, 0x00, 0xFF, 0xD9,
        ];
        assert_eq!(craft_image_codecs::probe_size(&oversized_jpeg).ok(), Some((8000, 8000)));

        let mut cos = pdfcraft_cos::Document::new_empty();
        let pages = cos.get(cos.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
        // Case A: PDF header reports 8000x8000 (skipped before image_file/decode).
        let mut big_dict = pdfcraft_cos::Dict::new();
        big_dict.set(b"Type".to_vec(), pdfcraft_cos::Object::name("XObject"));
        big_dict.set(b"Subtype".to_vec(), pdfcraft_cos::Object::name("Image"));
        big_dict.set(b"Width".to_vec(), pdfcraft_cos::Object::Int(8000));
        big_dict.set(b"Height".to_vec(), pdfcraft_cos::Object::Int(8000));
        big_dict.set(b"ColorSpace".to_vec(), pdfcraft_cos::Object::name("DeviceRGB"));
        big_dict.set(b"BitsPerComponent".to_vec(), pdfcraft_cos::Object::Int(8));
        big_dict.set(b"Filter".to_vec(), pdfcraft_cos::Object::name("DCTDecode"));
        let big_ref = cos.add(pdfcraft_cos::Object::Stream(pdfcraft_cos::Stream::from_raw(big_dict, oversized_jpeg.clone())));

        // Case B: PDF header under-reports 1x1, while JPEG SOF0 header probes as 8000x8000 (skipped by probe_size before decode).
        let mut lying_dict = pdfcraft_cos::Dict::new();
        lying_dict.set(b"Type".to_vec(), pdfcraft_cos::Object::name("XObject"));
        lying_dict.set(b"Subtype".to_vec(), pdfcraft_cos::Object::name("Image"));
        lying_dict.set(b"Width".to_vec(), pdfcraft_cos::Object::Int(1));
        lying_dict.set(b"Height".to_vec(), pdfcraft_cos::Object::Int(1));
        lying_dict.set(b"ColorSpace".to_vec(), pdfcraft_cos::Object::name("DeviceRGB"));
        lying_dict.set(b"BitsPerComponent".to_vec(), pdfcraft_cos::Object::Int(8));
        lying_dict.set(b"Filter".to_vec(), pdfcraft_cos::Object::name("DCTDecode"));
        let lying_ref = cos.add(pdfcraft_cos::Object::Stream(pdfcraft_cos::Stream::from_raw(lying_dict, oversized_jpeg)));

        let mut small_dict = pdfcraft_cos::Dict::new();
        small_dict.set(b"Type".to_vec(), pdfcraft_cos::Object::name("XObject"));
        small_dict.set(b"Subtype".to_vec(), pdfcraft_cos::Object::name("Image"));
        small_dict.set(b"Width".to_vec(), pdfcraft_cos::Object::Int(i64::from(qw)));
        small_dict.set(b"Height".to_vec(), pdfcraft_cos::Object::Int(i64::from(qh)));
        small_dict.set(b"ColorSpace".to_vec(), pdfcraft_cos::Object::name("DeviceRGB"));
        small_dict.set(b"BitsPerComponent".to_vec(), pdfcraft_cos::Object::Int(8));
        let small_ref = cos.add(pdfcraft_cos::Object::Stream(pdfcraft_cos::Stream::from_raw(small_dict, qr_rgb)));

        let mut xobjects = pdfcraft_cos::Dict::new();
        xobjects.set(b"ImBig".to_vec(), pdfcraft_cos::Object::Ref(big_ref));
        xobjects.set(b"ImLying".to_vec(), pdfcraft_cos::Object::Ref(lying_ref));
        xobjects.set(b"ImSmall".to_vec(), pdfcraft_cos::Object::Ref(small_ref));
        let mut res = pdfcraft_cos::Dict::new();
        res.set(b"XObject".to_vec(), pdfcraft_cos::Object::Dict(xobjects));
        let content_ref = cos.add(pdfcraft_cos::Object::Stream(pdfcraft_cos::Stream::from_raw(
            pdfcraft_cos::Dict::new(),
            b"q 100 0 0 100 50 50 cm /ImBig Do Q q 100 0 0 100 50 200 cm /ImLying Do Q q 100 0 0 100 200 200 cm /ImSmall Do Q".to_vec(),
        )));
        let mut page = pdfcraft_cos::Dict::new();
        page.set(b"Type".to_vec(), pdfcraft_cos::Object::name("Page"));
        page.set(b"Parent".to_vec(), pdfcraft_cos::Object::Ref(pages));
        page.set(b"MediaBox".to_vec(), pdfcraft_cos::Object::Array(vec![0.into(), 0.into(), 400.into(), 400.into()]));
        page.set(b"Resources".to_vec(), pdfcraft_cos::Object::Dict(res));
        page.set(b"Contents".to_vec(), pdfcraft_cos::Object::Ref(content_ref));
        let page_ref = cos.add(page);
        cos.update_dict(pages, |d| {
            d.set(b"Kids".to_vec(), pdfcraft_cos::Object::Array(vec![pdfcraft_cos::Object::Ref(page_ref)]));
            d.set(b"Count".to_vec(), pdfcraft_cos::Object::Int(1));
        })
        .unwrap();
        let bytes = pdfcraft_cos::write_full(&cos, &pdfcraft_cos::SaveOptions::default()).unwrap();
        let mut session = crate::Session::new();
        let id = session.open("oversized.pdf".into(), None, std::sync::Arc::new(bytes), None).unwrap();
        let doc = session.get(id).unwrap();
        let imgs = doc.page_images(0);
        let mut budget_used = 0usize;
        assert!(doc.decode_embedded_image_for_scan(0, 0, &imgs[0], &mut budget_used).is_none());
        assert!(doc.decode_embedded_image_for_scan(0, 1, &imgs[1], &mut budget_used).is_none());
        assert_eq!(budget_used, 0);
        let codes = doc.scan_page_codes(0);
        assert!(codes.iter().any(|c| c.text == "OK-BUDGET"), "expected valid small QR despite oversized neighbors: {codes:?}");
    }

    #[test]
    fn test_generate_replacement_code_rejects_oversized_dark_span() {
        let err = Document::generate_replacement_code(CodeKind::Code128, "EXACT100", 100, 40)
            .expect_err("must reject 1D barcode whose module span exceeds target width");
        assert!(err.contains("exceeds requested dimensions"), "unexpected error: {err}");
    }
}
