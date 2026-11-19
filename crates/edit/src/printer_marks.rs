//! Printer marks (Acrobat's Print production ▸ Add Printer Marks): trim and
//! bleed marks, registration targets, colour bars and page information drawn outside the trim
//! box, as a page mark like a header, so they can be removed or replaced. The page is widened
//! when asked and the marks would otherwise fall outside it; the trim box stays where it is.
//!
//! Lines, targets and the information line are in registration colour, the `/All` Separation
//! (ISO 32000-2, 8.6.6.4), so they print on every plate, spot plates too; viewers show its
//! four-ink alternate. The colour bars are process inks and stay DeviceCMYK.

use pdfcraft_cos::{Dict, Document, Object};
use pdfcraft_fonts::{literal, win_ansi};

use crate::marks::{MarkKind, n};
use crate::place::{END, add_resources, begin, check, page_list, place};
use crate::{EditError, remove_marks};

/// Which marks, and how heavy.
#[derive(Clone, Debug, PartialEq)]
pub struct PrinterMarks {
    pub trim: bool,
    pub bleed: bool,
    pub registration: bool,
    pub color_bars: bool,
    pub page_info: bool,
    /// Line weight in points (Acrobat: 0.25, 0.5, 1).
    pub line_weight: f64,
    /// Widen the page by this many points on each side (at least what the marks need) when
    /// they would fall outside it; `None` leaves the page and clips the marks.
    pub make_room: Option<f64>,
    /// What the page information line says before the page number (the file's name).
    pub label: String,
    /// The date on the page information line.
    pub date: String,
}

impl Default for PrinterMarks {
    fn default() -> Self {
        Self {
            trim: true,
            bleed: true,
            registration: true,
            color_bars: true,
            page_info: true,
            line_weight: 0.25,
            make_room: Some(36.0),
            label: String::new(),
            date: String::new(),
        }
    }
}

/// How far a mark reaches beyond the trim box: a gap, then the mark itself.
const GAP: f64 = 6.0;
const LENGTH: f64 = 18.0;
/// Registration targets sit this far beyond the trim box, centred on each side.
const TARGET_OFFSET: f64 = 16.0;
const TARGET_RADIUS: f64 = 4.0;
const BAR: f64 = 10.0;
const INFO_SIZE: f64 = 7.0;
/// The colour space resource holding registration colour.
const REGISTRATION: &str = "PCReg";
/// The largest page side PDF viewers take (Annex C of ISO 32000-1 lists it as Acrobat's limit).
const MAX_PAGE_SIDE: f64 = 14_400.0;

/// `[/Separation /All /DeviceCMYK <tint>]`: full tint is all four process inks.
fn registration() -> Object {
    let reals = |v: &[f64]| Object::Array(v.iter().map(|x| Object::Real(*x)).collect());
    let mut tint = Dict::new();
    tint.set(b"FunctionType".to_vec(), Object::Int(2));
    tint.set(b"Domain".to_vec(), reals(&[0.0, 1.0]));
    tint.set(b"C0".to_vec(), reals(&[0.0; 4]));
    tint.set(b"C1".to_vec(), reals(&[1.0; 4]));
    tint.set(b"N".to_vec(), Object::Int(1));
    Object::Array(vec![Object::name("Separation"), Object::name("All"), Object::name("DeviceCMYK"), Object::Dict(tint)])
}

impl PrinterMarks {
    /// The line weights a run accepts, in points.
    pub const WEIGHTS: std::ops::RangeInclusive<f64> = 0.1..=4.0;
    /// The most a run widens a page by on each side at the caller's asking, in points.
    pub const MAX_ROOM: f64 = 288.0;

    fn any(&self) -> bool {
        self.trim || self.bleed || self.registration || self.color_bars || self.page_info
    }

    /// How far beyond the trim box the chosen marks reach.
    pub fn reach(&self) -> f64 {
        let mut r: f64 = 0.0;
        if self.trim || self.bleed {
            r = r.max(GAP + LENGTH);
        }
        if self.registration {
            r = r.max(TARGET_OFFSET + TARGET_RADIUS + 2.0);
        }
        if self.color_bars || self.page_info {
            r = r.max(GAP + BAR + 6.0);
        }
        r
    }

    /// The marks' content for a page whose trim box is `trim` and bleed box `bleed` (user
    /// space), `page` of `count` (1-based).
    pub fn content(&self, trim: [f64; 4], bleed: [f64; 4], page: usize, count: usize) -> String {
        let w = self.line_weight.clamp(*Self::WEIGHTS.start(), *Self::WEIGHTS.end());
        // Registration colour: every plate.
        let mut c = format!("/{REGISTRATION} CS 1 SCN /{REGISTRATION} cs 1 scn {} w 0 J\n", n(w));
        let [x0, y0, x1, y1] = trim;
        let line = |c: &mut String, ax: f64, ay: f64, bx: f64, by: f64| {
            c.push_str(&format!("{} {} m {} {} l S\n", n(ax), n(ay), n(bx), n(by)));
        };
        // Crop (trim) marks: at each corner, outside the box, along both edges.
        let corner_marks = |c: &mut String, b: [f64; 4]| {
            let [bx0, by0, bx1, by1] = b;
            for (x, dir) in [(bx0, -1.0), (bx1, 1.0)] {
                for y in [by0, by1] {
                    // Vertical mark beyond the top or bottom edge.
                    let (ya, yb) = if y == by0 { (y - GAP, y - GAP - LENGTH) } else { (y + GAP, y + GAP + LENGTH) };
                    line(c, x, ya, x, yb);
                    // Horizontal mark beyond the left or right edge.
                    line(c, x + dir * GAP, y, x + dir * (GAP + LENGTH), y);
                }
            }
        };
        if self.trim {
            corner_marks(&mut c, trim);
        }
        if self.bleed && bleed != trim {
            c.push_str(&format!("[{} {}] 0 d\n", n(w * 4.0), n(w * 4.0)));
            corner_marks(&mut c, bleed);
            c.push_str("[] 0 d\n");
        }
        if self.registration {
            let r = TARGET_RADIUS;
            let k = 0.5523 * r;
            let target = |c: &mut String, cx: f64, cy: f64| {
                c.push_str(&format!(
                    "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c S\n",
                    n(cx + r),
                    n(cy),
                    n(cx + r),
                    n(cy + k),
                    n(cx + k),
                    n(cy + r),
                    n(cx),
                    n(cy + r),
                    n(cx - k),
                    n(cy + r),
                    n(cx - r),
                    n(cy + k),
                    n(cx - r),
                    n(cy),
                    n(cx - r),
                    n(cy - k),
                    n(cx - k),
                    n(cy - r),
                    n(cx),
                    n(cy - r),
                    n(cx + k),
                    n(cy - r),
                    n(cx + r),
                    n(cy - k),
                    n(cx + r),
                    n(cy)
                ));
                line(c, cx - r * 1.6, cy, cx + r * 1.6, cy);
                line(c, cx, cy - r * 1.6, cx, cy + r * 1.6);
            };
            let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            target(&mut c, mx, y1 + TARGET_OFFSET);
            target(&mut c, mx, y0 - TARGET_OFFSET);
            target(&mut c, x0 - TARGET_OFFSET, my);
            target(&mut c, x1 + TARGET_OFFSET, my);
        }
        if self.color_bars {
            // Above the top edge, from the left: C, M, Y, K at 100%, then 75, 50, 25% K.
            let y = y1 + GAP + 2.0;
            let x = x0 + LENGTH + GAP + 12.0;
            let swatches: [(f64, f64, f64, f64); 7] = [
                (1.0, 0.0, 0.0, 0.0),
                (0.0, 1.0, 0.0, 0.0),
                (0.0, 0.0, 1.0, 0.0),
                (0.0, 0.0, 0.0, 1.0),
                (0.0, 0.0, 0.0, 0.75),
                (0.0, 0.0, 0.0, 0.5),
                (0.0, 0.0, 0.0, 0.25),
            ];
            for (i, (cy, m, yl, k)) in swatches.iter().enumerate() {
                c.push_str(&format!("{} {} {} {} k {} {} {} {} re f\n", n(*cy), n(*m), n(*yl), n(*k), n(x + i as f64 * BAR), n(y), n(BAR), n(BAR)));
            }
            c.push_str(&format!("/{REGISTRATION} cs 1 scn\n"));
        }
        if self.page_info {
            let mut info = String::new();
            if !self.label.trim().is_empty() {
                info.push_str(self.label.trim());
                info.push_str("  ");
            }
            info.push_str(&format!("{page} of {count}"));
            if !self.date.trim().is_empty() {
                info.push_str("  ");
                info.push_str(self.date.trim());
            }
            let info: String = info.chars().filter(|ch| pdfcraft_fonts::win_ansi_encodable(*ch)).collect();
            c.push_str(&format!("BT /PCHelv {} Tf 1 0 0 1 {} {} Tm ", n(INFO_SIZE), n(x0 + LENGTH + GAP + 12.0), n(y0 - GAP - BAR)));
            c.push_str(&ascii_literal(&literal(&win_ansi(&info))));
            c.push_str(" Tj ET\n");
        }
        c
    }
}

/// A PDF literal string made of WinAnsi bytes, as ASCII text: bytes from 0x80 up are written as
/// octal escapes, so they reach the content stream unchanged instead of being replaced when the
/// bytes are turned into a `String`.
fn ascii_literal(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if b < 0x80 {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("\\{b:03o}"));
        }
    }
    out
}

/// How far the chosen marks reach beyond the trim box on each side (left, bottom, right, top):
/// `reach`, or more where the bleed marks, which start at the bleed box, go further out.
fn side_reach(marks: &PrinterMarks, trim: [f64; 4], bleed: [f64; 4]) -> [f64; 4] {
    let base = marks.reach();
    if !(marks.bleed && bleed != trim) {
        return [base; 4];
    }
    let beyond = |bleed_edge: f64, trim_edge: f64| base.max((trim_edge - bleed_edge).abs() + GAP + LENGTH);
    [beyond(bleed[0], trim[0]), beyond(bleed[1], trim[1]), beyond(bleed[2], trim[2]), beyond(bleed[3], trim[3])]
}

/// Add printer marks to `pages` (0-based). With `replace`, marks already there are removed
/// first. Returns the pages that were widened (media or crop box) to make room.
pub fn add_printer_marks(doc: &mut Document, pages: &[usize], marks: &PrinterMarks, replace: bool) -> Result<Vec<usize>, EditError> {
    let all = page_list(doc);
    check(pages, all.len())?;
    if !marks.any() {
        return Err(EditError::Invalid("choose at least one kind of mark".into()));
    }
    if !PrinterMarks::WEIGHTS.contains(&marks.line_weight) {
        let (low, high) = (PrinterMarks::WEIGHTS.start(), PrinterMarks::WEIGHTS.end());
        return Err(EditError::Invalid(format!("the line weight must be from {low} to {high} points")));
    }
    if marks.make_room.is_some_and(|room| !(0.0..=PrinterMarks::MAX_ROOM).contains(&room)) {
        return Err(EditError::Invalid(format!("the room to make must be from 0 to {} points", PrinterMarks::MAX_ROOM)));
    }
    if replace {
        remove_marks(doc, pages, MarkKind::PrinterMarks)?;
    }
    let count = all.len();
    let mut widened = Vec::new();
    for &i in pages {
        let [media, crop, bleed, trim, art] = pdfcraft_organize_boxes(doc, i)?;
        let side = side_reach(marks, trim, bleed);
        let needed = [trim[0] - side[0], trim[1] - side[1], trim[2] + side[2], trim[3] + side[3]];
        let within = |b: [f64; 4]| b[0] <= needed[0] && b[1] <= needed[1] && b[2] >= needed[2] && b[3] >= needed[3];
        // The crop box is what shows, so the marks need room inside it, not only inside the media box.
        if !within(crop)
            && let Some(room) = marks.make_room
        {
            let new_media = if within(media) {
                None
            } else {
                let room = |s: f64| room.max(s);
                Some([
                    media[0].min(trim[0] - room(side[0])),
                    media[1].min(trim[1] - room(side[1])),
                    media[2].max(trim[2] + room(side[2])),
                    media[3].max(trim[3] + room(side[3])),
                ])
            };
            if new_media.is_some_and(|m| m[2] - m[0] > MAX_PAGE_SIDE || m[3] - m[1] > MAX_PAGE_SIDE) {
                return Err(EditError::Invalid(format!("page {} would be wider than the {MAX_PAGE_SIDE} points a PDF page can be", i + 1)));
            }
            // Without a wider media box, only the crop box grows, to the marks and no further than the media box.
            let new_crop = match new_media {
                Some(m) => m,
                None => [
                    crop[0].min(needed[0]).max(media[0]),
                    crop[1].min(needed[1]).max(media[1]),
                    crop[2].max(needed[2]).min(media[2]),
                    crop[3].max(needed[3]).min(media[3]),
                ],
            };
            let rect = |r: [f64; 4]| Object::Array(r.iter().map(|v| Object::Real(*v)).collect());
            let page = page_list(doc)[i].clone();
            doc.update_dict(page.obj, |d| {
                if let Some(m) = new_media {
                    d.set(b"MediaBox".to_vec(), rect(m));
                }
                // The crop box shows the marks too. The trim, bleed and art boxes default to the
                // crop box and are clipped to the media box, so each is written out as it was in
                // effect, or it would move with the wider boxes and a second run would widen the
                // page again.
                d.set(b"CropBox".to_vec(), rect(new_crop));
                for (key, value) in [(&b"BleedBox"[..], bleed), (&b"TrimBox"[..], trim), (&b"ArtBox"[..], art)] {
                    d.set(key.to_vec(), rect(value));
                }
            })?;
            widened.push(i);
        }
        let page = page_list(doc)[i].clone();
        let content = marks.content(trim, bleed, i + 1, count);
        let body = format!("{}{}{}", begin(MarkKind::PrinterMarks, "PrinterMarks", [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]), content, END);
        add_resources(doc, &page, None, Some(body.as_bytes()))?;
        let page = page_list(doc)[i].clone();
        // add_resources gave the page resources of its own.
        let mut res = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
        let mut spaces = res.get(b"ColorSpace").map(|c| doc.resolve(c)).and_then(|c| c.as_dict().cloned()).unwrap_or_default();
        spaces.set(REGISTRATION.as_bytes().to_vec(), registration());
        res.set(b"ColorSpace".to_vec(), Object::Dict(spaces));
        doc.update_dict(page.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?;
        let page = page_list(doc)[i].clone();
        place(doc, &page, MarkKind::PrinterMarks, body.into_bytes(), false)?;
    }
    Ok(widened)
}

/// A page's boxes: media, crop, bleed, trim, art (each inherited or falling back as the
/// standard says), without depending on the organize crate.
fn pdfcraft_organize_boxes(doc: &Document, page: usize) -> Result<[[f64; 4]; 5], EditError> {
    let p = page_list(doc).get(page).cloned().ok_or_else(|| EditError::Invalid(format!("page {} does not exist", page + 1)))?;
    let rect = |k: &[u8]| -> Option<[f64; 4]> {
        let a = p.dict.get(k).map(|o| doc.resolve(o))?;
        let v: Vec<f64> = a.as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
        (v.len() == 4 && v.iter().all(|x| x.is_finite())).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
    };
    let media = rect(b"MediaBox").unwrap_or([0.0, 0.0, 612.0, 792.0]);
    let clip = |r: [f64; 4]| [r[0].max(media[0]), r[1].max(media[1]), r[2].min(media[2]), r[3].min(media[3])];
    let crop = rect(b"CropBox").map(clip).unwrap_or(media);
    let (bleed, trim, art) = (rect(b"BleedBox").map(clip), rect(b"TrimBox").map(clip), rect(b"ArtBox").map(clip));
    Ok([media, crop, bleed.unwrap_or(crop), trim.unwrap_or(crop), art.unwrap_or(crop)])
}
