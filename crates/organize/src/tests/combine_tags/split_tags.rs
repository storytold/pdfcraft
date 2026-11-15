//! Extract Pages and Split keep the structure tree of the pages they copy (PDFBOX-6009's shape).

use super::*;

/// A content stream marking `items` (tag, MCID, text) as marked content, then `extra`.
fn marked(items: &[(&str, i64, &str)], extra: &str) -> String {
    let mut s = String::new();
    for (tag, m, t) in items {
        s.push_str(&format!("/{tag} <</MCID {m}>> BDC BT /F1 12 Tf 20 20 Td ({t}) Tj ET EMC "));
    }
    s.push_str(extra);
    format!("<< /Length {} >>\nstream\n{s}\nendstream", s.len())
}

/// A three-page tagged document under one `Document` element: a heading and the start of a
/// paragraph on page 1, the paragraph's end (by a marked-content reference) and a link (by an
/// object reference) on page 2, a table cell on page 3 and a span inside a form XObject page 3
/// draws. An empty section holds no content. Role map, class map, ID tree, `/Lang` and
/// `/MarkInfo /UserProperties` are set; `ua` claims PDF/UA-1.
fn doc_three(ua: bool) -> Document {
    doc_three_link_named_on(ua, 4)
}

/// [`doc_three`], its link's object reference naming page object `link_pg` (4: page 2, where it is).
fn doc_three_link_named_on(ua: bool, link_pg: u32) -> Document {
    let xmp = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
               <rdf:Description rdf:about=\"\" xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\" pdfuaid:part=\"1\"/></rdf:RDF></x:xmpmeta>";
    let metadata = if ua { " /Metadata 26 0 R" } else { "" };
    let form = "/Span <</MCID 0>> BDC BT /F1 9 Tf 0 0 Td (F) Tj ET EMC";
    let b: Vec<String> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 12 0 R /MarkInfo << /Marked true /UserProperties true >> /Lang (de-DE){metadata} >>"), // 1
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >>".into(), // 2
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 9 0 R >> >> /StructParents 0 >>".into(), // 3
        "<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Resources << /Font << /F1 9 0 R >> >> /StructParents 1 /Annots [10 0 R] >>".into(), // 4
        "<< /Type /Page /Parent 2 0 R /Contents 8 0 R /Resources << /Font << /F1 9 0 R >> /XObject << /Fm0 11 0 R >> >> /StructParents 2 >>".into(), // 5
        marked(&[("H1", 0, "P1"), ("P", 1, "P1b")], ""), // 6
        marked(&[("P", 0, "P2")], ""),                   // 7
        marked(&[("TD", 0, "P3")], "/Fm0 Do"),           // 8
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(), // 9
        "<< /Type /Annot /Subtype /Link /Rect [0 0 50 50] /StructParent 3 /A << /S /URI /URI (https://example.com/) >> >>".into(), // 10
        format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] /StructParents 4 /Resources << /Font << /F1 9 0 R >> >> /Length {} >>\nstream\n{form}\nendstream",
            form.len()
        ), // 11
        "<< /Type /StructTreeRoot /K 13 0 R /ParentTree 24 0 R /ParentTreeNextKey 5 /RoleMap << /Heading /H1 >> /ClassMap << /Bold << /O /Layout /FontWeight 700 >> >> /IDTree 25 0 R >>".into(), // 12
        "<< /Type /StructElem /S /Document /P 12 0 R /K [14 0 R 15 0 R 16 0 R 17 0 R 18 0 R] >>".into(), // 13
        "<< /Type /StructElem /S /Heading /P 13 0 R /Pg 3 0 R /K 0 /ID (h1) /C /Bold >>".into(), // 14
        "<< /Type /StructElem /S /P /P 13 0 R /Pg 3 0 R /K [1 << /Type /MCR /Pg 4 0 R /MCID 0 >>] /ID (p1) >>".into(), // 15
        format!("<< /Type /StructElem /S /Link /P 13 0 R /Pg {link_pg} 0 R /K << /Type /OBJR /Obj 10 0 R >> /ID (link) >>"), // 16
        "<< /Type /StructElem /S /Sect /P 13 0 R /ID (empty) >>".into(), // 17
        "<< /Type /StructElem /S /Table /P 13 0 R /K [19 0 R 22 0 R] >>".into(), // 18
        "<< /Type /StructElem /S /TR /P 18 0 R /K 20 0 R >>".into(), // 19
        "<< /Type /StructElem /S /TD /P 19 0 R /Pg 5 0 R /K 0 /ID (td) >>".into(), // 20
        "null".into(), // 21
        "<< /Type /StructElem /S /Span /P 18 0 R /Pg 5 0 R /K << /Type /MCR /Stm 11 0 R /MCID 0 >> >>".into(), // 22
        "null".into(), // 23
        "<< /Nums [0 [14 0 R 15 0 R] 1 [15 0 R] 2 [20 0 R] 3 16 0 R 4 [22 0 R]] >>".into(), // 24
        "<< /Names [(empty) 17 0 R (h1) 14 0 R (link) 16 0 R (p1) 15 0 R (td) 20 0 R] >>".into(), // 25
        format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{xmp}\nendstream", xmp.len()), // 26
        "<< /Title (Three) >>".into(), // 27
    ];
    open(build(&b, "/Root 1 0 R /Info 27 0 R"))
}

/// Every element under `d`, depth first.
fn elements(doc: &Document, d: &Dict, into: &mut Vec<Dict>) {
    for k in kids(doc, d) {
        if k.contains(b"S") {
            into.push(k.clone());
            elements(doc, &k, into);
        }
    }
}

/// The structure types of the document's elements, depth first.
fn types(doc: &Document) -> Vec<String> {
    let root = tree_root(doc).expect("a structure tree");
    let mut all = Vec::new();
    elements(doc, &root, &mut all);
    all.iter().map(|e| String::from_utf8_lossy(e.name(b"S").unwrap()).into_owned()).collect()
}

fn list_of(doc: &Document, d: &Dict) -> Vec<Object> {
    match d.get(b"K").map(|k| doc.resolve(k)).as_deref() {
        Some(Object::Array(a)) => a.clone(),
        Some(other) => vec![other.clone()],
        None => Vec::new(),
    }
}

/// A reader's check of the parent tree: each page's marked content leads to an element that
/// lists that MCID on that page, and each element's content on a page leads back to it.
fn assert_parent_tree_consistent(doc: &Document) {
    let root = tree_root(doc).expect("a structure tree");
    let next = root.get(b"ParentTreeNextKey").and_then(Object::as_int).expect("/ParentTreeNextKey");
    let pages = pages(doc).unwrap();
    for (i, p) in pages.iter().enumerate() {
        let Some(key) = page_dict(doc, i).get(b"StructParents").and_then(Object::as_int) else { continue };
        assert!(key < next, "page {i}: key {key} below /ParentTreeNextKey {next}");
        let entry = doc.resolve(&parent_entry(doc, key)).as_array().cloned().expect("an array for a page");
        for (mcid, e) in entry.iter().enumerate() {
            let Some(e) = doc.resolve(e).as_dict().cloned() else { continue };
            let pg = e.get(b"Pg").and_then(Object::as_ref);
            let listed = list_of(doc, &e).iter().any(|k| match k {
                Object::Int(m) => *m as usize == mcid && pg == Some(p.obj),
                other => doc.resolve(other).as_dict().is_some_and(|r| {
                    r.get(b"MCID").and_then(Object::as_int) == Some(mcid as i64)
                        && !r.contains(b"Stm")
                        && r.get(b"Pg").and_then(Object::as_ref).or(pg) == Some(p.obj)
                }),
            });
            assert!(listed, "page {i}, MCID {mcid}: its element doesn't list it");
        }
    }
    let mut all = Vec::new();
    elements(doc, &root, &mut all);
    for e in &all {
        let pg = e.get(b"Pg").and_then(Object::as_ref);
        for k in list_of(doc, e) {
            let (mcid, page) = match &k {
                Object::Int(m) => (*m, pg),
                other => match doc.resolve(other).as_dict() {
                    Some(r) if r.contains(b"MCID") && !r.contains(b"Stm") => {
                        (r.get(b"MCID").and_then(Object::as_int).unwrap(), r.get(b"Pg").and_then(Object::as_ref).or(pg))
                    }
                    _ => continue,
                },
            };
            let page = page.expect("content names its page");
            let i = pages.iter().position(|p| p.obj == page).expect("its page is in the document");
            let owner = owner_of_content(doc, i, mcid as usize);
            assert_eq!(owner.name(b"S"), e.name(b"S"), "page {i}, MCID {mcid}");
        }
    }
}

fn ids(doc: &Document) -> Vec<String> {
    let root = tree_root(doc).unwrap();
    let Some(tree) = root.get(b"IDTree").map(|t| doc.resolve(t)) else { return Vec::new() };
    let names = tree.as_dict().unwrap().get(b"Names").unwrap().as_array().cloned().unwrap();
    names.iter().filter_map(|n| n.as_string().map(|s| s.to_text())).collect()
}

fn mark_info(doc: &Document) -> Dict {
    catalog(doc).get(b"MarkInfo").map(|m| doc.resolve(m)).and_then(|m| m.as_dict().cloned()).expect("/MarkInfo")
}

fn map_name(doc: &Document, d: &Dict, key: &[u8], name: &[u8]) -> Option<Vec<u8>> {
    doc.resolve(d.get(key)?).as_dict()?.name(name).map(<[u8]>::to_vec)
}

/// The element owning MCID 0 of the document's form XObject with `/StructParents`.
fn owner_of_stream(doc: &Document) -> Dict {
    let key = doc
        .object_numbers()
        .into_iter()
        .find_map(|n| match &*doc.get(ObjRef::new(n, doc.generation(n))) {
            Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Form") => s.dict.get(b"StructParents").and_then(Object::as_int),
            _ => None,
        })
        .expect("a form XObject with /StructParents");
    let entry = doc.resolve(&parent_entry(doc, key)).as_array().cloned().unwrap();
    doc.resolve(&entry[0]).as_dict().cloned().unwrap()
}

#[test]
fn split_every_page_keeps_each_parts_tags() {
    // PDFBOX-6009's shape: a tagged file split page by page lost its tags in every part.
    let parts: Vec<Document> = split(&doc_three(false), &SplitBy::PageCount(1)).unwrap().iter().map(full_roundtrip).collect();
    assert_eq!(parts.iter().map(labels).collect::<Vec<_>>(), [["P1"], ["P2"], ["P3"]]);
    let want: [&[&str]; 3] = [&["Document", "Heading", "P"], &["Document", "P", "Link"], &["Document", "Table", "TR", "TD", "Span"]];
    let want_ids: [&[&str]; 3] = [&["h1", "p1"], &["link", "p1"], &["td"]];
    for (i, part) in parts.iter().enumerate() {
        assert_eq!(text(&catalog(part), b"Lang"), "de-DE", "part {i}");
        let mi = mark_info(part);
        assert_eq!((mi.get(b"Marked"), mi.get(b"UserProperties")), (Some(&Object::Bool(true)), Some(&Object::Bool(true))), "part {i}");
        // The source's own Document element is the top, directly under the new root; the empty
        // section is not carried.
        assert_eq!(types(part), want[i], "part {i}");
        assert_eq!(ids(part), want_ids[i], "part {i}");
        let root = tree_root(part).unwrap();
        assert_eq!(map_name(part, &root, b"RoleMap", b"Heading"), Some(b"H1".to_vec()));
        assert!(part.resolve(root.get(b"ClassMap").unwrap()).as_dict().unwrap().contains(b"Bold"));
        assert_parent_tree_consistent(part);
    }
    // The paragraph spanning pages 1 and 2 keeps, in each part, the content on that part's page.
    assert_eq!(owner_of_content(&parts[0], 0, 1).name(b"S"), Some(&b"P"[..]));
    assert_eq!(owner_of_content(&parts[1], 0, 0).name(b"S"), Some(&b"P"[..]));
    assert_eq!(owner_of_annot(&parts[1], 0, 0).name(b"S"), Some(&b"Link"[..]));
    assert_eq!(owner_of_stream(&parts[2]).name(b"S"), Some(&b"Span"[..]));
    // Not the whole file: no PDF/UA claim (as Combine of some pages).
    assert!(parts.iter().all(|p| pdfx_of(p).xmp.is_none_or(|x| !x.contains("pdfuaid"))));
}

#[test]
fn extract_pages_keeps_the_tags_of_the_pages_taken() {
    let src = doc_three(true);
    let out = full_roundtrip(&extract_pages(&src, &[1, 2]).unwrap());
    assert_eq!(labels(&out), ["P2", "P3"]);
    assert_eq!(types(&out), ["Document", "P", "Link", "Table", "TR", "TD", "Span"]);
    assert_eq!(ids(&out), ["link", "p1", "td"]);
    assert_parent_tree_consistent(&out);
    assert!(pdfx_of(&out).xmp.is_none_or(|x| !x.contains("pdfuaid")), "some pages of a PDF/UA file are not claimed to be one");

    // Every page: the whole tree, the empty section included, and the PDF/UA claim stands.
    let all = full_roundtrip(&extract_pages(&src, &[0, 1, 2]).unwrap());
    assert_eq!(types(&all), ["Document", "Heading", "P", "Link", "Sect", "Table", "TR", "TD", "Span"]);
    assert_parent_tree_consistent(&all);
    assert!(pdfx_of(&all).xmp.expect("XMP").contains("<pdfuaid:part>1</pdfuaid:part>"));
}

#[test]
fn extract_of_untagged_pages_adds_no_tags() {
    let out = full_roundtrip(&extract_pages(&doc_b(), &[0]).unwrap());
    let cat = catalog(&out);
    assert!(!cat.contains(b"StructTreeRoot") && !cat.contains(b"MarkInfo"));
}

#[test]
fn split_of_a_cyclic_structure_tree_stays_bounded() {
    // Elements that list each other (14 and 15) and themselves (16), under a root that lists
    // itself and names itself as its parent and ID tree.
    let b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 12 0 R /MarkInfo << /Marked true >> >>".into(), // 1
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >>".into(),               // 2
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 9 0 R >> >> /StructParents 0 >>".into(), // 3
        "<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Resources << /Font << /F1 9 0 R >> >> /StructParents 1 >>".into(), // 4
        "null".into(),                                                                                  // 5
        marked(&[("P", 0, "C1")], ""),                                                                  // 6
        marked(&[("P", 0, "C2")], ""),                                                                  // 7
        "null".into(),                                                                                  // 8
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),                                // 9
        "null".into(),                                                                                  // 10
        "null".into(),                                                                                  // 11
        "<< /Type /StructTreeRoot /K [14 0 R 12 0 R 16 0 R] /ParentTree 12 0 R /IDTree 12 0 R >>".into(), // 12
        "null".into(),                                                                                  // 13
        "<< /S /Sect /P 12 0 R /Pg 3 0 R /K [15 0 R 0] >>".into(),                                      // 14
        "<< /S /P /P 14 0 R /Pg 4 0 R /K [14 0 R 0] >>".into(),                                         // 15
        "<< /S /Div /P 12 0 R /Pg 4 0 R /K [16 0 R] >>".into(),                                         // 16
    ];
    let src = open(build(&b, "/Root 1 0 R"));
    let parts: Vec<Document> = split(&src, &SplitBy::PageCount(1)).unwrap().iter().map(full_roundtrip).collect();
    assert_eq!(types(&parts[0]), ["Sect"]);
    assert_eq!(types(&parts[1]), ["Sect", "P"]);
    for p in &parts {
        assert_parent_tree_consistent(p);
    }
}

/// What a part keeps, for comparing two ways of making it.
fn kept(doc: &Document) -> (Vec<String>, Vec<String>) {
    (types(doc), ids(doc))
}

#[test]
fn split_keeps_the_same_tags_as_extract_of_each_part() {
    // Split reads the tree once and passes over the elements with nothing on a part's pages; a
    // part keeps exactly what Extract Pages of the same pages keeps. Here the link's reference
    // also names the wrong page (/Pg page 1): the annotation is on page 2, which lists it.
    for src in [doc_three(false), doc_three_link_named_on(false, 3)] {
        let n = page_count(&src).unwrap();
        let extractor = Extractor::new(&src);
        for start in 0..n {
            for end in start + 1..=n {
                let pages: Vec<usize> = (start..end).collect();
                let fast = full_roundtrip(&extractor.extract(&pages).unwrap());
                let slow = full_roundtrip(&extract_pages(&src, &pages).unwrap());
                assert_eq!(kept(&fast), kept(&slow), "pages {pages:?}");
                assert_parent_tree_consistent(&fast);
            }
        }
        // Page 2 alone keeps the link, whatever page its reference names.
        let second = full_roundtrip(&extractor.extract(&[1]).unwrap());
        assert!(types(&second).contains(&"Link".to_string()));
    }
}

/// Review on #7333: an object reference to something other than an annotation (an image here)
/// keeps its element in a part wherever its object comes along, whatever page the reference
/// names. The image is in resources page 1 shares with page 2, which draws it, so Extract Pages
/// of page 1 copies it and keeps the figure. The page index can't tell where such an object is
/// copied, so it reads the figure as anywhere, and Split keeps what Extract Pages keeps.
#[test]
fn split_keeps_an_object_reference_wherever_its_object_comes_along() {
    let image = "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /StructParent 1 /Length 1 >>\nstream\nA\nendstream";
    let b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 12 0 R /MarkInfo << /Marked true >> >>".into(), // 1
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >>".into(),               // 2
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources 5 0 R /StructParents 0 >>".into(),     // 3
        "<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Resources 5 0 R >>".into(),                      // 4
        "<< /Font << /F1 9 0 R >> /XObject << /Im0 10 0 R >> >>".into(),                                // 5
        marked(&[("P", 0, "C1")], ""),                                                                  // 6
        marked(&[], "q 10 0 0 10 20 20 cm /Im0 Do Q"),                                                  // 7
        "null".into(),                                                                                  // 8
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),                                // 9
        image.into(),                                                                                   // 10
        "null".into(),                                                                                  // 11
        "<< /Type /StructTreeRoot /K 13 0 R /ParentTree 16 0 R /ParentTreeNextKey 2 >>".into(),         // 12
        "<< /Type /StructElem /S /Document /P 12 0 R /K [14 0 R 15 0 R] >>".into(),                     // 13
        "<< /Type /StructElem /S /P /P 13 0 R /Pg 3 0 R /K 0 >>".into(),                                // 14
        "<< /Type /StructElem /S /Figure /P 13 0 R /Pg 4 0 R /Alt (A dot) /K << /Type /OBJR /Obj 10 0 R >> >>".into(), // 15
        "<< /Nums [0 [14 0 R] 1 15 0 R] >>".into(),                                                     // 16
    ];
    let src = open(build(&b, "/Root 1 0 R"));
    let extractor = Extractor::new(&src);
    for pages in [vec![0], vec![1], vec![0, 1]] {
        let fast = full_roundtrip(&extractor.extract(&pages).unwrap());
        let slow = full_roundtrip(&extract_pages(&src, &pages).unwrap());
        assert_eq!(kept(&fast), kept(&slow), "pages {pages:?}");
        assert!(types(&slow).contains(&"Figure".to_string()), "pages {pages:?}: the image came along");
    }
}

/// Review on #7333: a page taken twice is copied twice, and only its first copy carries the
/// page's tags (the second has no `/StructParents`). A selection repeating a page is then not
/// the whole tagged file, even when it lists every page, and keeps no PDF/UA claim.
#[test]
fn a_repeated_page_keeps_no_pdf_ua_claim() {
    let out = full_roundtrip(&extract_pages(&doc_three(true), &[0, 0, 1, 2]).unwrap());
    assert_eq!(labels(&out), ["P1", "P1", "P2", "P3"]);
    assert!(page_dict(&out, 0).contains(b"StructParents"));
    assert!(!page_dict(&out, 1).contains(b"StructParents"), "the second copy of page 1 is untagged");
    assert_parent_tree_consistent(&out);
    assert!(pdfx_of(&out).xmp.is_none_or(|x| !x.contains("pdfuaid")), "a repeated page drops the PDF/UA claim");
}
