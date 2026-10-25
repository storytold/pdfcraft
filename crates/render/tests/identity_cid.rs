//! A Type 0 font that loads, over `/Identity-H`, whose CIDFont names no Adobe character
//! collection and which the file carries no readable program for (#3529). Identity means the
//! codes number glyphs of the font named by `/BaseFont`; substitution can supply a face but not
//! that numbering, so such a code has no route to a glyph and draws `.notdef` instead of
//! whichever outline of the stand-in shares its number. Measured on macOS before the fix,
//! `<0030 0047 0050 0053>` inked 720 dark pixels on a page wide enough not to clip, and that
//! raster was pixel-identical to the same codes under `/BaseFont /Helvetica` — another face than
//! the one named, at positions that name nothing.
//!
//! The controls keep the refusal inside that boundary: `/ToUnicode` still decides which glyph a
//! mapped code draws, and an embedded program's numbering still yields four different outlines
//! rather than one refusal repeated. Each comparison runs between two documents built by the same
//! builder (same `/DW`, no `/W`, the same number of codes), so it holds at any page width or
//! subpixel phase.

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};
use std::sync::Arc;

/// A committed OFL face, used where a test needs a program the file really does carry.
const INTER: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");

/// A `/ToUnicode` CMap stream naming `mapping`, as producers write it.
fn to_unicode(mapping: &str) -> Vec<u8> {
    format!(
        "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /T def /CMapType 2 def \
         1 begincodespacerange <0000> <FFFF> endcodespacerange 1 beginbfchar {mapping} endbfchar \
         endcmap CMapName currentdict /CMap defineresource pop end end"
    )
    .into_bytes()
}

/// Parseable, and mapping nothing.
const TO_UNICODE_EMPTY: &[u8] = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /T0 def \
/CMapType 2 def 1 begincodespacerange <0000> <FFFF> endcodespacerange \
endcmap CMapName currentdict /CMap defineresource pop end end";

const IDENTITY: Option<&str> = Some("Identity");

fn one_page(mut doc: Document, page: ObjectId) -> Vec<u8> {
    let pages = doc.add_object(dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1});
    let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
    doc.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).expect("saving the fixture");
    bytes
}

fn page_of(doc: &mut Document, ops: String, font: ObjectId) -> ObjectId {
    let content = doc.add_object(Stream::new(Dictionary::new(), ops.into_bytes()));
    doc.add_object(dictionary! {
        "Type" => "Page",
        // Wide enough that a four-glyph line at 40 pt is never clipped: differences in ink are
        // then the glyphs, not the page edge.
        "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(400), Object::Integer(120)],
        "Contents" => content,
        "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}},
    })
}

/// A page showing `codes` in a Type 0 font over Identity. `ordering` writes `/CIDSystemInfo`
/// (omit it for a CIDFont naming no collection), `program` the descriptor's `/FontFile2`,
/// `to_unicode` the Type 0 font's `/ToUnicode`.
fn identity_page(ordering: Option<&str>, codes: &str, program: Option<&[u8]>, to_unicode: Option<&[u8]>) -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let descriptor_id = {
        let mut descriptor = dictionary! {
            "Type" => "FontDescriptor",
            "FontName" => "MinionPro-Regular",
            // Serif | NonSymbolic. These flags do not decide the stand-in: the fallback query
            // reads a `/FontDescriptor` on the Type0 font itself, and there is none here — which
            // is why measured output matches the Helvetica face rather than a serif one.
            "Flags" => 34,
            "FontBBox" => vec![Object::Integer(0), Object::Integer(-141), Object::Integer(996), Object::Integer(859)],
            "Ascent" => 859,
            "Descent" => -141,
        };
        if let Some(bytes) = program {
            descriptor.set("FontFile2", doc.add_object(Stream::new(Dictionary::new(), bytes.to_vec())));
        }
        doc.add_object(descriptor)
    };

    let mut cid = dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType0",
        "BaseFont" => "MinionPro-Regular",
        "FontDescriptor" => descriptor_id,
        "DW" => 1000,
    };
    if let Some(ordering) = ordering {
        // Literal strings, not names: as names this reads as carrying no collection at all, which
        // would quietly move the test onto a different branch (see `names_the_collection_in_strings`).
        let mut system = Dictionary::new();
        system.set("Registry", Object::string_literal("Adobe"));
        system.set("Ordering", Object::string_literal(ordering));
        system.set("Supplement", Object::Integer(6));
        cid.set("CIDSystemInfo", system);
    }
    let cid = doc.add_object(cid);

    let mut font = dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => "MinionPro-Regular",
        "Encoding" => "Identity-H",
        "DescendantFonts" => vec![Object::Reference(cid)],
    };
    if let Some(cmap) = to_unicode {
        font.set("ToUnicode", doc.add_object(Stream::new(Dictionary::new(), cmap.to_vec())));
    }
    let font = doc.add_object(font);

    let page = page_of(&mut doc, format!("BT /F1 40 Tf 8 60 Td <{codes}> Tj ET"), font);
    one_page(doc, page)
}

/// A simple WinAnsi page: a control that the rule stays inside Type 0.
fn win_ansi_page(base: &str, text: &str) -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let font = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => base, "Encoding" => "WinAnsiEncoding",
    });
    let page = page_of(&mut doc, format!("BT /F1 40 Tf 8 60 Td ({text}) Tj ET"), font);
    one_page(doc, page)
}

/// Dark pixels of page 1 at 1×: RGBA words with R, G and B all below 128.
fn ink(bytes: Vec<u8>) -> usize {
    let mut r = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
    let p = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, ..Default::default() });
    assert!(p.error.is_none(), "the page renders: {:?}", p.error);
    p.rgba.chunks(4).filter(|c| c[0] < 128 && c[1] < 128 && c[2] < 128).count()
}

#[test]
fn names_the_collection_in_strings() {
    // Written as names instead of strings, `/CIDSystemInfo` reads as no collection at all and the
    // refusal test below would prove nothing about Identity. Pin how it is written.
    let bytes = identity_page(IDENTITY, "0030", None, None);
    assert!(bytes.windows(b"(Adobe)".len()).any(|w| w == b"(Adobe)"), "literal strings expected, not names: {}", String::from_utf8_lossy(&bytes));
    assert!(bytes.windows(b"(Identity)".len()).any(|w| w == b"(Identity)"));
    assert!(!bytes.windows(b"/Ordering/".len()).any(|w| w == b"/Ordering/"), "an /Ordering name reads as no collection");
}

#[test]
fn identity_codes_over_a_missing_program_draw_nothing() {
    assert_eq!(ink(identity_page(IDENTITY, "", None, None)), 0, "an empty page is the baseline");
    // Before the fix this painted 720 dark pixels: IDs 48, 71, 80 and 83 of the stand-in. The
    // comparison with this fixture's own `.notdef` × 4 says why nothing appears — the codes are
    // treated exactly as unmappable CIDs — and holds whatever shape the substitute's `.notdef`
    // happens to have; the `== 0` line records what that looks like for these faces today.
    let codes = ink(identity_page(IDENTITY, "0030004700500053", None, None));
    let refused = ink(identity_page(IDENTITY, "0000000000000000", None, None));
    assert_eq!(codes, refused, "four unmapped codes draw exactly what four unmappable CIDs draw");
    assert_eq!(codes, 0);
    assert_eq!(ink(identity_page(IDENTITY, "0001", None, None)), 0, "a single code inside the substitute's range");
    assert_eq!(ink(identity_page(IDENTITY, "0500", None, None)), 0, "and one past its end");
    assert_eq!(ink(identity_page(None, "0030004700500053", None, None)), 0, "a CIDFont naming no collection at all");
}

#[test]
fn a_tounicode_map_decides_which_codes_draw() {
    // Compared within one fixture, so the only variable is which glyph the map points at: `W`
    // inks more than `R`, a code the map leaves out adds nothing, and a codespacerange-only map
    // maps nothing. Before this fix every unmapped code still drew the substitute's glyph at that
    // ID; a guard conditioned on there being no `/ToUnicode` at all failed the same way.
    let r = ink(identity_page(IDENTITY, "0030", None, Some(&to_unicode("<0030> <0052>"))));
    let w = ink(identity_page(IDENTITY, "0030", None, Some(&to_unicode("<0030> <0057>"))));
    assert!(r > 0, "a code the map names draws");
    assert!(w > r, "and it is the letter the map names ({r} dark pixels for R, {w} for W)");
    assert_eq!(ink(identity_page(IDENTITY, "0047", None, Some(&to_unicode("<0030> <0052>")))), 0, "a code the map leaves out draws nothing");
    assert_eq!(ink(identity_page(IDENTITY, "00300047", None, Some(&to_unicode("<0030> <0052>")))), r, "an unmapped code adds no ink");
    assert_eq!(ink(identity_page(IDENTITY, "0030", None, Some(TO_UNICODE_EMPTY))), 0, "a map that names nothing maps nothing");
}

#[test]
fn identity_codes_over_an_embedded_program_still_draw() {
    // With the program in the file, its own numbering is meaningful. Refusing these codes would
    // draw `.notdef` at the same four positions, so compare against exactly that — this fixture's
    // own `.notdef` × 4, identical outlines at identical advances, so the comparison does not
    // depend on page width or subpixel phase. A bare "> 0" cannot see a refusal: this face's
    // `.notdef` has outlines of its own.
    let ids = ink(identity_page(IDENTITY, "0030004700500053", Some(INTER), None));
    let refused = ink(identity_page(IDENTITY, "0000000000000000", Some(INTER), None));
    assert!(ids > 0, "the four IDs draw something ({ids} dark pixels)");
    assert_ne!(ids, refused, "and not one refusal repeated ({refused} dark pixels)");
}

#[test]
fn a_win_ansi_page_still_draws() {
    // A simple font never reaches `Type0Font::map_code`, so this only pins that the rule was not
    // written somewhere wider than Type 0. The Adobe-collection route, which must keep drawing,
    // needs a licensed face for the script and is therefore covered by the craft-fonts CI jobs.
    assert!(ink(win_ansi_page("Helvetica", "ABC")) > 0);
}
