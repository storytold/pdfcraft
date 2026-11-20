//! Memory regression for the colour space lookup of `cs` and `CS`: what it keeps for a run is what
//! names give, not the `/ColorSpace` objects it reads, however many different ones a page's forms
//! name. A test binary of its own, so that the one test is all the process runs while its peak is
//! measured.

use std::collections::BTreeMap;
use std::sync::Arc;

use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style, add_annotation, rect_quad};
use pdfcraft_cos::Document;
use pdfcraft_redact::apply;

/// Where an object is: at an offset of the file, or a member of an object stream.
enum At {
    File(usize),
    Member(u32, u16),
}

/// A file written object by object, its objects listed by a cross-reference stream, which can
/// place them in object streams.
struct File {
    out: Vec<u8>,
    xref: BTreeMap<u32, At>,
}

impl File {
    fn new() -> Self {
        File { out: b"%PDF-1.7\n".to_vec(), xref: BTreeMap::new() }
    }

    fn obj(&mut self, n: u32, body: &[u8]) {
        self.xref.insert(n, At::File(self.out.len()));
        self.out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        self.out.extend_from_slice(body);
        self.out.extend_from_slice(b"\nendobj\n");
    }

    fn stream(&mut self, n: u32, dict: &str, data: &[u8]) {
        let mut body = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        self.obj(n, &body);
    }

    /// Object `member`, `body`, as the one member of object stream `n`, compressed.
    fn in_object_stream(&mut self, n: u32, member: u32, body: &[u8]) {
        let mut data = format!("{member} 0 ").into_bytes();
        let first = data.len();
        data.extend_from_slice(body);
        let compressed = pdfcraft_filters::encode_flate(&data);
        self.stream(n, &format!("/Type /ObjStm /N 1 /First {first} /Filter /FlateDecode"), &compressed);
        self.xref.insert(member, At::Member(n, 0));
    }

    fn finish(mut self) -> Vec<u8> {
        let xref = self.xref.keys().max().map_or(1, |n| n + 1);
        let at = self.out.len();
        self.xref.insert(xref, At::File(at));
        let mut rows = Vec::new();
        for n in 0..=xref {
            match self.xref.get(&n) {
                None => rows.extend_from_slice(&[0, 0, 0, 0, 0, 0xff, 0xff]),
                Some(At::File(offset)) => {
                    rows.push(1);
                    rows.extend_from_slice(&u32::try_from(*offset).unwrap().to_be_bytes());
                    rows.extend_from_slice(&[0, 0]);
                }
                Some(At::Member(stream, index)) => {
                    rows.push(2);
                    rows.extend_from_slice(&stream.to_be_bytes());
                    rows.extend_from_slice(&index.to_be_bytes());
                }
            }
        }
        let size = xref + 1;
        self.stream(xref, &format!("/Type /XRef /Size {size} /W [1 4 2] /Root 1 0 R"), &rows);
        self.out.extend_from_slice(format!("startxref\n{at}\n%%EOF\n").as_bytes());
        self.out
    }
}

/// How much the process's peak resident memory grows, in MiB, while `f` runs. `None` where the
/// peak can't be reset (`/proc/self/clear_refs` is Linux's).
fn peak_growth<T>(f: impl FnOnce() -> T) -> Option<(T, u64)> {
    fn peak_kib() -> Option<u64> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        status.lines().find_map(|l| l.strip_prefix("VmHWM:"))?.split_whitespace().next()?.parse().ok()
    }
    std::fs::write("/proc/self/clear_refs", "5").ok()?;
    let before = peak_kib()?;
    let out = f();
    Some((out, peak_kib()?.saturating_sub(before) >> 10))
}

/// A page that selects a pattern, fills a box under the mark with it, and draws forms, each with a
/// `/ColorSpace` of its own that `/CSX cs` looks a name up in, each a different object in an object
/// stream of its own:
/// - four large ones: two arrays of a million numbers and two dictionaries of a million names.
///   Kept for the page as they were read, they grew Apply's peak by over half a GiB.
/// - 2,000 with one name each, of 256 KiB. Kept as names, uncounted by their length, they grew it
///   by about as much again.
///
/// Read one at a time and let go, and names kept within the allowance, it grows by under 512 MiB.
#[test]
fn the_colour_spaces_a_page_reads_are_not_kept() {
    const LARGE: u32 = 4;
    const NAMED: u32 = 2000;
    const FORMS: u32 = LARGE + NAMED;
    let mut file = File::new();
    file.obj(1, b"<< /Type /Catalog /Pages 2 0 R >>");
    file.obj(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    let xobjects: String = (0..FORMS).map(|i| format!("/F{i} {} 0 R ", 10 + i)).collect();
    file.obj(
        3,
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Pattern << /P0 5 0 R >> /XObject << {xobjects}>> >> >>").as_bytes(),
    );
    let draws: String = (0..FORMS).map(|i| format!("/F{i} Do ")).collect();
    file.stream(4, "", format!("/Pattern cs /P0 scn 20 240 100 30 re f {draws}").as_bytes());
    file.stream(
        5,
        "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 300 300] /XStep 300 /YStep 300 /Resources << >>",
        b"0 0 300 300 re f",
    );
    let array = format!("[{}]", "0 ".repeat(1 << 20));
    let names: String = (0..1 << 20).map(|i| format!("/C{i} 0 ")).collect();
    let dict = format!("<< {names}>>");
    let named = format!("<< /{} /DeviceGray >>", "N".repeat(256 << 10));
    for i in 0..FORMS {
        let (form, space) = (10 + i, 10 + FORMS + i);
        file.stream(
            form,
            &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /ColorSpace {space} 0 R >>"),
            b"/CSX cs 0 0 1 1 re f",
        );
        let body = match i {
            i if i >= LARGE => named.as_bytes(),
            i if i % 2 == 0 => array.as_bytes(),
            _ => dict.as_bytes(),
        };
        file.in_object_stream(10 + 2 * FORMS + i, space, body);
    }
    let mut doc = Document::open(Arc::new(file.finish())).unwrap();
    let shape = Shape::Redact { quads: vec![rect_quad([0.0, 200.0, 300.0, 300.0])], overlay: String::new(), look: Default::default() };
    let style = Style::default_for(&shape);
    add_annotation(&mut doc, &NewAnnotation { page: 0, shape, style, contents: String::new(), author: "Synthetic test".into() }, &Meta::default())
        .unwrap();

    // Only Linux can reset the peak. There it must be measured, or the bound below would pass
    // unchecked; elsewhere the test has nothing to measure with.
    let Some((result, grown)) = peak_growth(|| apply(&mut doc, None)) else {
        if cfg!(target_os = "linux") {
            panic!("the peak resident memory couldn't be measured");
        }
        return;
    };
    result.unwrap();
    assert!(grown < 512, "Apply grew the peak by {grown} MiB");
}
