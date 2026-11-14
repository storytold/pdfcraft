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
