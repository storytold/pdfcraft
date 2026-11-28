//! The request a font resolver gets for a CIDFont the document doesn't embed carries the style
//! of the descendant's font descriptor (ISO 32000-2 §9.7.4.1, §9.7.6.1, §9.8). Contributor-original
//! synthetic fixtures.
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_interpret::font::{FallbackFontQuery, FontQuery};
use hayro::hayro_interpret::hayro_cmap::CidFamily;
use hayro::hayro_syntax::Pdf;
use hayro::{RenderCache, RenderSettings, render};
use std::sync::{Arc, Mutex};

/// A one-page document drawing two characters in a Type 0 font whose descendant is a
/// non-embedded CIDFontType0 with the given entries after `/CIDSystemInfo`. The name says
/// nothing about the style and matches no standard font.
fn fixture(descendant_extra: &str, descriptor: Option<&str>) -> Vec<u8> {
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        stream("", "BT /F1 24 Tf 10 40 Td <11b30ed4> Tj ET"),
        "<< /Type /Font /Subtype /Type0 /BaseFont /CraftCJK-W3 /Encoding /Identity-H /DescendantFonts [6 0 R] >>".into(),
        format!(
            "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /CraftCJK-W3 \
             /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 2 >> {descendant_extra} >>"
        ),
    ];
    if let Some(descriptor) = descriptor {
        objects.push(descriptor.to_string());
    }
    pdf(&objects)
}

/// The fallback queries the resolver received while drawing the page.
fn fallback_queries(bytes: Vec<u8>) -> Vec<FallbackFontQuery> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let settings = InterpreterSettings {
        font_resolver: Arc::new(move |query| {
            if let FontQuery::Fallback(f) = query {
                sink.lock().unwrap().push(f.clone());
            }
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

#[test]
fn a_cid_font_fallback_request_carries_its_descendants_style() {
    // Flags: Serif (bit 2), Nonsymbolic (bit 6), Italic (bit 7) = 2 + 32 + 64.
    let descriptor = "<< /Type /FontDescriptor /FontName /CraftCJK-W3 /Flags 98 /FontWeight 700 \
                      /FontBBox [0 -120 1000 880] /ItalicAngle -12 /Ascent 880 /Descent -120 /CapHeight 700 /StemV 120 >>";
    let queries = fallback_queries(fixture("/FontDescriptor 7 0 R", Some(descriptor)));
    let query = queries.first().expect("the resolver is asked for a fallback");
    assert_eq!(query.post_script_name.as_deref(), Some("CraftCJK-W3"));
    assert!(query.character_collection.as_ref().is_some_and(|cc| cc.family == CidFamily::AdobeGB1), "{query:?}");
    assert!(query.is_serif, "Serif flag of the descendant's descriptor: {query:?}");
    assert!(query.is_italic, "Italic flag of the descendant's descriptor: {query:?}");
    assert_eq!(query.font_weight, 700, "/FontWeight of the descendant's descriptor: {query:?}");
}

#[test]
fn a_cid_font_without_a_descriptor_asks_for_the_default_style() {
    let queries = fallback_queries(fixture("", None));
    let query = queries.first().expect("the resolver is asked for a fallback");
    assert_eq!(query.post_script_name.as_deref(), Some("CraftCJK-W3"));
    assert!(!query.is_serif && !query.is_italic && !query.is_bold, "{query:?}");
    assert_eq!(query.font_weight, 400);
}

/// A page showing `A` (GBK-EUC-H code 0x41, Adobe-GB1 CID 34) at 40 pt in a non-embedded Type 0
/// font named `name` of the collection `ordering`, without a font descriptor. The Identity ordering
/// takes the Identity-H encoding, since a predefined CMap's own collection wins over
/// `/CIDSystemInfo`.
fn named(name: &str, ordering: &str) -> Vec<u8> {
    let encoding = if ordering == "Identity" { "Identity-H" } else { "GBK-EUC-H" };
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 80 60] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        stream("", "BT /F1 40 Tf 10 15 Td (A) Tj ET"),
        format!("<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /{encoding} /DescendantFonts [6 0 R] >>"),
        format!(
            "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /{name} /CIDSystemInfo << /Registry (Adobe) /Ordering ({ordering}) /Supplement 2 >> >>"
        ),
    ])
}

/// Every query the resolver received while drawing the page: `Some` for a fallback, `None` for a
/// standard-font query.
fn queries(bytes: Vec<u8>) -> Vec<Option<FallbackFontQuery>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let standard = InterpreterSettings::default().font_resolver;
    let settings = InterpreterSettings {
        font_resolver: Arc::new(move |query| {
            sink.lock().unwrap().push(match query {
                FontQuery::Fallback(f) => Some(f.clone()),
                FontQuery::Standard(_) => None,
            });
            standard(query)
        }),
        ..InterpreterSettings::default()
    };
    let pdf = Pdf::new(bytes).unwrap();
    let pages = pdf.pages();
    let cache = RenderCache::new();
    render(pages.first().unwrap(), &cache, &settings, &RenderSettings::default());
    seen.lock().unwrap().clone()
}

/// hayro-interpret patch 72: a CJK font whose name only hints at a standard family
/// (`MonotypeSung-Light` holds "mono") is asked for as a fallback carrying the hint, so a
/// resolver with a face for its script stands it in. Before, hayro chose Courier itself and the
/// resolver was asked only for Courier. A name that names the family outright (Arial, Times,
/// Courier), or a font of no CJK collection, still asks for the standard font.
#[test]
fn a_cjk_font_whose_name_hints_at_a_standard_family_asks_for_a_fallback() {
    let hint = |q: &[Option<FallbackFontQuery>]| match q.first() {
        Some(Some(f)) => Some(f.standard_hint.map(|s| format!("{s:?}"))),
        Some(None) => None,
        None => panic!("the resolver is asked"),
    };
    let asked = queries(named("MonotypeSung-Light", "GB1"));
    assert_eq!(hint(&asked), Some(Some("Courier".to_owned())), "a fallback hinted at Courier: {asked:?}");
    assert!(asked.first().and_then(Option::as_ref).and_then(|q| q.character_collection.as_ref()).is_some_and(|c| c.family == CidFamily::AdobeGB1));
    for (name, ordering) in [("MingSerif-Regular", "Korea1"), ("SomeSans-Medium", "CNS1"), ("MonoHei", "KR")] {
        assert!(matches!(hint(&queries(named(name, ordering))), Some(Some(_))), "{name} ({ordering})");
    }
    for (name, ordering) in [("ArialUnicodeMS", "GB1"), ("Times-Roman", "GB1"), ("CourierCJK", "Japan1"), ("MonotypeSung-Light", "Identity")] {
        assert_eq!(hint(&queries(named(name, ordering))), None, "{name} ({ordering}): a standard query");
    }
}

/// Without a face for the script, the hint stands: the default resolver draws the same Courier
/// a standard query for it would.
#[test]
fn a_hinted_cjk_font_without_a_stand_in_keeps_the_hinted_font() {
    let draw = |bytes: Vec<u8>| {
        let pdf = Pdf::new(bytes).unwrap();
        let pages = pdf.pages();
        let cache = RenderCache::new();
        render(pages.first().unwrap(), &cache, &InterpreterSettings::default(), &RenderSettings::default()).data_as_u8_slice().to_vec()
    };
    let hinted = draw(named("MonotypeSung-Light", "GB1"));
    assert!(hinted.as_chunks::<4>().0.iter().any(|p| p[3] >= 128), "A is drawn");
    assert!(hinted == draw(named("Courier", "GB1")), "the hinted Courier, as before");
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

/// A page showing the two-byte code `code` (hex, Identity-H) at 40 pt in a non-embedded Type 0
/// font of the collection `Adobe-<ordering>`, without `/ToUnicode` or a font descriptor.
fn identity_coded(ordering: &str, code: &str) -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 80 60] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        stream("", &format!("BT /F1 40 Tf 10 15 Td <{code}> Tj ET")),
        "<< /Type /Font /Subtype /Type0 /BaseFont /CraftKR-W3 /Encoding /Identity-H /DescendantFonts [6 0 R] >>".into(),
        format!(
            "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /CraftKR-W3 /CIDSystemInfo << /Registry (Adobe) /Ordering ({ordering}) /Supplement 0 >> >>"
        ),
    ])
}

/// An Adobe-KR font without `/ToUnicode` has no Unicode to look a character up by, and Adobe-KR
/// has no predefined UCS2 CMap, so its CIDs counted as no Adobe collection's: the substitute drew
/// its own glyph at the CID's index, an unrelated glyph. It draws `.notdef`, as the other Adobe
/// CJK collections do. Adobe-Identity, whose CIDs a substitute may share, still draws by index.
#[test]
fn an_adobe_kr_cid_without_unicode_is_not_drawn_as_the_substitutes_glyph() {
    let draw = |bytes: Vec<u8>| {
        let pdf = Pdf::new(bytes).unwrap();
        let pages = pdf.pages();
        let cache = RenderCache::new();
        render(pages.first().unwrap(), &cache, &InterpreterSettings::default(), &RenderSettings::default()).data_as_u8_slice().to_vec()
    };
    let inked = |rgba: &[u8]| rgba.as_chunks::<4>().0.iter().filter(|p| p[3] >= 128).count();
    let identity = draw(identity_coded("Identity", "0024"));
    assert!(
        identity != draw(identity_coded("Identity", "0000")),
        "control: Adobe-Identity draws the substitute's glyph 36 ({} inked)",
        inked(&identity)
    );
    let kr = draw(identity_coded("KR", "0024"));
    assert!(kr == draw(identity_coded("KR", "0000")), "Adobe-KR CID 36 draws .notdef, not the substitute's glyph 36 ({} inked)", inked(&kr));
}
