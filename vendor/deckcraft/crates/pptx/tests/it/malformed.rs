//! Hostile input: the importer must return `Err` or a usable presentation, never panic.

use std::io::{Cursor, Write};

use deckcraft_model::defaults;

fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    for (n, d) in files {
        z.start_file(*n, o).expect("start");
        z.write_all(d).expect("write");
    }
    z.finish().expect("finish").into_inner()
}

fn valid() -> Vec<u8> {
    deckcraft_pptx::export(&defaults::new_presentation(None)).expect("export")
}

/// Every entry of a valid export, as (name, bytes).
fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    (0..z.len())
        .map(|i| {
            let mut f = z.by_index(i).expect("entry");
            let mut d = vec![];
            std::io::Read::read_to_end(&mut f, &mut d).expect("read");
            (f.name().to_string(), d)
        })
        .collect()
}

fn rezip(es: &[(String, Vec<u8>)]) -> Vec<u8> {
    let v: Vec<(&str, &[u8])> = es.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
    zip_of(&v)
}

#[test]
fn random_bytes_and_empty() {
    assert!(deckcraft_pptx::import(&[]).is_err());
    assert!(deckcraft_pptx::import(b"PK\x03\x04garbage").is_err());
    let mut seed = 7u64;
    for len in [1usize, 10, 100, 1000, 10_000] {
        let v: Vec<u8> = (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (seed >> 33) as u8
            })
            .collect();
        assert!(deckcraft_pptx::import(&v).is_err());
    }
}

#[test]
fn truncated_zip() {
    let v = valid();
    for cut in [10, v.len() / 4, v.len() / 2, v.len() - 30, v.len() - 1] {
        let _ = deckcraft_pptx::import(&v[..cut]);
    }
}

#[test]
fn zip_without_presentation() {
    let z = zip_of(&[("[Content_Types].xml", b"<Types/>"), ("hello.txt", b"hi")]);
    assert!(deckcraft_pptx::import(&z).is_err());
}

#[test]
fn missing_parts_still_import() {
    let es = entries(&valid());
    // Drop each part in turn; import must not panic, and must succeed unless the main part is gone.
    for i in 0..es.len() {
        let mut v = es.clone();
        let (name, _) = v.remove(i);
        let r = deckcraft_pptx::import(&rezip(&v));
        if name == "ppt/presentation.xml" {
            assert!(r.is_err());
        } else if let Ok(p) = r {
            assert!(p.validate().is_empty(), "dropping {name}: {:?}", p.validate());
            // Whatever we read must export again.
            deckcraft_pptx::export(&p).expect("re-export");
        }
    }
}

#[test]
fn bad_xml_in_each_part() {
    let es = entries(&valid());
    for i in 0..es.len() {
        for bad in [&b"<p:sld><unclosed"[..], b"\xff\xfe\x00garbage", b"", b"<?xml version=\"1.0\"?>", b"<a:b c='1' c='2'/>"] {
            let mut v = es.clone();
            v[i].1 = bad.to_vec();
            if let Ok(p) = deckcraft_pptx::import(&rezip(&v)) {
                deckcraft_pptx::export(&p).expect("re-export");
            }
        }
    }
}

#[test]
fn huge_numbers_and_nonsense_attributes() {
    let es = entries(&valid());
    let mut v = es.clone();
    for (n, d) in v.iter_mut() {
        if n.ends_with(".xml") {
            let s = String::from_utf8_lossy(d)
                .replace("x=\"", "x=\"99999999999999999999")
                .replace("cx=\"", "cx=\"-")
                .replace("sz=\"", "sz=\"1e309")
                .replace("rot=\"", "rot=\"NaN")
                .replace("idx=\"", "idx=\"4294967296")
                .replace("id=\"", "id=\"0");
            *d = s.into_bytes();
        }
    }
    if let Ok(p) = deckcraft_pptx::import(&rezip(&v)) {
        for s in &p.slides {
            deckcraft_model::walk(&s.shapes, &mut |sh, _| {
                if let Some(x) = sh.xfrm {
                    assert!(x.is_finite());
                }
            });
        }
        deckcraft_pptx::export(&p).expect("re-export");
    }
}

#[test]
fn deep_nesting_is_bounded() {
    let es = entries(&valid());
    let deep = format!(
        "<?xml version=\"1.0\"?><p:sld xmlns:p=\"x\" xmlns:a=\"y\"><p:cSld><p:spTree>{}{}</p:spTree></p:cSld></p:sld>",
        "<p:grpSp>".repeat(200),
        "</p:grpSp>".repeat(200)
    );
    let mut v = es.clone();
    if let Some(e) = v.iter_mut().find(|(n, _)| n == "ppt/slides/slide1.xml") {
        e.1 = deep.into_bytes();
    }
    let p = deckcraft_pptx::import(&rezip(&v)).expect("import");
    deckcraft_pptx::export(&p).expect("re-export");
    // Beyond the XML depth limit the part is rejected, the deck still loads.
    let deeper = format!("<p:sld>{}</p:sld>", "<a>".repeat(5000));
    if let Some(e) = v.iter_mut().find(|(n, _)| n == "ppt/slides/slide1.xml") {
        e.1 = deeper.into_bytes();
    }
    let _ = deckcraft_pptx::import(&rezip(&v));
}

#[test]
fn relationship_cycles_and_bad_targets() {
    let es = entries(&valid());
    let mut v = es.clone();
    if let Some(e) = v.iter_mut().find(|(n, _)| n == "ppt/slides/_rels/slide1.xml.rels") {
        e.1 = br#"<Relationships xmlns="x"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../../../../../etc/passwd"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slide1.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide" Target="slide1.xml"/></Relationships>"#.to_vec();
    }
    let p = deckcraft_pptx::import(&rezip(&v)).expect("import");
    assert_eq!(p.slides.len(), 1);
    assert!(p.validate().is_empty());
}

/// Fixed-seed random byte mutations of a valid export (inside the zip entries, so the XML is hit).
#[test]
fn fuzz_mutations() {
    let p = {
        let mut s = deckcraft_engine::Session::new();
        deckcraft_engine::sample::open_sample(&mut s).expect("sample");
        s.doc().expect("doc").doc.as_ref().clone()
    };
    let es = entries(&deckcraft_pptx::export(&p).expect("export"));
    let mut seed = 0x5eed_u64;
    let mut rnd = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n.max(1) as u64) as usize
    };
    let iters = if cfg!(debug_assertions) { 300 } else { 2000 };
    for _ in 0..iters {
        let mut v = es.clone();
        let k = rnd(v.len());
        let d = &mut v[k].1;
        for _ in 0..1 + rnd(8) {
            if d.is_empty() {
                break;
            }
            let at = rnd(d.len());
            match rnd(4) {
                0 => d[at] = rnd(256) as u8,
                1 => {
                    d.truncate(at);
                }
                2 => d.insert(at, b"<>\"&="[rnd(5)]),
                _ => {
                    let end = (at + rnd(64)).min(d.len());
                    d.drain(at..end);
                }
            }
        }
        if let Ok(q) = deckcraft_pptx::import(&rezip(&v)) {
            assert!(q.validate().is_empty());
            deckcraft_pptx::export(&q).expect("re-export");
        }
    }
    // Raw byte flips of the whole zip, too.
    let bytes = deckcraft_pptx::export(&p).expect("export");
    for _ in 0..iters {
        let mut b = bytes.clone();
        for _ in 0..1 + rnd(16) {
            let at = rnd(b.len());
            b[at] = rnd(256) as u8;
        }
        let _ = deckcraft_pptx::import(&b);
    }
}

#[test]
fn zip_bomb_entry_is_capped() {
    // A highly compressible 64 MiB XML part: must import (or fail) without blowing up.
    let big = vec![b' '; 64 * 1024 * 1024];
    let mut es = entries(&valid());
    es.push(("ppt/big.xml".into(), big));
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (n, d) in &es {
        z.start_file(n.as_str(), o).expect("start");
        z.write_all(d).expect("write");
    }
    let bytes = z.finish().expect("finish").into_inner();
    assert!(bytes.len() < 2 * 1024 * 1024);
    let p = deckcraft_pptx::import(&bytes).expect("import");
    assert_eq!(p.slides.len(), 1);
}

#[test]
fn export_of_odd_models_is_valid() {
    use deckcraft_model::{LayoutId, Shape, ShapeId, ShapeKind, Slide, SlideId};
    use std::sync::Arc;
    // No masters, dangling layout, duplicate and zero shape ids, NaN geometry, control characters.
    let mut p = defaults::new_presentation(None);
    let mut s = Slide { id: SlideId(9999), layout: LayoutId(12345), ..Default::default() };
    for id in [0, 1, 5, 5] {
        s.shapes.push(Shape {
            id: ShapeId(id),
            name: "bad\u{0}name\u{b}".into(),
            xfrm: Some(deckcraft_geom::Xfrm::new(f64::NAN, f64::INFINITY, -5.0, 1e300)),
            text: Some(deckcraft_model::TextBody::from_text("tab\tand\u{1}control\nnew line")),
            ..Default::default()
        });
    }
    s.shapes.push(Shape {
        id: ShapeId(7),
        kind: ShapeKind::Opaque { xml: "<not xml".into(), preview: None, label: "Thing".into(), parts: vec![] },
        ..Default::default()
    });
    p.slides.push(Arc::new(s));
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let q = deckcraft_pptx::import(&bytes).expect("import");
    assert_eq!(q.slides.len(), 2);
    assert!(q.validate().is_empty(), "{:?}", q.validate());
    let mut empty = p.clone();
    empty.masters.clear();
    let q = deckcraft_pptx::import(&deckcraft_pptx::export(&empty).expect("export")).expect("import");
    assert!(!q.masters.is_empty());
}

/// A slide the file lists but that can't be read is left out, and `import_checked` names it
/// where `import` would have said nothing (#203).
#[test]
fn unreadable_slide_is_reported() {
    let mut p = defaults::new_presentation(None);
    let mut second = p.slides.first().expect("a slide").as_ref().clone();
    second.id = deckcraft_model::SlideId(second.id.0 + 1000);
    p.slides.push(std::sync::Arc::new(second));
    let bytes = deckcraft_pptx::export(&p).expect("export");

    let (q, lost) = deckcraft_pptx::import_checked(&bytes).expect("import");
    assert_eq!((q.slides.len(), lost.len()), (2, 0), "{lost:?}");

    let mut es = entries(&bytes);
    let slide2 = es.iter_mut().find(|(n, _)| n == "ppt/slides/slide2.xml").expect("slide2");
    slide2.1 = b"<p:sld".to_vec();
    let broken = rezip(&es);
    let (q, lost) = deckcraft_pptx::import_checked(&broken).expect("import");
    assert_eq!(q.slides.len(), 1);
    assert_eq!(lost, vec!["ppt/slides/slide2.xml".to_string()]);
    assert_eq!(deckcraft_pptx::import(&broken).expect("import").slides.len(), 1);
}

fn table_deck(rows: usize, cols: usize) -> deckcraft_model::Presentation {
    use deckcraft_model::table::Row;
    use deckcraft_model::{Shape, ShapeId, ShapeKind, Table, Xfrm};
    let mut p = defaults::new_presentation(None);
    let id = ShapeId(p.alloc_id().expect("ids"));
    let t = Table { cols: vec![1.0; cols], rows: vec![Row { height: 1.0, cells: vec![Default::default(); cols] }; rows], ..Default::default() };
    let slide = std::sync::Arc::make_mut(&mut p.slides[0]);
    slide.shapes.push(Shape { id, name: "T".into(), xfrm: Some(Xfrm::new(0.0, 0.0, 10.0, 10.0)), kind: ShapeKind::Table(t), ..Default::default() });
    p
}

/// A table past the cell cap opens with its leading rows and an import warning; saving one that
/// is over the cap is an error, never a silent trim.
#[test]
fn oversized_table_warns_on_import_and_refuses_export() {
    let max = deckcraft_model::table::MAX_CELLS;
    let cols = 1000;
    let rows = max / cols;
    // A table exactly at the cap round-trips with no warning.
    let bytes = deckcraft_pptx::export(&table_deck(rows, cols)).expect("export at the cap");
    let (_, _, warnings) = deckcraft_pptx::import_report(&bytes).expect("import");
    assert!(warnings.is_empty(), "{warnings:?}");
    // One more row in the file: kept truncated, and reported.
    let mut es = entries(&bytes);
    let slide = es.iter_mut().find(|(n, _)| n == "ppt/slides/slide1.xml").expect("slide1");
    let xml = String::from_utf8(std::mem::take(&mut slide.1)).expect("utf8");
    let start = xml.rfind("<a:tr ").expect("row");
    let end = xml.rfind("</a:tr>").expect("row end") + "</a:tr>".len();
    slide.1 = format!("{}{}{}", &xml[..end], &xml[start..end], &xml[end..]).into_bytes();
    let (q, _, warnings) = deckcraft_pptx::import_report(&rezip(&es)).expect("import");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let t = q.slides[0].shapes.iter().find_map(|s| if let deckcraft_model::ShapeKind::Table(t) = &s.kind { Some(t) } else { None }).expect("table");
    assert_eq!((t.rows.len(), t.cols.len()), (rows, cols));
    // A model table past the cap is refused on export.
    let err = deckcraft_pptx::export(&table_deck(rows + 1, cols)).expect_err("over the cap");
    assert!(err.to_string().contains("table"), "{err}");
}

/// Missing slide relationships must not turn a damaged deck into a clean partial load (#1910).
#[test]
fn missing_slide_relationships_are_reported() {
    let mut p = defaults::new_presentation(None);
    let layout = p.slides[0].layout;
    for _ in 1..13 {
        let slide = defaults::new_slide(&mut p, layout).expect("slide");
        p.slides.push(std::sync::Arc::new(slide));
    }
    let mut es = entries(&deckcraft_pptx::export(&p).expect("export"));
    let pres = es.iter_mut().find(|(n, _)| n == "ppt/presentation.xml").expect("presentation");
    let xml = String::from_utf8(pres.1.clone()).expect("utf8");
    let doc = deckcraft_pptx::parse_xml(xml.as_bytes()).expect("xml");
    let slides = doc.root.child("sldIdLst").expect("slide list");
    let mut broken = xml.clone();
    for slide in slides.children_named("sldId").skip(6) {
        let rid = slide.attr("r:id").expect("relationship id");
        broken = broken.replace(&format!("r:id=\"{rid}\""), &format!("r:id=\"missing-{rid}\""));
    }
    pres.1 = broken.into_bytes();
    let damaged = rezip(&es);
    let (q, lost) = deckcraft_pptx::import_checked(&damaged).expect("import");
    assert_eq!(q.slides.len(), 6);
    assert_eq!(lost.len(), 7, "every omitted slide must be reported: {lost:?}");
    let loaded = deckcraft_engine::cmd::file::loaded_file("Original.pptx", &damaged).expect("load");
    let mut session = deckcraft_engine::Session::new();
    let token = session.stage_loaded(loaded);
    let result = session.execute("file.adopt", &serde_json::json!({"path": "Original.pptx", "token": token})).expect("adopt");
    assert_eq!(result["lostSlides"].as_array().expect("lost slides").len(), 7);
    assert!(session.doc().expect("doc").path.is_none(), "Save must not overwrite the damaged source");
    assert!(session.doc().expect("doc").title.ends_with("(Repaired)"));
    let (q, lost) = deckcraft_pptx::import_checked(&deckcraft_pptx::export(&p).expect("export")).expect("import");
    assert_eq!(q.slides.len(), 13);
    assert!(lost.is_empty());
}

/// Every listed entry has its own slide part and relationship: the resource cap must not
/// silently turn an otherwise readable large deck into a complete-looking partial load.
#[test]
fn over_limit_slide_lists_are_refused() {
    fn deck(count: usize) -> Vec<u8> {
        let mut es = entries(&valid());
        let pres = es.iter_mut().find(|(n, _)| n == "ppt/presentation.xml").expect("presentation");
        let mut xml = String::from_utf8(pres.1.clone()).expect("utf8");
        let start = xml.find("<p:sldIdLst>").expect("slide list");
        let end = xml.find("</p:sldIdLst>").expect("slide list end") + "</p:sldIdLst>".len();
        let ids: String = (1..=count).map(|i| format!(r#"<p:sldId id="{}" r:id="capSlide{i}"/>"#, 255 + i)).collect();
        xml.replace_range(start..end, &format!("<p:sldIdLst>{ids}</p:sldIdLst>"));
        pres.1 = xml.into_bytes();
        let rels = es.iter_mut().find(|(n, _)| n == "ppt/_rels/presentation.xml.rels").expect("relationships");
        let mut xml = String::from_utf8(rels.1.clone()).expect("utf8");
        let targets: String = (1..=count).map(|i| format!(r#"<Relationship Id="capSlide{i}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{i}.xml"/>"#)).collect();
        xml = xml.replace("</Relationships>", &format!("{targets}</Relationships>"));
        rels.1 = xml.into_bytes();
        let types = es.iter_mut().find(|(n, _)| n == "[Content_Types].xml").expect("content types");
        let xml = String::from_utf8(types.1.clone()).expect("utf8");
        let overrides: String = (2..=count).map(|i| format!(r#"<Override PartName="/ppt/slides/slide{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#)).collect();
        types.1 = xml.replace("</Types>", &format!("{overrides}</Types>")).into_bytes();
        let blank = br#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld></p:sld>"#.to_vec();
        es.iter_mut().find(|(n, _)| n == "ppt/slides/slide1.xml").expect("slide").1 = blank.clone();
        let slide_rels = es.iter().find(|(n, _)| n == "ppt/slides/_rels/slide1.xml.rels").expect("slide relationships").1.clone();
        for i in 2..=count {
            es.push((format!("ppt/slides/slide{i}.xml"), blank.clone()));
            es.push((format!("ppt/slides/_rels/slide{i}.xml.rels"), slide_rels.clone()));
        }
        rezip(&es)
    }
    let at_limit = deck(10_000);
    let (p, lost) = deckcraft_pptx::import_checked(&at_limit).expect("exactly at the limit");
    assert_eq!(p.slides.len(), 10_000);
    assert!(lost.is_empty());
    drop(p);

    let over_limit = deck(10_001);
    for error in [
        deckcraft_pptx::import_checked(&over_limit).expect_err("must refuse a partial load"),
        deckcraft_pptx::import(&over_limit).expect_err("plain import must also refuse"),
    ] {
        assert!(matches!(error, deckcraft_pptx::PptxError::Unsupported(_)));
        assert!(error.to_string().contains("10000"), "{error}");
        assert!(error.to_string().contains("slide"), "{error}");
    }
    assert!(
        deckcraft_engine::cmd::file::loaded_file("Original.pptx", &over_limit).is_err(),
        "no partial file may be adopted with the original save path"
    );
}

/// Many slides naming one notes part: it is read for the first slide only, not copied onto every
/// slide that names it.
#[test]
fn a_shared_notes_part_is_read_once() {
    use std::sync::Arc;
    let mut p = defaults::new_presentation(None);
    let layout = p.slides[0].layout;
    for _ in 0..3 {
        let s = defaults::new_slide(&mut p, layout).expect("slide");
        p.slides.push(Arc::new(s));
    }
    for (i, s) in p.slides.iter_mut().enumerate() {
        Arc::make_mut(s).notes = deckcraft_model::text::TextBody::from_text(&format!("notes {i}"));
    }
    let mut es = entries(&deckcraft_pptx::export(&p).expect("export"));
    for (n, d) in es.iter_mut() {
        if n.starts_with("ppt/slides/_rels/") {
            let x = String::from_utf8(d.clone()).expect("utf8");
            let x = x
                .replace("notesSlide2.xml", "notesSlide1.xml")
                .replace("notesSlide3.xml", "notesSlide1.xml")
                .replace("notesSlide4.xml", "notesSlide1.xml");
            *d = x.into_bytes();
        }
    }
    let q = deckcraft_pptx::import(&rezip(&es)).expect("import");
    assert_eq!(q.slides.len(), 4);
    assert_eq!(q.slides[0].notes.text(), "notes 0");
    assert!(q.slides[1..].iter().all(|s| s.notes.text().is_empty()), "a shared notes part is not copied");
}

#[test]
fn notes_master_and_handout_master_shape_count_bounds() {
    use std::fmt::Write as _;
    use std::sync::Arc;
    let mut p = defaults::new_presentation(None);
    Arc::make_mut(&mut p.slides[0]).notes = deckcraft_model::text::TextBody::from_text("note");
    p.handout_master = Some(Arc::new(deckcraft_model::Master { name: "Handouts".into(), ..Default::default() }));
    let mut es = entries(&deckcraft_pptx::export(&p).expect("export"));
    let mut many = String::new();
    for i in 2..=20_050 {
        let _ = write!(many, r#"<p:sp><p:nvSpPr><p:cNvPr id="{i}" name="S{i}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/></p:sp>"#);
    }
    for (n, d) in es.iter_mut() {
        if n == "ppt/notesMasters/notesMaster1.xml" || n == "ppt/handoutMasters/handoutMaster1.xml" {
            let xml = String::from_utf8(d.clone()).expect("utf8");
            if let Some(pos) = xml.rfind("</p:spTree>") {
                let mut patched = String::with_capacity(xml.len() + many.len());
                patched.push_str(&xml[..pos]);
                patched.push_str(&many);
                patched.push_str(&xml[pos..]);
                *d = patched.into_bytes();
            }
        }
    }
    let q = deckcraft_pptx::import(&rezip(&es)).expect("bounded import");
    assert_eq!(q.notes_master.as_ref().expect("notes_master").shapes.len(), 20_000);
    assert_eq!(q.handout_master.as_ref().expect("handout_master").shapes.len(), 20_000);
    deckcraft_pptx::export(&q).expect("re-export");
}
