use super::*;
use pdfcraft_cos::{SaveOptions, write_full};
use std::sync::Arc;
fn fixture(content: &str, attrs: &str, extra: &[&str]) -> Document {
    fixture_with("4 0 R", content, attrs, extra)
}
fn fixture_with(contents: &str, content: &str, attrs: &str, extra: &[&str]) -> Document {
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>".into(),
        format!("<< /Type /Page /Parent 2 0 R /Contents {contents} /Resources << /XObject << /Nested 5 0 R >> >> {attrs} >>"),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    objects.extend(extra.iter().map(|s| s.to_string()));
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        bytes.extend(format!("{o:010} 00000 n \n").as_bytes());
    }
    bytes.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    Document::open(Arc::new(bytes)).unwrap()
}
fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-8, "{a} != {b}");
}
fn new(kind: Kind, points: Vec<Point>) -> NewMeasurement {
    NewMeasurement {
        page: 0,
        kind,
        points,
        scale: Scale::new(2.0, "m", 3).unwrap(),
        style: Style { color: [0.0, 0.47, 0.84], ..Style::default() },
        label: "Room 1".into(),
        author: "A".into(),
    }
}
#[test]
fn calibrated_lengths_concave_areas_and_axis_scales() {
    let scale = Scale::calibrate([20.0, 30.0], [23.0, 34.0], 10.0, "m", 3).unwrap();
    let distance = reading(Kind::Distance, &[[20.0, 30.0], [23.0, 34.0]], &scale).unwrap();
    close(distance.value, 10.0);
    close(distance.delta_x, 6.0);
    close(distance.delta_y, 8.0);
    assert_eq!(distance.label, "10.000 m");
    let path = [[0.0, 0.0], [3.0, 0.0], [3.0, 4.0], [0.0, 0.0]];
    close(reading(Kind::Perimeter, &path, &scale).unwrap().value, 24.0);
    let concave = [[1e8, 1e8], [1e8 + 3.0, 1e8], [1e8 + 3.0, 1e8 + 1.0], [1e8 + 1.0, 1e8 + 1.0], [1e8 + 1.0, 1e8 + 3.0], [1e8, 1e8 + 3.0]];
    close(reading(Kind::Area, &concave, &scale).unwrap().value, 20.0);
    let reversed: Vec<_> = concave.into_iter().rev().collect();
    close(reading(Kind::Area, &reversed, &scale).unwrap().value, 20.0);
    let anisotropic = Scale { y: 3.0, ..Scale::new(2.0, "m", 2).unwrap() };
    close(reading(Kind::Distance, &[[0.0, 0.0], [3.0, 4.0]], &anisotropic).unwrap().value, 180f64.sqrt());
    close(reading(Kind::Area, &[[0.0, 0.0], [3.0, 0.0], [3.0, 4.0]], &anisotropic).unwrap().value, 36.0);
}
#[test]
fn viewport_precedence_first_point_user_unit_and_roundtrip() {
    let mut d = fixture("", "/UserUnit 2 /Rotate 90 /CropBox [10 20 210 320]", &[]);
    close(scale_at(&d, 0, [50.0, 50.0]).unwrap().x, 2.0 / 72.0);
    set_scale(&mut d, 0, [10.0, 20.0, 210.0, 320.0], "large", &Scale::new(1.0, "m", 2).unwrap()).unwrap();
    set_scale(&mut d, 0, [20.0, 30.0, 50.0, 60.0], "detail", &Scale::new(10.0, "m", 4).unwrap()).unwrap();
    close(scale_at(&d, 0, [25.0, 35.0]).unwrap().x, 10.0);
    close(scale_at(&d, 0, [100.0, 100.0]).unwrap().x, 1.0);
    // An annotation carries the chosen scale even when its second point leaves the viewport.
    let mut m = new(Kind::Distance, vec![[25.0, 35.0], [100.0, 35.0]]);
    m.scale = scale_at(&d, 0, m.points[0]).unwrap();
    add(&mut d, &m, &Meta { date: None, id: "measure".into() }).unwrap();
    let bytes = write_full(&d, &SaveOptions::default()).unwrap();
    let reopened = Document::open(Arc::new(bytes)).unwrap();
    let measurements = list(&reopened).measurements;
    close(measurements[0].reading.value, 750.0);
    assert_eq!(measurements[0].id, "measure");
    assert_eq!(scale_at(&reopened, 0, [25.0, 35.0]).unwrap().precision, 4);
}
#[test]
fn standard_annotations_captions_restyle_move_and_unknown_keys() {
    let mut d = fixture("", "", &[]);
    let meta = Meta { date: None, id: String::new() };
    for m in [
        new(Kind::Distance, vec![[0.0, 0.0], [3.0, 4.0]]),
        new(Kind::Perimeter, vec![[20.0, 0.0], [23.0, 0.0], [23.0, 4.0]]),
        new(Kind::Area, vec![[40.0, 0.0], [43.0, 0.0], [43.0, 4.0]]),
    ] {
        add(&mut d, &m, &meta).unwrap();
    }
    assert_eq!(list(&d).measurements.iter().map(|m| m.reading.value).collect::<Vec<_>>(), vec![10.0, 14.0, 24.0]);
    let (r, dict) = annot(&d, 0, 0).unwrap();
    assert_eq!(dict.name(b"IT"), Some(&b"LineDimension"[..]));
    d.update_dict(r, |d| d.set(b"VendorKey".to_vec(), Object::Int(42))).unwrap();
    pdfcraft_annot::set_style(&mut d, 0, 0, Some([1.0, 0.0, 0.0]), None, Some(2.0), &meta).unwrap();
    let (_, dict) = annot(&d, 0, 0).unwrap();
    assert_eq!(dict.int(b"VendorKey"), Some(42));
    let ap = d.resolve(dict.get(b"AP").unwrap());
    let normal = d.resolve(ap.as_dict().unwrap().get(b"N").unwrap());
    let Object::Stream(stream) = normal.as_ref() else { panic!("appearance missing") };
    let stream = String::from_utf8(stream.decoded().unwrap()).unwrap();
    assert!(stream.contains("/ActualText <31302E303030206D>"));
    assert!(stream.contains("q 1 1 1 rg"));
    assert!(stream.contains("0.08 0.1 0.14 rg"));
    pdfcraft_annot::move_annotation(&mut d, 0, 0, 50.0, 60.0, &meta).unwrap();
    assert_eq!(list(&d).measurements[0].points, vec![[50.0, 60.0], [53.0, 64.0]]);
    close(list(&d).measurements[0].reading.value, 10.0);
}
#[test]
fn rejects_invalid_geometry_and_unsupported_scales() {
    let mut d = fixture("", "", &[]);
    for points in [vec![[f64::NAN, 0.0], [2.0, 0.0]], vec![[0.0, 0.0]; 2], vec![[0.0, 0.0], [1e10, 0.0]]] {
        assert!(add(&mut d, &new(Kind::Distance, points), &Meta::default()).is_err());
    }
    assert!(add(&mut d, &new(Kind::Area, vec![[0.0, 0.0], [2.0, 2.0], [0.0, 2.0], [2.0, 0.0]]), &Meta::default()).is_err());
    assert!(Scale::new(f64::INFINITY, "m", 2).is_err());
    assert!(Scale::new(1.0, "m", 7).is_err());
    assert!(Scale::calibrate([0.0; 2], [0.0; 2], 1.0, "m", 2).is_err());
    let mut dict = Scale::default().dictionary().unwrap();
    dict.set(b"Subtype".to_vec(), Object::name("GEO"));
    assert!(Scale::read(&d, &Object::Dict(dict)).unwrap_err().to_string().contains("originating viewport"));
    assert!(set_scale(&mut d, 0, [0.0; 4], "bad", &Scale::default()).is_err());
}
#[test]
fn snap_paths_endpoints_midpoints_and_intersections() {
    let d = fixture("10 20 m 110 20 l S 60 0 m 60 80 l S 10 100 100 40 re S", "", &[]);
    let paths = snap::geometry(&d, 0).unwrap();
    assert_eq!(paths.segments.len(), 6);
    let opts = snap::SnapOptions::default();
    let hit = paths.snap([11.0, 20.0], 3.0, opts).unwrap().unwrap();
    assert_eq!(hit.kind, snap::SnapKind::Endpoint);
    assert_eq!(hit.point, [10.0, 20.0]);
    let hit = paths.snap([60.0, 41.0], 3.0, opts).unwrap().unwrap();
    assert_eq!(hit.kind, snap::SnapKind::Midpoint);
    assert_eq!(hit.point, [60.0, 40.0]);
    let hit = paths.snap([59.0, 21.0], 3.0, snap::SnapOptions { midpoints: false, ..opts }).unwrap().unwrap();
    assert_eq!(hit.kind, snap::SnapKind::Intersection);
    assert_eq!(hit.point, [60.0, 20.0]);
    let hit = paths.snap([32.0, 22.0], 3.0, opts).unwrap().unwrap();
    assert_eq!(hit.kind, snap::SnapKind::Path);
    assert_eq!(hit.point, [32.0, 20.0]);
    assert!(paths.snap([200.0, 200.0], 3.0, opts).unwrap().is_none());
    assert!(paths.snap([f64::NAN, 0.0], 3.0, opts).is_err());
}
#[test]
fn nested_forms_transforms_curves_and_cycles_are_bounded() {
    let body = "0 0 m 10 0 l S 0 0 m 0 10 10 10 10 0 c S /Nested Do";
    let form = format!(
        "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Matrix [2 0 0 3 0 0] /Resources << /XObject << /Nested 5 0 R >> >> /Length {} >>\nstream\n{body}\nendstream",
        body.len()
    );
    let d = fixture("q 1 0 0 1 100 200 cm /Nested Do Q", "", &[&form]);
    let geometry = snap::geometry(&d, 0).unwrap();
    assert_eq!(geometry.segments[0], [[100.0, 200.0], [120.0, 200.0]]);
    assert!(geometry.endpoints.contains(&[120.0, 200.0]));
    assert!(geometry.midpoints.contains(&[110.0, 222.5]));
    assert!(geometry.segments.len() > 4 && geometry.segments.len() < 200);
    let d = fixture(&"0 0 m 1 1 l S ".repeat(20_001), "", &[]);
    assert!(snap::geometry(&d, 0).unwrap().truncated);
}
#[test]
fn measurement_csv_quotes_newlines_and_blocks_formula_cells() {
    let mut d = fixture("", "", &[]);
    let mut m = new(Kind::Distance, vec![[0.0, 0.0], [3.0, 4.0]]);
    m.label = "=SUM(1,2)\n\"room\"".into();
    m.author = "  @cmd".into();
    add(&mut d, &m, &Meta::default()).unwrap();
    let csv = csv(&list(&d).measurements);
    assert!(csv.contains("\"'=SUM(1,2)\n\"\"room\"\"\""));
    assert!(csv.contains("\"'  @cmd\""));
    assert!(csv.starts_with("Page,Index,Type,Value"));
}

#[test]
fn coordinates_roundtrip_for_crop_rotation_and_user_unit() {
    for rotation in [0, 90, 180, 270] {
        let d = fixture("", &format!("/UserUnit 2 /Rotate {rotation} /CropBox [10 20 210 320]"), &[]);
        for view in [[0.0, 0.0], [37.125, 81.375], [123.0, 175.0]] {
            let user = view_to_user(&d, 0, view).unwrap();
            let actual = user_to_view(&d, 0, user).unwrap();
            close(actual[0], view[0]);
            close(actual[1], view[1]);
        }
        let a = view_to_user(&d, 0, [10.0, 20.0]).unwrap();
        let b = view_to_user(&d, 0, [70.0, 100.0]).unwrap();
        let scale = scale_at(&d, 0, a).unwrap();
        close(reading(Kind::Distance, &[a, b], &scale).unwrap().value, 100.0 / 72.0);
    }
}

#[test]
fn zero_decimal_scale_uses_standard_rounding_format() {
    let d = fixture("", "", &[]);
    let scale = Scale::new(1.0, "m", 0).unwrap();
    let dict = scale.dictionary().unwrap();
    let first = dict.get(b"D").unwrap().as_array().unwrap()[0].as_dict().unwrap();
    assert_eq!(first.name(b"F"), Some(&b"R"[..]));
    let read = Scale::read(&d, &Object::Dict(dict.clone())).unwrap();
    assert_eq!(read.dictionary().unwrap(), dict);
    assert_eq!(
        reading(Kind::Distance, &[[0.0, 0.0], [2.4, 0.0]], &read).unwrap(),
        reading(Kind::Distance, &[[0.0, 0.0], [2.4, 0.0]], &scale).unwrap()
    );
}

#[test]
fn imported_measurement_appearance_is_not_silently_replaced() {
    let mut d = fixture("", "", &[]);
    add(&mut d, &new(Kind::Distance, vec![[10.0, 20.0], [40.0, 60.0]]), &Meta::default()).unwrap();
    let (r, dict) = annot(&d, 0, 0).unwrap();
    let original = dict.get(b"AP").unwrap().clone();
    d.update_dict(r, |d| {
        d.remove(b"PCMeasureValue");
    })
    .unwrap();
    assert!(pdfcraft_annot::set_appearance(&mut d, r).is_err());
    assert_eq!(annot(&d, 0, 0).unwrap().1.get(b"AP"), Some(&original));
    close(list(&d).measurements[0].reading.value, 100.0);
}

#[test]
fn degenerate_and_dense_paths_keep_snap_targets_bounded() {
    // Rejected (zero-length) segments record no endpoints or midpoints at all.
    let d = fixture(&"0 0 0 0 re f ".repeat(100_000), "", &[]);
    let g = snap::geometry(&d, 0).unwrap();
    assert!(g.segments.is_empty() && g.endpoints.is_empty() && g.midpoints.is_empty(), "{} {}", g.endpoints.len(), g.midpoints.len());
    // Every paint drains into the page geometry; the page-wide target cap still holds.
    let d = fixture(&"0 0 1 1 re f ".repeat(30_000), "", &[]);
    let g = snap::geometry(&d, 0).unwrap();
    assert!(g.truncated);
    assert!(g.endpoints.len() + g.midpoints.len() <= 40_000 + 12, "{}", g.endpoints.len() + g.midpoints.len());
    assert!(g.segments.len() <= 20_000 + 4);
    // Unpainted curves and lines are capped too.
    let d = fixture(&"0 0 m 5 9 1 9 7 0 c 0 3 l ".repeat(30_000), "", &[]);
    let g = snap::geometry(&d, 0).unwrap();
    assert!(g.truncated && g.endpoints.is_empty());
}
#[test]
fn deep_graphics_state_and_unreadable_streams_are_skipped_not_fatal() {
    let d = fixture(&format!("0 0 m 10 0 l S {}", "q ".repeat(300)), "", &[]);
    let g = snap::geometry(&d, 0).unwrap();
    assert!(g.truncated);
    assert_eq!(g.segments.len(), 1);
    // An unsupported filter cannot be decoded at all (corrupt Flate is decoded tolerantly).
    let broken = "<< /Length 10 /Filter /NoSuchDecode >>\nstream\nnot-zlib!!\nendstream";
    let d = fixture_with("[4 0 R 5 0 R]", "0 0 m 10 0 l S", "", &[broken]);
    let g = snap::geometry(&d, 0).unwrap();
    assert_eq!(g.unreadable, 1);
    assert_eq!(g.segments.len(), 1);
    let d = fixture_with("5 0 R", "", "", &[broken]);
    assert_eq!(snap::geometry(&d, 0).unwrap().unreadable, 1);
}
#[test]
fn unsupported_measurements_are_reported_not_fatal() {
    let mut d = fixture("", "", &[]);
    add(&mut d, &new(Kind::Distance, vec![[0.0, 0.0], [3.0, 4.0]]), &Meta::default()).unwrap();
    add(&mut d, &new(Kind::Distance, vec![[10.0, 0.0], [13.0, 4.0]]), &Meta::default()).unwrap();
    // A malformed imported compound format with a negative conversion.
    let (r, _) = annot(&d, 0, 1).unwrap();
    let format = |unit: &str, c: f64| {
        let mut f = Dict::new();
        f.set(b"Type".to_vec(), Object::name("NumberFormat"));
        f.set(b"U".to_vec(), PdfString::text(unit));
        f.set(b"C".to_vec(), Object::Real(c));
        Object::Dict(f)
    };
    d.update_dict(r, |a| {
        let mut m = Scale::default().dictionary().unwrap();
        m.set(b"D".to_vec(), Object::Array(vec![format("ft", 1.0 / 864.0), format("in", -12.0)]));
        a.set(b"Measure".to_vec(), Object::Dict(m));
    })
    .unwrap();
    let listing = list(&d);
    assert_eq!(listing.measurements.len(), 1);
    assert_eq!(listing.unsupported.len(), 1);
    assert_eq!((listing.unsupported[0].page, listing.unsupported[0].index), (0, 1));
    assert!(listing.unsupported[0].reason.contains("conversion"), "{}", listing.unsupported[0].reason);
    // Invalid geometry on an imported annotation is reported the same way.
    let (r, _) = annot(&d, 0, 0).unwrap();
    d.update_dict(r, |a| a.set(b"L".to_vec(), nums([0.0, 0.0, f64::NAN, 1.0]))).unwrap();
    let listing = list(&d);
    assert!(listing.measurements.is_empty());
    assert_eq!(listing.unsupported.len(), 2);
}
#[test]
fn non_ascii_units_roundtrip_with_exact_caption_text() {
    let mut d = fixture("", "", &[]);
    let mut m = new(Kind::Area, vec![[0.0, 0.0], [3.0, 0.0], [3.0, 4.0]]);
    m.scale = Scale { area_unit: "m²".into(), ..Scale::new(2.0, "m", 1).unwrap() };
    add(&mut d, &m, &Meta::default()).unwrap();
    let bytes = write_full(&d, &SaveOptions::default()).unwrap();
    let d = Document::open(Arc::new(bytes)).unwrap();
    let listing = list(&d);
    assert!(listing.unsupported.is_empty(), "{:?}", listing.unsupported);
    assert_eq!(listing.measurements[0].scale.area_unit, "m²");
    assert_eq!(listing.measurements[0].reading.label, "24.0 m²");
    let (_, dict) = annot(&d, 0, 0).unwrap();
    let ap = d.resolve(dict.get(b"AP").unwrap());
    let normal = d.resolve(ap.as_dict().unwrap().get(b"N").unwrap());
    let Object::Stream(stream) = normal.as_ref() else { panic!("appearance missing") };
    let data = stream.decoded().unwrap();
    // The outlines retain the exact Unicode caption in standard ActualText.
    let hex: String = PdfString::text("24.0 m²").bytes.iter().map(|b| format!("{b:02X}")).collect();
    assert!(String::from_utf8_lossy(&data).contains(&format!("/ActualText <{hex}>")));
    assert!(!data.windows(3).any(|w| w == "\u{fffd}".as_bytes()));
    assert!(Scale::new(1.0, "m\n", 2).is_err());
    assert!(Scale::new(1.0, &"x".repeat(25), 2).is_err());
}
#[test]
fn closed_polygon_with_repeated_first_vertex_is_not_self_intersecting() {
    let mut d = fixture("", "", &[]);
    let points = vec![[0.0, 0.0], [3.0, 0.0], [3.0, 4.0], [0.0, 4.0], [0.0, 0.0]];
    add(&mut d, &new(Kind::Area, points.clone()), &Meta::default()).unwrap();
    let listing = list(&d);
    assert!(listing.unsupported.is_empty(), "{:?}", listing.unsupported);
    close(listing.measurements[0].reading.value, 48.0);
    assert_eq!(listing.measurements[0].points, points);
    // A closing duplicate doesn't turn too few vertices into a polygon.
    assert!(add(&mut d, &new(Kind::Area, vec![[0.0, 0.0], [3.0, 0.0], [0.0, 0.0]]), &Meta::default()).is_err());
}
#[test]
fn indirect_viewport_array_stays_indirect() {
    let mut d = fixture("", "/VP 5 0 R", &["[]"]);
    set_scale(&mut d, 0, [0.0, 0.0, 100.0, 100.0], "plan", &Scale::new(1.0, "m", 2).unwrap()).unwrap();
    let page = crate::page(&d, 0).unwrap();
    let r = page.dict.get(b"VP").and_then(Object::as_ref).expect("VP stays a reference");
    assert_eq!(d.get(r).as_array().map(Vec::len), Some(1));
    close(scale_at(&d, 0, [50.0, 50.0]).unwrap().x, 1.0);
}

#[test]
fn compound_fractional_formats_carry_reduce_and_preserve_unknown_data() {
    use crate::format::{self, Fraction, NumberFormat, NumberFormats};
    let mut miles = NumberFormat::decimal("mi", 1.0, 2);
    miles.suffix = " ".into();
    miles.thousands = ",".into();
    let mut feet = NumberFormat::decimal("ft", 5280.0, 2);
    feet.suffix = " ".into();
    feet.thousands = ",".into();
    let mut inches = NumberFormat::decimal("in", 12.0, 2);
    inches.fraction = Fraction::Fraction;
    inches.denominator = 8;
    inches.fixed = false;
    let distance = vec![miles, feet, inches];
    assert_eq!(format::label(1.4505, &distance).unwrap(), "1 mi 2,378 ft 7 5/8 in");
    let formats = NumberFormats {
        x: vec![NumberFormat::decimal("mi", 0.00139, 5)],
        y: None,
        distance,
        area: vec![NumberFormat::decimal("acres", 640.0, 2)],
        cyx: None,
    };
    let scale = Scale::from_formats(formats, "1 in = 0.1 mi").unwrap();
    let mut dictionary = scale.dictionary().unwrap();
    dictionary.set(b"VendorData".to_vec(), Object::Int(39));
    let mut doc = fixture("", "", &[]);
    let loaded = Scale::read(&doc, &Object::Dict(dictionary.clone())).unwrap();
    assert_eq!(loaded.dictionary().unwrap(), dictionary);
    let m = NewMeasurement { scale: loaded.clone(), ..new(Kind::Distance, vec![[0.0, 0.0], [1.4505 / loaded.x, 0.0]]) };
    add(&mut doc, &m, &Meta::default()).unwrap();
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    let reopened = Document::open(Arc::new(bytes)).unwrap();
    let listing = list(&reopened);
    assert!(listing.unsupported.is_empty());
    assert_eq!(listing.measurements[0].reading.label, "1 mi 2,378 ft 7 5/8 in");
    assert_eq!(listing.measurements[0].scale.dictionary().unwrap().int(b"VendorData"), Some(39));

    let mut foot = NumberFormat::decimal("ft", 1.0, 2);
    foot.suffix = " ".into();
    let mut inch = NumberFormat::decimal("in", 12.0, 2);
    inch.fraction = Fraction::Fraction;
    inch.denominator = 16;
    inch.fixed = false;
    let compound = vec![foot, inch.clone()];
    assert_eq!(format::label(1.99999, &compound).unwrap(), "2 ft");
    assert_eq!(format::label(1.125, &compound).unwrap(), "1 ft 1 1/2 in");
    assert_eq!(format::label(-1.125, &compound).unwrap(), "-1 ft 1 1/2 in");
    inch.fixed = true;
    assert_eq!(format::label(1.5, &[inch.clone()]).unwrap(), "1 8/16 in");
    inch.fraction = Fraction::Truncate;
    assert_eq!(format::label(1.9, &[inch]).unwrap(), "1 in");
}

#[test]
fn number_formats_use_indirect_values_prefixes_and_local_separators() {
    let doc = fixture("", "", &["<< /Type /NumberFormat /U (m) /C 1 /F /D /D 100 /FD true /RT (.) /RD (,) /PS () /SS (:) /O /P >>", "[5 0 R]"]);
    let formats = format::read_array(&doc, &Object::Ref(ObjRef { num: 6, generation: 0 })).unwrap();
    assert_eq!(format::label(1234.5, &formats).unwrap(), "m:1.234,50");
    let mut f = NumberFormat::decimal("m", 1.0, 2);
    f.denominator = 20;
    assert_eq!(format::label(1.13, &[f.clone()]).unwrap(), "1.15 m");
    f.denominator = 0;
    assert!(format::label(1.0, &[f]).is_err());
    assert!(format::label(f64::NAN, &formats).is_err());
    assert!(format::read_array(&doc, &Object::Array(vec![])).is_err());
    let mut f = NumberFormat::decimal("m", 1.0, 2);
    f.prefix = "\n".into();
    assert!(format::dictionary(&[f]).is_err());
}

#[test]
fn viewport_update_remove_preserve_indirection_precedence_and_saved_scale() {
    let mut d = fixture(
        "",
        "/VP 5 0 R",
        &[
            "[6 0 R]",
            "<< /Type /Viewport /BBox [0 0 100 100] /Name (original) /Unknown 73 /Measure << /Subtype /RL /R (1 pt = 1 m) /X [<< /U (m) /C 1 >>] /D [<< /U (m) /C 1 >>] /A [<< /U (m^2) /C 1 >>] >> >>",
        ],
    );
    let scale = scale_at(&d, 0, [10.0, 10.0]).unwrap();
    add(&mut d, &NewMeasurement { scale, ..new(Kind::Distance, vec![[10.0, 10.0], [20.0, 10.0]]) }, &Meta::default()).unwrap();
    viewports::update(&mut d, 0, 0, [0.0, 0.0, 150.0, 150.0], "updated", &Scale::new(2.0, "m", 2).unwrap()).unwrap();
    let p = page(&d, 0).unwrap();
    let vp = p.dict.get(b"VP").and_then(Object::as_ref).expect("edited array stays indirect");
    let viewport = d.get(vp).as_array().unwrap()[0].as_ref().expect("edited viewport stays indirect");
    assert_eq!(d.get(viewport).as_dict().unwrap().int(b"Unknown"), Some(73));
    assert_eq!(text(d.get(ObjRef::new(6, 0)).as_dict().unwrap(), b"Name"), "original");
    assert_eq!(viewports::list(&d, 0).unwrap()[0].name, "updated");
    close(scale_at(&d, 0, [10.0, 10.0]).unwrap().x, 2.0);
    close(list(&d).measurements[0].reading.value, 10.0);
    let before = write_full(&d, &SaveOptions::default()).unwrap();
    assert!(viewports::update(&mut d, 0, 7, [0.0, 0.0, 150.0, 150.0], "bad", &Scale::default()).is_err());
    assert_eq!(write_full(&d, &SaveOptions::default()).unwrap(), before);
    viewports::remove(&mut d, 0, 0).unwrap();
    assert!(viewports::list(&d, 0).unwrap().is_empty());
    close(list(&d).measurements[0].reading.value, 10.0);
    let bytes = write_full(&d, &SaveOptions::default()).unwrap();
    let reopened = Document::open(Arc::new(bytes)).unwrap();
    assert!(viewports::list(&reopened, 0).unwrap().is_empty());
    close(list(&reopened).measurements[0].reading.value, 10.0);
}

#[test]
fn imported_format_edits_use_the_same_units_live_and_after_reopen() {
    let mut scale = Scale::from_formats(
        NumberFormats {
            x: vec![NumberFormat::decimal("ft", 1.0, 2)],
            y: None,
            distance: vec![NumberFormat::decimal("ft", 1.0, 2)],
            area: vec![NumberFormat::decimal("ft²", 1.0, 2)],
            cyx: None,
        },
        "1 pt = 1 ft",
    )
    .unwrap();
    scale.distance_factor = 12.0;
    scale.unit = "in".into();
    scale.area_factor = 144.0;
    scale.area_unit = "in²".into();
    let doc = Document::new_empty();
    let object = Object::Dict(scale.dictionary().unwrap());
    let reopened = Scale::read(&doc, &object).unwrap();
    for kind in [Kind::Distance, Kind::Area] {
        let points = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
        let before = reading(kind, &points, &scale).unwrap();
        let after = reading(kind, &points, &reopened).unwrap();
        assert_eq!(before.label, after.label);
        assert!(before.label.contains("in"));
    }
}

#[test]
fn editing_shared_viewports_keeps_other_pages_and_original_references_unchanged() {
    let mut d = fixture(
        "",
        "/VP 5 0 R",
        &[
            "[6 0 R]",
            "<< /Type /Viewport /BBox [0 0 100 100] /Name (shared) /Measure << /Subtype /RL /R (1 pt = 1 m) /X [<< /U (m) /C 1 >>] /D [<< /U (m) /C 1 >>] /A [<< /U (m²) /C 1 >>] >> >>",
        ],
    );
    let first = page(&d, 0).unwrap();
    let second = d.add(Object::Dict(first.dict.clone()));
    d.update_dict(ObjRef::new(2, 0), |pages| {
        pages.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(first.obj), Object::Ref(second)]));
        pages.set(b"Count".to_vec(), Object::Int(2));
    })
    .unwrap();
    viewports::update(&mut d, 0, 0, [0.0, 0.0, 100.0, 100.0], "detail", &Scale::new(2.0, "m", 2).unwrap()).unwrap();
    assert_eq!(viewports::list(&d, 0).unwrap()[0].name, "detail");
    assert_eq!(viewports::list(&d, 1).unwrap()[0].name, "shared");
    close(scale_at(&d, 1, [50.0, 50.0]).unwrap().x, 1.0);
    set_scale(&mut d, 1, [0.0, 0.0, 100.0, 100.0], "overview", &Scale::default()).unwrap();
    assert_eq!(d.get(ObjRef::new(5, 0)).as_array().unwrap().len(), 1);
    viewports::remove(&mut d, 0, 0).unwrap();
    assert_eq!(viewports::list(&d, 1).unwrap().len(), 2);
}

#[test]
fn unsupported_caption_fails_before_modifying_the_document() {
    let mut d = fixture("", "", &[]);
    let mut m = new(Kind::Distance, vec![[0.0, 0.0], [3.0, 4.0]]);
    m.scale.unit = "\u{10ffff}".into();
    let before = write_full(&d, &SaveOptions::default()).unwrap();
    let error = add(&mut d, &m, &Meta::default()).unwrap_err().to_string();
    assert!(error.contains("U+10FFFF"), "{error}");
    assert_eq!(before, write_full(&d, &SaveOptions::default()).unwrap());
}

#[test]
fn editing_one_number_format_preserves_other_indirect_arrays_and_unknown_fields() {
    let mut doc = fixture("", "", &[]);
    let mut distance = NumberFormat::decimal("ft", 1.0, 2);
    let mut vendor = Dict::new();
    vendor.set(b"VendorFormat".to_vec(), 73i64);
    distance.source = Some(vendor);
    let formats = NumberFormats {
        x: vec![NumberFormat::decimal("m", 2.0, 3)],
        y: Some(vec![NumberFormat::decimal("cm", 300.0, 1)]),
        cyx: Some(0.01),
        distance: vec![distance],
        area: vec![NumberFormat::decimal("acre", 0.25, 4)],
    };
    let mut dictionary = Scale::from_formats(formats, "drawing ratio").unwrap().dictionary().unwrap();
    dictionary.set(b"VendorMeasure".to_vec(), 91i64);
    let mut refs = Vec::new();
    for key in [b"X".as_slice(), b"Y", b"D", b"A"] {
        let reference = doc.add(dictionary.get(key).unwrap().clone());
        dictionary.set(key.to_vec(), Object::Ref(reference));
        refs.push(reference);
    }
    let scale = Scale::read(&doc, &Object::Dict(dictionary.clone())).unwrap();
    let mut edited = scale.formats.clone().unwrap();
    edited.area[0].denominator = 1000;
    let updated = scale.with_formats(edited, "updated ratio").unwrap();
    let result = updated.dictionary().unwrap();
    for (key, reference) in [b"X".as_slice(), b"Y", b"D"].into_iter().zip(&refs) {
        assert!(matches!(result.get(key), Some(Object::Ref(r)) if r==reference));
    }
    assert!(matches!(result.get(b"A"), Some(Object::Array(_))));
    assert_eq!(result.int(b"VendorMeasure"), Some(91));
    let resolved = doc.resolve(result.get(b"D").unwrap());
    let value = doc.resolve(&resolved.as_array().unwrap()[0]);
    assert_eq!(value.as_dict().unwrap().int(b"VendorFormat"), Some(73));
    let mut without_y = updated.formats.clone().unwrap();
    without_y.y = None;
    without_y.cyx = None;
    let simplified = updated.with_formats(without_y, "updated ratio").unwrap().dictionary().unwrap();
    assert!(!simplified.contains(b"Y"));
    assert!(!simplified.contains(b"CYX"));
    assert!(matches!(dictionary.get(b"Y"),Some(Object::Ref(r)) if *r==refs[1]));
    assert_eq!(doc.get(refs[3]).as_array().unwrap().len(), 1);
}

mod embedded_three_d_tests {
    use super::*;
    fn model_string(bytes: &mut Vec<u8>, value: &str) {
        bytes.extend((value.len() as u16).to_le_bytes());
        bytes.extend(value.as_bytes());
    }
    fn block_bytes(kind: u32, data: Vec<u8>) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(kind.to_le_bytes());
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(0u32.to_le_bytes());
        bytes.extend(data);
        while !bytes.len().is_multiple_of(4) {
            bytes.push(0);
        }
        bytes
    }
    fn fixture_u3d() -> Vec<u8> {
        let mut d = Vec::new();
        model_string(&mut d, "part");
        d.extend(0u32.to_le_bytes());
        d.extend(1u32.to_le_bytes());
        for c in [1u32, 3, 0, 0, 0, 0, 1, 0, 0, 0, 3, 3, 0, 0, 0] {
            d.extend(c.to_le_bytes());
        }
        for _ in 0..8 {
            d.extend(1.0f32.to_le_bytes());
        }
        d.extend(0u32.to_le_bytes());
        let mut payload = block_bytes(0xFFFFFF31, d);
        let mut n = Vec::new();
        model_string(&mut n, "visible-part");
        n.extend(1u32.to_le_bytes());
        model_string(&mut n, "");
        for value in pdfcraft_model::three_d::IDENTITY {
            n.extend((value as f32).to_le_bytes());
        }
        model_string(&mut n, "part");
        n.extend(3u32.to_le_bytes());
        payload.extend(block_bytes(0xFFFFFF22, n));
        let declaration_size = payload.len() + 44;
        let mut b = Vec::new();
        model_string(&mut b, "part");
        b.extend(0u32.to_le_bytes());
        for c in [1u32, 3, 0, 0, 0, 0] {
            b.extend(c.to_le_bytes());
        }
        for p in [[0.0f32, 0.0, 0.0], [3.0, 0.0, 0.0], [0.0, 4.0, 0.0]] {
            for v in p {
                b.extend(v.to_le_bytes());
            }
        }
        for v in [0u32, 0, 1, 2] {
            b.extend(v.to_le_bytes());
        }
        payload.extend(block_bytes(0xFFFFFF3B, b));
        let mut header = Vec::new();
        header.extend(0u16.to_le_bytes());
        header.extend(0u16.to_le_bytes());
        header.extend(12u32.to_le_bytes());
        header.extend((declaration_size as u32).to_le_bytes());
        header.extend((payload.len() as u64 + 44).to_le_bytes());
        header.extend(106u32.to_le_bytes());
        header.extend(0.001f64.to_le_bytes());
        let mut bytes = block_bytes(0x00443355, header);
        bytes.extend(payload);
        bytes
    }

    fn document() -> Document {
        let mut doc = fixture("", "", &[]);
        let mut stream = Dict::new();
        stream.set(b"Type".to_vec(), Object::Name(b"3D".to_vec()));
        stream.set(b"Subtype".to_vec(), Object::Name(b"U3D".to_vec()));
        let mut first = Dict::new();
        first.set(b"XN".to_vec(), Object::String(PdfString::text("Front")));
        first.set(b"IN".to_vec(), Object::String(PdfString::text("front")));
        first.set(b"MS".to_vec(), Object::Name(b"M".to_vec()));
        first.set(b"C2W".to_vec(), nums([1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 10.]));
        let first = doc.add(Object::Dict(first));
        let second = doc.add(Object::Dict(Dict::from_iter([
            (b"XN".to_vec(), Object::String(PdfString::text("Back"))),
            (b"IN".to_vec(), Object::String(PdfString::text("back"))),
        ])));
        let views = doc.add(Object::Array(vec![Object::Ref(first), Object::Ref(second)]));
        stream.set(b"VA".to_vec(), Object::Ref(views));
        stream.set(b"DV".to_vec(), Object::Name(b"L".to_vec()));
        let model = doc.add(Object::Stream(pdfcraft_cos::Stream::flate(stream, &fixture_u3d())));
        let reference =
            doc.add(Object::Dict(Dict::from_iter([(b"Type".to_vec(), Object::Name(b"3DRef".to_vec())), (b"3DD".to_vec(), Object::Ref(model))])));
        let annotation = doc.add(Object::Dict(Dict::from_iter([
            (b"Subtype".to_vec(), Object::Name(b"3D".to_vec())),
            (b"Rect".to_vec(), nums([10., 20., 300., 400.])),
            (b"3DD".to_vec(), Object::Ref(reference)),
            (b"3DV".to_vec(), Object::String(PdfString::text("front"))),
            (b"Contents".to_vec(), Object::String(PdfString::text("Original triangle"))),
        ])));
        let annotations = doc.add(Object::Array(vec![Object::Ref(annotation)]));
        let page = pdfcraft_model::pages(&doc)[0].obj;
        doc.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Ref(annotations))).unwrap();
        doc
    }
    #[test]
    fn embedded_geometry_default_view_refs_units_and_saved_reopen() {
        let doc = document();
        let models = three_d::models(&doc, 0).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].format, "U3D");
        assert_eq!(models[0].name, "Original triangle");
        let artwork = three_d::artwork(&doc, 0, 0).unwrap();
        assert_eq!(artwork.rect, [10., 20., 300., 400.]);
        assert_eq!(artwork.views.len(), 2);
        assert_eq!(artwork.default_view.as_ref().unwrap().name, "Front");
        assert_eq!(artwork.default_view.as_ref().unwrap().camera_to_world.unwrap()[11], 10.);
        assert_eq!(artwork.units, three_d::Units { unit: "m".into(), factor: 0.001, declared: true });
        assert_eq!(artwork.scene.pick([1., 1., 10.], [0., 0., -1.]).unwrap().unwrap().point, [1., 1., 0.]);
        let reopened = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
        assert_eq!(three_d::artwork(&reopened, 0, 0).unwrap().scene, artwork.scene);
        assert!(three_d::artwork(&doc, 0, 1).is_err());
        assert!(three_d::models(&doc, 1).is_err());
    }
    #[test]
    fn model_units_follow_creation_user_display_precedence_and_ignore_unlabelled_scales() {
        let doc = document();
        let mut values = Dict::from_iter([
            (b"TU".to_vec(), Object::String(PdfString::text("mm"))),
            (b"TSm".to_vec(), Object::Real(2.)),
            (b"TSn".to_vec(), Object::Real(3.)),
        ]);
        let mut annotation = Dict::new();
        annotation.set(b"3DU".to_vec(), Object::Dict(values.clone()));
        let result = three_d::units(&doc, &annotation, Some(0.001)).unwrap();
        close(result.factor, 2. / 3.);
        assert_eq!(result.unit, "mm");
        values.set(b"USm".to_vec(), Object::Real(4.));
        annotation.set(b"3DU".to_vec(), Object::Dict(values.clone()));
        close(three_d::units(&doc, &annotation, None).unwrap().factor, 2. / 3.);
        values.set(b"UU".to_vec(), Object::String(PdfString::text("cm")));
        values.set(b"USn".to_vec(), Object::Real(5.));
        values.set(b"DU".to_vec(), Object::String(PdfString::text("m")));
        values.set(b"DSm".to_vec(), Object::Real(0.01));
        annotation.set(b"3DU".to_vec(), Object::Dict(values.clone()));
        close(three_d::units(&doc, &annotation, None).unwrap().factor, 0.008);
        values.set(b"DSn".to_vec(), Object::Real(0.));
        annotation.set(b"3DU".to_vec(), Object::Dict(values));
        assert!(three_d::units(&doc, &annotation, None).is_err());
    }
    #[test]
    fn world_space_linear_perpendicular_angle_and_circle_geometry() {
        use three_d::{Geometry, Measurement, Units};
        let units = Units { unit: "mm".into(), factor: 1000., declared: true };
        let linear = Geometry::Linear { a: [1., 2., 3.], b: [4., 6., 15.] };
        close(linear.value().unwrap(), 13.);
        let perpendicular = Geometry::Perpendicular { a: [1., 2., 3.], b: [4., 6., 15.], direction: [0., 0., 5.] };
        close(perpendicular.value().unwrap(), 5.);
        let angular = Geometry::Angular { a: [1., 0., 0.], b: [0., 1., 0.], first_direction: [3., 0., 0.], second_direction: [0., 9., 0.] };
        close(angular.value().unwrap(), std::f64::consts::FRAC_PI_2);
        let (circle, plane) = Geometry::circle([3., 0., 7.], [0., 3., 7.], [-3., 0., 7.], false).unwrap();
        close(circle.value().unwrap(), 3.);
        assert_eq!(circle.anchors()[0], [0., 0., 7.]);
        assert_eq!(plane, [0., 0., 1.]);
        assert!(Geometry::circle([0.; 3], [1.; 3], [2.; 3], false).is_err());
        let (diameter, _) = Geometry::circle([3., 0., 7.], [0., 3., 7.], [-3., 0., 7.], true).unwrap();
        close(diameter.value().unwrap(), 6.);
        // A dimension plane contains the measured direction, including Z.
        assert!(Measurement::new(linear.clone(), &units, [0., 0., 1.], [2., 4., 5.]).is_err());
        let measurement = Measurement::new(linear, &units, [4., -3., 0.], [2.5, 4., 9.]).unwrap();
        close(measurement.value, 13000.);
        assert_eq!(measurement.caption(), "13000.000 mm");
        let measurement = Measurement::new(angular, &units, [0., 0., 1.], [2., 4., 0.]).unwrap();
        close(measurement.value, 90.);
        assert_eq!(measurement.caption(), "90.000 °");
        assert!(Measurement::new(Geometry::Linear { a: [0.; 3], b: [0.; 3] }, &units, [0., 0., 1.], [0.; 3]).is_err());
    }
    #[test]
    fn standard_three_d_measurement_persistence_preserves_shared_sources_and_unknown_fields() {
        use three_d::{Geometry, Measurement, NewMeasurement};
        let mut doc = document();
        let before = doc.clone();
        let artwork = three_d::artwork(&doc, 0, 0).unwrap();
        let unknown = doc.add(Object::String(PdfString::text("kept")));
        let geometries = [
            Geometry::Linear { a: [0.; 3], b: [3., 4., 0.] },
            Geometry::Perpendicular { a: [0.; 3], b: [3., 4., 0.], direction: [0., 1., 0.] },
            Geometry::Angular { a: [1., 0., 0.], b: [0., 1., 0.], first_direction: [1., 0., 0.], second_direction: [0., 1., 0.] },
            Geometry::Radial { center: [0.; 3], on_circle: [3., 0., 0.], diameter: true, arc: Some([[0., 3., 0.], [-3., 0., 0.]]) },
        ];
        for (index, geometry) in geometries.into_iter().enumerate() {
            let mut measurement = Measurement::new(geometry, &artwork.units, [0., 0., 1.], [2., 2., 0.]).unwrap();
            measurement.label = format!("Measurement {}", index + 1);
            measurement.user_text = "checked".into();
            measurement.source.set(b"VendorExtension".to_vec(), Object::Ref(unknown));
            let new = NewMeasurement { page: 0, annotation: 0, view: None, camera: None, measurement: measurement.clone() };
            assert_eq!(three_d::add(&mut doc, &new).unwrap(), index);
            let listing = three_d::measurements(&doc, 0, 0, None).unwrap();
            assert!(listing.unsupported.is_empty());
            let saved = &listing.measurements[index];
            assert_eq!(saved.geometry, measurement.geometry);
            close(saved.value, measurement.value);
            assert_eq!(saved.source.get(b"VendorExtension"), Some(&Object::Ref(unknown)));
            assert_eq!(saved.source.name(b"Type"), Some(b"3DMeasure".as_slice()));
        }
        assert!(three_d::measurements(&before, 0, 0, None).unwrap().measurements.is_empty());
        assert_eq!(three_d::artwork(&doc, 0, 0).unwrap().scene, artwork.scene);
        assert_eq!(three_d::artwork(&doc, 0, 0).unwrap().views.len(), 2);
        assert_eq!(three_d::artwork(&doc, 0, 0).unwrap().default_view.unwrap().camera_to_world, artwork.default_view.unwrap().camera_to_world);
        let mut changed = three_d::measurements(&doc, 0, 0, None).unwrap().measurements[0].clone();
        changed.user_text = "edited".into();
        changed.source = Dict::new();
        three_d::update(&mut doc, &NewMeasurement { page: 0, annotation: 0, view: None, camera: None, measurement: changed }, 0).unwrap();
        assert_eq!(three_d::measurements(&doc, 0, 0, None).unwrap().measurements[0].source.get(b"VendorExtension"), Some(&Object::Ref(unknown)));
        let root = doc.get(doc.root().unwrap());
        let ext = doc.resolve(root.as_dict().unwrap().get(b"Extensions").unwrap());
        let adobe = doc.resolve(ext.as_dict().unwrap().get(b"ADBE").unwrap());
        assert_eq!(adobe.as_dict().unwrap().int(b"ExtensionLevel"), Some(3));
        let reopened = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
        assert_eq!(three_d::measurements(&reopened, 0, 0, None).unwrap().measurements.len(), 4);
        three_d::remove(&mut doc, 0, 0, None, 1).unwrap();
        assert_eq!(three_d::measurements(&doc, 0, 0, None).unwrap().measurements.len(), 3);
        assert_eq!(three_d::measurements(&doc, 0, 0, Some(1)).unwrap().measurements.len(), 0);
        let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
        assert!(three_d::remove(&mut doc, 0, 0, None, 99).is_err());
        assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), bytes);
    }
    #[test]
    fn pdf_camera_projection_rays_clipping_and_navigation_use_world_coordinates() {
        use crate::three_d_camera::{Binding, Camera, Projection};
        let mut camera = Camera {
            matrix: [1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., -10.],
            center: [0.; 3],
            target: [200., 100.],
            projection: Projection::Perspective { field_of_view: 90., binding: Binding::Width },
            near: 1.,
            far: None,
        };
        let viewport = [400., 200.];
        for (binding, x) in
            [(Binding::Width, 220.), (Binding::Height, 210.), (Binding::Minimum, 210.), (Binding::Maximum, 220.), (Binding::Number(40.), 204.)]
        {
            camera.projection = Projection::Perspective { field_of_view: 90., binding };
            let pixel = camera.project([1., 0., 0.], viewport).unwrap().unwrap();
            close(pixel[0], x);
            close(pixel[1], 100.);
            let [origin, ray] = camera.ray([pixel[0], pixel[1]], viewport).unwrap();
            let distance = 10. / ray[2];
            for (a, b) in origin.into_iter().zip(ray).zip([1., 0., 0.]) {
                close(a.0 + a.1 * distance, b);
            }
        }
        camera.projection = Projection::Orthographic { scale: 2., binding: Binding::Absolute };
        let pixel = camera.project([1., 1., 0.], viewport).unwrap().unwrap();
        close(pixel[0], 204.);
        close(pixel[1], 96.);
        let doc = document();
        let artwork = three_d::artwork(&doc, 0, 0).unwrap();
        let hit = camera.pick(&artwork.scene, [pixel[0], pixel[1]], viewport).unwrap().unwrap();
        for (a, b) in hit.point.into_iter().zip([1., 1., 0.]) {
            close(a, b);
        }
        camera.far = Some(5.);
        assert!(camera.pick(&artwork.scene, [pixel[0], pixel[1]], viewport).unwrap().is_none());
        camera.far = None;
        let mut scene = artwork.scene.clone();
        let mut matrix = pdfcraft_model::three_d::IDENTITY;
        matrix[14] = -9.5;
        scene.instances.push(pdfcraft_model::three_d::Instance { name: "clipped".into(), mesh: 0, transform: matrix, visibility: 3 });
        for (a, b) in camera.pick(&scene, [pixel[0], pixel[1]], viewport).unwrap().unwrap().point.into_iter().zip([1., 1., 0.]) {
            close(a, b);
        }
        camera.projection = Projection::Perspective { field_of_view: 90., binding: Binding::Width };
        let before = camera.project([1., 1., 0.], viewport).unwrap().unwrap();
        camera.zoom(2.).unwrap();
        let zoomed = camera.project([1., 1., 0.], viewport).unwrap().unwrap();
        close(zoomed[0] - 200., 2. * (before[0] - 200.));
        camera.pan([20., -10.], viewport).unwrap();
        let panned = camera.project([1., 1., 0.], viewport).unwrap().unwrap();
        close(panned[0] - zoomed[0], 20.);
        close(panned[1] - zoomed[1], -10.);
        let center = camera.center;
        let distance = length_test(subtract_test(camera.eye(), center));
        camera.orbit(0.3, -0.2).unwrap();
        close(length_test(subtract_test(camera.eye(), center)), distance);
        let center_pixel = camera.project(center, viewport).unwrap().unwrap();
        close(center_pixel[0], 200.);
        close(center_pixel[1], 100.);
        let snapshot = camera.clone();
        assert!(camera.zoom(0.).is_err());
        assert_eq!(camera, snapshot);
        assert!(camera.orbit(f64::NAN, 0.).is_err());
        assert_eq!(camera, snapshot);
        camera.matrix[0] = 0.;
        camera.matrix[1] = 0.;
        camera.matrix[2] = 0.;
        assert!(camera.validate().is_err());
        assert!(camera.ray([0., 0.], viewport).is_err());
    }
    fn subtract_test(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }
    fn length_test(a: [f64; 3]) -> f64 {
        a.iter().map(|v| v * v).sum::<f64>().sqrt()
    }
    #[test]
    fn saved_three_d_measurements_capture_orbited_camera_and_fail_atomically() {
        use crate::three_d_camera::Camera;
        use three_d::{Geometry, Measurement, NewMeasurement};
        let mut doc = document();
        let artwork = three_d::artwork(&doc, 0, 0).unwrap();
        let mut camera = Camera::fit(&artwork.scene, [290., 380.]).unwrap();
        camera.orbit(0.4, -0.3).unwrap();
        camera.zoom(1.5).unwrap();
        let measurement = Measurement::new(Geometry::Linear { a: [0.; 3], b: [3., 4., 0.] }, &artwork.units, [0., 0., 1.], [2., 2., 0.]).unwrap();
        let new = NewMeasurement { page: 0, annotation: 0, view: None, measurement, camera: Some(camera.clone()) };
        three_d::add(&mut doc, &new).unwrap();
        let reopened = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
        let artwork = three_d::artwork(&reopened, 0, 0).unwrap();
        let restored = Camera::from_artwork(&reopened, &artwork, None).unwrap();
        for point in [[0.; 3], [3., 4., 0.], [0., 4., 0.]] {
            let before = camera.project(point, [640., 480.]).unwrap().unwrap();
            let after = restored.project(point, [640., 480.]).unwrap().unwrap();
            for (a, b) in before.into_iter().zip(after) {
                close(a, b);
            }
        }
        let root = doc.root().unwrap();
        doc.update_dict(root, |d| d.set(b"Extensions".to_vec(), Object::Name(b"invalid".to_vec()))).unwrap();
        let before = write_full(&doc, &SaveOptions::default()).unwrap();
        assert!(three_d::add(&mut doc, &new).is_err());
        assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
    }
    #[test]
    fn locked_three_d_annotations_remain_readable_and_refuse_all_measurement_edits_atomically() {
        use three_d::{Geometry, Measurement, NewMeasurement};
        for flags in [Object::Int(64), Object::Int(128), Object::Int(512), Object::Int(-1), Object::Name(b"invalid".to_vec())] {
            let mut doc = document();
            let artwork = three_d::artwork(&doc, 0, 0).unwrap();
            let measurement = Measurement::new(Geometry::Linear { a: [0.; 3], b: [3., 4., 0.] }, &artwork.units, [0., 0., 1.], [1., 1., 0.]).unwrap();
            let new = NewMeasurement { page: 0, annotation: 0, view: None, measurement, camera: None };
            three_d::add(&mut doc, &new).unwrap();
            let page = pdfcraft_model::pages(&doc)[0].dict.clone();
            let values = doc.resolve(page.get(b"Annots").unwrap());
            let Object::Ref(reference) = values.as_array().unwrap()[0] else { panic!("expected an indirect annotation") };
            doc.update_dict(reference, |d| d.set(b"F".to_vec(), flags)).unwrap();
            assert!(three_d::artwork(&doc, 0, 0).unwrap().edit_error.is_some());
            assert_eq!(three_d::measurements(&doc, 0, 0, None).unwrap().measurements.len(), 1);
            let before = write_full(&doc, &SaveOptions::default()).unwrap();
            assert!(three_d::add(&mut doc, &new).is_err());
            assert!(three_d::update(&mut doc, &new, 0).is_err());
            assert!(three_d::remove(&mut doc, 0, 0, None, 0).is_err());
            assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
        }
    }
}

#[test]
fn picked_three_d_constructions_and_markup_use_model_geometry() {
    use crate::three_d::{Geometry, Kind, Measurement, Units};
    let units = Units { unit: "mm".into(), factor: 2., declared: true };
    let normal = [0., 0., 1.];
    let line = Measurement::from_points(Kind::Linear, &[[0., 0., 0.], [3., 4., 0.]], &units, normal, false).unwrap();
    assert_eq!(line.value, 10.);
    assert_eq!(line.segments().unwrap()[0], [[0., 0., 0.], [3., 4., 0.]]);
    let perp = Measurement::from_points(Kind::Perpendicular, &[[2., 5., 0.], [0., 0., 0.], [4., 0., 0.]], &units, normal, false).unwrap();
    assert_eq!(perp.value, 10.);
    assert_eq!(perp.geometry.anchors()[1], [2., 0., 0.]);
    assert_eq!(perp.segments().unwrap().len(), 4);
    let angle = Measurement::from_points(Kind::Angular, &[[4., 0., 0.], [0., 0., 0.], [0., 3., 0.]], &units, normal, false).unwrap();
    assert_eq!(angle.value, 90.);
    assert_eq!(angle.segments().unwrap().len(), 34);
    let mut circle = Measurement::from_points(Kind::Radial, &[[3., 0., 0.], [0., 3., 0.], [-3., 0., 0.]], &units, normal, true).unwrap();
    assert_eq!(circle.value, 12.);
    circle.show_circle = true;
    assert_eq!(circle.segments().unwrap().len(), 66);
    assert!(Geometry::from_points(Kind::Linear, &[[0., 0., 0.]], normal, false).is_err());
    assert!(Geometry::from_points(Kind::Perpendicular, &[[2., 0., 0.], [0., 0., 0.], [4., 0., 0.]], normal, false).is_err());
    let parallel = Measurement::from_points(Kind::Linear, &[[0., 0., 0.], [0., 0., 2.]], &units, normal, false).unwrap();
    assert_eq!(parallel.value, 4.);
    let (large_circle, plane) = Geometry::circle([1e8, 0., 0.], [0., 1e8, 0.], [-1e8, 0., 0.], false).unwrap();
    assert!((large_circle.value().unwrap() - 1e8).abs() < 1e-5);
    assert_eq!(plane, [0., 0., 1.]);
}

#[test]
fn degenerate_projection_parameters_and_half_turn_constructions_are_checked() {
    use crate::three_d::{Kind, Measurement, Units};
    use crate::three_d_camera::{Binding, Camera, Projection};
    let half_turn =
        Measurement::from_points(Kind::Angular, &[[0., 0., 2.], [0., 0., 0.], [0., 0., -2.]], &Units::default(), [0., 0., 1.], false).unwrap();
    assert_eq!(half_turn.value, 180.);
    assert_eq!(half_turn.segments().unwrap().len(), 34);
    let good = Camera {
        matrix: [1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0.],
        center: [0., 0., 1.],
        target: [100., 100.],
        projection: Projection::Perspective { field_of_view: 45., binding: Binding::Width },
        near: 0.001,
        far: None,
    };
    good.validate().unwrap();
    let mut tiny = good.clone();
    tiny.target[0] = 1e-300;
    assert!(tiny.validate().is_err());
    let mut infinite = good.clone();
    infinite.projection = Projection::Perspective { field_of_view: 1e-300, binding: Binding::Width };
    assert!(infinite.validate().is_err());
    let mut nan = good.clone();
    nan.projection = Projection::Orthographic { scale: 1., binding: Binding::Number(f64::NAN) };
    assert!(nan.validate().is_err());
    let mut overflow = good;
    overflow.projection = Projection::Orthographic { scale: 1e12, binding: Binding::Number(1e12) };
    assert!(overflow.project_camera([1e12, 1e12, 1.], [2048., 2048.]).is_err());
}
