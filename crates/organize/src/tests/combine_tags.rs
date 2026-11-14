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
    let xmp = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
               <rdf:Description rdf:about=\"\" xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\" pdfuaid:part=\"1\"/></rdf:RDF></x:xmpmeta>";
    let metadata = if ua { " /Metadata 16 0 R" } else { "" };
    let b: Vec<String> = vec![
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
    ];
    open(build(&b, "/Root 1 0 R /Info 17 0 R"))
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
    let (a, b) = (doc_tagged("A", "en-US", "H1", true), doc_tagged("B", "fr-FR", "H2", true));
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

    // The role maps merged, the first source's mapping standing; both IDs listed.
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
