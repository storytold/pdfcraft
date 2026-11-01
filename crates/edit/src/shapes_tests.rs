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
