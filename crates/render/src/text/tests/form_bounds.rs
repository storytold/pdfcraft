use super::super::*;
use lopdf::{Document, Object, Stream, dictionary};

#[test]
fn near_degenerate_outer_form_excludes_wholly_outside_text() {
    // Decimal literals retain f64 CTM values without depending on a tiny f32 BBox.
    let tiny = format!("0.{}1", "0".repeat(199));
    let inverse = format!("1{}.0", "0".repeat(200));
    let page = format!(
        "q 1 0 0 1 0 400 cm {tiny} 0 0 {tiny} 0 0 cm /Fm Do Q \
         BT /F1 6 Tf 20 20 Td (PAGE) Tj ET"
    );
    let body = format!("{inverse} 0 0 {inverse} 0 0 cm BT /F1 6 Tf 10 -10 Td (OUTSIDE) Tj ET");
    let bounds = [0., 0., 1., 1.];
    let control = document("", bounds, IDENTITY, &page, None);
    assert_text(&control, "PAGE");
    let pdf = document(&body, bounds, IDENTITY, &page, None);
    assert_text(&pdf, "PAGE");
}

#[test]
fn finite_small_clips_keep_inside_and_contact_but_exclude_separated_points() {
    use kurbo::Point;
    for scale in [1e-200, 1e-10, 1.0, 1e100] {
        let path = Rect::new(0., 0., scale, scale).to_path(0.1);
        for path in [path.clone(), path.reverse_subpaths()] {
            let mut clips = clips::TextClips::default();
            clips.push(&path);
            assert!(clips.visible(&[Point::new(0.5 * scale, 0.5 * scale); 4]), "inside {scale}");
            assert!(clips.visible(&[Point::new(scale, 0.5 * scale); 4]), "contact {scale}");
            assert!(!clips.visible(&[Point::new(2.0 * scale, 0.5 * scale); 4]), "outside {scale}");
        }
    }
}

#[test]
fn near_degenerate_nested_clips_keep_their_intersection_and_restore() {
    use kurbo::Point;
    let small = Rect::new(0., 0., 1e-200, 1e-200).to_path(0.1);
    let large = Rect::new(0., 0., 50., 50.).to_path(0.1);
    for (first, second, visible_after_pop) in [(&large, &small, true), (&small, &large, false)] {
        let mut clips = clips::TextClips::default();
        clips.push(first);
        clips.push(second);
        assert!(clips.visible(&[Point::new(0.5e-200, 0.5e-200); 4]));
        assert!(!clips.visible(&[Point::new(10., 10.); 4]));
        clips.pop();
        assert_eq!(clips.visible(&[Point::new(10., 10.); 4]), visible_after_pop);
        clips.pop();
        assert!(clips.visible(&[Point::new(10., 10.); 4]));
    }
}

#[test]
fn extreme_axis_ratios_preserve_boundary_contact() {
    use kurbo::{PathEl, Point};
    for height in [1e120, 1e200] {
        let path = BezPath::from_vec(vec![
            PathEl::MoveTo(Point::new(0., 0.)),
            PathEl::LineTo(Point::new(1e-200, height)),
            PathEl::LineTo(Point::new(2e-200, height)),
            PathEl::LineTo(Point::new(1e-200, 0.)),
            PathEl::ClosePath,
        ]);
        let mut clips = clips::TextClips::default();
        clips.push(&path);
        assert!(clips.visible(&[Point::new(0., 0.); 4]), "corner {height}");
        assert!(!clips.visible(&[Point::new(1., 1.); 4]), "outside {height}");
    }
}

fn document(body: &str, bbox: [f32; 4], matrix: [f32; 6], page_body: &str, inner: Option<(&str, [f32; 4], [f32; 6])>) -> Pdf {
    to_pdf(form_document(body, bbox, matrix, page_body, inner))
}

fn form_document(body: &str, bbox: [f32; 4], matrix: [f32; 6], page_body: &str, inner: Option<(&str, [f32; 4], [f32; 6])>) -> Document {
    let mut doc = Document::with_version("1.7");
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier" });
    let mut resources = dictionary! { "Font" => dictionary! { "F1" => font } };
    if let Some((ops, bounds, transform)) = inner {
        let form = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Form", "BBox" => bounds.map(Object::Real).to_vec(),
                "Matrix" => transform.map(Object::Real).to_vec(), "Resources" => resources.clone(),
            },
            ops.as_bytes().to_vec(),
        ));
        resources.set("XObject", dictionary! { "Inner" => form });
    }
    let form = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form", "BBox" => bbox.map(Object::Real).to_vec(),
            "Matrix" => matrix.map(Object::Real).to_vec(), "Resources" => resources,
        },
        body.as_bytes().to_vec(),
    ));
    let content = doc.add_object(Stream::new(dictionary! {}, page_body.as_bytes().to_vec()));
    let pages = doc.new_object_id();
    let page = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages, "MediaBox" => vec![0.into(), 0.into(), 400.into(), 400.into()],
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font }, "XObject" => dictionary! { "Fm" => form } }, "Contents" => content,
    });
    doc.objects.insert(pages, dictionary! { "Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1 }.into());
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    doc.trailer.set("Root", catalog);
    doc
}

fn to_pdf(mut doc: Document) -> Pdf {
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    Pdf::new(std::sync::Arc::new(bytes)).unwrap()
}

const IDENTITY: [f32; 6] = [1., 0., 0., 1., 0., 0.];
const BOUNDS: [f32; 4] = [0., 0., 50., 50.];
const BOTH: &str = "BT /F1 6 Tf 10 10 Td (INSIDE) Tj ET BT /F1 6 Tf 100 100 Td (OUTSIDE) Tj ET";

fn assert_text(pdf: &Pdf, expected: &str) {
    for filter in [TextFilter::default(), TextFilter { images: true, ..Default::default() }] {
        assert_eq!(extract_page_filtered(pdf, 0, &InterpreterSettings::default(), filter).unwrap().plain_text(), expected);
    }
}

#[test]
fn transformed_forms_exclude_outside_text() {
    for matrix in [
        IDENTITY,
        [1., 0., 0., 1., 20., 30.],
        [2., 0., 0., 2., 0., 0.],
        [0.707, 0.707, -0.707, 0.707, 100., 100.],
        [1., 0., 0.7, 1., 20., 20.],
        [-1., 0., 0., 1., 150., 20.],
    ] {
        assert_text(&document(BOTH, BOUNDS, matrix, "/Fm Do", None), "INSIDE");
    }
}

#[test]
fn enclosing_rectangles_do_not_keep_hidden_rotated_or_sheared_glyphs() {
    for (ops, matrix) in [
        ("BT /F1 6 Tf 60 25 Td (A) Tj ET", [0.707, 0.707, -0.707, 0.707, 100., 100.]),
        ("BT /F1 6 Tf -10 45 Td (A) Tj ET", [1., 0., 0.7, 1., 20., 20.]),
    ] {
        assert_text(&document(ops, BOUNDS, matrix, "/Fm Do", None), "");
    }
}

#[test]
fn nested_intersections_and_restoration_preserve_siblings() {
    let page = "/Fm Do BT /F1 6 Tf 100 150 Td (D) Tj ET";
    let outer = "/Inner Do BT /F1 6 Tf 1 40 Td (C) Tj ET";
    let disjoint = document(outer, [0., 0., 20., 50.], IDENTITY, page, Some(("BT /F1 50 Tf 10 20 Td (A) Tj ET", [30., 0., 50., 50.], IDENTITY)));
    assert_text(&disjoint, "D\nC");
    let overlap = document(outer, BOUNDS, IDENTITY, page, Some(("BT /F1 6 Tf 2 10 Td (A) Tj 18 0 Td (B) Tj ET", BOUNDS, [1., 0., 0., 1., 40., 0.])));
    assert_text(&overlap, "D\nC\nA");
    assert_text(&document(BOTH, BOUNDS, IDENTITY, "q 0 0 50 50 re W n /Fm Do Q BT /F1 6 Tf 100 150 Td (AFTER) Tj ET", None), "AFTER\nINSIDE");
}

#[test]
fn actual_text_does_not_restore_clipped_replacement() {
    for (ops, expected, redaction) in [
        ("/Span <</ActualText (ACTUAL-IN)>> BDC BT /F1 6 Tf 10 10 Td (IN) Tj ET EMC", "ACTUAL-IN", "ACTUAL-IN"),
        ("/Span <</ActualText (ACTUAL-OUT)>> BDC BT /F1 6 Tf 100 100 Td (OUT) Tj ET EMC", "", "ACTUAL-OUT"),
        (
            "/Span <</ActualText (SECRET VISIBLE)>> BDC BT /F1 6 Tf 100 10 Td (SECRET) Tj ET BT /F1 6 Tf 10 10 Td (VISIBLE) Tj ET EMC",
            "VISIBLE",
            "SECRET VISIBLE",
        ),
    ] {
        let pdf = document(ops, BOUNDS, IDENTITY, "/Fm Do", None);
        assert_text(&pdf, expected);
        assert_eq!(extract_page_for_redaction(&pdf, 0, &InterpreterSettings::default()).unwrap().plain_text(), redaction);
    }
}

#[test]
fn empty_form_bounds_exclude_all_text() {
    for bbox in [[0., 0., 0., 50.], [0., 0., 50., 0.]] {
        assert_text(&document(BOTH, bbox, IDENTITY, "/Fm Do", None), "");
    }
}

#[test]
fn boundary_and_invisible_text_preserve_existing_policy() {
    assert_text(&document("BT /F1 6 Tf 49 10 Td (A) Tj ET", BOUNDS, IDENTITY, "/Fm Do", None), "A");
    let pdf = document("BT /F1 6 Tf 3 Tr 10 10 Td (OCR) Tj ET", BOUNDS, IDENTITY, "/Fm Do", None);
    assert_eq!(extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap().plain_text(), "OCR");
    assert_eq!(
        extract_page_filtered(&pdf, 0, &InterpreterSettings::default(), TextFilter { images: true, ..Default::default() }).unwrap().plain_text(),
        ""
    );
}

#[test]
fn unsupported_clips_and_repeated_forms_restore_supported_ancestors() {
    // The existing layout treats these widely separated runs as separate lines.
    assert_eq!(super::page_text("BT /F2 6 Tf 10 10 Td (IN) Tj 100 0 Td (IN) Tj ET BT /F2 6 Tf 200 100 Td (PAGE) Tj ET"), "PAGE\nIN\nIN");
    let curve = "q 0 0 m 0 50 50 50 50 0 c h W n BT /F1 6 Tf 100 100 Td (HIDDEN) Tj ET Q BT /F1 6 Tf 10 10 Td (IN) Tj ET";
    assert_text(&document(curve, BOUNDS, IDENTITY, "/Fm Do q 1 0 0 1 100 0 cm /Fm Do Q BT /F1 6 Tf 200 100 Td (PAGE) Tj ET", None), "PAGE\nIN\nIN");
}

#[test]
fn mixed_actual_text_does_not_replace_later_redraws_or_weaken_filters() {
    let mixed = "/Span <</ActualText (SECRET VISIBLE)>> BDC /Span <</ActualText (NESTED)>> BDC BT /F1 6 Tf 100 10 Td (SECRET) Tj ET EMC BT /F1 6 Tf 10 10 Td (VISIBLE) Tj ET EMC BT /F1 6 Tf 10 10 Td (VISIBLE) Tj ET";
    assert_text(&document(mixed, BOUNDS, IDENTITY, "/Fm Do", None), "VISIBLE");
    let semantic = "/Span <</ActualText (REPLACEMENT)>> BDC /Figure BMC BT /F1 6 Tf 10 30 Td (EXCLUDED) Tj ET EMC BT /F1 6 Tf 100 10 Td (HIDDEN) Tj ET BT /F1 6 Tf 10 10 Td (KEPT) Tj ET EMC";
    let pdf = document(semantic, BOUNDS, IDENTITY, "/Fm Do", None);
    let error = extract_page_filtered(&pdf, 0, &InterpreterSettings::default(), TextFilter { images: true, ..Default::default() }).unwrap_err();
    assert!(error.contains("mixes included and excluded"));
}

#[test]
fn upstream_crop_box_and_clipping_only_mode_are_preserved() {
    // Current upstream excludes text outside the page independently of form clipping.
    assert_eq!(super::page_text("BT /F2 6 Tf 500 500 Td (OUTSIDE-PAGE) Tj ET"), "");
    // Upstream PR3116 emits mode7 for extraction, including when image text is filtered.
    assert_text(&document("BT /F1 6 Tf 7 Tr 10 10 Td (CLIP) Tj ET", BOUNDS, IDENTITY, "/Fm Do", None), "CLIP");
}

#[test]
fn clip_geometry_retains_contact_and_degeneracy_but_rejects_separation() {
    use kurbo::Point;
    let quad = |r: Rect| [Point::new(r.x0, r.y0), Point::new(r.x1, r.y0), Point::new(r.x1, r.y1), Point::new(r.x0, r.y1)];
    let mut clips = clips::TextClips::default();
    clips.push(&Rect::new(0., 0., 50., 50.).to_path(0.1));
    assert!(clips.visible(&quad(Rect::new(50., 10., 55., 15.))), "tangent contact stays conservative");
    assert!(clips.visible(&quad(Rect::new(10., 10., 10., 15.))), "a line inside is not certainly hidden");
    assert!(!clips.visible(&quad(Rect::new(60., 10., 60., 15.))), "a disjoint line is hidden");
    assert!(clips.visible(&quad(Rect::new(49., 10., 55., 15.))), "straddling text remains complete");
    let uncertain = BezPath::from_vec(vec![kurbo::PathEl::MoveTo(Point::new(f64::NAN, 0.))]);
    clips.push(&uncertain);
    assert!(!clips.visible(&quad(Rect::new(60., 10., 65., 15.))), "unknown geometry preserves the ancestor");
    clips.pop();
    clips.push(&Rect::new(0., 0., 0., 50.).to_path(0.1));
    assert!(!clips.visible(&quad(Rect::new(0., 10., 0., 10.))), "exactly empty clip hides even a point");
    clips.pop();
    clips.pop();
    assert!(clips.visible(&quad(Rect::new(60., 10., 65., 15.))), "pops restore the unbounded page");
}

#[test]
fn intersected_quadrilaterals_keep_more_than_four_boundary_edges() {
    use kurbo::Point;
    let mut clips = clips::TextClips::default();
    clips.push(&Rect::new(-2., -2., 2., 2.).to_path(0.1));
    let diamond = BezPath::from_vec(vec![
        kurbo::PathEl::MoveTo(Point::new(0., -3.)),
        kurbo::PathEl::LineTo(Point::new(3., 0.)),
        kurbo::PathEl::LineTo(Point::new(0., 3.)),
        kurbo::PathEl::LineTo(Point::new(-3., 0.)),
        kurbo::PathEl::ClosePath,
    ]);
    clips.push(&diamond);
    for sign in [-1., 1.] {
        let inside = [Point::new(sign * 1.8, 0.); 4];
        let outside = [Point::new(sign * 1.8, sign * 1.8); 4];
        assert!(clips.visible(&inside));
        assert!(!clips.visible(&outside));
    }
}

#[test]
fn mixed_replacement_does_not_invent_unicode_for_unmapped_survivors() {
    let ops = "/Span <</ActualText (SECRET)>> BDC BT /F1 6 Tf 100 10 Td <81> Tj ET BT /F1 6 Tf 10 10 Td <80> Tj ET EMC";
    let mut doc = form_document(ops, BOUNDS, IDENTITY, "/Fm Do", None);
    let font_id = *doc
        .objects
        .iter()
        .find(|(_, value)| value.as_dict().ok().and_then(|d| d.get(b"Type").ok()).and_then(|v| v.as_name().ok()) == Some(b"Font"))
        .unwrap()
        .0;
    let glyph = doc.add_object(Stream::new(dictionary! {}, b"600 0 d0 0 0 500 700 re f".to_vec()));
    doc.objects.insert(font_id, dictionary! {
        "Type" => "Font", "Subtype" => "Type3", "FontBBox" => vec![0.into(),0.into(),600.into(),700.into()],
        "FontMatrix" => vec![0.001.into(),0.into(),0.into(),0.001.into(),0.into(),0.into()],
        "FirstChar" => 128, "LastChar" => 129, "Widths" => vec![600.into(),600.into()],
        "Encoding" => dictionary! { "Type" => "Encoding", "Differences" => vec![128.into(), Object::Name(b"UnknownGlyph".to_vec()),Object::Name(b"B".to_vec())] },
        "CharProcs" => dictionary! { "UnknownGlyph" => glyph, "B" => glyph }, "Resources" => dictionary! {},
    }.into());
    // The original replacement is usable when both glyphs are inside. Once the mapped glyph is clipped,
    // The non-ASCII survivor has no Unicode mapping, so no fragment of SECRET can be assigned to it honestly.
    assert_text(&to_pdf(doc.clone()), "");
    let form_id = *doc
        .objects
        .iter()
        .find(|(_, value)| value.as_stream().ok().is_some_and(|s| s.dict.get(b"Subtype").ok().and_then(|v| v.as_name().ok()) == Some(b"Form")))
        .unwrap()
        .0;
    doc.get_object_mut(form_id).unwrap().as_stream_mut().unwrap().dict.set("BBox", vec![0.into(), 0.into(), 150.into(), 50.into()]);
    assert_text(&to_pdf(doc), "SECRET");
}

#[test]
fn dense_text_inside_simple_and_nested_forms_remains_complete() {
    let line = "A".repeat(1000);
    let body: String = (0..70).map(|row| format!("BT /F1 0.25 Tf 100 {} Td ({line}) Tj ET\n", 100 + row * 2)).collect();
    let bounds = [0., 0., 400., 400.];
    let expected = vec![line; 70].join("\n");
    assert_text(&document(&body, bounds, IDENTITY, "/Fm Do", None), &expected);
    // Rotated inner and outer bounds intersect in more than four edges. All text stays
    // in their common center, so clipping complexity must not become a glyph-count limit.
    assert_text(&document("/Inner Do", bounds, IDENTITY, "/Fm Do", Some((&body, bounds, [0.707, 0.707, -0.707, 0.707, 200., -82.8]))), &expected);
}

#[test]
fn uncertain_and_compound_clips_do_not_clear_the_ancestor() {
    use kurbo::Point;
    let mut clips = clips::TextClips::default();
    clips.push(&Rect::new(0., 0., 50., 50.).to_path(0.1));
    let inside = [Point::new(10., 10.); 4];
    let outside = [Point::new(60., 10.); 4];
    let mut compound = Rect::new(1., 1., 2., 2.).to_path(0.1);
    compound.extend(Rect::new(3., 3., 4., 4.).to_path(0.1).elements().iter().copied());
    clips.push(&compound);
    assert!(clips.visible(&inside));
    assert!(!clips.visible(&outside));
    clips.pop();
    // Subtracting these finite endpoints overflows; do not treat uncertainty as an empty clip.
    clips.push(&Rect::new(-f64::MAX, -f64::MAX, f64::MAX, f64::MAX).to_path(0.1));
    assert!(clips.visible(&inside));
    assert!(!clips.visible(&outside));
    clips.pop();
    clips.push(&Rect::new(0., 0., 1e-10, 1e-10).to_path(0.1));
    assert!(clips.visible(&[Point::new(0.5e-10, 0.5e-10); 4]));
}

#[test]
fn sequential_clips_preserve_text_and_later_bounds() {
    // Restored clips do not increase live complexity, even on a large vector page.
    let mut body = "q 0 0 m 50 0 l 60 50 l 10 50 l h W n Q\n".repeat(40_000);
    body.push_str("q 0 0 m 20 0 l 25 50 l 5 50 l h W n BT /F1 6 Tf 35 10 Td (HIDDEN) Tj ET Q ");
    body.push_str("BT /F1 6 Tf 10 10 Td (INSIDE) Tj ET BT /F1 6 Tf 100 100 Td (OUTSIDE) Tj ET");
    let pdf = document(&body, BOUNDS, IDENTITY, "/Fm Do BT /F1 6 Tf 100 100 Td (PAGE) Tj ET", None);
    assert_text(&pdf, "PAGE\nINSIDE");
}

#[test]
fn redaction_retains_inside_page_text_outside_form_bounds() {
    let pdf = document("BT /F1 6 Tf 100 10 Td (CLIPPED) Tj ET", BOUNDS, IDENTITY, "/Fm Do", None);
    let visual = extract_page_filtered(&pdf, 0, &InterpreterSettings::default(), TextFilter::default()).unwrap();
    assert_eq!(visual.plain_text(), "");
    assert_eq!(extract_page_for_redaction(&pdf, 0, &InterpreterSettings::default()).unwrap().plain_text(), "CLIPPED");
}
