//! Original tiny appearance signatures: physical sizing, padding and atomic undo.

use std::sync::Arc;

use pdfcraft_cos::{Document as Cos, Object, SaveOptions};
use pdfcraft_engine::{
    Edit, MarkFile, NewAnnotation, Session, Shape, SignatureImage, SignaturePlacementSize, Style, signature_placement_with_size,
    typed_signature_shape,
};

fn page(rotation: u16, unit: f64) -> (Session, pdfcraft_engine::DocId) {
    let mut session = Session::new().with_clock(|| 1_700_000_000);
    let bytes = session.create_blank(300.0, 400.0, 1).unwrap();
    let mut cos = Cos::open(bytes).unwrap();
    let p = pdfcraft_annot::page_refs(&cos).unwrap()[0];
    cos.update_dict(p, |d| {
        let crop = || Object::Array([10.0, 20.0, 310.0, 420.0].map(Object::Real).to_vec());
        d.set(b"MediaBox".to_vec(), crop());
        d.set(b"CropBox".to_vec(), crop());
        d.set(b"Rotate".to_vec(), Object::Int(i64::from(rotation)));
        d.set(b"UserUnit".to_vec(), Object::Real(unit));
    })
    .unwrap();
    let bytes = Arc::new(pdfcraft_cos::write_full(&cos, &SaveOptions::default()).unwrap());
    let id = session.open_new("original-signature-page.pdf", bytes).unwrap();
    (session, id)
}

fn annotation(shape: Shape) -> Edit {
    Edit::AddAnnotation(NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: String::new(), author: "Synthetic User".into() })
}

fn image() -> SignatureImage {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 60, 20);
        encoder.set_color(png::ColorType::Rgba);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[20, 40, 60, 128].repeat(60 * 20)).unwrap();
    }
    SignatureImage::from_bytes(&bytes).unwrap()
}

fn placement(kind: usize, info: &pdfcraft_render::PageInfo, at: [f64; 2]) -> Edit {
    match kind {
        0 => annotation(typed_signature_shape(at, "Synthetic User", 32.0, i64::from(info.rotation)).unwrap()),
        1 => annotation(Shape::Signature {
            // Uneven segments exercise the smoothed-ink control bounds and fixed padding.
            strokes: vec![vec![[80.0, 160.0], [110.0, 185.0], [155.0, 165.0], [190.0, 180.0]]],
        }),
        _ => image().edit(0, info, at, false, "Synthetic User").unwrap(),
    }
}

#[test]
fn signature_size_matches_physical_rect_for_all_graphics_rotations_and_user_units() {
    for rotation in [0, 90, 180, 270] {
        for unit in [1.0, 2.5] {
            for kind in 0..3 {
                let (mut s, id) = page(rotation, unit);
                let info = s.get(id).unwrap().info.pages[0].clone();
                let at = [100.0, 180.0];
                let size = SignaturePlacementSize::new(65.0, 17.0).unwrap();
                let expected = size.rect_at(&info, at).unwrap();
                let before = s.save_bytes(id).unwrap();
                let edit = signature_placement_with_size(placement(kind, &info, at), &info, at, size).unwrap();
                s.apply(id, edit).unwrap();
                let d = s.get(id).unwrap();
                assert_eq!(d.info.annotations.len(), 1);
                let actual = d.signature_placement_size(0, 0).unwrap().unwrap();
                assert!((actual.width() - 65.0).abs() < 0.001, "{rotation}/{unit}/{kind}: {actual:?}");
                assert!((actual.height() - 17.0).abs() < 0.001, "{rotation}/{unit}/{kind}: {actual:?}");
                for (a, b) in d.info.annotations[0].rect.into_iter().map(f64::from).zip(expected) {
                    assert!((a - b).abs() < 0.001, "the preview and actual anchor must agree");
                }
                let after = s.save_bytes(id).unwrap();
                s.undo(id).unwrap();
                assert_eq!(s.save_bytes(id).unwrap(), before, "one undo removes the complete placement");
                assert!(s.get(id).unwrap().can_undo().is_none());
                s.redo(id).unwrap();
                assert_eq!(s.save_bytes(id).unwrap(), after);
                let reopened = s.open_new("reopened.pdf", after).unwrap();
                let actual = s.get(reopened).unwrap().signature_placement_size(0, 0).unwrap().unwrap();
                assert!((actual.width() - 65.0).abs() < 0.001 && (actual.height() - 17.0).abs() < 0.001);
            }
        }
    }
}

#[test]
fn signature_size_keeps_legacy_geometry_without_an_explicit_request() {
    let (mut s, id) = page(0, 1.0);
    let info = s.get(id).unwrap().info.pages[0].clone();
    s.apply(id, placement(0, &info, [30.0, 250.0])).unwrap();
    assert_eq!(s.get(id).unwrap().signature_placement_size(0, 0).unwrap().unwrap().height(), 32.0);
    s.apply(id, image().edit(0, &info, [30.0, 180.0], true, "").unwrap()).unwrap();
    let actual = s.get(id).unwrap().signature_placement_size(0, 1).unwrap().unwrap();
    assert_eq!((actual.width(), actual.height()), (72.0, 24.0));
    assert_eq!(&image().rgba()[..4], &[20, 40, 60, 128]);
}

#[test]
fn remembered_drawn_signature_places_at_the_same_physical_size_on_userunit_75() {
    // The real desktop Draw/remember/place workflow succeeds on UserUnit 1, but
    // used to reject the same physical size on a later UserUnit 75 page.
    let size = SignaturePlacementSize::new(111.1086923480034, 20.456523060798645).unwrap();
    for unit in [1.0, 75.0] {
        let (mut s, id) = page(0, unit);
        let info = s.get(id).unwrap().info.pages[0].clone();
        let at = [100.0, 180.0];
        let strokes = vec![vec![[120.0, 170.0], [152.0, 187.0]], vec![[151.0, 187.0], [175.0, 170.0]], vec![[174.0, 170.0], [228.0, 187.0]]];
        let before = s.save_bytes(id).unwrap();
        let edit = signature_placement_with_size(annotation(Shape::Signature { strokes }), &info, at, size).unwrap();
        s.apply(id, edit).expect("the remembered drawn signature must remain placeable on UserUnit 75");
        let actual = s.get(id).unwrap().signature_placement_size(0, 0).unwrap().unwrap();
        assert!((actual.width() - size.width()).abs() < 0.001, "UserUnit {unit}: {actual:?}");
        assert!((actual.height() - size.height()).abs() < 0.001, "UserUnit {unit}: {actual:?}");
        s.undo(id).unwrap();
        assert_eq!(s.save_bytes(id).unwrap(), before, "one undo removes the entire placement");
        assert!(s.get(id).unwrap().can_undo().is_none());
    }
}

#[test]
fn signature_size_rejects_hostile_dimensions_and_transforms() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 0.0, 0.009, 14400.01] {
        assert!(SignaturePlacementSize::new(bad, 10.0).is_err());
        assert!(SignaturePlacementSize::new(10.0, bad).is_err());
    }
    let (s, id) = page(0, 1.0);
    let info = &s.get(id).unwrap().info.pages[0];
    let size = SignaturePlacementSize::new(65.0, 17.0).unwrap();
    for at in [[f64::NAN, 1.0], [1.0, f64::INFINITY], [f64::MAX, 1.0]] {
        assert!(size.rect_at(info, at).is_err());
    }
    for kind in 0..6 {
        let mut invalid = info.clone();
        match kind {
            0 => invalid.crop[2] = invalid.crop[0],
            1 => invalid.crop[1] = f32::NAN,
            2 => invalid.width = 0.0,
            3 => invalid.height = f32::INFINITY,
            4 => invalid.rotation = 45,
            _ => invalid.width *= 2.0,
        }
        assert!(size.rect_at(&invalid, [100.0, 180.0]).is_err());
    }
}

#[test]
fn signature_size_rejects_wrong_kinds_and_runtime_failures_atomically() {
    let (mut s, id) = page(0, 1.0);
    let info = s.get(id).unwrap().info.pages[0].clone();
    let size = SignaturePlacementSize::new(65.0, 17.0).unwrap();
    assert!(signature_placement_with_size(annotation(Shape::Rectangle { rect: [10.0, 10.0, 50.0, 50.0] }), &info, [20.0, 100.0], size).is_err());
    let before = s.save_bytes(id).unwrap();
    let before_dirty = s.get(id).unwrap().dirty;
    // A PDF stamp with a forged signature name is not an image signature. Its import
    // succeeds before the kind check fails, exercising rollback of newly added objects.
    let fake = Edit::AddCustomStamp {
        page: 0,
        rect: [20.0, 100.0, 80.0, 120.0],
        name: "Signature".into(),
        file: MarkFile { name: "original.pdf".into(), bytes: before.clone(), page: 0 },
        author: String::new(),
    };
    let fake = signature_placement_with_size(fake, &info, [20.0, 100.0], size).unwrap();
    assert!(s.apply(id, fake).is_err());
    assert_eq!(s.save_bytes(id).unwrap(), before);
    assert_eq!(s.get(id).unwrap().dirty, before_dirty);
    assert!(s.get(id).unwrap().can_undo().is_none());
    let tiny = SignaturePlacementSize::new(0.01, 0.01).unwrap();
    let edit = signature_placement_with_size(placement(1, &info, [20.0, 100.0]), &info, [20.0, 100.0], tiny).unwrap();
    assert!(s.apply(id, edit).is_err(), "an ink rectangle smaller than its stroke padding cannot fit");
    assert_eq!(s.save_bytes(id).unwrap(), before);
    assert_eq!(s.get(id).unwrap().dirty, before_dirty);
    assert!(s.get(id).unwrap().info.annotations.is_empty());
    assert!(s.get(id).unwrap().can_undo().is_none());
    // Refusal does not poison the next successful placement.
    s.apply(id, signature_placement_with_size(placement(2, &info, [20.0, 100.0]), &info, [20.0, 100.0], size).unwrap()).unwrap();
    assert_eq!(s.get(id).unwrap().info.annotations.len(), 1);
}

#[test]
fn signature_size_query_ignores_marks_widgets_and_unrelated_stamps() {
    let (mut s, id) = page(0, 1.0);
    for shape in [
        Shape::Rectangle { rect: [10.0, 10.0, 50.0, 50.0] },
        Shape::Mark { rect: [60.0, 10.0, 80.0, 30.0], mark: pdfcraft_engine::FillMark::Check },
        Shape::Typewriter { rect: [90.0, 10.0, 140.0, 30.0], font_size: 10.0 },
        Shape::Stamp { rect: [150.0, 10.0, 220.0, 40.0], stamp: pdfcraft_engine::StampKind::Draft, by: None },
    ] {
        s.apply(id, annotation(shape)).unwrap();
    }
    for index in 0..4 {
        assert_eq!(s.get(id).unwrap().signature_placement_size(0, index).unwrap(), None);
    }
    assert!(s.get(id).unwrap().signature_placement_size(0, 99).is_err());
    assert!(s.get(id).unwrap().signature_placement_size(1, 0).is_err());
}

#[test]
fn signature_size_does_not_resize_existing_instances_and_renders_the_full_image() {
    let (mut s, id) = page(90, 2.5);
    let info = s.get(id).unwrap().info.pages[0].clone();
    let at = [100.0, 180.0];
    let size = SignaturePlacementSize::new(65.0, 17.0).unwrap();
    s.apply(id, signature_placement_with_size(placement(2, &info, at), &info, at, size).unwrap()).unwrap();
    let original = s.get(id).unwrap().info.annotations[0].rect;
    let rendered = pdfcraft_render::PageRenderer::new(s.get(id).unwrap().display.clone(), pdfcraft_render::RenderConfig::default())
        .render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(rendered.error.is_none(), "{:?}", rendered.error);
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (i, pixel) in rendered.rgba.as_chunks::<4>().0.iter().enumerate() {
        if pixel[0] < 200 && pixel[1] < 200 && pixel[2] < 200 {
            let x = i as u32 % rendered.width;
            let y = i as u32 / rendered.width;
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    assert!(x0 <= x1 && y0 <= y1, "the original transparent image must draw");
    assert!((64..=67).contains(&(x1 - x0 + 1)), "physical width: {}", x1 - x0 + 1);
    assert!((16..=19).contains(&(y1 - y0 + 1)), "physical height: {}", y1 - y0 + 1);
    let second = SignaturePlacementSize::new(90.0, 25.0).unwrap();
    let at = [150.0, 270.0];
    s.apply(id, signature_placement_with_size(placement(2, &info, at), &info, at, second).unwrap()).unwrap();
    let document = s.get(id).unwrap();
    assert_eq!(document.info.annotations.len(), 2);
    // Inspection sorts annotations by their displayed vertical position, independently
    // of their stable page /Annots indices. The higher second placement sorts first.
    let first = document.info.annotations.iter().find(|a| a.page == 0 && a.index == 0).unwrap();
    assert_eq!(first.rect, original);
    let actual = s.get(id).unwrap().signature_placement_size(0, 1).unwrap().unwrap();
    assert!((actual.width() - 90.0).abs() < 0.001 && (actual.height() - 25.0).abs() < 0.001);
}

fn rendered_signature_ink_bounds(bytes: Arc<Vec<u8>>) -> [f64; 4] {
    const SCALE: f32 = 2.0;
    let rendered = pdfcraft_render::PageRenderer::new(bytes, pdfcraft_render::RenderConfig::default()).render(pdfcraft_render::RenderRequest {
        page: 0,
        scale: SCALE,
        ..Default::default()
    });
    assert!(rendered.error.is_none(), "{:?}", rendered.error);
    assert!(rendered.width > 0 && rendered.height > 0);
    let mut bounds: Option<[u32; 4]> = None;
    for (index, pixel) in rendered.rgba.as_chunks::<4>().0.iter().enumerate() {
        if pixel[0] < 200 && pixel[1] < 200 && pixel[2] < 200 {
            let index = u32::try_from(index).unwrap();
            let (x, y) = (index % rendered.width, index / rendered.width);
            bounds = Some(match bounds {
                Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)],
                None => [x, y, x + 1, y + 1],
            });
        }
    }
    // Exclusive right/bottom edges describe the full painted pixel footprint in points.
    bounds.expect("the saved signature appearance must paint visible ink").map(|edge| f64::from(edge) / f64::from(SCALE))
}

fn compact_physical_page(rotation: u16, unit: f64) -> (Session, pdfcraft_engine::DocId) {
    let mut session = Session::new().with_clock(|| 1_700_000_000);
    let bytes = session.create_blank(300.0, 400.0, 1).unwrap();
    let mut cos = Cos::open(bytes).unwrap();
    let page = pdfcraft_annot::page_refs(&cos).unwrap()[0];
    cos.update_dict(page, |d| {
        let crop = || Object::Array([10.0 / unit, 20.0 / unit, 310.0 / unit, 420.0 / unit].map(Object::Real).to_vec());
        d.set(b"MediaBox".to_vec(), crop());
        d.set(b"CropBox".to_vec(), crop());
        d.set(b"Rotate".to_vec(), Object::Int(i64::from(rotation)));
        d.set(b"UserUnit".to_vec(), Object::Real(unit));
    })
    .unwrap();
    let bytes = Arc::new(pdfcraft_cos::write_full(&cos, &SaveOptions::default()).unwrap());
    let id = session.open_new("physical-signature-control.pdf", bytes).unwrap();
    (session, id)
}

fn assert_signature_stroke_and_local_appearance(bytes: Arc<Vec<u8>>, unit: f64, physical_width: f64) {
    let cos = Cos::open(bytes).unwrap();
    let page = cos.get(pdfcraft_annot::page_refs(&cos).unwrap()[0]);
    let annotations = cos.resolve(page.as_dict().unwrap().get(b"Annots").unwrap());
    let annotation = cos.resolve(&annotations.as_array().unwrap()[0]);
    let d = annotation.as_dict().unwrap();
    let border = cos.resolve(d.get(b"BS").unwrap());
    let width = border.as_dict().unwrap().get(b"W").unwrap().as_f64().unwrap();
    assert!((width * unit - physical_width).abs() < 1e-9, "the stored raw stroke keeps its physical width");
    let ap = cos.resolve(d.get(b"AP").unwrap());
    let normal = cos.resolve(ap.as_dict().unwrap().get(b"N").unwrap());
    let Object::Stream(stream) = &*normal else { panic!("normal appearance must be a stream") };
    let bbox: Vec<f64> = stream.dict.get(b"BBox").unwrap().as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert_eq!(&bbox[..2], &[0.0, 0.0], "the appearance is local, even on a nonzero crop");
    assert!((bbox[2].max(bbox[3]) - 1000.0).abs() < 1e-6);
    let content = String::from_utf8(stream.decoded().unwrap()).unwrap();
    let tokens: Vec<&str> = content.split_whitespace().collect();
    let index = tokens.iter().position(|token| *token == "w").unwrap();
    let local_width = tokens[index - 1].parse::<f64>().unwrap();
    assert!(local_width > 0.0, "a positive physical stroke must not become a hairline");
    let rect: Vec<f64> = d.get(b"Rect").unwrap().as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    let longest_physical_side = (rect[2] - rect[0]).max(rect[3] - rect[1]) * unit;
    assert!((local_width * longest_physical_side / 1000.0 - physical_width).abs() < 0.001);
}

#[test]
fn drawn_signature_keeps_physical_ink_after_resize_style_and_reopen_across_user_units() {
    for rotation in [0, 90, 180, 270] {
        for unit in [1.0, 2.5, 75.0, 75000.0] {
            let (mut session, id) = compact_physical_page(rotation, unit);
            let info = session.get(id).unwrap().info.pages[0].clone();
            let at = [100.0 / unit, 180.0 / unit];
            // Independent straight diagonals have exact control bounds. Rotate the
            // template back, as the desktop does before the engine sees its strokes.
            let [a, b, c, d, ..] = pdfcraft_model::view_matrix_for(i64::from(rotation), [0.0; 4]);
            let strokes = vec![vec![[0.0, 0.0], [50.0, 15.0], [100.0, 30.0]], vec![[0.0, 30.0], [50.0, 15.0], [100.0, 0.0]]]
                .into_iter()
                .map(|stroke| stroke.into_iter().map(|[x, y]| [at[0] + a * x + c * y, at[1] + b * x + d * y]).collect())
                .collect();
            let shape = Shape::Signature { strokes };
            let edit = Edit::AddAnnotation(NewAnnotation {
                page: 0,
                style: Style::default_for(&shape),
                shape,
                contents: String::new(),
                author: String::new(),
            });
            let size = SignaturePlacementSize::new(65.0, 17.0).unwrap();
            session.apply(id, signature_placement_with_size(edit, &info, at, size).unwrap()).unwrap();
            let bytes = session.save_bytes(id).unwrap();
            assert_signature_stroke_and_local_appearance(bytes.clone(), unit, 1.5);
            let reopened = session.open_new("drawn-roundtrip.pdf", bytes).unwrap();
            let rect = SignaturePlacementSize::new(75.0, 21.0).unwrap().rect_at(&info, at).unwrap();
            session.apply(reopened, Edit::ResizeAnnotation { page: 0, index: 0, rect }).unwrap();
            session
                .apply(reopened, Edit::StyleAnnotation { page: 0, index: 0, color: Some([0.0; 3]), opacity: None, width: None, endings: None })
                .unwrap();
            let bytes = session.save_bytes(reopened).unwrap();
            assert_signature_stroke_and_local_appearance(bytes.clone(), unit, 1.5);
            let actual = rendered_signature_ink_bounds(bytes);
            let [x, y] = info.user_to_view(at[0] as f32, at[1] as f32).map(f64::from);
            let expected = [x + 1.0, y - 10.5 + 1.0, x + 75.0 - 1.0, y + 10.5 - 1.0];
            for (edge, wanted) in actual.into_iter().zip(expected) {
                assert!((edge - wanted).abs() <= 1.0, "rotation {rotation}, UserUnit {unit}: ink {actual:?}, physical control {expected:?}");
            }
        }
    }
}

#[test]
fn drawn_signature_dots_keep_their_physical_radius_after_reopen_and_resize() {
    for unit in [1.0, 75.0, 75000.0] {
        let (mut session, id) = compact_physical_page(0, unit);
        let info = session.get(id).unwrap().info.pages[0].clone();
        let at = [100.0 / unit, 180.0 / unit];
        let shape = Shape::Signature { strokes: vec![vec![[0.0, 0.0]], vec![[10.0, 10.0], [10.0, 10.0]]] };
        let edit =
            Edit::AddAnnotation(NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: String::new(), author: String::new() });
        session.apply(id, signature_placement_with_size(edit, &info, at, SignaturePlacementSize::new(17.0, 17.0).unwrap()).unwrap()).unwrap();
        let before = session.save_bytes(id).unwrap();
        let reopened = session.open("dot-roundtrip.pdf", None, before.clone(), None).unwrap();
        let rect = SignaturePlacementSize::new(31.0, 21.0).unwrap().rect_at(&info, at).unwrap();
        session.apply(reopened, Edit::ResizeAnnotation { page: 0, index: 0, rect }).unwrap();
        let after = session.save_bytes(reopened).unwrap();
        assert_signature_stroke_and_local_appearance(after.clone(), unit, 1.5);
        let actual = rendered_signature_ink_bounds(after.clone());
        let [x, y] = info.user_to_view(at[0] as f32, at[1] as f32).map(f64::from);
        let expected = [x + 1.0, y - 9.5, x + 30.0, y + 9.5];
        for (edge, wanted) in actual.into_iter().zip(expected) {
            assert!((edge - wanted).abs() <= 1.0, "UserUnit {unit}: dot footprint {actual:?}, expected {expected:?}");
        }
        let pixels = rendered_signature_rgba(after.clone(), 4.0);
        let dark = pixels.2.as_chunks::<4>().0.iter().filter(|p| p[0] < 200 && p[1] < 200 && p[2] < 200).count();
        assert!((20..100).contains(&dark), "UserUnit {unit}: two physical dots must not become long raw-unit stubs ({dark} dark pixels)");
        session.undo(reopened).unwrap();
        assert_eq!(session.save_bytes(reopened).unwrap(), before, "one undo removes only the complete resize");
        session.redo(reopened).unwrap();
        assert_eq!(session.save_bytes(reopened).unwrap(), after);
    }
}

fn typed_signature_visible_fractions() -> [f64; 4] {
    let outline = pdfcraft_engine::script_outline("Synthetic User");
    let [x0, y0, x1, y1] = outline.bounds();
    // Layout bounds include ascent, descent and advance space. Only contour extrema
    // describe the visible ink; read them before the PDF appearance transform runs.
    let ink = outline
        .contours
        .iter()
        .filter(|contour| contour.len() > 2)
        .flatten()
        .fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |[left, bottom, right, top], [x, y]| {
            [left.min(*x), bottom.min(*y), right.max(*x), top.max(*y)]
        });
    assert!(ink.iter().all(|edge| edge.is_finite()));
    assert!(x1 > x0 && y1 > y0);
    let [left, bottom, right, top] = ink;
    let fractions = [(left - x0) / (x1 - x0), (bottom - y0) / (y1 - y0), (right - x0) / (x1 - x0), (top - y0) / (y1 - y0)];
    assert!(fractions.iter().all(|fraction| fraction.is_finite() && (0.0..=1.0).contains(fraction)));
    fractions
}

#[test]
fn signature_size_typed_and_drawn_appearances_render_at_requested_physical_bounds_after_save() {
    let size = SignaturePlacementSize::new(65.0, 17.0).unwrap();
    let [ink_left, ink_bottom, ink_right, ink_top] = typed_signature_visible_fractions();
    for rotation in [0, 90, 180, 270] {
        for unit in [1.0, 2.5] {
            for kind in 0..2 {
                let (mut session, id) = page(rotation, unit);
                let info = session.get(id).unwrap().info.pages[0].clone();
                let at = [100.0, 180.0];
                let edit = if kind == 0 {
                    placement(0, &info, at)
                } else {
                    // Three collinear points per stroke exercise the smooth-ink path while
                    // keeping its extrema exact, unlike the irregular curve fixture above.
                    annotation(Shape::Signature {
                        strokes: vec![vec![[80.0, 160.0], [135.0, 172.5], [190.0, 185.0]], vec![[80.0, 185.0], [135.0, 172.5], [190.0, 160.0]]],
                    })
                };
                session.apply(id, signature_placement_with_size(edit, &info, at, size).unwrap()).unwrap();
                let bytes = session.save_bytes(id).unwrap();
                let reopened = session.open_new("signature-render-roundtrip.pdf", bytes.clone()).unwrap();
                let actual_size = session.get(reopened).unwrap().signature_placement_size(0, 0).unwrap().unwrap();
                assert!((actual_size.width() - 65.0).abs() < 0.001 && (actual_size.height() - 17.0).abs() < 0.001);

                // Render the stored bytes directly: no engine display copy can hide a broken
                // saved appearance. The white page has no other ink, so misplaced paint counts.
                let actual = rendered_signature_ink_bounds(bytes);
                let [x, y] = info.user_to_view(at[0] as f32, at[1] as f32).map(f64::from);
                let expected = if kind == 0 {
                    // Source font contours occupy only part of the layout box. Their upright
                    // physical fractions are independent of the AP writer and page rotation.
                    [x + ink_left * 65.0, y + 8.5 - ink_top * 17.0, x + ink_right * 65.0, y + 8.5 - ink_bottom * 17.0]
                } else {
                    // The stroke radius and extra padding are physical points, so the
                    // visible inset stays one point on every target page.
                    [x + 1.0, y - 8.5 + 1.0, x + 65.0 - 1.0, y + 8.5 - 1.0]
                };
                assert!(actual.iter().chain(&expected).all(|edge| edge.is_finite()));
                for (edge, wanted) in actual.into_iter().zip(expected) {
                    assert!(
                        (edge - wanted).abs() <= 1.5,
                        "rotation {rotation}, UserUnit {unit}, kind {kind}: rendered {actual:?}, expected {expected:?} (1.5 pt raster tolerance)"
                    );
                }
            }
        }
    }
}

#[test]
fn signature_size_uses_normal_annotation_permissions() {
    let (mut s, id) = page(0, 1.0);
    s.apply(
        id,
        Edit::Protect(pdfcraft_engine::Protection {
            permissions_password: Some("synthetic-owner".into()),
            algorithm: pdfcraft_cos::Algorithm::Aes128,
            ..Default::default()
        }),
    )
    .unwrap();
    let protected = s.save_bytes(id).unwrap();
    let restricted = s.open_new("restricted.pdf", protected).unwrap();
    let info = s.get(restricted).unwrap().info.pages[0].clone();
    let edit =
        signature_placement_with_size(placement(0, &info, [100.0, 180.0]), &info, [100.0, 180.0], SignaturePlacementSize::new(65.0, 17.0).unwrap())
            .unwrap();
    let before = s.save_bytes(restricted).unwrap();
    let before_dirty = s.get(restricted).unwrap().dirty;
    assert!(matches!(s.apply(restricted, edit), Err(pdfcraft_engine::EditError::NotPermitted("comments"))));
    assert_eq!(s.save_bytes(restricted).unwrap(), before);
    assert_eq!(s.get(restricted).unwrap().dirty, before_dirty);
    assert!(s.get(restricted).unwrap().can_undo().is_none());
}

#[derive(Clone, Copy, Debug)]
enum RawInkStyle {
    ZeroDash,
    ScaledDash,
    Solid,
    NegativeWidth,
    InvalidAlpha,
}

fn raw_signature_style_fixture(unit: f64, style: RawInkStyle, remove_appearance: bool) -> (Session, pdfcraft_engine::DocId, Arc<Vec<u8>>) {
    let (mut session, id) = compact_physical_page(0, unit);
    let strokes = vec![
        vec![[80.0 / unit, 160.0 / unit], [110.0 / unit, 185.0 / unit], [190.0 / unit, 180.0 / unit]],
        vec![[80.0 / unit, 185.0 / unit], [135.0 / unit, 172.5 / unit], [190.0 / unit, 160.0 / unit]],
    ];
    let shape = Shape::Signature { strokes };
    session.apply(id, annotation(shape)).unwrap();
    let original = session.save_bytes(id).unwrap();

    // Replace the generated style and optionally remove its AP so display must regenerate
    // paint from the actual /InkList, /BS, and /CA entries.
    let mut cos = Cos::open(original).unwrap();
    let page_ref = pdfcraft_annot::page_refs(&cos).unwrap()[0];
    let page = cos.get(page_ref);
    let annots = cos.resolve(page.as_dict().unwrap().get(b"Annots").unwrap());
    let annotation = annots.as_array().unwrap()[0].as_ref().unwrap();
    cos.update_dict(annotation, |dict| {
        if remove_appearance {
            dict.remove(b"AP");
        }
        let mut bs = pdfcraft_cos::Dict::new();
        let (width, style_name, dash) = match style {
            // The exact invalid-dash case under review: valid W=1, dashed style, all-zero D.
            RawInkStyle::ZeroDash => (1.0, "D", Some(vec![0.0, 0.0])),
            // Same one-physical-point stroke and dash lengths on a UserUnit 75 page.
            RawInkStyle::ScaledDash => (1.0 / unit, "D", Some(vec![2.0 / unit, 1.0 / unit])),
            RawInkStyle::Solid => (1.0 / unit, "S", None),
            RawInkStyle::NegativeWidth => (-1.0, "S", None),
            RawInkStyle::InvalidAlpha => (1.0, "S", None),
        };
        bs.set(b"W".to_vec(), Object::Real(width));
        bs.set(b"S".to_vec(), Object::name(style_name));
        if let Some(dash) = dash {
            bs.set(b"D".to_vec(), Object::Array(dash.into_iter().map(Object::Real).collect()));
        }
        dict.set(b"BS".to_vec(), Object::Dict(bs));
        dict.set(b"C".to_vec(), Object::Array(vec![Object::Int(0); 3]));
        if matches!(style, RawInkStyle::InvalidAlpha) {
            dict.set(b"CA".to_vec(), Object::name("not-a-number"));
        }
    })
    .unwrap();
    let fixture = Arc::new(pdfcraft_cos::write_full(&cos, &SaveOptions::default()).unwrap());
    let reopened = session.open("missing-appearance-signature.pdf", None, fixture.clone(), None).unwrap();
    assert_loaded_appearance_state_is_unchanged(&session, reopened, &fixture);
    (session, reopened, fixture)
}

fn assert_loaded_appearance_state_is_unchanged(session: &Session, id: pdfcraft_engine::DocId, source: &Arc<Vec<u8>>) {
    let doc = session.get(id).unwrap();
    assert_eq!(session.save_bytes(id).unwrap().as_ref(), source.as_ref(), "display-only fallback must not edit the source PDF");
    assert!(!doc.dirty);
    assert!(doc.can_undo().is_none(), "display-only fallback must not add undo history");
}

fn rendered_signature_rgba(display: Arc<Vec<u8>>, scale: f32) -> (u32, u32, Vec<u8>) {
    let rendered = pdfcraft_render::PageRenderer::new(display, pdfcraft_render::RenderConfig::default()).render(pdfcraft_render::RenderRequest {
        page: 0,
        scale,
        ..Default::default()
    });
    assert!(rendered.error.is_none(), "{:?}", rendered.error);
    assert!(rendered.width > 0 && rendered.height > 0);
    (rendered.width, rendered.height, rendered.rgba.to_vec())
}

#[test]
fn userunit_signature_style_fallbacks_keep_visible_ink_without_mutating_source() {
    // The invalid all-zero dash is compared with the ordinary solid fallback at the same
    // physical page and both raster scales. This fails if normalization drops its display AP.
    let (solid, solid_id, solid_source) = raw_signature_style_fixture(1.0, RawInkStyle::Solid, true);
    let (zero_dash, zero_dash_id, zero_dash_source) = raw_signature_style_fixture(1.0, RawInkStyle::ZeroDash, true);
    let dark_count = |pixels: &[u8]| pixels.as_chunks::<4>().0.iter().filter(|p| p[0] < 200 && p[1] < 200 && p[2] < 200).count();
    let solid_pixels = rendered_signature_rgba(solid.get(solid_id).unwrap().display.clone(), 1.0);
    let missing_pixels = rendered_signature_rgba(zero_dash.get(zero_dash_id).unwrap().display.clone(), 1.0);
    let control_count = dark_count(&solid_pixels.2);
    let missing_count = dark_count(&missing_pixels.2);
    assert!(control_count > 0, "the valid solid positive control must paint");
    assert!(missing_count > 0, "invalid all-zero dash must stay visible: {missing_count} dark pixels, solid control {control_count}");
    let solid_bounds = rendered_signature_ink_bounds(solid.get(solid_id).unwrap().display.clone());
    let dash_bounds = rendered_signature_ink_bounds(zero_dash.get(zero_dash_id).unwrap().display.clone());
    assert_eq!(dash_bounds, solid_bounds, "an all-zero invalid dash must recover as solid visible ink");
    for scale in [1.0, 2.0] {
        assert!(
            rendered_signature_rgba(zero_dash.get(zero_dash_id).unwrap().display.clone(), scale)
                == rendered_signature_rgba(solid.get(solid_id).unwrap().display.clone(), scale),
            "all-zero dash fallback must match the solid appearance at scale {scale}"
        );
    }
    assert_loaded_appearance_state_is_unchanged(&solid, solid_id, &solid_source);
    assert_loaded_appearance_state_is_unchanged(&zero_dash, zero_dash_id, &zero_dash_source);
    for style in [RawInkStyle::NegativeWidth, RawInkStyle::InvalidAlpha] {
        let (fallback, fallback_id, source) = raw_signature_style_fixture(1.0, style, true);
        for scale in [1.0, 2.0] {
            assert!(
                rendered_signature_rgba(fallback.get(fallback_id).unwrap().display.clone(), scale)
                    == rendered_signature_rgba(solid.get(solid_id).unwrap().display.clone(), scale),
                "malformed style {style:?} must match the solid default at scale {scale}"
            );
        }
        assert_loaded_appearance_state_is_unchanged(&fallback, fallback_id, &source);
    }
    let (scaled_dash, scaled_dash_id, scaled_source) = raw_signature_style_fixture(75.0, RawInkStyle::ScaledDash, true);
    let display = scaled_dash.get(scaled_dash_id).unwrap().display.clone();
    let (scaled_solid, scaled_solid_id, _) = raw_signature_style_fixture(75.0, RawInkStyle::Solid, true);
    let bounds = rendered_signature_ink_bounds(display.clone());
    assert!(bounds[0] < bounds[2] && bounds[1] < bounds[3]);
    for scale in [1.0, 2.0] {
        assert!(
            rendered_signature_rgba(display.clone(), scale)
                != rendered_signature_rgba(scaled_solid.get(scaled_solid_id).unwrap().display.clone(), scale),
            "a valid scaled dash must remain distinct from its same-page solid control"
        );
    }
    assert_loaded_appearance_state_is_unchanged(&scaled_dash, scaled_dash_id, &scaled_source);
}

#[test]
fn editing_invalid_dash_signature_refuses_atomically_and_preserves_its_existing_appearance() {
    // Keep the original valid /AP while making /BS invalid. A strict edit must refuse to
    // replace that appearance, dictionary, bytes, dirty state, or undo history.
    let (mut session, id, source) = raw_signature_style_fixture(1.0, RawInkStyle::ZeroDash, false);
    let before = session.save_bytes(id).unwrap();
    let dirty = session.get(id).unwrap().dirty;
    let undo = session.get(id).unwrap().can_undo().map(str::to_owned);
    let redo = session.get(id).unwrap().can_redo().map(str::to_owned);
    let rect = [80.0, 160.0, 190.0, 185.0];
    let result = session.apply(id, Edit::ResizeAnnotation { page: 0, index: 0, rect });
    assert!(result.is_err(), "an explicit edit must refuse an invalid dash rather than normalize over it");
    assert_eq!(session.save_bytes(id).unwrap(), before);
    assert_eq!(session.get(id).unwrap().dirty, dirty);
    assert_eq!(session.get(id).unwrap().can_undo().map(str::to_owned), undo);
    assert_eq!(session.get(id).unwrap().can_redo().map(str::to_owned), redo);
    assert_loaded_appearance_state_is_unchanged(&session, id, &source);
}

fn original_signature_pdf_with_raw_opacity(token: &str) -> Arc<Vec<u8>> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>".to_owned(),
        format!(
            "<< /Type /Annot /Subtype /Ink /Rect [0 0 100 100] /C [0 0 0] /Subj (Signature) /InkList [[10 10 90 90]] /BS << /W 1 /S /S >> /CA {token} >>"
        ),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    Arc::new(bytes)
}

#[test]
fn signature_display_clamps_oversized_numeric_opacity_from_original_pdf_bytes() {
    let huge = "9".repeat(309);
    let oversized_source = original_signature_pdf_with_raw_opacity(&huge);
    // Prove that the actual original-PDF parser produces the suspected nonfinite value.
    // A test that used Object::Real(inf) and then write_full would test opacity0 instead.
    let parsed = Cos::open(oversized_source.clone()).unwrap();
    let annotation = parsed.get(pdfcraft_cos::ObjRef::new(4, 0));
    let alpha = annotation.as_dict().unwrap().get(b"CA").unwrap().as_f64().unwrap();
    assert!(alpha.is_infinite() && alpha.is_sign_positive(), "the original309-digit CA must parse as positive infinity: {alpha}");
    assert!(annotation.as_dict().unwrap().get(b"AP").is_none(), "source starts without a normal appearance");

    let solid_source = original_signature_pdf_with_raw_opacity("1");
    let mut session = Session::new().with_clock(|| 1_700_000_000);
    let oversized = session.open("original-oversized-opacity.pdf", None, oversized_source.clone(), None).unwrap();
    let solid = session.open("original-solid-opacity-control.pdf", None, solid_source.clone(), None).unwrap();
    for scale in [1.0, 2.0] {
        let control = rendered_signature_rgba(session.get(solid).unwrap().display.clone(), scale);
        let actual = rendered_signature_rgba(session.get(oversized).unwrap().display.clone(), scale);
        let dark = |rgba: &[u8]| rgba.as_chunks::<4>().0.iter().filter(|pixel| pixel[0] < 200 && pixel[1] < 200 && pixel[2] < 200).count();
        let control_dark = dark(&control.2);
        let actual_dark = dark(&actual.2);
        assert!(control_dark > 0, "finite opacity1 positive control must paint at scale {scale}");
        assert!(
            actual_dark > 0,
            "positive-infinity opacity must retain historical visible clamp: {actual_dark} dark pixels, opacity1 control {control_dark}, scale {scale}"
        );
        assert!(actual == control, "display clamp must match all RGBA pixels of opacity1 at scale {scale}");
    }
    assert_loaded_appearance_state_is_unchanged(&session, oversized, &oversized_source);
    assert_loaded_appearance_state_is_unchanged(&session, solid, &solid_source);
    let shown = Cos::open(session.get(oversized).unwrap().display.clone()).unwrap();
    assert!(shown.get(pdfcraft_cos::ObjRef::new(4, 0)).as_dict().unwrap().get(b"AP").is_some());

    // Negative infinity historically clamps to0, not1. Require a generated AP too, since
    // blank pixels alone would also pass if normalization incorrectly skipped the artwork.
    let negative_source = original_signature_pdf_with_raw_opacity(&format!("-{huge}"));
    let parsed = Cos::open(negative_source.clone()).unwrap();
    let alpha = parsed.get(pdfcraft_cos::ObjRef::new(4, 0)).as_dict().unwrap().get(b"CA").unwrap().as_f64().unwrap();
    assert!(alpha.is_infinite() && alpha.is_sign_negative());
    let zero_source = original_signature_pdf_with_raw_opacity("0");
    let negative = session.open("original-negative-opacity.pdf", None, negative_source.clone(), None).unwrap();
    let zero = session.open("original-zero-opacity-control.pdf", None, zero_source.clone(), None).unwrap();
    for scale in [1.0, 2.0] {
        assert!(
            rendered_signature_rgba(session.get(negative).unwrap().display.clone(), scale)
                == rendered_signature_rgba(session.get(zero).unwrap().display.clone(), scale),
            "negative-infinity opacity must match opacity0 at scale {scale}"
        );
    }
    let shown = Cos::open(session.get(negative).unwrap().display.clone()).unwrap();
    assert!(
        shown.get(pdfcraft_cos::ObjRef::new(4, 0)).as_dict().unwrap().get(b"AP").is_some(),
        "negative-infinity display must build an opacity0 AP"
    );
    assert_loaded_appearance_state_is_unchanged(&session, negative, &negative_source);
    assert_loaded_appearance_state_is_unchanged(&session, zero, &zero_source);
}

// PRIVATE uncompiled fragment for crates/engine/tests/it/signature_size.rs.
// Reuses rendered_signature_rgba, rendered_signature_ink_bounds and
// assert_loaded_appearance_state_is_unchanged from that current test module.
fn original_signature_spline_viewport_pdf(clipped: bool, with_control_appearance: bool) -> Arc<Vec<u8>> {
    let rect = if clipped { "-1 -2 24 102" } else { "-1 -10 24 102" };
    let ap = if with_control_appearance { "/AP << /N 5 0 R >>" } else { "" };
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [-5 -15 35 110] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>".to_owned(),
        format!(
            "<< /Type /Annot /Subtype /Ink /Rect [{rect}] /C [0 0 0] /Subj (Signature) /InkList [[0 0 20 0 21 100 22 0]] /BS << /W 1 /S /S >> {ap} >>"
        ),
    ];
    if with_control_appearance {
        // Independent closed-form Catmull-Rom cubics for the four original points.
        // The first segment has y(t)=-50*t*t*(1-t), minimum -200/27 at t=2/3.
        // Its control y=-100/6 lies outside the unclipped Rect even though the ink fits.
        // No call to the production hull scanner or appearance writer supplies this oracle.
        let content = concat!(
            "0 0 0 RG\n1 w 1 J 1 j\n0 0 m\n",
            "3.3333333333333335 0 16.5 -16.666666666666668 20 0 c\n",
            "23.5 16.666666666666668 20.666666666666668 100 21 100 c\n",
            "21.333333333333332 100 21.833333333333332 16.666666666666668 22 0 c\nS\n"
        );
        objects.push(format!(
            "<< /Type /XObject /Subtype /Form /FormType 1 /BBox [{rect}] /Resources << >> /Length {} >>\nstream\n{content}endstream",
            content.len()
        ));
    }
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    Arc::new(bytes)
}

#[test]
fn imported_signature_spline_controls_outside_rect_keep_the_existing_display_viewport() {
    for clipped in [false, true] {
        let source = original_signature_spline_viewport_pdf(clipped, false);
        let control_source = original_signature_spline_viewport_pdf(clipped, true);
        let parsed = Cos::open(source.clone()).unwrap();
        let annotation = parsed.get(pdfcraft_cos::ObjRef::new(4, 0));
        assert!(annotation.as_dict().unwrap().get(b"AP").is_none());
        let mut session = Session::new().with_clock(|| 1_700_000_000);
        let id = session.open("original-spline-viewport.pdf", None, source.clone(), None).unwrap();
        let control = session.open("independent-spline-appearance.pdf", None, control_source.clone(), None).unwrap();
        for scale in [1.0, 2.0] {
            let expected = rendered_signature_rgba(session.get(control).unwrap().display.clone(), scale);
            let actual = rendered_signature_rgba(session.get(id).unwrap().display.clone(), scale);
            assert_eq!((actual.0, actual.1), (expected.0, expected.1));
            let dark = |rgba: &[u8]| rgba.as_chunks::<4>().0.iter().filter(|pixel| pixel[0] < 200 && pixel[1] < 200 && pixel[2] < 200).count();
            let expected_dark = dark(&expected.2);
            let actual_dark = dark(&actual.2);
            assert!(expected_dark > 0, "independent existing-AP spline must paint; clipped={clipped}, scale={scale}");
            assert!(
                actual_dark > 0,
                "safe spline controls outside the annotation viewport must not drop its display AP: {actual_dark} dark pixels, existing-AP control {expected_dark}; clipped={clipped}, scale={scale}"
            );
            let count_delta = actual_dark.abs_diff(expected_dark);
            assert!(
                count_delta <= (expected_dark / 20).max(3),
                "normalized viewport changed visible spline coverage: {actual_dark} vs {expected_dark}; clipped={clipped}, scale={scale}"
            );
            // Full pixel identity is not required: normalized f32 AP framing can move an
            // antialiased edge slightly relative to a raw-space independent appearance.
        }
        let actual_bounds = rendered_signature_ink_bounds(session.get(id).unwrap().display.clone());
        let expected_bounds = rendered_signature_ink_bounds(session.get(control).unwrap().display.clone());
        for (actual, expected) in actual_bounds.into_iter().zip(expected_bounds) {
            assert!(
                (actual - expected).abs() <= 1.0,
                "imported viewport changed the visible spline bounds: {actual_bounds:?} vs {expected_bounds:?}"
            );
        }
        let displayed = Cos::open(session.get(id).unwrap().display.clone()).unwrap();
        assert!(displayed.get(pdfcraft_cos::ObjRef::new(4, 0)).as_dict().unwrap().get(b"AP").is_some());
        assert_loaded_appearance_state_is_unchanged(&session, id, &source);
        assert_loaded_appearance_state_is_unchanged(&session, control, &control_source);
    }
}

#[test]
fn indirect_signature_dash_refusal_preserves_document_and_nonempty_undo_redo_history() {
    for case in ["wrong_type", "cycle", "deep", "too_long", "huge", "scaled_huge", "tiny"] {
        let (mut session, _, source) = raw_signature_style_fixture(1.0, RawInkStyle::Solid, false);
        let mut cos = Cos::open(source).unwrap();
        let page = cos.get(pdfcraft_annot::page_refs(&cos).unwrap()[0]);
        let annots = cos.resolve(page.as_dict().unwrap().get(b"Annots").unwrap());
        let annotation = annots.as_array().unwrap()[0].as_ref().unwrap();
        let pattern = match case {
            "wrong_type" => Object::name("bad-dash"),
            "too_long" => Object::Array(vec![Object::Real(1.0); 65]),
            "huge" => Object::Array(vec![Object::Real(1e308), Object::Real(1.0)]),
            "scaled_huge" => Object::Array(vec![Object::Real(1e38), Object::Real(1.0)]),
            "tiny" => Object::Array(vec![Object::Real(1e-320), Object::Real(1.0)]),
            _ => Object::Null,
        };
        let value = cos.add(pattern);
        if case == "cycle" {
            cos.set(value, Object::Ref(value));
        } else if case == "deep" {
            let second = cos.add(Object::Array(vec![Object::Real(3.0)]));
            cos.set(value, Object::Ref(second));
        }
        cos.update_dict(annotation, |d| {
            let Object::Dict(bs) = d.get_mut(b"BS").unwrap() else { panic!("direct BS") };
            bs.set(b"S".to_vec(), Object::name("D"));
            bs.set(b"D".to_vec(), Object::Ref(value));
        })
        .unwrap();
        let source = Arc::new(pdfcraft_cos::write_full(&cos, &SaveOptions::default()).unwrap());
        let id = session.open("indirect-dash-synthetic.pdf", None, source, None).unwrap();
        // Leave both a real undo entry and a real redo entry before testing refusal.
        session.apply(id, Edit::SetInfo { key: "Title".into(), value: "kept title".into() }).unwrap();
        session.apply(id, Edit::SetInfo { key: "Author".into(), value: "redo author".into() }).unwrap();
        session.undo(id).unwrap();
        let before = session.save_bytes(id).unwrap();
        let display = session.get(id).unwrap().display.clone();
        let dirty = session.get(id).unwrap().dirty;
        let undo = session.get(id).unwrap().can_undo().map(str::to_owned);
        let redo = session.get(id).unwrap().can_redo().map(str::to_owned);
        assert!(undo.is_some() && redo.is_some());
        let result = session.apply(
            id,
            Edit::StyleAnnotation { page: 0, index: 0, color: Some([1.0, 0.0, 0.0]), opacity: Some(0.5), width: Some(2.0), endings: None },
        );
        assert!(result.is_err(), "{case}: malformed or unrepresentable dash must refuse");
        assert_eq!(session.save_bytes(id).unwrap(), before, "{case}: document/AP bytes");
        assert_eq!(session.get(id).unwrap().display, display, "{case}: display");
        assert_eq!(session.get(id).unwrap().dirty, dirty, "{case}: dirty state");
        assert_eq!(session.get(id).unwrap().can_undo().map(str::to_owned), undo, "{case}: undo");
        assert_eq!(session.get(id).unwrap().can_redo().map(str::to_owned), redo, "{case}: redo");
    }
}
