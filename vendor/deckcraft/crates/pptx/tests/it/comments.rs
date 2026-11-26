//! Comment threads through PPTX: the modern part keeps replies and resolved state, the legacy
//! part stays readable, and a package with both imports every thread once.

use std::io::Read;
use std::sync::Arc;

use deckcraft_model::{Comment, Presentation, defaults};

fn deck() -> Presentation {
    let mut p = defaults::new_presentation(None);
    let s = Arc::make_mut(&mut p.slides[0]);
    s.comments = vec![
        Comment {
            author: "Ada".into(),
            initials: "AL".into(),
            text: "Top thread\nsecond line".into(),
            date: "2025-03-04T05:06:07Z".into(),
            x: 90.0,
            y: 45.0,
            resolved: true,
            replies: vec![
                Comment { author: "Bob".into(), text: "A <reply> & more".into(), date: "2025-03-04T06:00:00Z".into(), ..Default::default() },
                Comment {
                    author: "Ada".into(),
                    text: "Another reply".into(),
                    date: "2025-03-04T06:30:00Z".into(),
                    resolved: true,
                    ..Default::default()
                },
            ],
            shape: None,
        },
        Comment {
            author: "Bob".into(),
            initials: "BC".into(),
            text: "Open thread".into(),
            date: "2025-03-05T01:02:03Z".into(),
            x: 120.0,
            y: 60.0,
            resolved: false,
            replies: vec![],
            shape: None,
        },
    ];
    p
}

fn part(bytes: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
    let mut out = String::new();
    z.by_name(name).expect("part").read_to_string(&mut out).expect("utf8");
    out
}

/// Rewrite the package, dropping or replacing entries by name.
fn rewrite(bytes: &[u8], drop: &[&str]) -> Vec<u8> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for i in 0..z.len() {
        let mut f = z.by_index(i).expect("entry");
        if drop.iter().any(|d| f.name() == *d) {
            continue;
        }
        out.start_file(f.name(), zip::write::SimpleFileOptions::default()).expect("start");
        std::io::copy(&mut f, &mut out).expect("copy");
    }
    out.finish().expect("finish").into_inner()
}

#[test]
fn threads_and_resolved_round_trip() {
    let p = deck();
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let q = deckcraft_pptx::import(&bytes).expect("import");
    let (a, b) = (&p.slides[0].comments, &q.slides[0].comments);
    assert_eq!(b.len(), a.len(), "comments duplicated: {b:?}");
    assert_eq!(b[0].author, "Ada");
    assert_eq!(b[0].initials, "AL");
    assert_eq!(b[0].text, "Top thread\nsecond line");
    assert_eq!(b[0].date, "2025-03-04T05:06:07Z");
    assert!(b[0].resolved);
    assert_eq!((b[0].x, b[0].y), (90.0, 45.0));
    assert_eq!(b[0].replies.len(), 2);
    assert_eq!(b[0].replies[0].author, "Bob");
    assert_eq!(b[0].replies[0].text, "A <reply> & more");
    assert_eq!(b[0].replies[0].date, "2025-03-04T06:00:00Z");
    assert_eq!(b[0].replies[1].text, "Another reply");
    assert!(!b[0].replies[0].resolved);
    assert!(b[0].replies[1].resolved);
    assert_eq!(b[1].author, "Bob");
    assert!(!b[1].resolved);
    assert_eq!(b[1].text, "Open thread");
}

#[test]
fn package_carries_both_parts_and_the_slide_anchor() {
    let bytes = deckcraft_pptx::export(&deck()).expect("export");
    let modern = part(&bytes, "ppt/comments/modernComment1.xml");
    assert!(modern.contains("p188:cmLst"), "{modern}");
    // The thread and the second reply are resolved; the first reply is not.
    assert_eq!(modern.matches(r#"status="resolved""#).count(), 2, "{modern}");
    assert!(modern.contains("p188:replyLst") && modern.contains("A &lt;reply&gt; &amp; more"), "{modern}");
    assert!(modern.contains(r#"<a:t>second line</a:t>"#), "{modern}");
    let legacy = part(&bytes, "ppt/comments/comment1.xml");
    assert!(legacy.contains("<p:cm ") && legacy.contains("Top thread"), "{legacy}");
    let authors = part(&bytes, "ppt/authors.xml");
    assert!(authors.contains("p188:authorLst") && authors.contains("Ada"), "{authors}");
    assert!(part(&bytes, "ppt/commentAuthors.xml").contains("p:cmAuthorLst"));
    let rels = part(&bytes, "ppt/slides/_rels/slide1.xml.rels");
    assert!(rels.contains("2018/10/relationships/comments"), "{rels}");
    // The slide's commentRel anchor names the relationship that points at THIS modern part —
    // the wiring PowerPoint needs and DeckCraft's own reader does not check.
    let slide = part(&bytes, "ppt/slides/slide1.xml");
    assert!(slide.contains("commentRel") && slide.contains("{6950BFC3-D8DA-4A85-94F7-54DA5524770B}"), "{slide}");
    let anchor_at = slide.find("commentRel").expect("anchor");
    let rid = slide[anchor_at..].split(r#"r:id=""#).nth(1).and_then(|s| s.split('"').next()).expect("anchor r:id");
    let rel = rels.split("<Relationship ").find(|r| r.contains(&format!(r#"Id="{rid}""#))).expect("anchor rel");
    assert!(rel.contains("2018/10/relationships/comments") && rel.contains("../comments/modernComment1.xml"), "{rel}");
    let ct = part(&bytes, "[Content_Types].xml");
    assert!(ct.contains("application/vnd.ms-powerpoint.comments+xml") && ct.contains("application/vnd.ms-powerpoint.authors+xml"), "{ct}");
}

/// Comments on several slides: every comment slide gets its own numbered pair of parts and its
/// own anchor; comment-free slides get none.
#[test]
fn comments_on_several_slides_round_trip() {
    let mut p = deck();
    let layout = p.slides[0].layout;
    while p.slides.len() < 3 {
        let s = defaults::new_slide(&mut p, layout).expect("slide");
        p.slides.push(Arc::new(s));
    }
    let s = Arc::make_mut(&mut p.slides[2]);
    s.comments = vec![Comment { author: "Cara".into(), text: "Slide three".into(), date: "2025-03-06T00:00:00Z".into(), ..Default::default() }];
    let bytes = deckcraft_pptx::export(&p).expect("export");
    assert!(part(&bytes, "ppt/comments/modernComment1.xml").contains("Top thread"));
    // The parts are numbered by comment slide: the comment-free middle slide is skipped.
    assert!(part(&bytes, "ppt/comments/comment2.xml").contains("Slide three"));
    assert!(part(&bytes, "ppt/comments/modernComment2.xml").contains("Slide three"));
    let rels2 = part(&bytes, "ppt/slides/_rels/slide2.xml.rels");
    assert!(!rels2.contains("relationships/comments"), "{rels2}");
    let q = deckcraft_pptx::import(&bytes).expect("import");
    assert_eq!(q.slides[0].comments.len(), 2);
    assert!(q.slides[1].comments.is_empty());
    assert_eq!(q.slides[2].comments.len(), 1);
    assert_eq!(q.slides[2].comments[0].text, "Slide three");
}

/// A slide that names a modern part another slide already owns, plus its own legacy part, must
/// not import the legacy copy: modern precedence holds even when the modern part is not re-read.
#[test]
fn owned_modern_part_still_suppresses_later_legacy() {
    let mut p = deck();
    let layout = p.slides[0].layout;
    let s2 = defaults::new_slide(&mut p, layout).expect("slide");
    p.slides.push(Arc::new(s2));
    Arc::make_mut(&mut p.slides[1]).comments = vec![Comment { author: "Cara".into(), text: "Slide two".into(), ..Default::default() }];
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let rels2 = part(&bytes, "ppt/slides/_rels/slide2.xml.rels").replace("modernComment2.xml", "modernComment1.xml");
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for i in 0..z.len() {
        let mut f = z.by_index(i).expect("entry");
        out.start_file(f.name(), zip::write::SimpleFileOptions::default()).expect("start");
        if f.name() == "ppt/slides/_rels/slide2.xml.rels" {
            std::io::Write::write_all(&mut out, rels2.as_bytes()).expect("write");
        } else {
            std::io::copy(&mut f, &mut out).expect("copy");
        }
    }
    let q = deckcraft_pptx::import(&out.finish().expect("finish").into_inner()).expect("import");
    assert_eq!(q.slides[0].comments.len(), 2);
    assert!(q.slides[1].comments.is_empty(), "{:?}", q.slides[1].comments);
}

/// A legacy-only package (what older tools read and write) still imports: 1/8 pt positions,
/// flattened replies.
#[test]
fn legacy_only_package_imports() {
    let bytes = deckcraft_pptx::export(&deck()).expect("export");
    let trimmed = rewrite(
        &bytes,
        &[
            "ppt/comments/modernComment1.xml",
            "ppt/authors.xml",
            "ppt/slides/_rels/slide1.xml.rels", // rewritten below without the modern rel
        ],
    );
    // Re-add the slide rels without the modern-comments relationship and without the anchor ext.
    let legacy_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments" Target="../comments/comment1.xml"/></Relationships>"#;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(&trimmed)).expect("zip");
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for i in 0..z.len() {
        let mut f = z.by_index(i).expect("entry");
        out.start_file(f.name(), zip::write::SimpleFileOptions::default()).expect("start");
        if f.name() == "ppt/slides/slide1.xml" {
            let mut xml = String::new();
            f.read_to_string(&mut xml).expect("utf8");
            if let Some(start) = xml.find("<p:extLst>") {
                let end = xml.find("</p:extLst>").map(|e| e + "</p:extLst>".len()).unwrap_or(xml.len());
                xml.replace_range(start..end, "");
            }
            std::io::Write::write_all(&mut out, xml.as_bytes()).expect("write");
        } else {
            std::io::copy(&mut f, &mut out).expect("copy");
        }
    }
    out.start_file("ppt/slides/_rels/slide1.xml.rels", zip::write::SimpleFileOptions::default()).expect("start");
    std::io::Write::write_all(&mut out, legacy_rels.as_bytes()).expect("write");
    let legacy_pkg = out.finish().expect("finish").into_inner();

    let q = deckcraft_pptx::import(&legacy_pkg).expect("import");
    let b = &q.slides[0].comments;
    // The legacy part flattened the two replies into siblings: four top-level comments.
    assert_eq!(b.len(), 4, "{b:?}");
    assert_eq!(b[0].text, "Top thread\nsecond line");
    // The legacy format has no resolved state — exactly what the modern part adds.
    assert!(!b[0].resolved);
    assert_eq!((b[0].x, b[0].y), (90.0, 45.0), "legacy positions are 1/8 pt");
    assert!(b.iter().all(|c| c.replies.is_empty()));
}

/// A modern-only package (no legacy part) imports threads and EMU positions.
#[test]
fn modern_only_package_imports() {
    let bytes = deckcraft_pptx::export(&deck()).expect("export");
    let trimmed = rewrite(&bytes, &["ppt/comments/comment1.xml"]);
    let q = deckcraft_pptx::import(&trimmed).expect("import");
    let b = &q.slides[0].comments;
    assert_eq!(b.len(), 2, "{b:?}");
    assert_eq!(b[0].replies.len(), 2);
    assert!(b[0].resolved);
    assert_eq!((b[0].x, b[0].y), (90.0, 45.0), "modern positions are EMU");
}

/// Every prefix an exported part uses is bound: PowerPoint refuses a package whose part is not
/// namespace-well-formed (issue #974, the modern comments part used `a:` without declaring it).
#[test]
fn every_written_xml_part_binds_its_prefixes() {
    use quick_xml::events::Event;
    use quick_xml::name::ResolveResult;

    let bytes = deckcraft_pptx::export(&deck()).expect("export");
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).expect("zip");
    let mut modern = 0;
    for i in 0..z.len() {
        let mut f = z.by_index(i).expect("entry");
        let name = f.name().to_string();
        if !(name.ends_with(".xml") || name.ends_with(".rels")) {
            continue;
        }
        let mut data = vec![];
        f.read_to_end(&mut data).expect("read");
        modern += usize::from(name.contains("modernComment"));
        let mut r = quick_xml::NsReader::from_reader(data.as_slice());
        loop {
            match r.read_resolved_event() {
                Ok((ns, Event::Start(e) | Event::Empty(e))) => {
                    assert!(!matches!(ns, ResolveResult::Unknown(_)), "{name}: unbound prefix on <{}>", String::from_utf8_lossy(e.name().as_ref()));
                    for attr in e.attributes() {
                        let attr = attr.unwrap_or_else(|err| panic!("{name}: bad attribute: {err}"));
                        let (ns, _) = r.resolver().resolve_attribute(attr.key);
                        assert!(
                            !matches!(ns, ResolveResult::Unknown(_)),
                            "{name}: unbound prefix on attribute {}",
                            String::from_utf8_lossy(attr.key.as_ref())
                        );
                    }
                }
                Ok((_, Event::Eof)) => break,
                Ok(_) => {}
                Err(e) => panic!("{name}: not well-formed: {e}"),
            }
        }
    }
    assert!(modern > 0, "the deck should write a modern comments part");
}
