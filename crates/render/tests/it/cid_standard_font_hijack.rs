//! #3684: a CID font of an Adobe CJK collection is never the Latin standard 14, whatever its
//! `/BaseFont` name reads.
//!
//! hayro's `select_standard_font` matches `helvetica`, `arial`/`sans`, `courier`, `mono`,
//! `times` and `serif` as *substrings* of the `/BaseFont` name. A non-embedded Chinese CID
//! font named e.g. `NotoSansCJKsc` or `SourceHanSerifSC` therefore used to be taken for a Latin
//! standard font: it never reached the CJK fallback chain, so its glyphs drew nothing at all
//! (upstream issue #3684, "All Chinese character disappeared when open specific pdf file").
//!
//! The fix (vendored hayro-interpret, `font/cid.rs`) skips the standard-font branch whenever the
//! font belongs to one of the four Adobe CJK collections, which are exactly the families that
//! have a UCS2 CMap. The regression is checked on the *query the resolver receives* rather than
//! on pixels: the branch decides `FontQuery::Standard` versus `FontQuery::Fallback`, and that
//! decision does not depend on which faces happen to be installed on the machine running the
//! test. A pixel-level check would skip itself on every machine without an installed CJK face,
//! which is most CI machines — the reason the original test missed this case entirely.
//!
//! Contributor-original synthetic fixtures.

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_interpret::font::FontQuery;
use hayro::hayro_interpret::hayro_cmap::CidFamily;
use hayro::hayro_syntax::Pdf;
use hayro::{RenderCache, RenderSettings, render};
use std::sync::{Arc, Mutex};

/// The kind of request the font resolver received while drawing the page.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Requested {
    /// A Latin standard font: the resolver was asked for one of the 14.
    Standard,
    /// A fallback: the resolver was asked for a face to draw the font with.
    Fallback { post_script_name: Option<String>, family: Option<CidFamily> },
}

/// A one-page document drawing one character with a non-embedded Type 0 font whose descendant
/// is a CIDFont of the given Adobe collection, named `base_font`.
fn fixture(base_font: &str, ordering: &str, code: &str) -> Vec<u8> {
    let objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        stream("", &format!("BT /F1 24 Tf 10 40 Td <{code}> Tj ET")),
        format!("<< /Type /Font /Subtype /Type0 /BaseFont /{base_font} /Encoding /Identity-H /DescendantFonts [6 0 R] >>"),
        format!(
            "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /{base_font} \
             /CIDSystemInfo << /Registry (Adobe) /Ordering ({ordering}) /Supplement 2 >> \
             /FontDescriptor 7 0 R >>"
        ),
        "<< /Type /FontDescriptor /FontName /{base_font} /Flags 4 /FontBBox [0 -120 1000 880] \
         /ItalicAngle 0 /Ascent 880 /Descent -120 /CapHeight 700 /StemV 80 >>"
            .replace("{base_font}", base_font),
    ];
    pdf(&objects)
}

/// The font requests the resolver received while drawing the page.
fn requests(bytes: Vec<u8>) -> Vec<Requested> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let settings = InterpreterSettings {
        font_resolver: Arc::new(move |query| {
            let request = match query {
                FontQuery::Standard(_) => Requested::Standard,
                FontQuery::Fallback(f) => Requested::Fallback {
                    post_script_name: f.post_script_name.clone(),
                    family: f.character_collection.as_ref().map(|cc| cc.family.clone()),
                },
            };
            sink.lock().unwrap().push(request);
            // No face is returned: the test only asks which branch was taken.
            None
        }),
        ..InterpreterSettings::default()
    };
    let pdf = Pdf::new(bytes).unwrap();
    let pages = pdf.pages();
    let page = pages.first().unwrap();
    let cache = RenderCache::new();
    render(page, &cache, &settings, &RenderSettings::default());
    seen.lock().unwrap().clone()
}

/// The mechanism B regression: a CJK CID font whose name reads like a Latin standard font still
/// asks for a fallback, with the collection it belongs to, instead of asking for a standard font.
#[test]
fn a_cjk_cid_font_named_like_a_latin_standard_font_asks_for_a_fallback() {
    // (base font, collection ordering, code) — the code bytes are CIDs, not Unicode.
    let cases = [
        // `sans` is a substring of `NotoSansCJKsc`, and `sans` alone used to be enough to take
        // the font for a Helvetica: the Chinese text drew nothing at all.
        ("NotoSansCJKsc", "GB1", "D6D0CEC4"),
        // `serif` is matched as a substring too, the same way.
        ("SourceHanSerifSC", "GB1", "D6D0CEC4"),
        ("STSongSerif-Regular", "GB1", "D6D0CEC4"),
        ("SimHeiSans-Regular", "GB1", "D6D0CEC4"),
        // The other keywords `select_standard_font` matches.
        ("FakeCourierCJK", "GB1", "D6D0CEC4"),
        ("FakeMonoCJK", "GB1", "D6D0CEC4"),
        ("FakeTimesCJK", "GB1", "D6D0CEC4"),
        ("FakeArialCJK", "GB1", "D6D0CEC4"),
        ("FakeHelveticaCJK", "GB1", "D6D0CEC4"),
        // The same hijack in the other three CJK collections: a Traditional Chinese face, a
        // Japanese one and a Korean one, each named like a Latin standard font.
        ("NotoSansCJKtc", "CNS1", "7E419AD45B57"),
        ("SourceHanSerifJP", "Japan1", "11B30ED4"),
        ("NotoSansCJKkr", "Korea1", "D55CAE00"),
    ];
    for (base_font, ordering, code) in cases {
        let seen = requests(fixture(base_font, ordering, code));
        let request = seen.first().expect("the resolver is asked for a font");
        assert_ne!(request, &Requested::Standard, "{base_font}: a CJK CID font must not be taken for a Latin standard font (#3684)");
        let Requested::Fallback { post_script_name, family } = request else {
            panic!("{base_font}: expected a fallback request, got {request:?}");
        };
        assert_eq!(post_script_name.as_deref(), Some(base_font), "{base_font}: the name reaches the resolver");
        let expected = match ordering {
            "GB1" => CidFamily::AdobeGB1,
            "CNS1" => CidFamily::AdobeCNS1,
            "Japan1" => CidFamily::AdobeJapan1,
            "Korea1" => CidFamily::AdobeKorea1,
            other => panic!("{other}: unknown ordering"),
        };
        assert_eq!(family.as_ref(), Some(&expected), "{base_font}: the collection reaches the resolver");
    }
}

/// The other side of the fix: a CID font whose name matches no standard font keeps asking for
/// a fallback, and one that is *not* of a CJK collection still takes the standard-font branch
/// when its name matches one. Without the second half the fix could simply have stopped
/// recognising standard fonts altogether.
#[test]
fn a_non_cjk_font_named_like_a_latin_standard_font_still_asks_for_the_standard_font() {
    // Identity is not one of the four CJK collections, so the standard-font branch is kept.
    let seen = requests(fixture("FakeSans", "Identity", "0041"));
    assert_eq!(seen.first(), Some(&Requested::Standard), "a non-CJK font keeps its standard-font stand-in");

    // A CID font of a CJK collection with a name that matches no standard font: the fallback
    // branch, as before the fix.
    let seen = requests(fixture("CraftCJK-W3", "GB1", "D6D0CEC4"));
    assert!(matches!(seen.first(), Some(Requested::Fallback { .. })), "a CJK font without a standard-font name asks for a fallback: {seen:?}");
}

/// A CID font of a *custom* collection is not one of the four Adobe CJK collections, so it
/// keeps the standard-font branch when its name matches one: the fix must not change it.
#[test]
fn a_custom_collection_font_named_like_a_latin_standard_font_asks_for_the_standard_font() {
    let objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        stream("", "BT /F1 24 Tf 10 40 Td <0041> Tj ET"),
        "<< /Type /Font /Subtype /Type0 /BaseFont /FakeSans /Encoding /Identity-H /DescendantFonts [6 0 R] >>".into(),
        "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /FakeSans \
         /CIDSystemInfo << /Registry (Custom) /Ordering (Custom) /Supplement 0 >> /FontDescriptor 7 0 R >>"
            .into(),
        "<< /Type /FontDescriptor /FontName /FakeSans /Flags 4 /FontBBox [0 -120 1000 880] \
         /ItalicAngle 0 /Ascent 880 /Descent -120 /CapHeight 700 /StemV 80 >>"
            .into(),
    ];
    let seen = requests(pdf(&objects));
    assert_eq!(seen.first(), Some(&Requested::Standard), "a custom collection keeps the standard-font branch");
}

fn pdf(objects: &[String]) -> Vec<u8> {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

fn stream(dict: &str, body: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{body}\nendstream", body.len())
}
