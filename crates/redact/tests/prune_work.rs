//! Work regressions for redaction's resource cleanups: what a dictionary of many entries costs
//! them, once the content that uses them is read. Each name the content uses is found once, not by
//! reading the content again for every entry, and the entries that go are removed in one pass:
//! removing them one at a time moved the entries after each, every time. A test binary of its own,
//! so that none of the crate's other tests shares its process, and its tests run one at a time.
//! Each compares two timings of its own, the least of three runs each, with many entries and with
//! one, not a fixed time. The bound leaves room for slower machines: each regression took seconds
//! against milliseconds.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style, add_annotation, rect_quad};
use pdfcraft_cos::Document;
use pdfcraft_redact::apply;
use pdfcraft_redact::sanitize::{Hidden, remove_hidden};
use pdfcraft_testkit::{Pdf, stream_obj};

/// The entries of a dictionary: `/{prefix}0 {value} /{prefix}1 {value} …`.
fn entries(prefix: &str, n: usize, value: &str) -> String {
    (0..n).map(|i| format!("/{prefix}{i} {value} ")).collect()
}

/// One page with `/Font << /F1 Helvetica {fonts} >>`, `resources` and `extra` objects (from object
/// 5 on), drawing a word under a mark (so a redaction rewrites the page) and then `ops`.
fn page(fonts: &str, resources: &str, ops: &[u8], extra: Vec<Vec<u8>>) -> Document {
    let content = [b"BT /F1 10 Tf 20 260 Td (SECRET) Tj ET ".as_slice(), ops].concat();
    let mut objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> {fonts}>> {resources} >> >>"
        )
        .into_bytes(),
        stream_obj("", content),
    ];
    objs.extend(extra);
    let mut doc = Document::open(Arc::new(Pdf::new().objs(objs).build())).unwrap();
    let shape = Shape::Redact { quads: vec![rect_quad([0.0, 200.0, 300.0, 300.0])], overlay: String::new(), look: Default::default() };
    let style = Style::default_for(&shape);
    add_annotation(&mut doc, &NewAnnotation { page: 0, shape, style, contents: String::new(), author: "Synthetic test".into() }, &Meta::default())
        .unwrap();
    doc
}

/// The tests' lock: they run one at a time, so that none is timed while another works.
static SERIAL: Mutex<()> = Mutex::new(());

fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How long `run` takes on a document `make` builds, the least of three runs.
fn timed(make: impl Fn() -> Document, run: impl Fn(&mut Document)) -> Duration {
    (0..3)
        .map(|_| {
            let mut doc = make();
            let start = Instant::now();
            run(&mut doc);
            start.elapsed()
        })
        .min()
        .unwrap_or_default()
}

/// How long applying the marks of a document `make` builds takes.
fn redaction(make: impl Fn(usize) -> Document, n: usize) -> Duration {
    timed(|| make(n), |doc| assert!(apply(doc, None).unwrap().glyphs > 0, "the word under the mark goes"))
}

/// Whether `many` took about as long as `one`.
fn about_as_long(many: Duration, one: Duration) -> bool {
    many < one * 10 + Duration::from_secs(1)
}

const MANY: usize = 60_000;

/// A tiling pattern, a Type 3 font of one glyph shape, and a 1 by 1 image, for entries to name.
const PATTERN: &str = "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << >>";
const IMAGE: &str = "/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8";

/// Property lists the page names nowhere go, in one pass.
#[test]
fn unused_property_lists_go_in_one_pass() {
    let _serial = one_at_a_time();
    let doc = |n: usize| page("", &format!("/Properties << {}>>", entries("M", n, "<< /Lang (en) >>")), b"", vec![]);
    let (one, many) = (redaction(doc, 1), redaction(doc, MANY));
    assert!(about_as_long(many, one), "{MANY} entries {many:?}, one {one:?}");
}

/// Property lists the page names are found by reading its content once, not once for each list.
#[test]
fn property_lists_in_use_are_found_by_reading_the_content_once() {
    let _serial = one_at_a_time();
    let n = MANY;
    let ops: Vec<u8> = (0..n).map(|i| format!("/P /M{i} BDC EMC ")).collect::<String>().into_bytes();
    let doc = |entries_: usize| page("", &format!("/Properties << {}>>", entries("M", entries_, "<< /Lang (en) >>")), &ops, vec![]);
    let (one, many) = (redaction(doc, 1), redaction(doc, n));
    assert!(about_as_long(many, one), "{n} entries {many:?}, one {one:?}");
}

/// The XObjects a rewritten form lists and doesn't draw go, in one pass.
#[test]
fn a_rewritten_forms_unused_xobjects_go_in_one_pass() {
    let _serial = one_at_a_time();
    let doc = |n: usize| {
        let form = stream_obj(
            &format!(
                "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> /XObject << {}>> >>",
                entries("X", n, "6 0 R")
            ),
            b"BT /F1 10 Tf 20 240 Td (FORMSECRET) Tj ET",
        );
        page("", "/XObject << /Fm 5 0 R >>", b"/Fm Do", vec![form, stream_obj(IMAGE, b"o")])
    };
    let (one, many) = (redaction(doc, 1), redaction(doc, MANY));
    assert!(about_as_long(many, one), "{MANY} entries {many:?}, one {one:?}");
}

/// Patterns the page selects and nothing left paints with go, in one pass, however often each is
/// selected.
#[test]
fn unpainted_patterns_go_in_one_pass() {
    let _serial = one_at_a_time();
    let doc = |n: usize| {
        let ops: Vec<u8> = (0..n).map(|i| format!("/Pattern cs /P{i} scn ")).collect::<String>().into_bytes();
        page("", &format!("/Pattern << {}>>", entries("P", n, "5 0 R")), &ops, vec![stream_obj(PATTERN, b"0 0 5 5 re f")])
    };
    let (one, many) = (redaction(doc, 1), redaction(doc, MANY));
    assert!(about_as_long(many, one), "{MANY} entries {many:?}, one {one:?}");
}

/// Type 3 fonts the page lists and draws with nowhere go from its `/Font`, in one pass.
#[test]
fn undrawn_type3_fonts_go_in_one_pass() {
    let _serial = one_at_a_time();
    let doc = |n: usize| {
        let font = b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] /FontMatrix [0.1 0 0 0.1 0 0] /CharProcs << /a 6 0 R >> \
                     /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [10] >>"
            .to_vec();
        page(&entries("T", n, "5 0 R"), "", b"", vec![font, stream_obj("", b"10 0 d0 0 0 10 10 re f")])
    };
    let (one, many) = (redaction(doc, 1), redaction(doc, MANY));
    assert!(about_as_long(many, one), "{MANY} entries {many:?}, one {one:?}");
}

/// The glyphs of a Type 3 font that nothing left draws go from its `/CharProcs`, in one pass.
#[test]
fn a_type3_fonts_undrawn_glyphs_go_in_one_pass() {
    let _serial = one_at_a_time();
    let doc = |n: usize| {
        let font = format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] /FontMatrix [0.1 0 0 0.1 0 0] /CharProcs << /a 6 0 R /b 6 0 R {}>> \
             /Encoding << /Differences [97 /a /b] >> /FirstChar 97 /LastChar 98 /Widths [10 10] >>",
            entries("g", n, "6 0 R")
        );
        page(
            "/T3 5 0 R",
            "",
            b"BT /T3 10 Tf 20 260 Td (a) Tj ET BT /T3 10 Tf 20 20 Td (b) Tj ET",
            vec![font.into_bytes(), stream_obj("", b"10 0 d0 0 0 10 10 re f")],
        )
    };
    let (one, many) = (redaction(doc, 1), redaction(doc, MANY));
    assert!(about_as_long(many, one), "{MANY} glyphs {many:?}, one {one:?}");
}

/// For the tests whose regression copied a resource dictionary for each operator that named one of
/// its entries: as many entries as operators, the copies cost the square of this, seconds already.
const SQUARED: usize = 4_000;

/// A Type 3 font of one glyph shape (object 5, its glyph object 6), and content drawing it beside
/// the mark, so that the Type 3 cleanup reads the page.
const TYPE3: &str = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] /FontMatrix [0.1 0 0 0.1 0 0] /CharProcs << /a 6 0 R >> \
                     /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [10] >>";
const DRAWS_TYPE3: &str = "BT /T3 10 Tf 20 20 Td (a) Tj ET";

/// A page with the Type 3 font, `resources`, and `ops` before the text that draws it; `extra`
/// objects from object 7 on.
fn type3_page(fonts: &str, resources: &str, ops: &str, extra: Vec<Vec<u8>>) -> Document {
    let objs = [vec![TYPE3.as_bytes().to_vec(), stream_obj("", b"10 0 d0 0 0 10 10 re f")], extra].concat();
    page(&format!("/T3 5 0 R {fonts}"), resources, format!("{ops} {DRAWS_TYPE3}").as_bytes(), objs)
}

/// The graphics states the content selects are read once each, however many there are and however
/// often each is selected: by the Type 3 cleanup's scan of the glyphs drawn, and by its pruning of
/// the fonts no longer drawn. Each read the whole `/ExtGState` again for every `gs`.
#[test]
fn graphics_states_are_read_once_however_often_the_content_selects_them() {
    let _serial = one_at_a_time();
    let doc = |n: usize| {
        let ops: String = (0..n).map(|i| format!("/S{i} gs ")).collect();
        type3_page("", &format!("/ExtGState << {}>>", entries("S", n, "<< /LW 1 >>")), &ops, vec![])
    };
    let (one, many) = (redaction(doc, 1), redaction(doc, SQUARED));
    assert!(about_as_long(many, one), "{SQUARED} states {many:?}, one {one:?}");
}

/// The fonts the content selects are read once each by the Type 3 cleanup's scan: it read the whole
/// `/Font` again for every `Tf`.
#[test]
fn fonts_are_read_once_however_often_the_content_selects_them() {
    let _serial = one_at_a_time();
    let doc = |n: usize| {
        let ops: String = (0..n).map(|i| format!("BT /F{i} 10 Tf ET ")).collect();
        type3_page(&entries("F", n, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"), "", &ops, vec![])
    };
    let (one, many) = (redaction(doc, 1), redaction(doc, SQUARED));
    assert!(about_as_long(many, one), "{SQUARED} fonts {many:?}, one {one:?}");
}

/// The XObjects and patterns the content draws and selects are read once each by the Type 3
/// cleanup's scan: it read the whole `/XObject` again for every `Do`, and `/Pattern` for every `scn`.
#[test]
fn xobjects_and_patterns_are_read_once_however_often_the_content_names_them() {
    let _serial = one_at_a_time();
    let doc = |n: usize| {
        let ops: String = (0..n).map(|i| format!("/X{i} Do /Pattern cs /P{i} scn ")).collect();
        let resources = format!("/XObject << {}>> /Pattern << {}>>", entries("X", n, "7 0 R"), entries("P", n, "8 0 R"));
        type3_page("", &resources, &ops, vec![stream_obj(IMAGE, b"o"), stream_obj(PATTERN, b"0 0 5 5 re f")])
    };
    let (one, many) = (redaction(doc, 1), redaction(doc, SQUARED));
    assert!(about_as_long(many, one), "{SQUARED} XObjects and patterns {many:?}, one {one:?}");
}

/// A form the content draws again and again is read once by the Type 3 cleanup's scan, and so are
/// its own resources: they were copied for every `Do`, before the form was found already read.
#[test]
fn a_forms_own_resources_are_read_once_however_often_it_is_drawn() {
    let _serial = one_at_a_time();
    let doc = |n: usize| {
        let ops: String = (0..n).map(|_| "/Fm Do ".to_string()).collect();
        let form = stream_obj(
            &format!("/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /XObject << {}>> >>", entries("X", n, "8 0 R")),
            b"0 0 5 5 re f",
        );
        type3_page("", "/XObject << /Fm 7 0 R >>", &ops, vec![form, stream_obj(IMAGE, b"o")])
    };
    let (one, many) = (redaction(doc, 1), redaction(doc, SQUARED));
    assert!(about_as_long(many, one), "{SQUARED} draws {many:?}, one {one:?}");
}

/// Type 3 fonts that have no glyph left go from the page's `/Font` together, in one pass: one at
/// a time, each copied the whole `/Font` again.
#[test]
fn type3_fonts_with_no_glyph_left_go_together() {
    let _serial = one_at_a_time();
    let doc = |n: usize| {
        let ops: String = (0..n).map(|i| format!("BT /G{i} 10 Tf ET ")).collect();
        let fonts: String = (0..n).map(|i| format!("/G{i} {} 0 R ", 7 + i)).collect();
        let glyphless = (0..n).map(|_| TYPE3.as_bytes().to_vec()).collect();
        type3_page(&fonts, "", &ops, glyphless)
    };
    let (one, many) = (redaction(doc, 1), redaction(doc, SQUARED));
    assert!(about_as_long(many, one), "{SQUARED} fonts {many:?}, one {one:?}");
}

/// Images a page lists and draws nowhere go when hidden information is removed, in one pass.
#[test]
fn undrawn_images_go_in_one_pass() {
    let _serial = one_at_a_time();
    let doc = |n: usize| page("", &format!("/XObject << {}>>", entries("I", n, "5 0 R")), b"", vec![stream_obj(IMAGE, b"o")]);
    let removed = |n: usize| timed(|| doc(n), |doc| assert!(remove_hidden(doc, &[Hidden::Metadata]).is_ok()));
    let (one, many) = (removed(1), removed(MANY));
    assert!(about_as_long(many, one), "{MANY} entries {many:?}, one {one:?}");
}
