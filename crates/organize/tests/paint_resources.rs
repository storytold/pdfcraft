//! Extraction must keep resources used by paint programs the XObject pruner does not interpret.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};
use pdfcraft_organize::{SplitBy, extract_pages, pages, split};

fn stream(attributes: &str, content: &str) -> String {
    format!("<< {attributes} /Length {} >>\nstream\n{content}\nendstream", content.len())
}

fn build(objects: &[String]) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

/// Page content draws ImA; another paint program draws ImB through the same resource owner.
/// Unused has no paint use, so separately owned resources can still be pruned.
fn fixture(extra_resources: &str, page_content: &str, annotation: &str, paint_attributes: &str, paint: &str) -> Document {
    build(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 13 0 R] /Count 2 /MediaBox [0 0 160 160] /Resources 5 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Annots [12 0 R] /Resources 5 0 R >>".into(),
        stream("", &format!("q 20 0 0 20 10 10 cm /ImA Do Q {page_content}")),
        format!("<< /XObject 6 0 R {extra_resources} >>"),
        "<< /ImA 7 0 R /ImB 8 0 R /Unused 9 0 R >>".into(),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", "2"),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", "x"),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", &"z".repeat(2000)),
        stream(paint_attributes, paint),
        "<< /S /Luminosity /G 10 0 R >>".into(),
        format!("<< /Type /Annot /Subtype /Stamp /Rect [50 50 110 110] /F 4 {annotation} >>"),
        "<< /Type /Page /Parent 2 0 R /Contents 14 0 R >>".into(),
        stream("", "q 20 0 0 20 10 10 cm /ImA Do Q"),
    ])
}

fn roundtrip(doc: &Document) -> Document {
    Document::open(Arc::new(write_full(doc, &SaveOptions::default()).unwrap())).unwrap()
}

fn xobjects(doc: &Document, page: usize) -> Vec<Vec<u8>> {
    let page = doc.get(pages(doc).unwrap()[page].obj);
    let resources = doc.resolve(page.as_dict().unwrap().get(b"Resources").unwrap());
    let xobjects = doc.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
    xobjects.as_dict().unwrap().iter().map(|(name, _)| name.clone()).collect()
}

fn assert_shared_image_survives(doc: &Document, selection: &[usize]) {
    let original = write_full(doc, &SaveOptions::default()).unwrap();
    let extracted = roundtrip(&extract_pages(doc, selection).unwrap());
    assert!(xobjects(&extracted, selection.len() - 1).iter().any(|name| name == b"ImB"), "a paint program still draws ImB");
    assert_eq!(write_full(doc, &SaveOptions::default()).unwrap(), original, "source remains unchanged");
}

#[test]
fn extract_preserves_shared_annotation_appearance_resources() {
    for kind in ["N", "R", "D"] {
        for state in ["10 0 R", "<< /Off 10 0 R /On 10 0 R >>"] {
            for resources in ["/Resources 5 0 R", "/Resources << /XObject 6 0 R >>", "", "/Resources null", "/Resources 999 0 R", "/Resources []"] {
                let doc = fixture(
                    "",
                    "",
                    &format!("/AP << /{kind} {state} >> /AS /Missing"),
                    &format!("/Subtype /Form /BBox [0 0 1 1] {resources}"),
                    "/ImB Do",
                );
                assert_shared_image_survives(&doc, &[0]);
            }
        }
    }
}

#[test]
fn extract_preserves_indirect_appearance_and_annotation_containers() {
    let mut doc = fixture("", "", "/AP << /N << /Off 10 0 R /On 10 0 R >> >>", "/Subtype /Form /BBox [0 0 1 1] /Resources 5 0 R", "/ImB Do");
    let ap = doc.add(doc.get(ObjRef::new(12, 0)).as_dict().unwrap().get(b"AP").unwrap().clone());
    doc.update_dict(ObjRef::new(12, 0), |annotation| annotation.set(b"AP".to_vec(), Object::Ref(ap))).unwrap();
    let annotations = doc.add(Object::Array(vec![Object::Ref(ObjRef::new(12, 0)), Object::Null, Object::Int(7)]));
    doc.update_dict(ObjRef::new(3, 0), |page| page.set(b"Annots".to_vec(), Object::Ref(annotations))).unwrap();
    assert_shared_image_survives(&doc, &[0]);
}

#[test]
fn extract_preserves_appearance_resources_on_a_page_without_contents_or_resources() {
    let mut doc = fixture("", "", "/AP << /N 10 0 R >>", "/BBox [0 0 1 1] /Resources 5 0 R", "/ImB Do");
    let page = pages(&doc).unwrap()[0].obj;
    doc.update_dict(page, |page| {
        page.remove(b"Contents");
        page.set(b"Resources".to_vec(), Object::Dict(Default::default()));
    })
    .unwrap();
    assert_shared_image_survives(&doc, &[0, 1]);
}

#[test]
fn extract_protects_each_caller_of_a_shared_resource_less_appearance() {
    let mut doc = fixture("", "", "/AP << /N 10 0 R >>", "/Subtype /Form /BBox [0 0 1 1]", "/ImB Do");
    let other_resources = doc.add((*doc.get(ObjRef::new(5, 0))).clone());
    let other_xobjects = doc.add((*doc.get(ObjRef::new(6, 0))).clone());
    doc.update_dict(other_resources, |resources| resources.set(b"XObject".to_vec(), Object::Ref(other_xobjects))).unwrap();
    doc.update_dict(ObjRef::new(13, 0), |page| {
        page.set(b"Resources".to_vec(), Object::Ref(other_resources));
        page.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(12, 0))]));
    })
    .unwrap();
    let extracted = roundtrip(&extract_pages(&doc, &[0, 1]).unwrap());
    for page in 0..2 {
        assert!(xobjects(&extracted, page).iter().any(|name| name == b"ImB"), "caller page {page}");
    }
}

#[test]
fn extract_preserves_shared_tiling_pattern_and_soft_mask_resources() {
    let pattern = fixture(
        "/Pattern << /Tile 10 0 R >>",
        "/Pattern cs /Tile scn 50 50 60 60 re f",
        "",
        "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 1 1] /XStep 1 /YStep 1 /Resources 5 0 R",
        "/ImB Do",
    );
    assert_shared_image_survives(&pattern, &[0]);
    let mask = fixture(
        "/ExtGState << /GS << /SMask 11 0 R >> >>",
        "/GS gs 50 50 60 60 re f",
        "",
        "/Subtype /Form /BBox [0 0 1 1] /Group << /S /Transparency /CS /DeviceGray >> /Resources 5 0 R",
        "/ImB Do",
    );
    assert_shared_image_survives(&mask, &[0]);
    let inherited_mask = fixture("/ExtGState << /GS << /SMask 11 0 R >> >>", "/GS gs 50 50 60 60 re f", "", "/BBox [0 0 1 1]", "/ImB Do");
    assert_shared_image_survives(&inherited_mask, &[0]);
}

#[test]
fn extract_preserves_resources_of_a_type3_font_selected_by_extgstate() {
    let mut doc = fixture("", "/GS gs BT 50 80 Td (A) Tj ET", "", "", "");
    let glyph = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"1 0 0 0 1 1 d1 /ImB Do".to_vec())));
    let mut charprocs = Dict::new();
    charprocs.set(b"A".to_vec(), Object::Ref(glyph));
    let mut font = Dict::new();
    font.set(b"Subtype".to_vec(), Object::name("Type3"));
    font.set(b"CharProcs".to_vec(), Object::Dict(charprocs));
    let font = doc.add(font);
    let mut state = Dict::new();
    state.set(b"Font".to_vec(), Object::Array(vec![Object::Ref(font), Object::Int(50)]));
    let mut states = Dict::new();
    states.set(b"GS".to_vec(), Object::Dict(state));
    let states = doc.add(states);
    doc.update_dict(ObjRef::new(5, 0), |resources| resources.set(b"ExtGState".to_vec(), Object::Ref(states))).unwrap();
    assert_shared_image_survives(&doc, &[0]);
    // Discovering this catalog first through a different Type 3 font must not swallow the
    // inherited-resource requirement of the graphics-state font selected by page content.
    let mut font = Dict::new();
    font.set(b"Subtype".to_vec(), Object::name("Type3"));
    let mut font_resources = Dict::new();
    font_resources.set(b"ExtGState".to_vec(), Object::Ref(states));
    font.set(b"Resources".to_vec(), Object::Dict(font_resources));
    let font = doc.add(font);
    let mut fonts = Dict::new();
    fonts.set(b"UnusedType3".to_vec(), Object::Ref(font));
    doc.update_dict(ObjRef::new(5, 0), |resources| resources.set(b"Font".to_vec(), Object::Dict(fonts))).unwrap();
    assert_shared_image_survives(&doc, &[0]);
    // The same font graph can inherit from two different retained page resource owners.
    doc.update_dict(ObjRef::new(5, 0), |resources| {
        resources.remove(b"ExtGState");
    })
    .unwrap();
    let second_resources = doc.add((*doc.get(ObjRef::new(5, 0))).clone());
    let second_xobjects = doc.add((*doc.get(ObjRef::new(6, 0))).clone());
    doc.update_dict(second_resources, |resources| resources.set(b"XObject".to_vec(), Object::Ref(second_xobjects))).unwrap();
    doc.update_dict(ObjRef::new(13, 0), |page| page.set(b"Resources".to_vec(), Object::Ref(second_resources))).unwrap();
    let extracted = extract_pages(&doc, &[0, 1]).unwrap();
    for page in 0..2 {
        assert!(xobjects(&extracted, page).iter().any(|name| name == b"ImB"), "font's inherited caller {page}");
    }
}

#[test]
fn extract_skips_pruning_after_a_paint_graph_exceeds_the_work_budget() {
    let mut doc = fixture("", "", "/AP << /N 10 0 R >>", "/Subtype /Form /BBox [0 0 1 1] /Resources << >>", "");
    let mut ap = Dict::new();
    ap.set(b"N".to_vec(), Object::Array(vec![Object::Null; 10_001]));
    doc.update_dict(ObjRef::new(12, 0), |annotation| annotation.set(b"AP".to_vec(), Object::Dict(ap))).unwrap();
    let extracted = roundtrip(&extract_pages(&doc, &[0]).unwrap());
    assert_eq!(xobjects(&extracted, 0), xobjects(&doc, 0), "incomplete discovery cannot authorize pruning");
}

#[test]
fn extract_terminates_on_cyclic_paint_resources() {
    let mut doc = fixture("/Pattern << /Tile 10 0 R >>", "", "", "/PatternType 1 /Resources 5 0 R", "/ImB Do");
    let cycle = doc.add(Object::Null);
    let mut dict = Dict::new();
    dict.set(b"Loop".to_vec(), Object::Ref(cycle));
    dict.set(b"Shared".to_vec(), Object::Ref(ObjRef::new(6, 0)));
    doc.set(cycle, Object::Dict(dict));
    doc.update_dict(ObjRef::new(10, 0), |paint| paint.set(b"Cycle".to_vec(), Object::Ref(cycle))).unwrap();
    assert_shared_image_survives(&doc, &[0]);
}

#[test]
fn extract_still_prunes_when_appearance_resources_are_separate() {
    let mut doc = fixture("", "", "/AP << /N 10 0 R >>", "/Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /ImB 8 0 R >> >>", "/ImB Do");
    // Opaque font programs/profile streams have no Resources but are not paint programs.
    let opaque = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"opaque program bytes".to_vec())));
    doc.update_dict(ObjRef::new(10, 0), |paint| paint.set(b"FontFile".to_vec(), Object::Ref(opaque))).unwrap();
    let extracted = roundtrip(&extract_pages(&doc, &[0]).unwrap());
    assert_eq!(xobjects(&extracted, 0), [b"ImA".to_vec()]);
    let page = extracted.get(pages(&extracted).unwrap()[0].obj);
    let annotations = extracted.resolve(page.as_dict().unwrap().get(b"Annots").unwrap());
    let annotation = extracted.resolve(&annotations.as_array().unwrap()[0]);
    let ap = extracted.resolve(annotation.as_dict().unwrap().get(b"AP").unwrap());
    let normal = extracted.resolve(ap.as_dict().unwrap().get(b"N").unwrap());
    let resources = extracted.resolve(normal.as_dict().unwrap().get(b"Resources").unwrap());
    let images = extracted.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
    assert!(images.as_dict().unwrap().contains(b"ImB"));
}

#[test]
fn split_preserves_shared_appearance_resources() {
    let doc = fixture("", "", "/AP << /N 10 0 R >>", "/Subtype /Form /BBox [0 0 1 1] /Resources 5 0 R", "/ImB Do");
    let parts = split(&doc, &SplitBy::PageCount(1)).unwrap();
    assert!(xobjects(&roundtrip(&parts[0]), 0).iter().any(|name| name == b"ImB"));
    assert_eq!(xobjects(&roundtrip(&parts[1]), 0), [b"ImA".to_vec()], "a page without the appearance still prunes");
}

#[test]
fn extract_still_prunes_when_many_pages_share_an_ordinary_graphics_state() {
    let mut doc = fixture("/ExtGState << /GS << /ca 0.5 >> >>", "/GS gs", "", "", "");
    let source_page = doc.get(ObjRef::new(3, 0));
    let mut kids = Vec::new();
    for _ in 0..4000 {
        let mut page = source_page.as_dict().unwrap().clone();
        page.remove(b"Annots");
        kids.push(Object::Ref(doc.add(page)));
    }
    doc.update_dict(ObjRef::new(2, 0), |pages| {
        pages.set(b"Kids".to_vec(), Object::Array(kids));
        pages.set(b"Count".to_vec(), 4000);
    })
    .unwrap();
    let extracted = extract_pages(&doc, &(0..4000).collect::<Vec<_>>()).unwrap();
    assert_eq!(xobjects(&extracted, 0), [b"ImA".to_vec()], "ordinary shared graphics states must not consume the paint traversal budget per page");
}

#[test]
fn extract_shared_appearance_matches_poppler_pixels() {
    let pdftoppm = pdfcraft_oracle::or_skip!(pdfcraft_oracle::pdftoppm());
    let doc = fixture("", "", "/AP << /N 10 0 R >>", "/Subtype /Form /BBox [0 0 1 1] /Resources 5 0 R", "/ImB Do");
    let dir = std::env::temp_dir().join(format!("pdfcraft-paint-resources-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let render = |doc: &Document, name: &str| {
        let path = dir.join(format!("{name}.pdf"));
        std::fs::write(&path, write_full(doc, &SaveOptions::default()).unwrap()).unwrap();
        let result = pdftoppm.command().args(["-f", "1", "-singlefile", "-r", "72", "-gray"]).arg(path).output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        assert!(result.stderr.is_empty(), "{}", String::from_utf8_lossy(&result.stderr));
        result.stdout
    };
    let before = render(&doc, "source");
    let extracted = extract_pages(&doc, &[0]).unwrap();
    let after = render(&extracted, "extracted");
    assert_eq!(after, before, "extraction must preserve the displayed stamp");
    std::fs::remove_dir_all(dir).unwrap();
}
