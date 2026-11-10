use std::sync::Arc;

use pdfcraft_cos::Document;

use super::*;

/// A document from its objects (object 1 is the catalog, 2 the page tree, 3 the page).
fn build(objs: &[String]) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

fn stream(dict: &str, data: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{data}\nendstream", data.len())
}

/// A one-page document that paints `content`, with `resources` (inside `<< >>`) and extra
/// objects from 5 on.
fn page(content: &str, resources: &str, extra: &[String]) -> Document {
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << {resources} >> /Contents 4 0 R >>"),
        stream("", content),
    ];
    objs.extend(extra.iter().cloned());
    build(&objs)
}

fn close(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01
}

#[test]
fn reading_shapes_reports_unread_content() {
    let doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>".into(),
        stream("/Filter /UnknownFilter", "0 0 10 10 re S"),
    ]);
    let read = reading_shapes(&doc, 0).unwrap();
    assert!(read.shapes.is_empty());
    assert!(read.truncated, "unread artwork must not be silently treated as an empty page");
}

/// The rules, boxes and fills of a form, with their paint, through `cm`, `q`/`Q`, colour spaces,
/// dashes, curves, clips and a form XObject.
#[test]
fn reading_shapes_keeps_rules_boxes_and_their_paint() {
    let content = [
        // A black rule 0.7 wide, placed by cm inside q/Q.
        "q 1 0 0 1 50 700 cm 0 G 0.7 w 0 0 m 200 0 l S Q",
        // A gray filled box.
        "0.5 g 100 100 50 20 re f",
        // Red, dashed, and twice as wide under a scaling CTM.
        "q 2 0 0 2 0 0 cm 1 0 0 RG 1 w [3] 0 d 10 10 m 20 10 l S Q",
        // A CMYK stroke, and a Separation tint at full ink.
        "0 0 0 1 K 300 300 m 310 300 l S",
        "/Sep CS 1 SCN 400 400 m 410 410 l S",
        // A pattern fill can't be read as a colour, and an `n` path paints nothing.
        "/Pat cs /P0 scn 0 0 10 10 re f 5 5 m 6 6 l n",
        // Clipped away entirely; after Q the same rule shows.
        "q 0 0 10 10 re W n 0 G 500 500 m 600 500 l S Q 0 G 500 500 m 600 500 l S",
        // A filled curve, flattened.
        "0 g 100 600 m 100 650 150 650 150 600 c h f",
        // A rule drawn by a form XObject moved by its matrix.
        "/Fm1 Do",
    ]
    .join("\n");
    let resources = "/ColorSpace << /Sep [/Separation /Black /DeviceCMYK 6 0 R] /Pat /Pattern >> /XObject << /Fm1 5 0 R >>";
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Matrix [1 0 0 1 10 10]", "0 G 0 0 m 5 0 l S");
    let tint = "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 0 0 1] /N 1 >>".to_string();
    let doc = page(&content, resources, &[form, tint]);
    let read = reading_shapes(&doc, 0).unwrap();
    assert!(!read.truncated);
    let shapes = read.shapes;
    assert_eq!(shapes.len(), 8, "{shapes:#?}");

    let rule = &shapes[0];
    assert!(close(rule.subpaths[0].points[0], [50.0, 700.0]) && close(rule.subpaths[0].points[1], [250.0, 700.0]), "{rule:?}");
    assert_eq!(rule.stroke, Some(Stroke { color: [0.0; 3], width: 0.7, dashed: false }));
    assert_eq!(rule.fill, None);

    let boxed = &shapes[1];
    assert_eq!((boxed.fill, boxed.stroke), (Some([0.5; 3]), None));
    assert!(boxed.subpaths[0].closed && boxed.subpaths[0].points.len() == 4);
    assert_eq!(boxed.bounds, [100.0, 100.0, 150.0, 120.0]);

    let red = &shapes[2];
    assert!(close(red.subpaths[0].points[0], [20.0, 20.0]) && close(red.subpaths[0].points[1], [40.0, 20.0]));
    let stroke = red.stroke.unwrap();
    assert_eq!(stroke.color, [1.0, 0.0, 0.0]);
    assert!((stroke.width - 2.0).abs() < 1e-9 && stroke.dashed, "{stroke:?}");

    assert_eq!(shapes[3].stroke.map(|s| s.color), Some([0.0; 3]), "CMYK black");
    assert_eq!(shapes[4].stroke.map(|s| s.color), Some([0.0; 3]), "a full Separation tint is black");

    // Only the unclipped copy of the rule at y = 500.
    assert!(close(shapes[5].subpaths[0].points[0], [500.0, 500.0]));
    assert!(shapes.iter().filter(|s| s.subpaths[0].points.first().is_some_and(|p| close(*p, [500.0, 500.0]))).count() == 1);

    let curve = &shapes[6];
    assert!(curve.subpaths[0].closed);
    assert_eq!(curve.subpaths[0].points.len(), 1 + CURVE_STEPS);
    assert!(curve.bounds[3] > 635.0 && curve.bounds[3] < 640.0, "the curve's top, not its control points: {:?}", curve.bounds);

    let from_form = &shapes[7];
    assert!(close(from_form.subpaths[0].points[0], [10.0, 10.0]) && close(from_form.subpaths[0].points[1], [15.0, 10.0]), "{from_form:?}");
}

/// After `h` the current point is the subpath's start, and a segment from there opens a new
/// subpath; segments before any `m`, and hostile coordinates, are skipped without a panic.
#[test]
fn reading_shapes_is_lenient_with_odd_paths() {
    let doc = page("0 G 5 5 l S 10 10 m 20 10 l h 20 20 l S 1e300 0 m 3 3 l S NaN 0 m 4 4 l S 0 0 m 7 7 l S", "", &[]);
    let shapes = reading_shapes(&doc, 0).unwrap().shapes;
    let first = &shapes[0];
    assert_eq!(first.subpaths.len(), 2, "{first:?}");
    assert!(first.subpaths[0].closed && !first.subpaths[1].closed);
    assert!(close(first.subpaths[1].points[0], [10.0, 10.0]) && close(first.subpaths[1].points[1], [20.0, 20.0]));
    // The hostile moves leave nothing to draw from; the last rule is read.
    assert!(shapes.iter().all(|s| s.subpaths.iter().flat_map(|p| &p.points).all(|p| p[0].abs() <= MAX_COORD)));
    assert!(close(shapes.last().unwrap().subpaths[0].points[1], [7.0, 7.0]));
    assert!(matches!(reading_shapes(&doc, 1), Err(EditError::NoSuchPage(1))));
}

/// A page of more paths than [`MAX_PAGE_SHAPES`] keeps the first ones and says it stopped.
#[test]
fn reading_shapes_stops_at_its_limit_and_says_so() {
    let doc = page(&"0 0 m 1 1 l S ".repeat(MAX_PAGE_SHAPES + 10), "", &[]);
    let read = reading_shapes(&doc, 0).unwrap();
    assert_eq!(read.shapes.len(), MAX_PAGE_SHAPES);
    assert!(read.truncated);
    let doc = page(&"0 0 m 1 1 l S ".repeat(10), "", &[]);
    assert!(!reading_shapes(&doc, 0).unwrap().truncated);
}

/// A clip of no area shows nothing: a path through a clip of no height, or across two clips
/// that don't meet, is left out, as a renderer draws none of it.
#[test]
fn reading_shapes_leaves_out_what_an_empty_clip_hides() {
    let content = [
        // A clip of no height: a page-wide rule along it shows nothing.
        "q 0 300 612 0 re W n 0 G 0 300 m 612 300 l S Q",
        // Two clips that don't meet: a box over both shows nothing.
        "q 100 100 100 100 re W n 300 300 100 100 re W n 0 g 150 150 200 200 re f Q",
        // The same box without them shows.
        "0 g 150 150 200 200 re f",
    ]
    .join("\n");
    let shapes = reading_shapes(&page(&content, "", &[]), 0).unwrap().shapes;
    assert_eq!(shapes.len(), 1, "{shapes:#?}");
    assert_eq!(shapes[0].bounds, [150.0, 150.0, 350.0, 350.0]);
}

#[test]
fn office_regression_excess_saves_do_not_leak_their_transform() {
    let data = format!("{}q 1 0 0 1 100 100 cm 0 0 10 10 re S Q 10 20 m 30 40 l S {}", "q ".repeat(256), "Q ".repeat(256));
    let read = reading_shapes(&page(&data, "", &[]), 0).unwrap();
    assert!(read.truncated);
    assert_eq!(read.shapes.len(), 1);
    assert_eq!(read.shapes[0].subpaths[0].points, [[10.0, 20.0], [30.0, 40.0]]);
}

#[test]
fn review_followup_none_colorants_do_not_paint() {
    for space in ["[/Separation /None /DeviceGray 5 0 R]", "[/DeviceN [/None] /DeviceGray 5 0 R]"] {
        let doc = page(
            "/Hidden cs 1 scn 0 0 10 10 re f 1 0 0 RG 20 0 10 10 re B /Hidden CS 1 SCN 0 1 0 rg 40 0 10 10 re B",
            &format!("/ColorSpace << /Hidden {space} >>"),
            &["<< /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >>".into()],
        );
        let read = reading_shapes(&doc, 0).unwrap();
        assert_eq!(read.shapes.len(), 2, "{space}");
        assert_eq!(read.shapes[0].fill, None);
        assert_eq!(read.shapes[0].stroke.unwrap().color, [1.0, 0.0, 0.0]);
        assert_eq!(read.shapes[1].stroke, None);
        assert_eq!(read.shapes[1].fill, Some([0.0, 1.0, 0.0]));
    }
}

#[test]
fn review_followup_shapes_honor_optional_content_and_form_membership() {
    for config in ["/OFF [5 0 R]", "/BaseState /OFF /ON [6 0 R]", "/AS [<< /Event /View /Category [/View] /OCGs [5 0 R] >>]"] {
        let doc = build(&[
            format!("<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R] /D << {config} >> >> >>"),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /Properties << /Off 5 0 R /On 6 0 R /Both 8 0 R >> /XObject << /Fm 7 0 R >> >> >>".into(),
            stream("", "/OC /Off BDC 0 0 10 10 re S /Span BMC 20 0 10 10 re S EMC EMC /OC /On BDC 40 0 10 10 re S EMC /Fm Do /OC /Both BDC 60 0 10 10 re S EMC 80 0 10 10 re S"),
            "<< /Type /OCG /Name (hidden) /Usage << /View << /ViewState /OFF >> >> >>".into(),
            "<< /Type /OCG /Name (shown) >>".into(),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 200 200] /OC 5 0 R", "100 0 10 10 re S"),
            "<< /Type /OCMD /OCGs [5 0 R 6 0 R] /P /AllOn >>".into(),
        ]);
        let read = reading_shapes(&doc, 0).unwrap();
        assert!(!read.truncated);
        assert_eq!(read.shapes.iter().map(|s| s.bounds[0]).collect::<Vec<_>>(), [40.0, 80.0], "{config}");
    }
}

#[test]
fn review_followup_hidden_page_fill_does_not_become_a_word_background() {
    let doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R] /D << /OFF [5 0 R] >> >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /Properties << /Hidden 5 0 R >> >> >>".into(),
        stream("", "/OC /Hidden BDC 1 0 0 rg 0 0 200 200 re f EMC 0 1 0 rg 0 0 200 200 re f"),
        "<< /Type /OCG /Name (hidden) >>".into(),
    ]);
    assert_eq!(crate::page_background(&doc, 0).unwrap(), Some([0.0, 1.0, 0.0]));
    assert_eq!(reading_shapes(&doc, 0).unwrap().shapes.len(), 1);
}

#[test]
fn artwork_reader_resolves_configuration_once_for_pages_and_empty_appearances() {
    let doc = build(&[
        format!("<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [{}] /D << /OFF [5 0 R] >> >> >>", "5 0 R ".repeat(20_000)),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /Properties << /Hidden 5 0 R >> >> >>".into(),
        stream("", "/OC /Hidden BDC 1 0 0 rg 0 0 200 200 re f EMC 0 1 0 rg 0 0 200 200 re f"),
        "<< /Type /OCG >>".into(),
    ]);
    let before = visibility::CONFIGURATION_BUILDS.get();
    let reader = crate::ArtworkReader::new(&doc);
    for _ in 0..5000 {
        let read = reader.reading_shape_ops(&[], &Dict::new());
        assert!(!read.truncated);
        assert!(read.shapes.is_empty());
    }
    for _ in 0..3 {
        let shapes = reader.reading_shapes(0).unwrap();
        assert!(!shapes.truncated);
        assert_eq!(shapes.shapes.len(), 1);
        assert_eq!(reader.page_background(0).unwrap(), Some([0.0, 1.0, 0.0]));
    }
    assert_eq!(visibility::CONFIGURATION_BUILDS.get() - before, 1, "empty appearances must not rescan the catalog's layer lists");
}

#[test]
fn artwork_reader_keeps_evaluation_limits_and_caches_local() {
    let doc = page("0 0 10 10 re S", "", &["<< /Type /OCG >>".into()]);
    let reader = crate::ArtworkReader::new(&doc);
    // Exhaust the evaluation budget of one appearance, without exhausting configuration work.
    let members = "5 0 R ".repeat(100_000);
    let exhausted = parse(format!("/OC << /Type /OCMD /OCGs [{members}] >> BDC 0 0 10 10 re S EMC").as_bytes());
    let failed = reader.reading_shape_ops(&exhausted.ops, &Dict::new());
    assert!(failed.truncated);
    assert!(failed.shapes.is_empty());
    let normal = parse(b"/OC << /Type /OCMD /OCGs [5 0 R] >> BDC 0 0 10 10 re S EMC");
    let good = reader.reading_shape_ops(&normal.ops, &Dict::new());
    assert!(!good.truncated);
    assert_eq!(good.shapes.len(), 1);
    assert!(!reader.reading_shapes(0).unwrap().truncated);
}
