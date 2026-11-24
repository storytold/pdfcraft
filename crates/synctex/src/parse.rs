//! The `.synctex` reader. Lenient: a line it cannot read is skipped and counted.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::{Error, Rect};

/// The most a gzipped file may inflate to.
pub const MAX_INFLATED: u64 = 512 * 1024 * 1024;
/// The most boxes and point records kept, over all sheets. A file with more is refused, so what a
/// parse keeps in memory is bounded by this and not only by the size of the text: the shortest
/// record is a few bytes, and a box kept is some 70. TeX writes about one record per word, glue
/// or kern, so this is a document of several thousand pages.
pub const MAX_RECORDS: usize = 8_000_000;
/// The deepest box nesting kept; a box nested deeper is skipped and counted.
pub const MAX_DEPTH: usize = 256;

/// SyncTeX scaled points per PDF point.
const SP_PER_POINT: f64 = 65781.76;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoxKind {
    Horizontal,
    Vertical,
}

/// One point record (`x`, `k`, `g`, `$`, `r`), in points from the page's top-left.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Point {
    pub kind: char,
    pub tag: u32,
    pub line: u32,
    pub column: u32,
    pub x: f64,
    pub y: f64,
}

/// A box with the point records that belong directly to it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BoxRec {
    pub kind: BoxKind,
    pub tag: u32,
    pub line: u32,
    pub rect: Rect,
    pub points: Vec<Point>,
}

/// One output page.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Sheet {
    pub number: u32,
    pub boxes: Vec<BoxRec>,
}

/// A parsed SyncTeX file.
#[derive(Debug, Clone)]
pub struct SyncTex {
    pub(crate) inputs: Vec<(u32, PathBuf)>,
    pub(crate) sheets: Vec<Sheet>,
    skipped: usize,
}

impl SyncTex {
    /// Parse `.synctex` bytes, or `.synctex.gz` bytes (detected by the gzip magic).
    /// `base_dir` is the folder relative `Input:` paths are resolved against.
    pub fn parse(bytes: &[u8], base_dir: &Path) -> Result<SyncTex, Error> {
        parse_capped(bytes, base_dir, MAX_INFLATED)
    }

    /// The number of sheets (pages) read.
    pub fn sheets(&self) -> u32 {
        u32::try_from(self.sheets.len()).unwrap_or(u32::MAX)
    }

    /// The source files, in the order the file lists them.
    pub fn inputs(&self) -> impl Iterator<Item = &Path> {
        self.inputs.iter().map(|(_, p)| p.as_path())
    }

    /// How many lines were unreadable and left out.
    pub fn skipped(&self) -> usize {
        self.skipped
    }
}

pub(crate) fn parse_capped(bytes: &[u8], base_dir: &Path, cap: u64) -> Result<SyncTex, Error> {
    parse_limited(bytes, base_dir, cap, MAX_RECORDS)
}

/// [`parse_capped`] with the most boxes and points to keep named too.
pub(crate) fn parse_limited(bytes: &[u8], base_dir: &Path, cap: u64, max_records: usize) -> Result<SyncTex, Error> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let decoder = flate2::read::GzDecoder::new(bytes);
        let mut out = Vec::new();
        decoder.take(cap.saturating_add(1)).read_to_end(&mut out).map_err(|e| Error::Inflate(e.to_string()))?;
        if u64::try_from(out.len()).map_or(true, |n| n > cap) {
            return Err(Error::TooLarge);
        }
        parse_text(&out, base_dir, max_records)
    } else {
        parse_text(bytes, base_dir, max_records)
    }
}

/// The unit conversion from the header. `factor` gives scaled points; the offsets are in scaled
/// points too (`synctex(5)`), and the sum is divided by `SP_PER_POINT` last.
struct Scale {
    factor: f64,
    x_offset: f64,
    y_offset: f64,
}

impl Scale {
    fn x(&self, v: i64) -> Option<f64> {
        let r = (v as f64 * self.factor + self.x_offset) / SP_PER_POINT;
        r.is_finite().then_some(r)
    }
    fn y(&self, v: i64) -> Option<f64> {
        let r = (v as f64 * self.factor + self.y_offset) / SP_PER_POINT;
        r.is_finite().then_some(r)
    }
    /// A size has no offset.
    fn len(&self, v: i64) -> Option<f64> {
        let r = v as f64 * self.factor / SP_PER_POINT;
        r.is_finite().then_some(r)
    }
}

/// The fields of a box or point record: `tag,line[,column]:h,v[:W,H,D]`.
struct Fields {
    tag: u32,
    line: u32,
    column: u32,
    h: i64,
    v: i64,
    size: Option<[i64; 3]>,
}

fn parse_fields(s: &str) -> Option<Fields> {
    let mut parts = s.split(':');
    let mut head = parts.next()?.split(',');
    let tag = head.next()?.parse().ok()?;
    let line = head.next()?.parse().ok()?;
    let column = match head.next() {
        Some(c) => c.parse().ok()?,
        None => 0,
    };
    if head.next().is_some() {
        return None;
    }
    let mut pos = parts.next()?.split(',');
    let h = pos.next()?.parse().ok()?;
    let v = pos.next()?.parse().ok()?;
    let size = match parts.next() {
        Some(sz) => {
            let mut it = sz.split(',');
            let w = it.next()?.parse().ok()?;
            // Kerns carry only a width; boxes carry all three.
            match it.next() {
                Some(hh) => Some([w, hh.parse().ok()?, it.next()?.parse().ok()?]),
                None => Some([w, 0, 0]),
            }
        }
        None => None,
    };
    Some(Fields { tag, line, column, h, v, size })
}

fn box_rect(f: &Fields, sc: &Scale) -> Option<Rect> {
    let [w, h, d] = f.size.unwrap_or([0, 0, 0]);
    let x = sc.x(f.h)?;
    let y = sc.y(f.v)?;
    let (w, h, d) = (sc.len(w)?, sc.len(h)?, sc.len(d)?);
    let (xa, xb) = (x, x + w);
    let (ya, yb) = (y - h, y + d);
    let r = [xa.min(xb), ya.min(yb), xa.max(xb), ya.max(yb)];
    r.iter().all(|c| c.is_finite()).then_some(r)
}

fn input_path(raw: OsString, base_dir: &Path) -> PathBuf {
    let p = PathBuf::from(raw);
    let joined = if p.is_absolute() { p } else { base_dir.join(p) };
    // Dropping `.` components removes the `./` prefix SyncTeX writes.
    joined.components().filter(|c| *c != Component::CurDir).collect()
}

#[cfg(unix)]
fn os_string(bytes: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(bytes.to_vec())
}

#[cfg(not(unix))]
fn os_string(bytes: &[u8]) -> OsString {
    OsString::from(String::from_utf8_lossy(bytes).into_owned())
}

fn parse_text(bytes: &[u8], base_dir: &Path, max_records: usize) -> Result<SyncTex, Error> {
    let mut lines = bytes.split(|b| *b == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l));
    match lines.next() {
        Some(first) if first.starts_with(b"SyncTeX Version:") => {}
        _ => return Err(Error::NotSyncTex),
    }

    let mut inputs = Vec::new();
    let mut sheets: Vec<Sheet> = Vec::new();
    let mut skipped = 0usize;
    // Boxes and points kept so far, held to `max_records`.
    let mut records = 0usize;
    let mut count = move || {
        records = records.saturating_add(1);
        if records > max_records { Err(Error::TooManyRecords) } else { Ok(()) }
    };
    let (mut unit, mut magnification) = (1.0f64, 1000.0f64);
    let (mut x_offset, mut y_offset) = (0.0f64, 0.0f64);
    let mut scale = Scale { factor: 0.0, x_offset: 0.0, y_offset: 0.0 };
    let mut in_content = false;

    // The sheet being read, the stack of open boxes (indices into it) and the number of
    // boxes beyond `MAX_DEPTH` whose closers are still to come.
    let mut open_sheet: Option<Sheet> = None;
    let mut stack: Vec<usize> = Vec::new();
    let mut overflow = 0usize;

    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix(b"Input:") {
            let mut it = rest.splitn(2, |b| *b == b':');
            let tag = it.next().and_then(|t| std::str::from_utf8(t).ok()).and_then(|t| t.parse::<u32>().ok());
            match (tag, it.next()) {
                (Some(tag), Some(path)) => inputs.push((tag, input_path(os_string(path), base_dir))),
                _ => skipped += 1,
            }
            continue;
        }
        if !in_content {
            let Ok(text) = std::str::from_utf8(line) else {
                skipped += 1;
                continue;
            };
            if text == "Content:" {
                in_content = true;
                scale = Scale { factor: unit * magnification / 1000.0, x_offset, y_offset };
            } else if let Some((key, value)) = text.split_once(':') {
                let num = value.trim().parse::<f64>().ok().filter(|n| n.is_finite());
                match (key, num) {
                    ("Unit", Some(n)) => unit = n,
                    ("Magnification", Some(n)) => magnification = n,
                    ("X Offset", Some(n)) => x_offset = n,
                    ("Y Offset", Some(n)) => y_offset = n,
                    _ => {}
                }
            }
            continue;
        }

        let Ok(text) = std::str::from_utf8(line) else {
            skipped += 1;
            continue;
        };
        if text.starts_with("Postamble:") {
            break;
        }
        let mut chars = text.chars();
        let Some(first) = chars.next() else { continue };
        let rest = chars.as_str();
        match first {
            '{' => {
                if open_sheet.is_some() {
                    // The previous sheet never closed; it is not whole, so it is dropped.
                    skipped += 1;
                }
                match rest.parse::<u32>() {
                    Ok(number) => open_sheet = Some(Sheet { number, boxes: Vec::new() }),
                    Err(_) => {
                        open_sheet = None;
                        skipped += 1;
                    }
                }
                stack.clear();
                overflow = 0;
            }
            '}' => {
                if let Some(sheet) = open_sheet.take() {
                    sheets.push(sheet);
                } else {
                    skipped += 1;
                }
                stack.clear();
                overflow = 0;
            }
            '[' | '(' => {
                let Some(sheet) = open_sheet.as_mut() else {
                    skipped += 1;
                    continue;
                };
                // A box that cannot be opened still has a closer coming, so it counts as an
                // overflowed box: its closer must not pop the parent.
                let Some((f, rect)) = parse_fields(rest).and_then(|f| box_rect(&f, &scale).map(|r| (f, r))) else {
                    overflow = overflow.saturating_add(1);
                    skipped += 1;
                    continue;
                };
                if overflow > 0 || stack.len() >= MAX_DEPTH {
                    overflow = overflow.saturating_add(1);
                    skipped += 1;
                    continue;
                }
                let kind = if first == '[' { BoxKind::Vertical } else { BoxKind::Horizontal };
                count()?;
                sheet.boxes.push(BoxRec { kind, tag: f.tag, line: f.line, rect, points: Vec::new() });
                stack.push(sheet.boxes.len() - 1);
            }
            ']' | ')' => {
                if overflow > 0 {
                    overflow -= 1;
                } else if stack.pop().is_none() {
                    skipped += 1;
                }
            }
            'h' | 'v' | 'x' | 'k' | 'g' | '$' | 'r' => {
                let Some(sheet) = open_sheet.as_mut() else {
                    skipped += 1;
                    continue;
                };
                let Some(f) = parse_fields(rest) else {
                    skipped += 1;
                    continue;
                };
                if first == 'h' || first == 'v' {
                    // A void box: a box with no content, so it opens and closes at once.
                    let Some(rect) = box_rect(&f, &scale) else {
                        skipped += 1;
                        continue;
                    };
                    if overflow > 0 || stack.len() >= MAX_DEPTH {
                        skipped += 1;
                        continue;
                    }
                    let kind = if first == 'v' { BoxKind::Vertical } else { BoxKind::Horizontal };
                    count()?;
                    sheet.boxes.push(BoxRec { kind, tag: f.tag, line: f.line, rect, points: Vec::new() });
                    continue;
                }
                if overflow > 0 {
                    skipped += 1;
                    continue;
                }
                let (Some(x), Some(y)) = (scale.x(f.h), scale.y(f.v)) else {
                    skipped += 1;
                    continue;
                };
                let Some(b) = stack.last().and_then(|i| sheet.boxes.get_mut(*i)) else {
                    skipped += 1;
                    continue;
                };
                count()?;
                b.points.push(Point { kind: first, tag: f.tag, line: f.line, column: f.column, x, y });
            }
            // `!` (comment), `<`, `>`, `f` and anything else are ignored.
            _ => {}
        }
    }

    Ok(SyncTex { inputs, sheets, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const FIXTURE: &str = "SyncTeX Version:1
Input:1:/work/main.tex
Input:2:./chap/one.tex
Output:pdf
Magnification:1000
Unit:1
X Offset:0
Y Offset:0
Content:
!0
{1
[1,1:0,0:0,0,0
(1,12:6578176,13156352:19734528,657818,0
x1,12:6578176,13156352
x1,13:16445440,13156352
)
(2,5:6578176,19734528:19734528,657818,0
x2,5:6578176,19734528
)
]
}1
{2
[1,1:0,0:0,0,0
(1,40:6578176,13156352:19734528,657818,0
x1,40:6578176,13156352
)
]
}2
Postamble:
Count:9
";

    fn parse(s: &str) -> Result<SyncTex, Error> {
        SyncTex::parse(s.as_bytes(), Path::new("/work"))
    }

    #[test]
    fn fixture_parses() {
        let st = parse(FIXTURE).unwrap();
        assert_eq!(st.sheets(), 2);
        let inputs: Vec<&Path> = st.inputs().collect();
        assert_eq!(inputs, [Path::new("/work/main.tex"), Path::new("/work/chap/one.tex")]);
        assert_eq!(st.skipped(), 0);
        let b = &st.sheets[0].boxes[1];
        assert_eq!(b.kind, BoxKind::Horizontal);
        let want = [100.0, 190.0, 400.0, 200.0];
        for (g, w) in b.rect.iter().zip(want) {
            assert!((g - w).abs() < 0.01, "{:?}", b.rect);
        }
        assert_eq!(b.points.len(), 2);
        assert_eq!((b.points[1].tag, b.points[1].line), (1, 13));
    }

    #[test]
    fn gzip_is_detected() {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(FIXTURE.as_bytes()).unwrap();
        let gz = enc.finish().unwrap();
        let st = SyncTex::parse(&gz, Path::new("/work")).unwrap();
        assert_eq!(st.sheets(), 2);
    }

    #[test]
    fn inflate_cap_is_enforced() {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(FIXTURE.as_bytes()).unwrap();
        let gz = enc.finish().unwrap();
        assert!(matches!(parse_capped(&gz, Path::new("/work"), 10), Err(Error::TooLarge)));
    }

    /// Boxes and points count together, and one past the most kept refuses the file.
    #[test]
    fn the_record_cap_is_enforced() {
        let text = "SyncTeX Version:1\nInput:1:/work/main.tex\nContent:\n{1\n(1,1:0,0:10,10,0\nx1,1:0,0\nh1,2:0,0:10,10,0\n)\n}1\n";
        let limited = |max| parse_limited(text.as_bytes(), Path::new("/work"), MAX_INFLATED, max);
        assert!(matches!(limited(2), Err(Error::TooManyRecords)));
        assert_eq!(limited(3).unwrap().sheets(), 1);
    }

    #[test]
    fn offsets_are_scaled_points() {
        let text = FIXTURE.replace("X Offset:0", "X Offset:6578176").replace("Y Offset:0", "Y Offset:657818");
        let st = parse(&text).unwrap();
        let r = st.sheets[0].boxes[1].rect;
        for (g, w) in r.iter().zip([200.0, 200.0, 500.0, 210.0]) {
            assert!((g - w).abs() < 0.01, "{r:?}");
        }
    }

    #[test]
    fn not_synctex() {
        assert!(matches!(parse("%PDF-1.7\n"), Err(Error::NotSyncTex)));
    }

    #[test]
    fn bad_lines_are_counted() {
        let text = FIXTURE.replace("x1,13:", "(1,zz:6578176,13156352\nx1,13:");
        let st = parse(&text).unwrap();
        // The unclosed bad opener also swallows the point after it.
        assert_eq!(st.skipped(), 2);
        assert_eq!(st.sheets(), 2);
    }

    #[test]
    fn truncated_file_keeps_whole_sheets() {
        let cut = FIXTURE.find("(1,40").unwrap();
        let st = parse(&FIXTURE[..cut]).unwrap();
        assert!(st.sheets() >= 1);
        assert_eq!(st.sheets(), 1);
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let mut text = String::from("SyncTeX Version:1\nContent:\n{1\n");
        for _ in 0..(MAX_DEPTH + 10) {
            text.push_str("(1,1:0,0:10,10,0\n");
        }
        for _ in 0..(MAX_DEPTH + 10) {
            text.push_str(")\n");
        }
        text.push_str("}1\n");
        let st = parse(&text).unwrap();
        assert_eq!(st.sheets(), 1);
        assert_eq!(st.sheets[0].boxes.len(), MAX_DEPTH);
        assert_eq!(st.skipped(), 10);
    }

    #[test]
    fn negative_width_is_normalised() {
        let text = "SyncTeX Version:1\nContent:\n{1\n(1,1:6578176,6578176:-6578176,0,0\n)\n}1\n";
        let st = parse(text).unwrap();
        let r = st.sheets[0].boxes[0].rect;
        assert!(r[0] <= r[2] && r[1] <= r[3]);
    }

    /// Both lookups must return, whatever the text says.
    fn survives(text: &str) {
        if let Ok(st) = parse(text) {
            let _ = st.inverse(1, 120.0, 195.0);
            let _ = st.inverse(st.sheets().saturating_add(1), 0.0, 0.0);
            let _ = st.forward(Path::new("/work/main.tex"), 12);
            let _ = st.forward(Path::new("/work/main.tex"), u32::MAX);
        }
    }

    #[test]
    fn a_hundred_thousand_open_boxes_do_not_crash() {
        let mut text = String::from("SyncTeX Version:1\nInput:1:/work/main.tex\nContent:\n{1\n");
        for _ in 0..100_000 {
            text.push_str("(1,1:0,0:10,10,0\n");
        }
        survives(&text);
    }

    #[test]
    fn hostile_numbers_do_not_crash() {
        for c in ["99999999999999999999", "-9223372036854775808", "9223372036854775807"] {
            survives(&format!("SyncTeX Version:1\nInput:1:/work/main.tex\nContent:\n{{1\n(1,1:{c},{c}:{c},{c},{c}\nx1,1:{c},{c}\n)\n}}1\n"));
        }
        for h in ["Unit:0", "Magnification:0", "Unit:-1", "Unit:1e308\nMagnification:1e308", "X Offset:1e308"] {
            survives(&FIXTURE.replace("Unit:1\n", &format!("{h}\n")));
        }
    }

    #[test]
    fn hostile_structure_does_not_crash() {
        survives("SyncTeX Version:1\nContent:\n{4294967296\n}\n");
        survives("SyncTeX Version:1\nInput:1:/work/main.tex\nContent:\n{0\n(1,1:0,0:1,1,0\n)\n}0\n");
        survives("SyncTeX Version:1\nContent:\n}\n)\n]\n}1\n");
        survives(&FIXTURE.replace("Input:1:/work/main.tex", "Input:1:"));
        survives(&FIXTURE.replace("Input:2:./chap/one.tex", "Input:1:/other.tex"));
        let st = parse("SyncTeX Version:1\nContent:\n}\n)\n").unwrap();
        assert_eq!(st.sheets(), 0);
        assert!(st.skipped() >= 2);
    }

    #[test]
    fn an_empty_input_path_is_kept_without_crashing() {
        let st = parse(&FIXTURE.replace("Input:1:/work/main.tex", "Input:1:")).unwrap();
        assert_eq!(st.inputs().count(), 2);
    }

    #[test]
    fn a_page_past_the_last_sheet_is_nothing() {
        let cut = FIXTURE.find("(1,40").unwrap();
        let st = parse(&FIXTURE[..cut]).unwrap();
        assert_eq!(st.sheets(), 1);
        assert!(st.inverse(st.sheets() + 1, 120.0, 195.0).is_none());
    }

    #[test]
    fn a_skipped_box_keeps_its_closer() {
        let text = FIXTURE.replace("(1,12:6578176,13156352:19734528,657818,0", "(1,zz:6578176,13156352:19734528,657818,0");
        let st = parse(&text).unwrap();
        let p = st.inverse(1, 120.0, 295.0).unwrap();
        assert_eq!((p.file.as_path(), p.line), (Path::new("/work/chap/one.tex"), 5));
        assert!(st.skipped() >= 1);
        // The box and the two points inside it are lost; its closer is not a stray.
        assert_eq!(st.skipped(), 3);
    }

    #[test]
    fn a_point_after_a_skipped_box_stays_in_its_parent() {
        let text = "SyncTeX Version:1\nInput:1:/work/a.tex\nContent:\n{1\n(1,7:6578176,13156352:19734528,657818,0\n(1,zz:0,0:0,0,0\n)\nx1,9:6578176,13156352\n)\n}1\n";
        let st = parse(text).unwrap();
        let p = st.inverse(1, 120.0, 195.0).unwrap();
        assert_eq!(p.line, 9);
    }

    #[test]
    fn a_non_finite_box_keeps_its_closer() {
        let text = FIXTURE.replace("(1,12:6578176,13156352:19734528,657818,0", "(1,12:6578176,13156352:1e999,657818,0");
        let st = parse(&text).unwrap();
        assert_eq!(st.skipped(), 3);
        let p = st.inverse(1, 120.0, 295.0).unwrap();
        assert_eq!((p.file.as_path(), p.line), (Path::new("/work/chap/one.tex"), 5));
    }

    #[test]
    fn inputs_declared_in_the_content_are_kept() {
        let text = FIXTURE.replace("Input:2:./chap/one.tex\n", "").replace("(2,5:", "Input:2:./chap/one.tex\n(2,5:");
        let st = parse(&text).unwrap();
        assert_eq!(st.inputs().count(), 2);
        let p = st.inverse(1, 120.0, 295.0).unwrap();
        assert_eq!((p.file.as_path(), p.line), (Path::new("/work/chap/one.tex"), 5));
    }
}
