//! Print production ▸ Add printer marks: crop and bleed marks, registration targets, colour bars
//! and a page information line, drawn outside the trim box on a media box grown to hold them.
//!
//! The marks are new page content in default user space (ISO 32000-2 §8.3.2), drawn in 100% of
//! every process ink (`1 1 1 1 K`, the usual stand-in for registration colour) so they print on
//! every separation. The page keeps its trim and bleed boxes (set explicitly when they were the
//! defaults) and the crop box becomes the new media box, so a viewer shows the marks too.

use pdfcraft_cos::Document;
use pdfcraft_organize::{BoxSpec, PageBox};

use crate::EditError;

/// Which marks to draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrinterMarks {
    pub crop: bool,
    pub bleed: bool,
    pub registration: bool,
    pub color_bars: bool,
    pub page_info: bool,
    /// Line weight of the marks in points (Acrobat offers 0.125, 0.25 and 0.5).
    pub line_width: f64,
}

impl Default for PrinterMarks {
    fn default() -> Self {
        PrinterMarks { crop: true, bleed: true, registration: true, color_bars: true, page_info: true, line_width: 0.25 }
    }
}

impl PrinterMarks {
    pub fn any(&self) -> bool {
        self.crop || self.bleed || self.registration || self.color_bars || self.page_info
    }
}

/// How far crop marks stay clear of the trim edge beyond the bleed (points).
const GAP: f64 = 3.0;
/// Length of a crop mark (points).
const CROP_LEN: f64 = 18.0;
/// Length of a bleed mark (points).
const BLEED_LEN: f64 = 9.0;
/// Radius of a registration target's circle (points).
const TARGET_R: f64 = 5.0;
/// Side of one colour bar patch (points).
const PATCH: f64 = 10.0;

fn n(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".to_string() } else { s.to_string() }
}

fn line(out: &mut String, a: (f64, f64), b: (f64, f64)) {
    out.push_str(&format!("{} {} m {} {} l S\n", n(a.0), n(a.1), n(b.0), n(b.1)));
}

/// A registration target: a circle with a cross through it, centred on `c`.
fn target(out: &mut String, c: (f64, f64)) {
    // Four Bézier quarter circles (k = 4/3·(√2 − 1)).
    let (x, y, r) = (c.0, c.1, TARGET_R);
    let k = 0.552_284_75 * r;
    out.push_str(&format!(
        "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c S\n",
        n(x + r),
        n(y),
        n(x + r),
        n(y + k),
        n(x + k),
        n(y + r),
        n(x),
        n(y + r),
        n(x - k),
        n(y + r),
        n(x - r),
        n(y + k),
        n(x - r),
        n(y),
        n(x - r),
        n(y - k),
        n(x - k),
        n(y - r),
        n(x),
        n(y - r),
        n(x + k),
        n(y - r),
        n(x + r),
        n(y - k),
        n(x + r),
        n(y),
    ));
    let arm = r + 3.0;
    line(out, (x - arm, y), (x + arm, y));
    line(out, (x, y - arm), (x, y + arm));
}

/// Text Helvetica (WinAnsi) can draw: accents dropped from Vietnamese and other Latin letters,
/// anything else as `?`, and the string escaped for a literal.
fn pdf_text(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let base = match c {
            'à' | 'á' | 'ạ' | 'ả' | 'ã' | 'â' | 'ầ' | 'ấ' | 'ậ' | 'ẩ' | 'ẫ' | 'ă' | 'ằ' | 'ắ' | 'ặ' | 'ẳ' | 'ẵ' | 'ä' | 'å' => {
                'a'
            }
            'À' | 'Á' | 'Ạ' | 'Ả' | 'Ã' | 'Â' | 'Ầ' | 'Ấ' | 'Ậ' | 'Ẩ' | 'Ẫ' | 'Ă' | 'Ằ' | 'Ắ' | 'Ặ' | 'Ẳ' | 'Ẵ' | 'Ä' | 'Å' => {
                'A'
            }
            'è' | 'é' | 'ẹ' | 'ẻ' | 'ẽ' | 'ê' | 'ề' | 'ế' | 'ệ' | 'ể' | 'ễ' | 'ë' => 'e',
            'È' | 'É' | 'Ẹ' | 'Ẻ' | 'Ẽ' | 'Ê' | 'Ề' | 'Ế' | 'Ệ' | 'Ể' | 'Ễ' | 'Ë' => 'E',
            'ì' | 'í' | 'ị' | 'ỉ' | 'ĩ' | 'î' | 'ï' => 'i',
            'Ì' | 'Í' | 'Ị' | 'Ỉ' | 'Ĩ' | 'Î' | 'Ï' => 'I',
            'ò' | 'ó' | 'ọ' | 'ỏ' | 'õ' | 'ô' | 'ồ' | 'ố' | 'ộ' | 'ổ' | 'ỗ' | 'ơ' | 'ờ' | 'ớ' | 'ợ' | 'ở' | 'ỡ' | 'ö' => {
                'o'
            }
            'Ò' | 'Ó' | 'Ọ' | 'Ỏ' | 'Õ' | 'Ô' | 'Ồ' | 'Ố' | 'Ộ' | 'Ổ' | 'Ỗ' | 'Ơ' | 'Ờ' | 'Ớ' | 'Ợ' | 'Ở' | 'Ỡ' | 'Ö' => {
                'O'
            }
            'ù' | 'ú' | 'ụ' | 'ủ' | 'ũ' | 'ư' | 'ừ' | 'ứ' | 'ự' | 'ử' | 'ữ' | 'û' | 'ü' => 'u',
            'Ù' | 'Ú' | 'Ụ' | 'Ủ' | 'Ũ' | 'Ư' | 'Ừ' | 'Ứ' | 'Ự' | 'Ử' | 'Ữ' | 'Û' | 'Ü' => 'U',
            'ỳ' | 'ý' | 'ỵ' | 'ỷ' | 'ỹ' => 'y',
            'Ỳ' | 'Ý' | 'Ỵ' | 'Ỷ' | 'Ỹ' => 'Y',
            'đ' => 'd',
            'Đ' => 'D',
            c if c.is_ascii() && !c.is_ascii_control() => c,
            _ => '?',
        };
        if matches!(base, '(' | ')' | '\\') {
            out.push('\\');
        }
        out.push(base);
    }
    out
}

/// The marks for one page: its trim and bleed boxes, its page information line, and how far the
/// marks reach past the trim box (`extent`).
fn page_content(marks: &PrinterMarks, trim: [f64; 4], bleed: [f64; 4], info: &str) -> String {
    let [x0, y0, x1, y1] = trim;
    // How far the bleed reaches past the trim, on the widest side.
    let reach = [x0 - bleed[0], y0 - bleed[1], bleed[2] - x1, bleed[3] - y1].into_iter().fold(0.0_f64, f64::max).max(0.0);
    let start = reach + GAP;
    let lw = if marks.line_width.is_finite() && marks.line_width > 0.0 { marks.line_width.min(4.0) } else { 0.25 };
    let mut out = format!("q\n1 1 1 1 K\n1 1 1 1 k\n{} w\n0 J\n[] 0 d\n", n(lw));
    if marks.crop {
        for (cx, sx) in [(x0, -1.0), (x1, 1.0)] {
            for (cy, sy) in [(y0, -1.0), (y1, 1.0)] {
                line(&mut out, (cx + sx * start, cy), (cx + sx * (start + CROP_LEN), cy));
                line(&mut out, (cx, cy + sy * start), (cx, cy + sy * (start + CROP_LEN)));
            }
        }
    }
    if marks.bleed && reach > 0.0 {
        let [bx0, by0, bx1, by1] = bleed;
        for (cx, bx, sx) in [(x0, bx0, -1.0), (x1, bx1, 1.0)] {
            for (cy, by, sy) in [(y0, by0, -1.0), (y1, by1, 1.0)] {
                // At the bleed edge, clear of the crop marks' lines.
                line(&mut out, (cx + sx * start, by), (cx + sx * (start + BLEED_LEN), by));
                line(&mut out, (bx, cy + sy * start), (bx, cy + sy * (start + BLEED_LEN)));
            }
        }
    }
    let mid = start + CROP_LEN / 2.0;
    if marks.registration {
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        for c in [(cx, y1 + mid), (cx, y0 - mid), (x0 - mid, cy), (x1 + mid, cy)] {
            target(&mut out, c);
        }
    }
    if marks.color_bars {
        // Process inks, their overprints, then a black tint ramp.
        let patches: [[f64; 4]; 11] = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
            [1.0, 1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0, 0.0],
            [0.0, 1.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 0.25],
            [0.0, 0.0, 0.0, 0.5],
            [0.0, 0.0, 0.0, 0.75],
            [0.0, 0.0, 0.0, 0.0],
        ];
        // Right of the top registration target, as many as fit before the trim's right edge.
        let y = y1 + start + 2.0;
        let first = if marks.registration { (x0 + x1) / 2.0 + TARGET_R + 8.0 } else { x0 };
        let room = ((x1 - first) / PATCH).floor();
        let fit = if room.is_finite() && room > 0.0 { (room as usize).min(patches.len()) } else { 0 };
        for (i, [c, m, ye, k]) in patches.iter().take(fit).enumerate() {
            let x = first + i as f64 * PATCH;
            out.push_str(&format!("{} {} {} {} k {} {} {} {} re f\n", n(*c), n(*m), n(*ye), n(*k), n(x), n(y), n(PATCH), n(PATCH)));
        }
        if fit == patches.len() {
            // An outline around the white patch, so it reads as a patch.
            out.push_str(&format!("{} {} {} {} re S\n", n(first + 10.0 * PATCH), n(y), n(PATCH), n(PATCH)));
        }
    }
    if marks.page_info && !info.trim().is_empty() {
        out.push_str(&format!("0 0 0 1 k\nBT /PCHelv 7 Tf {} {} Td ({}) Tj ET\n", n(x0), n(y0 - start - CROP_LEN - 9.0), pdf_text(info)));
    }
    out.push_str("Q\n");
    out
}

/// How far the marks reach past the trim box: the media box grows to trim + this on every side.
fn extent(trim: [f64; 4], bleed: [f64; 4]) -> f64 {
    let reach = [trim[0] - bleed[0], trim[1] - bleed[1], bleed[2] - trim[2], bleed[3] - trim[3]].into_iter().fold(0.0_f64, f64::max).max(0.0);
    // The crop marks, then room for the page information line under them.
    reach + GAP + CROP_LEN + 18.0
}

/// Add printer marks to `pages` (0-based). `info(page)` is the page information line.
pub fn add_printer_marks(doc: &mut Document, pages: &[usize], marks: &PrinterMarks, info: &dyn Fn(usize) -> String) -> Result<(), EditError> {
    if !marks.any() {
        return Err(EditError::Invalid("choose at least one kind of printer mark".into()));
    }
    let boxes = pdfcraft_organize::page_boxes(doc)?;
    for &p in pages {
        if p >= boxes.len() {
            return Err(EditError::Invalid(format!("page {} does not exist", p.saturating_add(1))));
        }
    }
    for &p in pages {
        let Some(b) = boxes.get(p) else { continue };
        let (media, trim, bleed) = (b[0], b[3], b[2]);
        let e = extent(trim, bleed);
        let grown = [media[0].min(trim[0] - e), media[1].min(trim[1] - e), media[2].max(trim[2] + e), media[3].max(trim[3] + e)];
        if !grown.iter().all(|v| v.is_finite()) {
            return Err(EditError::Invalid(format!("page {} has a page box that isn't a number", p.saturating_add(1))));
        }
        // Trim and bleed first (they'd otherwise follow the crop box out), then the media and
        // crop boxes around the marks.
        pdfcraft_organize::set_page_box(doc, &[p], PageBox::Trim, BoxSpec::Rect(trim))?;
        pdfcraft_organize::set_page_box(doc, &[p], PageBox::Bleed, BoxSpec::Rect(bleed))?;
        pdfcraft_organize::set_page_box(doc, &[p], PageBox::Media, BoxSpec::Rect(grown))?;
        pdfcraft_organize::set_page_box(doc, &[p], PageBox::Crop, BoxSpec::Rect(grown))?;
        pdfcraft_edit::stamp(doc, p, "PrinterMarks", page_content(marks, trim, bleed, &info(p)).into_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_information_drops_accents_and_escapes() {
        assert_eq!(pdf_text("Bảng lương (T9) \\ Đà Nẵng"), "Bang luong \\(T9\\) \\\\ Da Nang");
        assert_eq!(pdf_text("日本"), "??");
    }

    #[test]
    fn marks_stay_outside_the_trim_box() {
        let trim = [36.0, 36.0, 576.0, 756.0];
        let bleed = [27.0, 27.0, 585.0, 765.0];
        let content = page_content(&PrinterMarks::default(), trim, bleed, "file.pdf Page 1");
        // Every line endpoint is outside the trim box (on or past an edge).
        for l in content.lines().filter(|l| l.ends_with(" l S")) {
            let v: Vec<f64> = l.split_whitespace().filter_map(|t| t.parse().ok()).collect();
            for pt in v.chunks(2) {
                let inside = pt[0] > trim[0] && pt[0] < trim[2] && pt[1] > trim[1] && pt[1] < trim[3];
                assert!(!inside, "{l}");
            }
        }
        assert!(extent(trim, bleed) > 9.0 + GAP + CROP_LEN);
        assert!(content.contains("(file.pdf Page 1) Tj"));
    }
}
