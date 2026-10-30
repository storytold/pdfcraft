use super::*;
use pdfcraft_cos::{Dict, Document as CosDocument, ObjRef, Object, PdfString, Stream};

fn tagged_text_fixture(structure_actual_text: bool) -> Vec<u8> {
    let mut cos = CosDocument::open(Arc::new(fixture(1))).unwrap();
    let page = ObjRef::new(4, 0);
    let properties = if structure_actual_text { "/MCID 0" } else { "/MCID 0 /ActualText (CORRECT LOGICAL TEXT)" };
    let content = format!(
        "/P << {properties} >> BDC BT /F1 12 Tf 20 220 Td (WRONG OCR TEXT) Tj ET EMC \
         BT /F1 12 Tf 20 100 Td (UNCHANGED NEIGHBOR) Tj ET"
    );
    cos.set(ObjRef::new(5, 0), Stream::from_raw(Dict::new(), content.into_bytes()));

    let mut element = Dict::new();
    element.set("Type", Object::name("StructElem"));
    element.set("S", Object::name("P"));
    element.set("Pg", Object::Ref(page));
    element.set("K", Object::Int(0));
    if structure_actual_text {
        element.set("ActualText", Object::String(PdfString::text("CORRECT LOGICAL TEXT")));
    }
    let element = cos.add(element);
    let mut parent_tree = Dict::new();
    parent_tree.set("Nums", Object::Array(vec![Object::Int(0), Object::Array(vec![Object::Ref(element)])]));
    let parent_tree = cos.add(parent_tree);
    let mut root = Dict::new();
    root.set("Type", Object::name("StructTreeRoot"));
    root.set("K", Object::Array(vec![Object::Ref(element)]));
    root.set("ParentTree", Object::Ref(parent_tree));
    let root = cos.add(root);
    cos.update_dict(element, |d| d.set("P", Object::Ref(root))).unwrap();
    cos.update_dict(page, |d| d.set("StructParents", Object::Int(0))).unwrap();
    let mut mark_info = Dict::new();
    mark_info.set("Marked", Object::Bool(true));
    cos.update_dict(cos.root().unwrap(), |d| {
        d.set("StructTreeRoot", Object::Ref(root));
        d.set("MarkInfo", Object::Dict(mark_info));
    })
    .unwrap();
    pdfcraft_cos::write_full(&cos, &Default::default()).unwrap()
}

fn assert_export_uses_logical_text(structure_actual_text: bool) {
    let bytes = Arc::new(tagged_text_fixture(structure_actual_text));
    let mut session = Session::new();
    let id = session.open("tagged.pdf", None, bytes, None).unwrap();
    let doc = session.get(id).unwrap();
    let before = doc.bytes.clone();
    let html = String::from_utf8(doc.export_office(compare::OfficeFormat::Html)).unwrap();
    assert_eq!(html.matches("CORRECT LOGICAL TEXT").count(), 1, "{html}");
    assert!(!html.contains("WRONG OCR TEXT"), "{html}");
    assert_eq!(html.matches("UNCHANGED NEIGHBOR").count(), 1, "{html}");
    assert_eq!(doc.bytes, before);
    assert!(!doc.dirty);
    assert!(doc.can_undo().is_none() && doc.can_redo().is_none());
}

#[test]
fn office_structure_actual_text_replaces_its_glyph_text_in_export() {
    assert_export_uses_logical_text(true);
}

#[test]
fn office_structure_actual_text_inline_control_keeps_logical_text() {
    assert_export_uses_logical_text(false);
}

fn new_cos() -> CosDocument {
    CosDocument::open(Arc::new(fixture(1))).unwrap()
}

fn set_content(cos: &mut CosDocument, content: impl Into<Vec<u8>>) {
    cos.set(ObjRef::new(5, 0), Stream::from_raw(Dict::new(), content.into()));
}

fn add_form(cos: &mut CosDocument, body: &[u8], struct_parents: Option<i64>) -> ObjRef {
    let mut dict = Dict::new();
    dict.set("Type", Object::name("XObject"));
    dict.set("Subtype", Object::name("Form"));
    dict.set("BBox", Object::Array(vec![0.into(), 0.into(), 200.into(), 300.into()]));
    if let Some(key) = struct_parents {
        dict.set("StructParents", Object::Int(key));
    }
    cos.add(Stream::from_raw(dict, body.to_vec()))
}

fn add_form_to_page(cos: &mut CosDocument, form: ObjRef, name: &str) {
    let page = ObjRef::new(4, 0);
    let mut page_dict = cos.get(page).as_dict().cloned().unwrap();
    let mut resources = page_dict.get(b"Resources").and_then(Object::as_dict).cloned().unwrap_or_default();
    let mut xobjects = resources.get(b"XObject").and_then(Object::as_dict).cloned().unwrap_or_default();
    xobjects.set(name, Object::Ref(form));
    resources.set("XObject", Object::Dict(xobjects));
    page_dict.set("Resources", Object::Dict(resources));
    cos.set(page, Object::Dict(page_dict));
}

fn add_tag(cos: &mut CosDocument, page: ObjRef, kids: Object, actual_text: Option<&str>) -> ObjRef {
    let mut element = Dict::new();
    element.set("Type", Object::name("StructElem"));
    element.set("S", Object::name("Span"));
    element.set("Pg", Object::Ref(page));
    element.set("K", kids);
    if let Some(text) = actual_text {
        element.set("ActualText", Object::String(PdfString::text(text)));
    }
    cos.add(element)
}

fn install_structure(cos: &mut CosDocument, root_kids: &[ObjRef], parent_tree: &[(i64, Vec<Object>)]) -> ObjRef {
    let mut root = Dict::new();
    root.set("Type", Object::name("StructTreeRoot"));
    root.set("K", Object::Array(root_kids.iter().copied().map(Object::Ref).collect()));
    let root = cos.add(root);
    for kid in root_kids {
        cos.update_dict(*kid, |dict| dict.set("P", Object::Ref(root))).unwrap();
    }
    let mut nums = Vec::new();
    for (key, values) in parent_tree {
        nums.push(Object::Int(*key));
        nums.push(Object::Array(values.clone()));
    }
    let mut tree = Dict::new();
    tree.set("Nums", Object::Array(nums));
    let tree = cos.add(tree);
    cos.update_dict(root, |dict| dict.set("ParentTree", Object::Ref(tree))).unwrap();
    let page = ObjRef::new(4, 0);
    cos.update_dict(page, |dict| dict.set("StructParents", Object::Int(0))).unwrap();
    let catalog = cos.root().unwrap();
    cos.update_dict(catalog, |dict| {
        dict.set("StructTreeRoot", Object::Ref(root));
        let mut mark_info = Dict::new();
        mark_info.set("Marked", Object::Bool(true));
        dict.set("MarkInfo", Object::Dict(mark_info));
    })
    .unwrap();
    root
}

fn export_html(bytes: Vec<u8>) -> (String, Vec<usize>) {
    let original = Arc::new(bytes);
    let mut session = Session::new();
    let id = session.open("actual-text.pdf", None, original, None).unwrap();
    let before = {
        let doc = session.get(id).unwrap();
        (doc.bytes.clone(), doc.dirty, doc.can_undo(), doc.can_redo())
    };
    let exported = session.get(id).unwrap().export_office_checked(compare::OfficeFormat::Html);
    let after_doc = session.get(id).unwrap();
    assert_eq!(&after_doc.bytes, &before.0, "Office export must not change the source PDF");
    assert_eq!(after_doc.dirty, before.1);
    assert_eq!(after_doc.can_undo(), before.2);
    assert_eq!(after_doc.can_redo(), before.3);
    (String::from_utf8(exported.bytes).unwrap(), exported.truncated_pages)
}

fn assert_html_counts(html: &str, expected: &[(&str, usize)]) {
    for (text, count) in expected {
        assert_eq!(html.matches(text).count(), *count, "expected {text:?} {count} times in {html}");
    }
}

fn bytes(cos: &CosDocument) -> Vec<u8> {
    pdfcraft_cos::write_full(cos, &Default::default()).unwrap()
}

/// A nested marked-content sequence claims two sibling structure elements as one reading owner.
#[test]
fn office_actual_text_sibling_owners_nested_content_falls_back_without_partial_replacement() {
    for sibling_actual in ["text", "missing", "non-string"] {
        let page = ObjRef::new(4, 0);
        let mut cos = new_cos();
        set_content(
            &mut cos,
            b"/P << /MCID 0 >> BDC BT /F1 12 Tf 20 220 Td (FIRST GLYPH) Tj ET \
              /P << /MCID 1 >> BDC BT /F1 12 Tf 20 180 Td (SECOND GLYPH) Tj ET EMC EMC \
              BT /F1 12 Tf 20 100 Td (UNCHANGED NEIGHBOR) Tj ET"
                .to_vec(),
        );
        let first = add_tag(&mut cos, page, Object::Int(0), Some("CORRECT LOGICAL TEXT"));
        let second = add_tag(&mut cos, page, Object::Int(1), (sibling_actual == "text").then_some("SECOND LOGICAL TEXT"));
        if sibling_actual == "non-string" {
            cos.update_dict(second, |dict| dict.set("ActualText", Object::Int(42))).unwrap();
        }
        install_structure(&mut cos, &[first, second], &[(0, vec![Object::Ref(first), Object::Ref(second)])]);

        let (html, truncated) = export_html(bytes(&cos));
        assert_eq!(truncated, [0], "sibling ActualText variant {sibling_actual} makes the page incomplete");
        assert_html_counts(
            &html,
            &[("CORRECT LOGICAL TEXT", 0), ("SECOND LOGICAL TEXT", 0), ("FIRST GLYPH", 1), ("SECOND GLYPH", 1), ("UNCHANGED NEIGHBOR", 1)],
        );
    }
}

/// A page replacement around a self-recursive form must not hide source glyphs or claim completeness.
#[test]
fn office_actual_text_self_recursive_form_falls_back_to_page_glyphs() {
    let page = ObjRef::new(4, 0);
    let mut cos = new_cos();
    let form = add_form(&mut cos, b"/Fm Do", None);
    add_form_to_page(&mut cos, form, "Fm");
    set_content(
        &mut cos,
        b"/P << /MCID 0 >> BDC BT /F1 12 Tf 20 220 Td (PAGE GLYPH) Tj ET /Fm Do EMC \
          BT /F1 12 Tf 20 100 Td (UNCHANGED NEIGHBOR) Tj ET"
            .to_vec(),
    );
    let element = add_tag(&mut cos, page, Object::Int(0), Some("CORRECT LOGICAL TEXT"));
    install_structure(&mut cos, &[element], &[(0, vec![Object::Ref(element)])]);

    let (html, truncated) = export_html(bytes(&cos));
    assert_eq!(truncated, [0], "recursive form content is incomplete");
    assert_html_counts(&html, &[("CORRECT LOGICAL TEXT", 0), ("PAGE GLYPH", 1), ("UNCHANGED NEIGHBOR", 1)]);
}

/// A form whose declared Flate stream cannot be decoded leaves its enclosing replacement unsafe.
#[test]
fn office_actual_text_unreadable_form_falls_back_to_page_glyphs() {
    let page = ObjRef::new(4, 0);
    let mut cos = new_cos();
    let mut form_dict = Dict::new();
    form_dict.set("Type", Object::name("XObject"));
    form_dict.set("Subtype", Object::name("Form"));
    form_dict.set("BBox", Object::Array(vec![0.into(), 0.into(), 200.into(), 300.into()]));
    form_dict.set("Filter", Object::name("FlateDecode"));
    let form = cos.add(Stream::from_raw(form_dict, b"not a Flate stream".to_vec()));
    add_form_to_page(&mut cos, form, "Fm");
    set_content(
        &mut cos,
        b"/P << /MCID 0 >> BDC BT /F1 12 Tf 20 220 Td (PAGE GLYPH) Tj ET /Fm Do EMC \
          BT /F1 12 Tf 20 100 Td (UNCHANGED NEIGHBOR) Tj ET"
            .to_vec(),
    );
    let element = add_tag(&mut cos, page, Object::Int(0), Some("CORRECT LOGICAL TEXT"));
    install_structure(&mut cos, &[element], &[(0, vec![Object::Ref(element)])]);

    let (html, truncated) = export_html(bytes(&cos));
    assert_eq!(truncated, [0], "an unreadable form leaves the page incomplete");
    assert_html_counts(&html, &[("CORRECT LOGICAL TEXT", 0), ("PAGE GLYPH", 1), ("UNCHANGED NEIGHBOR", 1)]);
}

/// A tag's noncontiguous MCIDs replace together without swallowing untagged or sibling text.
#[test]
fn office_actual_text_noncontiguous_members_keep_intervening_neighbors() {
    let page = ObjRef::new(4, 0);
    let mut cos = new_cos();
    set_content(
        &mut cos,
        b"/P << /MCID 0 >> BDC BT /F1 12 Tf 20 240 Td (FIRST GLYPH) Tj ET EMC \
          /P << /MCID 1 >> BDC BT /F1 12 Tf 20 200 Td (SIBLING GLYPH) Tj ET EMC \
          BT /F1 12 Tf 20 160 Td (UNCHANGED NEIGHBOR) Tj ET \
          /P << /MCID 2 >> BDC BT /F1 12 Tf 20 120 Td (LAST GLYPH) Tj ET EMC"
            .to_vec(),
    );
    let joined = add_tag(&mut cos, page, Object::Array(vec![Object::Int(0), Object::Int(2)]), Some("CORRECT LOGICAL TEXT"));
    let sibling = add_tag(&mut cos, page, Object::Int(1), None);
    install_structure(&mut cos, &[joined, sibling], &[(0, vec![Object::Ref(joined), Object::Ref(sibling), Object::Ref(joined)])]);

    let (html, truncated) = export_html(bytes(&cos));
    assert!(truncated.is_empty(), "all members are present and closed");
    assert_html_counts(&html, &[("CORRECT LOGICAL TEXT", 1), ("FIRST GLYPH", 0), ("LAST GLYPH", 0), ("SIBLING GLYPH", 1), ("UNCHANGED NEIGHBOR", 1)]);
}

/// Page and form content each own MCID zero in separate ParentTree namespaces.
#[test]
fn office_actual_text_page_and_form_mcid_zero_have_independent_owners() {
    let page = ObjRef::new(4, 0);
    let mut cos = new_cos();
    let form_body = b"/P << /MCID 0 >> BDC BT /F1 12 Tf 20 220 Td (FORM GLYPH) Tj ET EMC";
    let form = add_form(&mut cos, form_body, Some(1));
    add_form_to_page(&mut cos, form, "Fm");
    set_content(
        &mut cos,
        b"/P << /MCID 0 >> BDC BT /F1 12 Tf 20 260 Td (PAGE GLYPH) Tj ET EMC \
          /Fm Do BT /F1 12 Tf 20 100 Td (UNCHANGED NEIGHBOR) Tj ET"
            .to_vec(),
    );
    let page_tag = add_tag(&mut cos, page, Object::Int(0), Some("PAGE LOGICAL TEXT"));
    let mut mcr = Dict::new();
    mcr.set("Type", Object::name("MCR"));
    mcr.set("Pg", Object::Ref(page));
    mcr.set("Stm", Object::Ref(form));
    mcr.set("MCID", Object::Int(0));
    let form_tag = add_tag(&mut cos, page, Object::Dict(mcr), Some("FORM LOGICAL TEXT"));
    install_structure(&mut cos, &[page_tag, form_tag], &[(0, vec![Object::Ref(page_tag)]), (1, vec![Object::Ref(form_tag)])]);

    let (html, truncated) = export_html(bytes(&cos));
    assert!(truncated.is_empty());
    assert_html_counts(&html, &[("PAGE LOGICAL TEXT", 1), ("FORM LOGICAL TEXT", 1), ("PAGE GLYPH", 0), ("FORM GLYPH", 0), ("UNCHANGED NEIGHBOR", 1)]);
}

/// A page-owned replacement encloses both its page glyphs and an untagged form invocation.
#[test]
fn office_actual_text_page_owner_around_untagged_form_replaces_once() {
    let page = ObjRef::new(4, 0);
    let mut cos = new_cos();
    let form = add_form(&mut cos, b"BT /F1 12 Tf 20 180 Td (FORM GLYPH) Tj ET", None);
    add_form_to_page(&mut cos, form, "Fm");
    set_content(
        &mut cos,
        b"/P << /MCID 0 >> BDC BT /F1 12 Tf 20 240 Td (PAGE GLYPH) Tj ET /Fm Do EMC \
          BT /F1 12 Tf 20 100 Td (UNCHANGED NEIGHBOR) Tj ET"
            .to_vec(),
    );
    let element = add_tag(&mut cos, page, Object::Int(0), Some("OWNER LOGICAL TEXT"));
    install_structure(&mut cos, &[element], &[(0, vec![Object::Ref(element)])]);

    let (html, truncated) = export_html(bytes(&cos));
    assert!(truncated.is_empty());
    assert_html_counts(&html, &[("OWNER LOGICAL TEXT", 1), ("PAGE GLYPH", 0), ("FORM GLYPH", 0), ("UNCHANGED NEIGHBOR", 1)]);
}

/// Reusing one form-owned MCID for two invocations is ambiguous and retains both raw runs.
#[test]
fn office_actual_text_repeated_form_tag_invocations_fall_back() {
    let page = ObjRef::new(4, 0);
    let mut cos = new_cos();
    let form_body = b"/P << /MCID 0 >> BDC BT /F1 12 Tf 20 220 Td (FORM GLYPH) Tj ET EMC";
    let form = add_form(&mut cos, form_body, Some(1));
    add_form_to_page(&mut cos, form, "Fm");
    set_content(&mut cos, b"/Fm Do /Fm Do BT /F1 12 Tf 20 100 Td (UNCHANGED NEIGHBOR) Tj ET".to_vec());
    let mut mcr = Dict::new();
    mcr.set("Type", Object::name("MCR"));
    mcr.set("Pg", Object::Ref(page));
    mcr.set("Stm", Object::Ref(form));
    mcr.set("MCID", Object::Int(0));
    let element = add_tag(&mut cos, page, Object::Dict(mcr), Some("FORM LOGICAL TEXT"));
    install_structure(&mut cos, &[element], &[(1, vec![Object::Ref(element)])]);

    let (html, truncated) = export_html(bytes(&cos));
    assert_eq!(truncated, [0]);
    assert_html_counts(&html, &[("FORM LOGICAL TEXT", 0), ("FORM GLYPH", 2), ("UNCHANGED NEIGHBOR", 1)]);
}

/// An owner with a missing or unclosed member must retain its raw text rather than partially substitute.
#[test]
fn office_actual_text_missing_or_unclosed_member_falls_back() {
    for (name, content) in [
        (
            "missing",
            b"/P << /MCID 0 >> BDC BT /F1 12 Tf 20 220 Td (FIRST GLYPH) Tj ET EMC \
              BT /F1 12 Tf 20 100 Td (UNCHANGED NEIGHBOR) Tj ET"
                .as_slice(),
        ),
        (
            "unclosed",
            b"/P << /MCID 0 >> BDC BT /F1 12 Tf 20 220 Td (FIRST GLYPH) Tj ET \
              BT /F1 12 Tf 20 100 Td (UNCHANGED NEIGHBOR) Tj ET"
                .as_slice(),
        ),
    ] {
        let page = ObjRef::new(4, 0);
        let mut cos = new_cos();
        set_content(&mut cos, content.to_vec());
        let element = add_tag(&mut cos, page, Object::Array(vec![Object::Int(0), Object::Int(1)]), Some("CORRECT LOGICAL TEXT"));
        install_structure(&mut cos, &[element], &[(0, vec![Object::Ref(element), Object::Ref(element)])]);
        let (html, truncated) = export_html(bytes(&cos));
        assert_eq!(truncated, [0], "{name} member must mark reading incomplete");
        assert_html_counts(&html, &[("CORRECT LOGICAL TEXT", 0), ("FIRST GLYPH", 1), ("UNCHANGED NEIGHBOR", 1)]);
    }
}

/// An inline replacement crossing two distinct structure owners is not safe to place.
#[test]
fn office_actual_text_inline_span_crossing_structure_owners_falls_back() {
    let page = ObjRef::new(4, 0);
    let mut cos = new_cos();
    set_content(
        &mut cos,
        b"/Span << /ActualText (INLINE LOGICAL TEXT) >> BDC \
          /P << /MCID 0 >> BDC BT /F1 12 Tf 20 220 Td (FIRST GLYPH) Tj ET EMC \
          /P << /MCID 1 >> BDC BT /F1 12 Tf 20 180 Td (SECOND GLYPH) Tj ET EMC EMC \
          BT /F1 12 Tf 20 100 Td (UNCHANGED NEIGHBOR) Tj ET"
            .to_vec(),
    );
    let first = add_tag(&mut cos, page, Object::Int(0), Some("FIRST LOGICAL TEXT"));
    let second = add_tag(&mut cos, page, Object::Int(1), Some("SECOND LOGICAL TEXT"));
    install_structure(&mut cos, &[first, second], &[(0, vec![Object::Ref(first), Object::Ref(second)])]);

    let (html, truncated) = export_html(bytes(&cos));
    assert_eq!(truncated, [0]);
    assert_html_counts(
        &html,
        &[
            ("INLINE LOGICAL TEXT", 1),
            ("FIRST LOGICAL TEXT", 0),
            ("SECOND LOGICAL TEXT", 0),
            ("FIRST GLYPH", 0),
            ("SECOND GLYPH", 0),
            ("UNCHANGED NEIGHBOR", 1),
        ],
    );
}

/// The outermost valid StructElem replacement wins, even when its nested child has ActualText.
#[test]
fn office_actual_text_outer_nested_structelem_wins_including_empty_outer_and_inner() {
    for (outer_actual, inner_actual) in
        [("OUTER LOGICAL TEXT", Some("INNER LOGICAL TEXT")), ("OUTER LOGICAL TEXT", Some("")), ("", Some("INNER LOGICAL TEXT"))]
    {
        let page = ObjRef::new(4, 0);
        let mut cos = new_cos();
        set_content(
            &mut cos,
            b"/P << /MCID 0 >> BDC BT /F1 12 Tf 20 220 Td (RAW GLYPH) Tj ET EMC \
              BT /F1 12 Tf 20 100 Td (UNCHANGED NEIGHBOR) Tj ET"
                .to_vec(),
        );
        let mut outer_dict = Dict::new();
        outer_dict.set("Type", Object::name("StructElem"));
        outer_dict.set("S", Object::name("P"));
        outer_dict.set("Pg", Object::Ref(page));
        outer_dict.set("K", Object::Null);
        outer_dict.set("ActualText", Object::String(PdfString::text(outer_actual)));
        let outer = cos.add(outer_dict);
        let inner = add_tag(&mut cos, page, Object::Int(0), inner_actual);
        cos.update_dict(inner, |dict| dict.set("P", Object::Ref(outer))).unwrap();
        cos.update_dict(outer, |dict| dict.set("K", Object::Ref(inner))).unwrap();
        install_structure(&mut cos, &[outer], &[(0, vec![Object::Ref(inner)])]);

        let (html, truncated) = export_html(bytes(&cos));
        assert!(truncated.is_empty());
        let outer_count = usize::from(outer_actual == "OUTER LOGICAL TEXT");
        assert_html_counts(&html, &[("OUTER LOGICAL TEXT", outer_count), ("INNER LOGICAL TEXT", 0), ("RAW GLYPH", 0), ("UNCHANGED NEIGHBOR", 1)]);
    }
}

fn export_page_block_texts(bytes: Vec<u8>) -> Vec<String> {
    let mut session = Session::new();
    let id = session.open("actual-text-blocks.pdf", None, Arc::new(bytes), None).unwrap();
    let before = {
        let doc = session.get(id).unwrap();
        (doc.bytes.clone(), doc.dirty, doc.can_undo(), doc.can_redo())
    };
    let pages = session.get(id).unwrap().export_pages();
    let after = session.get(id).unwrap();
    assert_eq!(&after.bytes, &before.0);
    assert_eq!(after.dirty, before.1);
    assert_eq!(after.can_undo(), before.2);
    assert_eq!(after.can_redo(), before.3);
    pages.first().unwrap().blocks.iter().map(|block| block.text.clone()).collect()
}

/// An empty replacement inside a word joins the two neighboring runs without inventing a space.
#[test]
fn office_actual_text_empty_midword_matches_inline_empty_control() {
    for structure_actual_text in [true, false] {
        let mut cos = new_cos();
        let properties = if structure_actual_text { "/MCID 0" } else { "/MCID 0 /ActualText ()" };
        let content = format!("BT /F1 12 Tf 20 220 Td (a) Tj /Span << {properties} >> BDC (X) Tj EMC (b) Tj ET");
        set_content(&mut cos, content.into_bytes());
        if structure_actual_text {
            let page = ObjRef::new(4, 0);
            let element = add_tag(&mut cos, page, Object::Int(0), Some(""));
            install_structure(&mut cos, &[element], &[(0, vec![Object::Ref(element)])]);
        }
        assert_eq!(export_page_block_texts(bytes(&cos)), ["ab"], "structure mode: {structure_actual_text}");
    }
}

/// An empty replacement preserves logical run order under rotated and reversed text matrices.
#[test]
fn office_actual_text_empty_midword_preserves_rotated_text_run_order() {
    let matrices = [(1, 0, 0, 1), (0, 1, -1, 0), (-1, 0, 0, -1), (0, -1, 1, 0)];
    for (a, b, c, d) in matrices {
        for (replacement, expected) in [("", "ab"), ("Q", "aQb")] {
            for structure_actual_text in [true, false] {
                let mut cos = new_cos();
                let properties = if structure_actual_text {
                    "/MCID 0"
                } else if replacement.is_empty() {
                    "/MCID 0 /ActualText ()"
                } else {
                    "/MCID 0 /ActualText (Q)"
                };
                let content = format!(
                    "BT /F1 12 Tf {a} {b} {c} {d} 120 200 Tm (a) Tj \
                     /Span << {properties} >> BDC (X) Tj EMC (b) Tj ET"
                );
                set_content(&mut cos, content.into_bytes());
                if structure_actual_text {
                    let page = ObjRef::new(4, 0);
                    let element = add_tag(&mut cos, page, Object::Int(0), Some(replacement));
                    install_structure(&mut cos, &[element], &[(0, vec![Object::Ref(element)])]);
                }
                assert_eq!(
                    export_page_block_texts(bytes(&cos)),
                    [expected],
                    "matrix=({a}, {b}, {c}, {d}), replacement={replacement:?}, structure={structure_actual_text}"
                );
            }
        }
    }
}

/// Structure replacement text keeps intentional leading and trailing whitespace in the export block.
#[test]
fn office_actual_text_export_block_preserves_exact_edge_whitespace() {
    let page = ObjRef::new(4, 0);
    let mut cos = new_cos();
    set_content(&mut cos, b"/P << /MCID 0 >> BDC BT /F1 12 Tf 20 220 Td (GLYPH TEXT) Tj ET EMC".to_vec());
    let element = add_tag(&mut cos, page, Object::Int(0), Some("  logical  "));
    install_structure(&mut cos, &[element], &[(0, vec![Object::Ref(element)])]);

    assert_eq!(export_page_block_texts(bytes(&cos)), ["  logical  "]);
}

#[test]
fn office_actual_text_hyphenation_joins_unmarked_rows_after_logical_intro() {
    let page = ObjRef::new(4, 0);
    let mut cos = new_cos();
    set_content(
        &mut cos,
        b"/Span << /MCID 0 >> BDC BT /F1 12 Tf 20 220 Td (SOURCE INTRO) Tj ET EMC \
          BT /F1 12 Tf 20 200 Td (hyphen-) Tj ET \
          BT /F1 12 Tf 20 180 Td (ation) Tj ET"
            .to_vec(),
    );
    let element = add_tag(&mut cos, page, Object::Int(0), Some("Intro"));
    install_structure(&mut cos, &[element], &[(0, vec![Object::Ref(element)])]);

    assert_eq!(export_page_block_texts(bytes(&cos)), ["Intro hyphenation"]);
}

#[test]
fn office_actual_text_preserves_whitespace_only_values_and_omits_empty_value() {
    for actual_text in [" ", "\t", "\n", "\r\n", ""] {
        let page = ObjRef::new(4, 0);
        let mut cos = new_cos();
        set_content(&mut cos, b"/P << /MCID 0 >> BDC BT /F1 12 Tf 20 220 Td (GLYPH TEXT) Tj ET EMC".to_vec());
        let element = add_tag(&mut cos, page, Object::Int(0), Some(actual_text));
        install_structure(&mut cos, &[element], &[(0, vec![Object::Ref(element)])]);

        let blocks = export_page_block_texts(bytes(&cos));
        if actual_text.is_empty() {
            assert!(blocks.is_empty(), "empty ActualText should emit no block, got {blocks:?}");
        } else {
            assert_eq!(blocks.len(), 1, "ActualText {actual_text:?}: {blocks:?}");
            assert_eq!(blocks[0], actual_text, "whitespace-only ActualText must be preserved");
        }
    }
}

#[test]
fn office_actual_text_source_repairs_fall_back_without_partial_replacement() {
    let mut failures = Vec::new();
    for repair in ["page-decode", "page-skipped", "page-ambiguous", "form-skipped", "form-ambiguous"] {
        let page = ObjRef::new(4, 0);
        let mut cos = new_cos();
        let invocation = if repair.starts_with("form-") { "/Fm Do" } else { "" };
        let mut content = format!(
            "/P << /MCID 0 >> BDC BT /F1 12 Tf 20 220 Td (PAGE GLYPH) Tj ET {invocation} EMC \
             BT /F1 12 Tf 20 100 Td (UNCHANGED NEIGHBOR) Tj ET"
        )
        .into_bytes();
        if repair != "page-decode" {
            let mut damaged = if repair.starts_with("form-") { b"BT /F1 12 Tf 20 180 Td (FORM GLYPH) Tj ET".to_vec() } else { content.clone() };
            if repair.ends_with("skipped") {
                // A stray closing array delimiter needs content-tokenizer repair.
                damaged.extend_from_slice(b"\n]");
            } else {
                // The shared content parser reports a missing inline-image EI as ambiguous.
                damaged.extend_from_slice(b"\nBI /W 4 /H 1 /BPC 8 /CS /G ID abc");
            }
            if repair.starts_with("form-") {
                let form = add_form(&mut cos, &damaged, None);
                add_form_to_page(&mut cos, form, "Fm");
            } else {
                content = damaged;
            }
        }
        set_content(&mut cos, content);
        if repair == "page-decode" {
            let original = cos.get(page).as_dict().unwrap().get(b"Contents").unwrap().clone();
            let mut dict = Dict::new();
            dict.set("Filter", Object::name("FlateDecode"));
            let bad = cos.add(Stream::from_raw(dict, b"not compressed data".to_vec()));
            cos.update_dict(page, |dict| dict.set("Contents", Object::Array(vec![original, Object::Ref(bad)]))).unwrap();
        }
        let element = add_tag(&mut cos, page, Object::Int(0), Some("CORRECT LOGICAL TEXT"));
        install_structure(&mut cos, &[element], &[(0, vec![Object::Ref(element)])]);
        let (html, truncated) = export_html(bytes(&cos));
        if truncated != [0]
            || html.contains("CORRECT LOGICAL TEXT")
            || html.matches("PAGE GLYPH").count() != 1
            || html.matches("UNCHANGED NEIGHBOR").count() != 1
        {
            failures.push(format!("{repair}: incomplete={truncated:?}; {html}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
