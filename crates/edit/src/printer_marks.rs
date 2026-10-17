//! Printer marks (Acrobat's Print production ▸ Add Printer Marks): trim and
//! bleed marks, registration targets, colour bars and page information drawn outside the trim
//! box, as a page mark like a header, so they can be removed or replaced. The page is widened
//! when asked and the marks would otherwise fall outside it; the trim box stays where it is.

use pdfcraft_cos::{Document, Object};
use pdfcraft_fonts::{literal, win_ansi};

use crate::{END, EditError, MarkKind, add_resources, begin, check, n, page_list, place, remove_marks};

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

impl PrinterMarks {
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
        let w = self.line_weight.clamp(0.1, 4.0);
        // Registration colour: every ink.
        let mut c = format!("1 1 1 1 K 1 1 1 1 k {} w 0 J\n", n(w));
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
            c.push_str("1 1 1 1 k\n");
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
            c.push_str(&String::from_utf8_lossy(&literal(&win_ansi(&info))));
            c.push_str(" Tj ET\n");
        }
        c
    }
}

/// Add printer marks to `pages` (0-based). With `replace`, marks already there are removed
/// first. Returns the pages that were widened to make room.
pub fn add_printer_marks(doc: &mut Document, pages: &[usize], marks: &PrinterMarks, replace: bool) -> Result<Vec<usize>, EditError> {
    let all = page_list(doc);
    check(pages, all.len())?;
    if !marks.any() {
        return Err(EditError::Invalid("choose at least one kind of mark".into()));
    }
    if !marks.line_weight.is_finite() || marks.line_weight <= 0.0 {
        return Err(EditError::Invalid("the line weight must be a positive number of points".into()));
    }
    if replace {
        remove_marks(doc, pages, MarkKind::PrinterMarks)?;
    }
    let count = all.len();
    let reach = marks.reach();
    let mut widened = Vec::new();
    for &i in pages {
        let boxes = pdfcraft_organize_boxes(doc, i)?;
        let [media, _crop, bleed, trim, _art] = boxes;
        let needed = [trim[0] - reach, trim[1] - reach, trim[2] + reach, trim[3] + reach];
        let fits = media[0] <= needed[0] && media[1] <= needed[1] && media[2] >= needed[2] && media[3] >= needed[3];
        if !fits && let Some(room) = marks.make_room {
            let room = room.max(reach);
            let new_media = [media[0].min(trim[0] - room), media[1].min(trim[1] - room), media[2].max(trim[2] + room), media[3].max(trim[3] + room)];
            let page = page_list(doc)[i].clone();
            let rect = Object::Array(new_media.iter().map(|v| Object::Real(*v)).collect());
            doc.update_dict(page.obj, |d| {
                d.set(b"MediaBox".to_vec(), rect.clone());
                // The crop box shows the marks too; the trim, bleed and art boxes stay.
                d.set(b"CropBox".to_vec(), rect.clone());
            })?;
            widened.push(i);
        }
        let page = page_list(doc)[i].clone();
        let content = marks.content(trim, bleed, i + 1, count);
        let body = format!("{}{}{}", begin(MarkKind::PrinterMarks, "PrinterMarks", [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]), content, END);
        add_resources(doc, &page, None, Some(body.as_bytes()))?;
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
    let bleed = rect(b"BleedBox").map(clip).unwrap_or(crop);
    let trim = rect(b"TrimBox").map(clip).unwrap_or(crop);
    let art = rect(b"ArtBox").map(clip).unwrap_or(crop);
    Ok([media, crop, bleed, trim, art])
}
