//! Add-in tags (`p:custDataLst` → `ppt/tags/tagN.xml`) on the presentation, slides and shapes
//! survive a save and reopen.

use std::io::{Cursor, Read};
use std::sync::Arc;

use deckcraft_engine::Session;

fn t(n: &str, v: &str) -> (String, String) {
    (n.to_string(), v.to_string())
}

#[test]
fn tags_survive_round_trip() {
    let mut s = Session::new();
    deckcraft_engine::sample::open_sample(&mut s).expect("sample");
    let mut p = s.doc().expect("doc").doc.as_ref().clone();
    p.tags = vec![t("THINKCELLPRESENTATIONDONOTDELETE", "<xml & stuff>")];
    let slide = Arc::make_mut(&mut p.slides[0]);
    slide.tags = vec![t("TIMING", "5"), t("SECTION", "Intro")];
    let shape_id = slide.shapes[0].id;
    slide.shapes[0].tags = vec![t("THINKCELLSHAPEDONOTDELETE", "t!abc")];

    let bytes = deckcraft_pptx::export(&p).expect("export");
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.clone())).expect("zip");
    let mut ct = String::new();
    zip.by_name("[Content_Types].xml").expect("ct").read_to_string(&mut ct).expect("read");
    assert!(ct.contains("presentationml.tags+xml"));
    assert_eq!((0..zip.len()).filter(|i| zip.by_index(*i).is_ok_and(|f| f.name().starts_with("ppt/tags/"))).count(), 3);

    let back = deckcraft_pptx::import(&bytes).expect("import");
    assert_eq!(back.tags, p.tags);
    assert_eq!(back.slides[0].tags, p.slides[0].tags);
    let first = &back.slides[0].shapes[0];
    assert_eq!(first.name, p.slides[0].shapes.iter().find(|s| s.id == shape_id).map(|s| s.name.clone()).unwrap_or_default());
    assert_eq!(first.tags, vec![t("THINKCELLSHAPEDONOTDELETE", "t!abc")]);
    assert!(back.slides.iter().skip(1).all(|s| s.tags.is_empty()));
}

/// #1164: the same tags part listed over and over is read once, and the tags one import copies
/// into the model are capped however often a shape lists them.
#[test]
fn a_tags_part_listed_many_times_is_copied_once_and_capped() {
    let mut s = Session::new();
    deckcraft_engine::sample::open_sample(&mut s).expect("sample");
    for _ in 0..6 {
        s.execute("shape.insert", &serde_json::json!({"preset": "rect", "rect": [10, 10, 50, 50]})).expect("shape");
    }
    let mut p = s.doc().expect("doc").doc.as_ref().clone();
    let slide = Arc::make_mut(&mut p.slides[0]);
    // A quarter of the budget with its name and pair cost: the fifth shape listing it goes over.
    let pair = std::mem::size_of::<(String, String)>();
    slide.shapes[0].tags = vec![t("BIG", &"x".repeat(deckcraft_pptx::MAX_TAG_BYTES / 4 - 3 - pair))];
    let bytes = deckcraft_pptx::export(&p).expect("export");

    // Every shape on the first slide lists that one tags part 100 times.
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut xml = String::new();
    zip.by_name("ppt/slides/slide1.xml").expect("slide").read_to_string(&mut xml).expect("read");
    let start = xml.find("<p:custDataLst>").expect("tags listed");
    let end = xml[start..].find("</p:custDataLst>").expect("list ends") + start + "</p:custDataLst>".len();
    let list = &xml[start..end];
    let inner = list.trim_start_matches("<p:custDataLst>").trim_end_matches("</p:custDataLst>");
    let many = format!("<p:custDataLst>{}</p:custDataLst>", inner.repeat(100));
    let xml = xml.replace(list, "").replace("<p:nvPr/>", "<p:nvPr></p:nvPr>").replace("<p:nvPr>", &format!("<p:nvPr>{many}"));
    assert!(xml.matches(&many).count() >= 5, "enough shapes to go over the budget");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).expect("entry");
        let name = f.name().to_string();
        let mut data = vec![];
        f.read_to_end(&mut data).expect("read");
        out.start_file(&name, zip::write::SimpleFileOptions::default()).expect("start");
        std::io::Write::write_all(&mut out, if name == "ppt/slides/slide1.xml" { xml.as_bytes() } else { &data }).expect("write");
    }
    let bytes = out.finish().expect("finish").into_inner();

    let back = deckcraft_pptx::import(&bytes).expect("import");
    let tagged: Vec<_> = back.slides[0].shapes.iter().filter(|x| !x.tags.is_empty()).collect();
    assert_eq!(tagged.len(), 4, "four copies fit the budget");
    assert!(tagged.iter().all(|x| x.tags.len() == 1), "each shape keeps the part's tag once, not 100 times");
}

/// #1164: empty tags cost something too, or a part of 10,000 empty pairs listed by every shape
/// would be copied without ever reaching the budget.
#[test]
fn empty_tags_count_against_the_budget() {
    let pair = std::mem::size_of::<(String, String)>();
    let per_shape = 10_000 * pair;
    let shapes = deckcraft_pptx::MAX_TAG_BYTES / per_shape + 2;
    let mut s = Session::new();
    deckcraft_engine::sample::open_sample(&mut s).expect("sample");
    let mut p = s.doc().expect("doc").doc.as_ref().clone();
    let slide = Arc::make_mut(&mut p.slides[0]);
    let template = slide.shapes[0].clone();
    slide.shapes.clear();
    for i in 0..shapes {
        let mut sh = template.clone();
        sh.id = deckcraft_model::ShapeId(10_000 + i as u32);
        sh.ph = None;
        sh.tags = vec![t("", ""); 10_000];
        slide.shapes.push(sh);
    }
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let back = deckcraft_pptx::import(&bytes).expect("import");
    let pairs: usize = back.slides[0].shapes.iter().map(|x| x.tags.len()).sum();
    assert!(pairs * pair <= deckcraft_pptx::MAX_TAG_BYTES, "{pairs} pairs copied");
    assert!(pairs < shapes * 10_000, "the budget stopped the copying");
}

/// #1164: once the budget is exhausted, further tags parts are not parsed or cached into
/// `tags_by_part`.
#[test]
fn new_tag_parts_are_not_parsed_or_cached_after_the_budget_is_exhausted() {
    let mut s = Session::new();
    deckcraft_engine::sample::open_sample(&mut s).expect("sample");
    s.execute("shape.insert", &serde_json::json!({"preset": "rect", "rect": [10, 10, 50, 50]})).expect("shape");
    let mut p = s.doc().expect("doc").doc.as_ref().clone();
    let slide = Arc::make_mut(&mut p.slides[0]);
    assert!(slide.shapes.len() >= 2);
    // The first shape's tag fits the budget, and a second pair pushes it over the cap.
    let pair = std::mem::size_of::<(String, String)>();
    slide.shapes[0].tags = vec![t("BIG", &"x".repeat(deckcraft_pptx::MAX_TAG_BYTES - 3 - pair)), t("OVER", "1")];
    // The second shape has its own separate tag part.
    slide.shapes[1].tags = vec![t("SHAPE2_TAG", "dropped")];
    let bytes = deckcraft_pptx::export(&p).expect("export");

    // Corrupt the second shape's tag part (`tag2.xml`) with invalid XML in the zip.
    // If it were parsed, XML loading would fail. Because the budget was capped, it is never read.
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).expect("entry");
        let name = f.name().to_string();
        let mut data = vec![];
        f.read_to_end(&mut data).expect("read");
        out.start_file(&name, zip::write::SimpleFileOptions::default()).expect("start");
        let payload = if name == "ppt/tags/tag2.xml" { b"<not valid xml<<<<" as &[u8] } else { &data };
        std::io::Write::write_all(&mut out, payload).expect("write");
    }
    let corrupted = out.finish().expect("finish").into_inner();

    let back = deckcraft_pptx::import(&corrupted).expect("import succeeds because tag2.xml is not parsed");
    assert!(!back.slides[0].shapes[0].tags.is_empty(), "first shape tag was loaded");
    assert!(back.slides[0].shapes[1].tags.is_empty(), "second shape tag was dropped and never parsed");
}
