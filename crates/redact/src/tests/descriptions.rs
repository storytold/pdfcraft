//! Descriptions in the structure tree (alternate text, actual text, expansions) go with the
//! content they describe, on the element that owns it and on every element above it.

use super::surviving_references::full_save_holds;
use super::*;

/// A tagged page: "SECRET" (MCID 0) inside a section whose descriptions repeat it, and "Public"
/// (MCID 1). `paragraph_parent` is the secret paragraph's /P.
fn sectioned(paragraph_parent: &str) -> Document {
    let content = b"/P <</MCID 0>> BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj ET EMC /P <</MCID 1>> BDC BT /F1 10 Tf 10 100 Td (Public) Tj ET EMC";
    pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 7 0 R /MarkInfo << /Marked true >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", content),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Type /StructTreeRoot /K 8 0 R >>".to_vec(),
        b"<< /S /Document /P 7 0 R /K [11 0 R 10 0 R] >>".to_vec(),
        format!("<< /S /P /P {paragraph_parent} /Pg 3 0 R /K 0 >>").into_bytes(),
        b"<< /S /P /P 8 0 R /Pg 3 0 R /ActualText (Public) /K 1 >>".to_vec(),
        b"<< /S /Sect /P 8 0 R /ActualText (LEAK_ACTUAL) /Alt (LEAK_ALT) /E (LEAK_EXPANSION) /K 9 0 R >>".to_vec(),
    ])
}

#[test]
fn an_ancestor_describing_redacted_content_loses_its_descriptions() {
    // The paragraph names its section as parent, or (wrongly) the document: the section is found
    // either way, by its /K.
    for parent in ["11 0 R", "8 0 R"] {
        let mut doc = sectioned(parent);
        mark(&mut doc, 0, &[[5.0, 195.0, 60.0, 212.0]], "");
        apply(&mut doc, None).unwrap();
        assert!(!full_save_holds(&doc, b"LEAK_"), "a description of the redacted word survived (paragraph /P {parent})");
        let public = doc.get(ObjRef::new(10, 0)).as_dict().cloned().unwrap();
        assert!(public.contains(b"ActualText"), "an element beside it keeps its own");
    }
}

#[test]
fn a_link_element_that_keeps_its_text_loses_the_description_of_its_removed_annotation() {
    use crate::sanitize::{Hidden, remove_hidden};
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 8 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] /StructParents 0 >>"
            .to_vec(),
        stream("", b"/Link << /MCID 0 >> BDC BT /F1 10 Tf 10 230 Td (See the site) Tj ET EMC"),
        FONT.replace("95 0 R", "12 0 R").into_bytes(),
        b"<< /Type /Annot /Subtype /Link /Rect [10 228 80 240] /StructParent 1 /A << /S /URI /URI (https://example.org/) >> >>".to_vec(),
        b"null".to_vec(),
        b"<< /Type /StructTreeRoot /K 9 0 R /ParentTree << /Nums [0 [11 0 R] 1 11 0 R] >> /ParentTreeNextKey 2 >>".to_vec(),
        b"<< /Type /StructElem /S /Document /P 8 0 R /K 10 0 R >>".to_vec(),
        b"<< /Type /StructElem /S /P /P 9 0 R /ActualText (See LEAK_PARENT) /K 11 0 R >>".to_vec(),
        // The link element tags its text and the annotation; its descriptions repeat the address.
        b"<< /Type /StructElem /S /Link /P 10 0 R /Pg 3 0 R /Alt (LEAK_ALT) /ActualText (LEAK_ACTUAL) /E (LEAK_E) /T (LEAK_T) /K [0 << /Type /OBJR /Obj 6 0 R /Pg 3 0 R >>] >>".to_vec(),
        widths(),
    ]);
    let done = remove_hidden(&mut doc, &[Hidden::LinksActionsScripts]).unwrap();
    assert!(done.contains(&(Hidden::LinksActionsScripts, 1)), "{done:?}");
    assert!(!full_save_holds(&doc, b"LEAK_"), "a description of the removed link survived");
    let link = doc.get(ObjRef::new(11, 0)).as_dict().cloned().unwrap();
    assert_eq!(link.get(b"K"), Some(&Object::Array(vec![Object::Int(0)])), "the element keeps its text");
}
