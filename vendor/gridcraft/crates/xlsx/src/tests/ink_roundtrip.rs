//! Ink uses visible DrawingML geometry, and only representable polylines become editable ink.

use std::io::Read;
use std::sync::Arc;

use gridcraft_model::{Anchor, AnchorMode, Color, Shape, ShapeKind, Sheet, Workbook};

use super::minimal;
use crate::xml::{self, El};
use crate::{read_xlsx, write_xlsx};

const POINTS: &str = r#"<a:moveTo><a:pt x="-20" y="10"/></a:moveTo><a:lnTo><a:pt x="100" y="80"/></a:lnTo><a:lnTo><a:pt x="180" y="20"/></a:lnTo>"#;
const PATH: &str = r#"<a:path w="200" h="100" fill="none">"#;

fn drawing() -> String {
    format!(
        r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
        <xdr:oneCellAnchor><xdr:from><xdr:col>0</xdr:col><xdr:colOff>190500</xdr:colOff><xdr:row>0</xdr:row><xdr:rowOff>285750</xdr:rowOff></xdr:from><xdr:ext cx="952500" cy="476250"/>
        <xdr:sp><xdr:nvSpPr><xdr:cNvPr id="1" name="Independent polyline"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr>
        <a:xfrm><a:off x="0" y="0"/><a:ext cx="952500" cy="476250"/></a:xfrm>
        <a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l="0" t="0" r="r" b="b"/><a:pathLst>{PATH}{POINTS}</a:path></a:pathLst></a:custGeom>
        <a:noFill/><a:ln w="28575" cap="flat"><a:solidFill><a:srgbClr val="A12BCD"/></a:solidFill><a:miter/></a:ln>
        </xdr:spPr></xdr:sp><xdr:clientData/></xdr:oneCellAnchor></xdr:wsDr>"#
    )
}

fn read_drawing(drawing: &str) -> Workbook {
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/></Relationships>"#;
    let bytes = minimal(
        r#"<sheetData/><drawing r:id="rId1"/>"#,
        &[("xl/worksheets/_rels/sheet1.xml.rels", rels), ("xl/drawings/drawing1.xml", drawing)],
        "",
        "",
    );
    read_xlsx(&bytes).unwrap().0
}

fn points(shape: &Shape) -> Vec<[f64; 2]> {
    shape
        .text
        .split(';')
        .map(|pair| {
            let (x, y) = pair.split_once(',').unwrap();
            [x.parse().unwrap(), y.parse().unwrap()]
        })
        .collect()
}

fn has_caption(element: &El) -> bool {
    (element.name == "t" && !element.text.is_empty()) || element.children.iter().any(has_caption)
}

#[test]
fn exported_ink_has_a_visible_unfilled_path_and_correct_stroke() {
    let mut sheet = Sheet::new("Ink");
    for (id, width) in [(1, 3), (2, 12)] {
        sheet.shapes.push(Shape {
            id,
            kind: ShapeKind::Ink,
            anchor: Anchor { width: 100.0, height: 50.0, mode: AnchorMode::Absolute, ..Default::default() },
            fill: Color::Rgb(width << 16),
            line: Color::Rgb(0xA12BCD),
            text: "-10.0,5.0;50.0,40.0;90.0,10.0".into(),
            format: Default::default(),
            rotation: 0.0,
            math: Default::default(),
        });
    }
    let wb = Workbook { sheets: vec![Arc::new(sheet)], ..Default::default() };
    let bytes = write_xlsx(&wb).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut drawing = String::new();
    archive.by_name("xl/drawings/drawing1.xml").unwrap().read_to_string(&mut drawing).unwrap();
    let root = xml::parse(drawing.as_bytes()).unwrap();
    let shapes: Vec<_> = root.children.iter().filter_map(|anchor| anchor.child("sp")).collect();
    assert_eq!(shapes.len(), 2);
    for (shape, width) in shapes.iter().zip([3, 12]) {
        assert!(!has_caption(shape), "stroke coordinates must not be visible text");
        let properties = shape.child("spPr").unwrap();
        assert!(properties.child("prstGeom").is_none());
        assert!(properties.child("noFill").is_some());
        let paths: Vec<_> = properties.path(&["custGeom", "pathLst"]).unwrap().kids("path").collect();
        assert_eq!(paths.len(), 1);
        let path = paths[0];
        assert_eq!(path.attr("fill"), Some("none"));
        let w = path.attr_i64("w").unwrap();
        let h = path.attr_i64("h").unwrap();
        assert!(w > 0 && h > 0);
        assert_eq!(path.children.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["moveTo", "lnTo", "lnTo"]);
        for (command, [x, y]) in path.children.iter().zip([[-10.0, 5.0], [50.0, 40.0], [90.0, 10.0]]) {
            let point = command.child("pt").unwrap();
            let actual_x = point.attr_i64("x").unwrap() as f64 * 100.0 / w as f64;
            let actual_y = point.attr_i64("y").unwrap() as f64 * 50.0 / h as f64;
            assert!((actual_x - x).abs() < 0.001 && (actual_y - y).abs() < 0.001);
        }
        let line = properties.child("ln").unwrap();
        assert_eq!(line.attr_i64("w"), Some(9525 * width));
        assert_eq!(line.attr("cap"), Some("flat"));
        assert!(line.child("miter").is_some());
        let color = line.path(&["solidFill", "srgbClr"]).unwrap();
        assert_eq!(color.attr("val"), Some("A12BCD"));
        if width > 8 {
            assert_eq!(color.child_val("alpha"), Some("45000"));
        } else {
            assert!(color.child("alpha").is_none_or(|a| a.attr("val") == Some("100000")));
        }
    }
}

#[test]
fn numeric_polyline_from_an_independent_package_is_scaled_into_editable_ink() {
    for (width, alpha) in [(3, ""), (12, r#"<a:alpha val="45000"/>"#)] {
        let xml = drawing()
            .replace("w=\"28575\"", &format!("w=\"{}\"", width * 9525))
            .replace(r#"<a:srgbClr val="A12BCD"/>"#, &format!(r#"<a:srgbClr val="A12BCD">{alpha}</a:srgbClr>"#));
        let wb = read_drawing(&xml);
        let shapes = &wb.sheet(0).unwrap().shapes;
        assert_eq!(shapes.len(), 1);
        let shape = &shapes[0];
        assert_eq!(shape.kind, ShapeKind::Ink);
        assert_eq!(points(shape), [[-10.0, 5.0], [50.0, 40.0], [90.0, 10.0]]);
        assert_eq!(shape.line, Color::Rgb(0xA12BCD));
        assert_eq!(shape.fill, Color::Rgb(width << 16));
        assert_eq!((shape.anchor.dx, shape.anchor.dy), (20.0, 30.0));
        assert_eq!((shape.anchor.width, shape.anchor.height), (100.0, 50.0));
    }
}

#[test]
fn imported_points_are_not_snapped_to_native_tenth_units() {
    let xml = drawing().replace("w=\"200\"", "w=\"2000000\"").replace(
        POINTS,
        r#"<a:moveTo><a:pt x="2001" y="10"/></a:moveTo><a:lnTo><a:pt x="1000000" y="80"/></a:lnTo><a:lnTo><a:pt x="1800000" y="20"/></a:lnTo>"#,
    );
    let wb = read_drawing(&xml);
    let shape = &wb.sheet(0).unwrap().shapes[0];
    assert_eq!(shape.kind, ShapeKind::Ink);
    let original = points(shape);
    assert!((original[0][0] - 0.10005).abs() < 0.00000001, "an imported coordinate is not necessarily a native tenth-unit point");
    let (reopened, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    let after = &reopened.sheet(0).unwrap().shapes[0];
    assert_eq!(after.kind, ShapeKind::Ink);
    for (actual, expected) in points(after).iter().zip(original) {
        for axis in 0..2 {
            // A saved DrawingML coordinate is quantized to one EMU, plus f32 model precision.
            assert!((actual[axis] - expected[axis]).abs() < 0.00006);
        }
    }
}

#[test]
fn unsupported_geometry_or_appearance_is_not_reclassified_as_editable_ink() {
    let original = drawing();
    let cases = [
        ("formula coordinate", original.replace("x=\"-20\"", "x=\"wd2\"")),
        ("nonfinite coordinate", original.replace("x=\"-20\"", "x=\"NaN\"")),
        ("overflow coordinate", original.replace("x=\"-20\"", "x=\"9223372036854775808\"")),
        ("zero path width", original.replace("w=\"200\"", "w=\"0\"")),
        ("missing point", original.replace(r#"<a:pt x="100" y="80"/>"#, "")),
        (
            "curve",
            original
                .replace(r#"<a:lnTo><a:pt x="100" y="80"/></a:lnTo>"#, r#"<a:quadBezTo><a:pt x="40" y="20"/><a:pt x="100" y="80"/></a:quadBezTo>"#),
        ),
        ("closed path", original.replace("</a:path>", "<a:close/></a:path>")),
        ("multiple paths", original.replace("</a:pathLst>", &format!("{PATH}{POINTS}</a:path></a:pathLst>"))),
        ("filled path", original.replace("fill=\"none\"", "fill=\"norm\"")),
        ("filled shape", original.replace("<a:noFill/>", "<a:solidFill><a:srgbClr val=\"112233\"/></a:solidFill>")),
        ("rotation", original.replace("<a:xfrm>", "<a:xfrm rot=\"60000\">")),
        ("reflection", original.replace("<a:xfrm>", "<a:xfrm flipH=\"1\">")),
        ("square caps", original.replace("cap=\"flat\"", "cap=\"sq\"")),
        ("round join", original.replace("<a:miter/>", "<a:round/>")),
        ("dash", original.replace("</a:ln>", "<a:prstDash val=\"dash\"/></a:ln>")),
        ("arrow", original.replace("</a:ln>", "<a:headEnd type=\"triangle\"/></a:ln>")),
        (
            "caption",
            original.replace("</xdr:sp>", "<xdr:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Caption</a:t></a:r></a:p></xdr:txBody></xdr:sp>"),
        ),
        ("fractional width", original.replace("w=\"28575\"", "w=\"33338\"")),
        ("unsupported opacity", original.replace(r#"<a:srgbClr val="A12BCD"/>"#, r#"<a:srgbClr val="A12BCD"><a:alpha val="50000"/></a:srgbClr>"#)),
    ];
    for (name, xml) in cases {
        let wb = read_drawing(&xml);
        assert!(wb.sheet(0).unwrap().shapes.iter().all(|s| s.kind != ShapeKind::Ink), "{name} was silently converted to editable ink");
    }
}

#[test]
fn oversized_polyline_is_not_truncated_into_different_ink() {
    let mut commands = String::from(r#"<a:moveTo><a:pt x="0" y="0"/></a:moveTo>"#);
    for _ in 0..20_000 {
        commands.push_str(r#"<a:lnTo><a:pt x="100" y="80"/></a:lnTo>"#);
    }
    let wb = read_drawing(&drawing().replace(POINTS, &commands));
    assert!(wb.sheet(0).unwrap().shapes.iter().all(|s| s.kind != ShapeKind::Ink));
}
