//! Combine keeps the sources' structure trees (#7025).

use std::collections::HashSet;

use super::*;

/// A tagged page's content: its text as marked content `mcid`s.
fn tagged_body(text: &str, mcids: &[i64]) -> String {
    let mut s = String::new();
    for m in mcids {
        s.push_str(&format!("/P <</MCID {m}>> BDC BT /F1 12 Tf 20 20 Td ({text}) Tj ET EMC "));
    }
    format!("<< /Length {} >>\nstream\n{s}\nendstream", s.len())
}

/// A two-page tagged document: a heading (ID `h-<p>`) on its first page, a paragraph across
/// marked content 0 and 1 of its second, and a link on its first page tagged by an object
/// reference. Its role map sends `/Heading` to `heading_role`. `ua`: it claims PDF/UA-1.
fn doc_tagged(p: &str, lang: &str, heading_role: &str, ua: bool) -> Document {
    open(build(&tagged_objects(p, lang, heading_role, ua.then_some("pdfuaid:part=\"1\"")), "/Root 1 0 R /Info 17 0 R"))
}

/// [`doc_tagged`]'s objects (object `n` at `n - 1`), its XMP declaring `ua` (the `pdfuaid`
/// attributes), for a test to change before building.
fn tagged_objects(p: &str, lang: &str, heading_role: &str, ua: Option<&str>) -> Vec<String> {
    let xmp = format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
         <rdf:Description rdf:about=\"\" xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\" {}/></rdf:RDF></x:xmpmeta>",
        ua.unwrap_or_default()
    );
    let metadata = if ua.is_some() { " /Metadata 16 0 R" } else { "" };
    vec![
        format!("<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 9 0 R /MarkInfo << /Marked true >> /Lang ({lang}){metadata} >>"), // 1
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >>".into(),                                         // 2
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 7 0 R >> >> /StructParents 0 /Annots [8 0 R] >>".into(), // 3
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> >> /StructParents 1 >>".into(),         // 4
        tagged_body(&format!("{p}1"), &[0]),                                                                                      // 5
        tagged_body(&format!("{p}2"), &[0, 1]),                                                                                   // 6
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),                                                          // 7
        "<< /Type /Annot /Subtype /Link /Rect [0 0 50 50] /StructParent 2 /A << /S /URI /URI (https://example.com/) >> >>".into(), // 8
        format!(
            "<< /Type /StructTreeRoot /K 10 0 R /ParentTree 14 0 R /ParentTreeNextKey 3 /RoleMap << /Heading /{heading_role} >> /IDTree 15 0 R >>"
        ), // 9
        "<< /Type /StructElem /S /Document /P 9 0 R /K [11 0 R 12 0 R 13 0 R] >>".into(),                                         // 10
        format!("<< /Type /StructElem /S /Heading /P 10 0 R /Pg 3 0 R /K 0 /ID (h-{p}) /Alt (Heading {p}) >>"),                   // 11
        "<< /Type /StructElem /S /P /P 10 0 R /Pg 4 0 R /K [0 << /Type /MCR /Pg 4 0 R /MCID 1 >>] >>".into(),                     // 12
        "<< /Type /StructElem /S /Link /P 10 0 R /Pg 3 0 R /K << /Type /OBJR /Obj 8 0 R >> >>".into(),                            // 13
        "<< /Nums [0 [11 0 R] 1 [12 0 R 12 0 R] 2 13 0 R] >>".into(),                                                             // 14
        format!("<< /Names [(h-{p}) 11 0 R] >>"),                                                                                 // 15
        format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{xmp}\nendstream", xmp.len()),                           // 16
        format!("<< /Title (Part {p}) >>"),                                                                                       // 17
    ]
}

/// The PDF/UA identification in `doc`'s XMP metadata, if any.
fn ua_claim(doc: &Document) -> Option<String> {
    pdfx_of(doc).xmp.filter(|x| x.contains("pdfuaid"))
}

/// The combined tree's top-level `Document` element.
fn document_element(doc: &Document) -> Dict {
    let root = tree_root(doc).expect("a structure tree");
    doc.resolve(root.get(b"K").unwrap()).as_dict().cloned().unwrap()
}

/// Every element of `doc`'s structure tree, depth first.
fn all_elements(doc: &Document) -> Vec<Dict> {
    let mut found = Vec::new();
    let mut stack = vec![document_element(doc)];
    while let Some(e) = stack.pop() {
        stack.extend(kids(doc, &e).into_iter().filter(|k| k.contains(b"S")));
        found.push(e);
    }
    found
}

fn tree_root(doc: &Document) -> Option<Dict> {
    catalog(doc).get(b"StructTreeRoot").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned())
}

/// The parent tree's entry for `key` (a flat `/Nums` array, as Combine writes it).
fn parent_entry(doc: &Document, key: i64) -> Object {
    let root = tree_root(doc).expect("a structure tree");
    let tree = doc.resolve(root.get(b"ParentTree").expect("a parent tree"));
    let nums = doc.resolve(tree.as_dict().unwrap().get(b"Nums").expect("/Nums"));
    let nums = nums.as_array().unwrap();
    let at = nums.chunks(2).position(|p| p[0].as_int() == Some(key)).unwrap_or_else(|| panic!("no parent-tree entry {key}"));
    nums[at * 2 + 1].clone()
}

/// The element that owns marked content `mcid` on page `page`, found as a reader does: the
/// page's `/StructParents`, then the parent tree.
fn owner_of_content(doc: &Document, page: usize, mcid: usize) -> Dict {
    let key = page_dict(doc, page).get(b"StructParents").and_then(Object::as_int).unwrap_or_else(|| panic!("page {page} has no /StructParents"));
    let entry = parent_entry(doc, key);
    let list = doc.resolve(&entry);
    let element = list.as_array().unwrap().get(mcid).cloned().unwrap_or_else(|| panic!("page {page}: no entry for MCID {mcid}"));
    doc.resolve(&element).as_dict().cloned().expect("an element")
}

/// The element that owns annotation `annot` of page `page`, through its `/StructParent`.
fn owner_of_annot(doc: &Document, page: usize, annot: usize) -> Dict {
    let key = annots(doc, page)[annot].get(b"StructParent").and_then(Object::as_int).expect("the annotation's /StructParent");
    doc.resolve(&parent_entry(doc, key)).as_dict().cloned().expect("an element")
}

fn text(d: &Dict, key: &[u8]) -> String {
    d.get(key).and_then(|v| v.as_string()).map(|s| s.to_text()).unwrap_or_default()
}

fn kids(doc: &Document, d: &Dict) -> Vec<Dict> {
    let k = d.get(b"K").map(|k| doc.resolve(k));
    let list = match k.as_deref() {
        Some(Object::Array(a)) => a.clone(),
        Some(other) => vec![other.clone()],
        None => Vec::new(),
    };
    list.iter().filter_map(|k| doc.resolve(k).as_dict().cloned()).collect()
}

#[test]
fn combine_keeps_each_sources_tags_with_their_own_parent_tree_entries() {
    // #7025: the combined file had no structure tree, so it was untagged.
    let (a, b) = (doc_tagged("A", "en-US", "H1", true), doc_tagged("B", "fr-FR", "H1", true));
    let out = full_roundtrip(&combine(&[("a", &a), ("b", &b)]).unwrap());
    assert_eq!(labels(&out), ["A1", "A2", "B1", "B2"]);
    let cat = catalog(&out);
    let marked = cat.get(b"MarkInfo").map(|m| out.resolve(m)).and_then(|m| m.as_dict().and_then(|d| d.get(b"Marked").cloned()));
    assert_eq!(marked, Some(Object::Bool(true)));
    assert_eq!(text(&cat, b"Lang"), "en-US", "the first source's language");

    // One Document element holding each source's tree, in order.
    let root = tree_root(&out).expect("a structure tree");
    let document = out.resolve(root.get(b"K").unwrap()).as_dict().cloned().unwrap();
    assert_eq!(document.name(b"S"), Some(&b"Document"[..]));
    let parts = kids(&out, &document);
    assert_eq!(parts.iter().map(|p| p.name(b"S").unwrap().to_vec()).collect::<Vec<_>>(), [b"Part".to_vec(), b"Part".to_vec()]);
    assert_eq!(text(&parts[0], b"Lang"), "", "the first source is in the document's language");
    assert_eq!(text(&parts[1], b"Lang"), "fr-FR", "the second keeps its own");

    // Each page's content and link lead to their own source's elements.
    let pages = pages(&out).unwrap();
    for (first, p) in [(0, "A"), (2, "B")] {
        let heading = owner_of_content(&out, first, 0);
        assert_eq!((heading.name(b"S"), text(&heading, b"ID")), (Some(&b"Heading"[..]), format!("h-{p}")));
        assert_eq!(text(&heading, b"Alt"), format!("Heading {p}"));
        assert_eq!(heading.get(b"Pg").and_then(Object::as_ref), Some(pages[first].obj));
        let para = owner_of_content(&out, first + 1, 1);
        assert_eq!(para.name(b"S"), Some(&b"P"[..]));
        assert_eq!(para.get(b"Pg").and_then(Object::as_ref), Some(pages[first + 1].obj));
        let link = owner_of_annot(&out, first, 0);
        assert_eq!(link.name(b"S"), Some(&b"Link"[..]));
        let objr = kids(&out, &link).pop().unwrap();
        assert_eq!(
            objr.get(b"Obj").and_then(Object::as_ref),
            page_dict(&out, first).get(b"Annots").and_then(|a| out.resolve(a).as_array().unwrap()[0].as_ref())
        );
    }
    let keys: Vec<i64> = (0..4).map(|i| page_dict(&out, i).get(b"StructParents").and_then(Object::as_int).unwrap()).collect();
    assert_eq!(keys.iter().collect::<HashSet<_>>().len(), 4, "no two pages share a key: {keys:?}");
    assert!(root.get(b"ParentTreeNextKey").and_then(Object::as_int).unwrap() > *keys.iter().max().unwrap());

    // The role maps merged; both IDs listed.
    let role_map = out.resolve(root.get(b"RoleMap").unwrap()).as_dict().cloned().unwrap();
    assert_eq!(role_map.name(b"Heading"), Some(&b"H1"[..]));
    let ids = out.resolve(root.get(b"IDTree").unwrap()).as_dict().cloned().unwrap();
    let names = ids.get(b"Names").unwrap().as_array().unwrap();
    assert_eq!(names.iter().filter_map(|n| n.as_string().map(|s| s.to_text())).collect::<Vec<_>>(), ["h-A", "h-B"]);

    // Every source claims PDF/UA-1 and came along whole: so does the result, with a title.
    let xmp = pdfx_of(&out).xmp.expect("XMP metadata");
    assert!(xmp.contains("<pdfuaid:part>1</pdfuaid:part>"), "{xmp}");
    assert_eq!(info(&out, "Title").as_deref(), Some("Part A"));
}

#[test]
fn combine_grouped_keeps_tags_too() {
    let (a, b) = (doc_tagged("A", "en-US", "H1", false), doc_tagged("B", "en-US", "H1", false));
    let runs: [Run<'_>; 3] = [(0, "a", &a, Some(&[0])), (1, "b", &b, None), (0, "a", &a, Some(&[1]))];
    let out = full_roundtrip(&combine_grouped(&runs).unwrap());
    assert_eq!(labels(&out), ["A1", "B1", "B2", "A2"]);
    assert_eq!(text(&owner_of_content(&out, 0, 0), b"ID"), "h-A");
    assert_eq!(text(&owner_of_content(&out, 1, 0), b"ID"), "h-B");
    assert_eq!(owner_of_content(&out, 3, 1).name(b"S"), Some(&b"P"[..]));
    assert!(pdfx_of(&out).xmp.is_none(), "no source claims PDF/UA");
}

#[test]
fn combine_with_an_untagged_source_tags_only_the_tagged_pages() {
    let a = doc_tagged("A", "en-US", "H1", true);
    let out = full_roundtrip(&combine(&[("a", &a), ("b", &doc_b())]).unwrap());
    assert_eq!(labels(&out), ["A1", "A2", "B1", "B2"]);
    assert_eq!(text(&owner_of_content(&out, 0, 0), b"ID"), "h-A");
    for untagged in [2, 3] {
        assert!(!page_dict(&out, untagged).contains(b"StructParents"), "page {untagged} stays untagged");
    }
    // Not every source claims PDF/UA (nor is tagged): the result claims none.
    assert!(pdfx_of(&out).xmp.is_none_or(|x| !x.contains("pdfuaid")));
}

#[test]
fn combine_of_some_pages_keeps_only_their_tags_and_no_pdfua_claim() {
    let a = doc_tagged("A", "en-US", "H1", true);
    let out = full_roundtrip(&combine_selected(&[("a", &a, Some(&[1])), ("b", &a, None)]).unwrap());
    assert_eq!(labels(&out), ["A2", "A1", "A2"]);
    assert_eq!(owner_of_content(&out, 0, 0).name(b"S"), Some(&b"P"[..]));
    let root = tree_root(&out).unwrap();
    let document = out.resolve(root.get(b"K").unwrap()).as_dict().cloned().unwrap();
    let parts = kids(&out, &document);
    // The first part holds only the paragraph: the heading and link were on the page left out.
    assert_eq!(kids(&out, &parts[0]).iter().map(|e| e.name(b"S").unwrap().to_vec()).collect::<Vec<_>>(), [b"P".to_vec()]);
    assert_eq!(kids(&out, &parts[1]).len(), 3);
    assert!(pdfx_of(&out).xmp.is_none_or(|x| !x.contains("pdfuaid")), "a part of a PDF/UA file is not claimed to be one");
}

#[test]
fn combine_of_untagged_sources_adds_no_tags() {
    // doc_a's third page has a stray /StructParents with no tree behind it.
    let out = full_roundtrip(&combine(&[("a", &doc_a()), ("b", &doc_b())]).unwrap());
    let cat = catalog(&out);
    assert!(!cat.contains(b"StructTreeRoot") && !cat.contains(b"MarkInfo") && !cat.contains(b"Lang"));
    for i in 0..5 {
        assert!(!page_dict(&out, i).contains(b"StructParents"));
    }
}

#[test]
fn combine_of_a_hostile_structure_tree_stays_bounded() {
    // A tree whose elements cycle (10 → 11 → 10, 12 lists itself), whose marked-content IDs are
    // negative or huge, whose kids are junk, and a chain of elements deeper than any real tree.
    const CHAIN: usize = 2000;
    let mut b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 9 0 R /MarkInfo << /Marked true >> >>".into(), // 1
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),                    // 2
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /StructParents 0 >>".into(), // 3
        tagged_body("H1", &[0]),                                                                       // 4
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),                               // 5
        "null".into(),                                                                                 // 6
        "null".into(),                                                                                 // 7
        "null".into(),                                                                                 // 8
        "<< /Type /StructTreeRoot /K [10 0 R 12 0 R 13 0 R 9 0 R (junk) 99 0 R] /ParentTree 9 0 R /RoleMap 9 0 R >>".into(), // 9
        "<< /S /Sect /P 9 0 R /Pg 3 0 R /K [11 0 R 0 -5 99999999999 << /Type /MCR /MCID 7 /Pg 9 0 R >>] /A 9 0 R >>".into(), // 10
        "<< /S /P /P 10 0 R /Pg 3 0 R /K [10 0 R 0] >>".into(),                                        // 11
        "<< /S /Div /P 9 0 R /K [12 0 R << /Type /OBJR /Obj 1 0 R >>] >>".into(),                      // 12
        "<< /S /Div /P 9 0 R /Pg 3 0 R /K 14 0 R >>".into(),                                           // 13
    ];
    for i in 0..CHAIN {
        let n = 14 + i;
        let k = if i + 1 == CHAIN { "0".to_string() } else { format!("{} 0 R", n + 1) };
        b.push(format!("<< /S /Div /P {} 0 R /Pg 3 0 R /K {k} >>", n - 1));
    }
    let src = open(build(&b, "/Root 1 0 R"));
    let out = full_roundtrip(&combine(&[("x", &src), ("y", &doc_b())]).unwrap());
    assert_eq!(labels(&out), ["H1", "B1", "B2"]);
    let root = tree_root(&out).expect("what could be carried is");
    let document = out.resolve(root.get(b"K").unwrap()).as_dict().cloned().unwrap();
    // The section is kept once, with its paragraph (the cycle back to it cut), and the junk is not.
    let parts = kids(&out, &document);
    assert_eq!(parts[0].name(b"S"), Some(&b"Sect"[..]));
    let owner = owner_of_content(&out, 0, 0);
    assert!(matches!(owner.name(b"S"), Some(b"Sect" | b"P" | b"Div")));
    // The chain is cut off well before its end: walking the copy down stays shallow.
    let mut depth = 0;
    let mut at = parts.iter().find(|p| p.name(b"S") == Some(b"Div") && p.contains(b"Pg")).cloned();
    while let Some(e) = at {
        depth += 1;
        assert!(depth <= 300, "the carried chain is bounded");
        at = kids(&out, &e).into_iter().find(|k| k.contains(b"S"));
    }
    // The parent tree's arrays are sized by real IDs, not by the huge one.
    let nums = out.resolve(out.resolve(root.get(b"ParentTree").unwrap()).as_dict().unwrap().get(b"Nums").unwrap());
    for entry in nums.as_array().unwrap().chunks(2) {
        assert!(out.resolve(&entry[1]).as_array().is_none_or(|a| a.len() < 16));
    }
}

#[test]
fn combine_with_a_page_listed_twice_claims_no_pdfua() {
    // Only a page's first copy is tagged: the second carries untagged content, so the result
    // can't claim PDF/UA, by the Combiner or by grouped runs.
    let a = doc_tagged("A", "en-US", "H1", true);
    let listed = full_roundtrip(&combine_selected(&[("a", &a, Some(&[0, 1, 0]))]).unwrap());
    let runs: [Run<'_>; 2] = [(0, "a", &a, Some(&[0])), (0, "a", &a, Some(&[1, 0]))];
    let grouped = full_roundtrip(&combine_grouped(&runs).unwrap());
    for out in [listed, grouped] {
        assert_eq!(labels(&out), ["A1", "A2", "A1"]);
        assert_eq!(text(&owner_of_content(&out, 0, 0), b"ID"), "h-A");
        assert_eq!(owner_of_content(&out, 1, 1).name(b"S"), Some(&b"P"[..]));
        assert!(!page_dict(&out, 2).contains(b"StructParents"), "the repeat is the untagged copy");
        assert_eq!(ua_claim(&out), None);
    }
    // Each page once, the claim stands.
    let once = full_roundtrip(&combine_selected(&[("a", &a, Some(&[0, 1]))]).unwrap());
    assert!(ua_claim(&once).is_some());
}

#[test]
fn combine_grouped_claims_pdfua_only_when_each_files_tags_follow_its_pages() {
    let (a, b) = (doc_tagged("A", "en-US", "H1", true), doc_tagged("B", "en-US", "H1", true));
    // B's pages come between A's: each file's tags stay together, so A2 is read before B1.
    let split: [Run<'_>; 3] = [(0, "a", &a, Some(&[0])), (1, "b", &b, None), (0, "a", &a, Some(&[1]))];
    let out = full_roundtrip(&combine_grouped(&split).unwrap());
    assert_eq!(labels(&out), ["A1", "B1", "B2", "A2"]);
    let parts = kids(&out, &document_element(&out));
    let headings: Vec<String> = parts.iter().map(|p| text(&kids(&out, p)[0], b"ID")).collect();
    assert_eq!(headings, ["h-A", "h-B"], "the tags are in file order");
    assert_eq!(ua_claim(&out), None, "the tags no longer follow the pages");
    // A's runs next to each other: the tags follow the pages, and the claim stands.
    let together: [Run<'_>; 3] = [(0, "a", &a, Some(&[0])), (0, "a", &a, Some(&[1])), (1, "b", &b, None)];
    let out = full_roundtrip(&combine_grouped(&together).unwrap());
    assert_eq!(labels(&out), ["A1", "A2", "B1", "B2"]);
    assert!(ua_claim(&out).is_some_and(|x| x.contains("<pdfuaid:part>1</pdfuaid:part>")));
}

#[test]
fn combine_keeps_the_pdfua_2_revision() {
    let build_ua = |p: &str, ua: &str| open(build(&tagged_objects(p, "en-US", "H1", Some(ua)), "/Root 1 0 R /Info 17 0 R"));
    let ua2 = "pdfuaid:part=\"2\" pdfuaid:rev=\"2024\"";
    let (a, b) = (build_ua("A", ua2), build_ua("B", ua2));
    let xmp = ua_claim(&combine(&[("a", &a), ("b", &b)]).unwrap()).expect("a PDF/UA-2 claim");
    assert!(xmp.contains("<pdfuaid:part>2</pdfuaid:part>") && xmp.contains("<pdfuaid:rev>2024</pdfuaid:rev>"), "{xmp}");
    // Another revision of the part is another claim: the sources don't agree.
    let c = build_ua("C", "pdfuaid:part=\"2\" pdfuaid:rev=\"2023\"");
    assert_eq!(ua_claim(&combine(&[("a", &a), ("c", &c)]).unwrap()), None);
}

#[test]
fn combine_gives_each_element_its_own_id() {
    // Two files numbering their IDs the same way: the second's element gets a new ID, the ID
    // tree names each element by its own, and the table headers that named it follow.
    let mut objs = tagged_objects("A", "en-US", "H1", Some("pdfuaid:part=\"1\""));
    objs[11] =
        "<< /Type /StructElem /S /P /P 10 0 R /Pg 4 0 R /K [0 << /Type /MCR /Pg 4 0 R /MCID 1 >>] /A << /O /Table /Headers [(h-A)] >> >>".into();
    let a = open(build(&objs, "/Root 1 0 R /Info 17 0 R"));
    let out = full_roundtrip(&combine(&[("a", &a), ("again", &a)]).unwrap());
    let (first, second) = (owner_of_content(&out, 0, 0), owner_of_content(&out, 2, 0));
    assert_eq!((text(&first, b"ID"), text(&second, b"ID")), ("h-A".to_string(), "h-A-2".to_string()));
    let ids = out.resolve(tree_root(&out).unwrap().get(b"IDTree").unwrap()).as_dict().cloned().unwrap();
    let names = ids.get(b"Names").unwrap().as_array().unwrap().clone();
    assert_eq!(names.len(), 4);
    for pair in names.chunks(2) {
        let id = pair[0].as_string().unwrap().to_text();
        assert_eq!(text(out.resolve(&pair[1]).as_dict().unwrap(), b"ID"), id, "the ID tree names {id}'s own element");
    }
    let headers = |page| {
        let para = owner_of_content(&out, page, 1);
        let attrs = out.resolve(para.get(b"A").unwrap()).as_dict().cloned().unwrap();
        attrs.get(b"Headers").unwrap().as_array().unwrap().iter().map(|h| h.as_string().unwrap().to_text()).collect::<Vec<_>>()
    };
    assert_eq!(headers(1), ["h-A"]);
    assert_eq!(headers(3), ["h-A-2"]);
}

#[test]
fn combine_keeps_mark_info_user_properties() {
    let mut objs = tagged_objects("A", "en-US", "H1", None);
    objs[0] = "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 9 0 R /MarkInfo << /Marked true /UserProperties true >> /Lang (en-US) >>".into();
    let a = open(build(&objs, "/Root 1 0 R /Info 17 0 R"));
    let mark_info = |out: &Document| out.resolve(catalog(out).get(b"MarkInfo").unwrap()).as_dict().cloned().unwrap();
    let out = full_roundtrip(&combine(&[("b", &doc_tagged("B", "en-US", "H1", false)), ("a", &a)]).unwrap());
    assert_eq!(mark_info(&out).get(b"UserProperties"), Some(&Object::Bool(true)));
    // Without it, it isn't added.
    let out = combine(&[("b", &doc_tagged("B", "en-US", "H1", false))]).unwrap();
    assert!(!mark_info(&out).contains(b"UserProperties"));
}

#[test]
fn combine_shares_one_copy_of_a_value_class_map_entries_share() {
    // Many class names on one attribute array: copied once, not once per name.
    let mut objs = tagged_objects("A", "en-US", "H1", None);
    let classes: String = (0..50).map(|i| format!("/C{i} 18 0 R ")).collect();
    objs[8] = format!("<< /Type /StructTreeRoot /K 10 0 R /ParentTree 14 0 R /ParentTreeNextKey 3 /ClassMap << {classes}>> /IDTree 15 0 R >>");
    objs.push("[<< /O /Layout /Placement /Block >> << /O /Layout /SpaceBefore 4 >>]".into()); // 18
    let a = open(build(&objs, "/Root 1 0 R /Info 17 0 R"));
    let out = combine(&[("a", &a)]).unwrap();
    let class_map = out.resolve(tree_root(&out).unwrap().get(b"ClassMap").unwrap()).as_dict().cloned().unwrap();
    let copies: HashSet<_> = class_map.iter().map(|(_, v)| v.as_ref().expect("a reference to the copy")).collect();
    assert_eq!(class_map.len(), 50);
    assert_eq!(copies.len(), 1, "one copy for every name");
}

#[test]
fn combine_drops_an_object_reference_to_a_page_left_out() {
    // A form XObject both pages use; the figure names its rendering on the second page only.
    let mut objs = tagged_objects("A", "en-US", "H1", None);
    objs[2] = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 7 0 R >> /XObject << /Fx 18 0 R >> >> /StructParents 0 /Annots [8 0 R] >>".into();
    objs[3] =
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> /XObject << /Fx 18 0 R >> >> /StructParents 1 >>".into();
    objs[9] = "<< /Type /StructElem /S /Document /P 9 0 R /K [11 0 R 12 0 R 13 0 R 19 0 R] >>".into();
    objs.push("<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length 0 >>\nstream\n\nendstream".into()); // 18
    objs.push("<< /Type /StructElem /S /Figure /P 10 0 R /Alt (A figure) /K << /Type /OBJR /Obj 18 0 R /Pg 4 0 R >> >>".into()); // 19
    let a = open(build(&objs, "/Root 1 0 R /Info 17 0 R"));
    let out = full_roundtrip(&combine_selected(&[("a", &a, Some(&[0]))]).unwrap());
    let kept: Vec<Vec<u8>> = all_elements(&out).iter().filter_map(|e| e.name(b"S").map(<[u8]>::to_vec)).collect();
    assert!(kept.contains(&b"Heading".to_vec()));
    assert!(!kept.contains(&b"Figure".to_vec()), "the figure went with its page: {kept:?}");
    // The whole file keeps it, with its page.
    let out = full_roundtrip(&combine(&[("a", &a)]).unwrap());
    let figure = all_elements(&out).into_iter().find(|e| e.name(b"S") == Some(b"Figure")).expect("the figure");
    let objr = kids(&out, &figure).pop().unwrap();
    assert_eq!(objr.get(b"Pg").and_then(Object::as_ref), Some(pages(&out).unwrap()[1].obj));
}

#[test]
fn combine_bounds_the_parent_tree_across_pages_and_sources() {
    // Each page has one piece of content with the highest ID carried: a few entries in the file,
    // an array of a million entries for each page. The arrays together stay within the budget.
    const HIGH: i64 = 1 << 20;
    let mut b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 9 0 R /MarkInfo << /Marked true >> >>".into(), // 1
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >>".into(),        // 2
    ];
    for contents in [6, 13, 14] {
        b.push(format!("<< /Type /Page /Parent 2 0 R /Contents {contents} 0 R /Resources << /Font << /F1 7 0 R >> >> >>")); // 3, 4, 5
    }
    b.push(tagged_body("P1", &[HIGH])); // 6
    b.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into()); // 7
    b.push("null".into()); // 8
    b.push("<< /Type /StructTreeRoot /K [10 0 R 11 0 R 12 0 R] >>".into()); // 9
    for page in 3..6 {
        b.push(format!("<< /Type /StructElem /S /P /P 9 0 R /Pg {page} 0 R /K {HIGH} >>")); // 10, 11, 12
    }
    b.push(tagged_body("P2", &[HIGH])); // 13
    b.push(tagged_body("P3", &[HIGH])); // 14
    let src = open(build(&b, "/Root 1 0 R"));
    let out = combine(&[("x", &src), ("y", &src)]).unwrap();
    assert_eq!(labels(&out), ["P1", "P2", "P3", "P1", "P2", "P3"]);
    let tree = out.resolve(tree_root(&out).unwrap().get(b"ParentTree").unwrap());
    let nums = out.resolve(tree.as_dict().unwrap().get(b"Nums").unwrap());
    let slots: usize = nums.as_array().unwrap().chunks(2).map(|e| out.resolve(&e[1]).as_array().map_or(0, |a| a.len())).sum();
    assert!(slots <= 1 << 22, "{slots} parent-tree entries");
    // The first source fits; the second's content is past the budget and stays untagged.
    for page in 0..3 {
        assert!(page_dict(&out, page).contains(b"StructParents"), "page {page}");
        assert!(!page_dict(&out, page + 3).contains(b"StructParents"), "page {}", page + 3);
    }
}

/// `objs` (as [`tagged_objects`] gives them), built.
fn build_tagged(objs: &[String]) -> Document {
    open(build(objs, "/Root 1 0 R /Info 17 0 R"))
}

#[test]
fn combine_claims_pdfua_only_for_a_files_pages_once_in_their_own_order() {
    // The tags keep the file's order: with its pages reordered, page 2's content is read first.
    let a = doc_tagged("A", "en-US", "H1", true);
    let reordered = full_roundtrip(&combine_selected(&[("a", &a, Some(&[1, 0]))]).unwrap());
    assert_eq!(labels(&reordered), ["A2", "A1"]);
    assert_eq!(ua_claim(&reordered), None, "Combiner, [1, 0]");
    // Grouped: one run in another order, or two runs next to each other in another order.
    let one: [Run<'_>; 1] = [(0, "a", &a, Some(&[1, 0]))];
    let two: [Run<'_>; 2] = [(0, "a", &a, Some(&[1])), (0, "a", &a, Some(&[0]))];
    for runs in [&one[..], &two[..]] {
        let out = full_roundtrip(&combine_grouped(runs).unwrap());
        assert_eq!(labels(&out), ["A2", "A1"]);
        assert_eq!(ua_claim(&out), None, "grouped, [1, 0]");
    }
    // In order, the claim stands on both paths.
    let in_order: [Run<'_>; 2] = [(0, "a", &a, Some(&[0])), (0, "a", &a, Some(&[1]))];
    assert!(ua_claim(&combine_grouped(&in_order).unwrap()).is_some());
    assert!(ua_claim(&combine_selected(&[("a", &a, Some(&[0, 1]))]).unwrap()).is_some());
}

#[test]
fn combine_grouped_ignores_runs_with_no_pages() {
    let (a, b) = (doc_tagged("A", "en-US", "H1", true), doc_tagged("B", "en-US", "H1", true));
    // B's empty run shows nothing between A's runs: the pages and the tags both go A then B.
    let empty: [Run<'_>; 4] = [(0, "a", &a, Some(&[0])), (1, "b", &b, Some(&[])), (0, "a", &a, Some(&[1])), (1, "b", &b, Some(&[0, 1]))];
    let out = full_roundtrip(&combine_grouped(&empty).unwrap());
    assert_eq!(labels(&out), ["A1", "A2", "B1", "B2"]);
    assert!(ua_claim(&out).is_some(), "an empty run doesn't split a file");
    // B's empty run first: B is copied first, so its tags come before A's, but its pages after.
    let first: [Run<'_>; 3] = [(1, "b", &b, Some(&[])), (0, "a", &a, None), (1, "b", &b, None)];
    let out = full_roundtrip(&combine_grouped(&first).unwrap());
    assert_eq!(labels(&out), ["A1", "A2", "B1", "B2"]);
    assert_eq!(ua_claim(&out), None, "the tags don't follow the pages");
}

#[test]
fn combine_drops_the_pdfua_claim_when_a_role_means_something_else() {
    // A maps /Heading to /H1, B to /H2: the first stands, so B's headings become H1s.
    let (a, b) = (doc_tagged("A", "en-US", "H1", true), doc_tagged("B", "en-US", "H2", true));
    let out = full_roundtrip(&combine(&[("a", &a), ("b", &b)]).unwrap());
    let role_map = out.resolve(tree_root(&out).unwrap().get(b"RoleMap").unwrap()).as_dict().cloned().unwrap();
    assert_eq!(role_map.name(b"Heading"), Some(&b"H1"[..]));
    assert_eq!(ua_claim(&out), None);
    // The same mapping in both: the claim stands.
    let same = doc_tagged("B", "en-US", "H1", true);
    assert!(ua_claim(&combine(&[("a", &a), ("b", &same)]).unwrap()).is_some());
}

/// [`tagged_objects`] in the PDF 2.0 namespace (object 18), with a MathML namespace (19) listed
/// too, claiming PDF/UA-2; its `Document` element in the namespace `document_ns`.
fn namespaced_objects(p: &str, document_ns: u32) -> Vec<String> {
    let mut objs = tagged_objects(p, "en-US", "H1", Some("pdfuaid:part=\"2\" pdfuaid:rev=\"2024\""));
    objs[8] = "<< /Type /StructTreeRoot /K 10 0 R /ParentTree 14 0 R /ParentTreeNextKey 3 /IDTree 15 0 R /Namespaces [18 0 R 19 0 R] >>".into();
    objs[9] = format!("<< /Type /StructElem /S /Document /NS {document_ns} 0 R /P 9 0 R /K [11 0 R 12 0 R 13 0 R] >>");
    objs[10] = format!("<< /Type /StructElem /S /H1 /NS 18 0 R /P 10 0 R /Pg 3 0 R /K 0 /ID (h-{p}) >>");
    objs[11] = "<< /Type /StructElem /S /P /NS 18 0 R /P 10 0 R /Pg 4 0 R /K [0 << /Type /MCR /Pg 4 0 R /MCID 1 >>] >>".into();
    objs[12] = "<< /Type /StructElem /S /Link /NS 18 0 R /P 10 0 R /Pg 3 0 R /K << /Type /OBJR /Obj 8 0 R >> >>".into();
    objs.push("<< /Type /Namespace /NS (http://iso.org/pdf2/ssn) >>".into()); // 18
    objs.push("<< /Type /Namespace /NS (http://www.w3.org/1998/Math/MathML) >>".into()); // 19
    objs
}

#[test]
fn combine_carries_the_sources_namespaces() {
    let (a, b) = (build_tagged(&namespaced_objects("A", 18)), build_tagged(&namespaced_objects("B", 18)));
    let out = full_roundtrip(&combine(&[("a", &a), ("b", &b)]).unwrap());
    let root = tree_root(&out).unwrap();
    let listed: HashSet<ObjRef> =
        out.resolve(root.get(b"Namespaces").expect("/Namespaces")).as_array().unwrap().iter().filter_map(Object::as_ref).collect();
    assert_eq!(listed.len(), 4, "each source's two namespaces");
    for e in all_elements(&out) {
        let ns = e.get(b"NS").and_then(Object::as_ref).expect("every element in a namespace");
        assert!(listed.contains(&ns), "{:?} names a namespace the root lists", e.name(b"S"));
        assert_eq!(text(out.resolve(&Object::Ref(ns)).as_dict().unwrap(), b"NS"), "http://iso.org/pdf2/ssn");
    }
    assert_eq!(document_element(&out).name(b"S"), Some(&b"Document"[..]));
    let xmp = ua_claim(&out).expect("the PDF/UA-2 claim stands");
    assert!(xmp.contains("<pdfuaid:part>2</pdfuaid:part>"), "{xmp}");
    // A source's Document in another namespace can't become a Part there: no claim.
    let math = build_tagged(&namespaced_objects("B", 19));
    assert_eq!(ua_claim(&combine(&[("a", &a), ("b", &math)]).unwrap()), None);
}

#[test]
fn combine_carries_a_marked_content_id_held_indirectly() {
    let mut objs = tagged_objects("A", "en-US", "H1", None);
    objs[10] = "<< /Type /StructElem /S /Heading /P 10 0 R /Pg 3 0 R /K 18 0 R /ID (h-A) >>".into();
    objs.push("0".into()); // 18
    let out = full_roundtrip(&combine(&[("a", &build_tagged(&objs))]).unwrap());
    assert_eq!(text(&owner_of_content(&out, 0, 0), b"ID"), "h-A");
}

#[test]
fn combine_shares_one_copy_of_a_ref_array_elements_share() {
    let mut objs = tagged_objects("A", "en-US", "H1", None);
    objs[10] = "<< /Type /StructElem /S /Heading /P 10 0 R /Pg 3 0 R /K 0 /ID (h-A) /Ref 18 0 R >>".into();
    objs[11] = "<< /Type /StructElem /S /P /P 10 0 R /Pg 4 0 R /K [0 << /Type /MCR /Pg 4 0 R /MCID 1 >>] /Ref 18 0 R >>".into();
    objs.push("[13 0 R]".into()); // 18
    let out = combine(&[("a", &build_tagged(&objs))]).unwrap();
    let (heading, para) = (owner_of_content(&out, 0, 0), owner_of_content(&out, 1, 1));
    let shared = heading.get(b"Ref").and_then(Object::as_ref).expect("an indirect /Ref");
    assert_eq!(para.get(b"Ref").and_then(Object::as_ref), Some(shared), "one copy for both");
    let named = out.resolve(&Object::Ref(shared)).as_array().unwrap().clone();
    assert_eq!(out.resolve(&named[0]).as_dict().unwrap().name(b"S"), Some(&b"Link"[..]));
}

#[test]
fn combine_renames_headers_in_an_indirect_attribute_array() {
    let mut objs = tagged_objects("A", "en-US", "H1", None);
    objs[11] = "<< /Type /StructElem /S /P /P 10 0 R /Pg 4 0 R /K [0 << /Type /MCR /Pg 4 0 R /MCID 1 >>] /A 18 0 R >>".into();
    objs.push("[<< /O /Table /Headers [(h-A)] >> /Unrelated]".into()); // 18
    let a = build_tagged(&objs);
    let out = full_roundtrip(&combine(&[("a", &a), ("again", &a)]).unwrap());
    let attrs = |page| {
        let held = out.resolve(owner_of_content(&out, page, 1).get(b"A").unwrap()).as_array().cloned().unwrap();
        let headers = out.resolve(&held[0]).as_dict().unwrap().get(b"Headers").unwrap().as_array().unwrap().clone();
        (headers.iter().map(|h| h.as_string().unwrap().to_text()).collect::<Vec<_>>(), held[1].clone())
    };
    assert_eq!(attrs(1).0, ["h-A"]);
    assert_eq!(attrs(3), (vec!["h-A-2".to_string()], Object::name("Unrelated")));
}

#[test]
fn combine_drops_the_pdfua_claim_when_attributes_are_too_deep_to_copy() {
    let mut objs = tagged_objects("A", "en-US", "H1", Some("pdfuaid:part=\"1\""));
    let deep = format!("{}{}", "[".repeat(40), "]".repeat(40));
    objs[11] =
        format!("<< /Type /StructElem /S /P /P 10 0 R /Pg 4 0 R /K [0 << /Type /MCR /Pg 4 0 R /MCID 1 >>] /A << /O /UserProperties /P {deep} >> >>");
    assert_eq!(ua_claim(&combine(&[("a", &build_tagged(&objs))]).unwrap()), None);
    // Shallow attributes copy whole, and the claim stands.
    objs[11] =
        "<< /Type /StructElem /S /P /P 10 0 R /Pg 4 0 R /K [0 << /Type /MCR /Pg 4 0 R /MCID 1 >>] /A << /O /UserProperties /P [[[]]] >> >>".into();
    assert!(ua_claim(&combine(&[("a", &build_tagged(&objs))]).unwrap()).is_some());
}

#[test]
fn combine_reads_the_pdfua_part_with_spaces_around_the_equals_sign() {
    let objs = tagged_objects("A", "en-US", "H1", Some("pdfuaid:part = \"1\""));
    let xmp = ua_claim(&combine(&[("a", &build_tagged(&objs))]).unwrap()).expect("the claim is read");
    assert!(xmp.contains("<pdfuaid:part>1</pdfuaid:part>"), "{xmp}");
}

#[test]
fn combine_does_not_claim_pdfua_2_without_a_revision() {
    let a = build_tagged(&tagged_objects("A", "en-US", "H1", Some("pdfuaid:part=\"2\"")));
    assert_eq!(ua_claim(&combine(&[("a", &a), ("again", &a)]).unwrap()), None);
    // With one, it stands.
    let a = build_tagged(&tagged_objects("A", "en-US", "H1", Some("pdfuaid:part=\"2\" pdfuaid:rev=\"2024\"")));
    assert!(ua_claim(&combine(&[("a", &a), ("again", &a)]).unwrap()).is_some());
}
