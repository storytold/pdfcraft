use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, write_full};

use super::*;
use crate::pages;

const A4: (f64, f64) = (595.28, 841.89);

/// A document from its objects (object 1 the catalog, 2 the page tree).
fn build(objs: &[&str]) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

const CONTENT: &str = "<< /Length 8 >>\nstream\n0 0 m S\nendstream";

/// One Letter page holding `extra` in its dictionary.
fn letter(extra: &str) -> Document {
    build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        &format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R {extra} >>"),
        CONTENT,
    ])
}

fn request(width: f64, height: f64) -> ScaleRequest {
    ScaleRequest { width, height, mode: ScaleMode::Fit, orientation: ScaleOrientation::Match }
}

fn page(doc: &Document, i: usize) -> Dict {
    doc.get(pages(doc).unwrap()[i].obj).as_dict().cloned().unwrap()
}

fn rect_at(doc: &Document, d: &Dict, key: &[u8]) -> [f64; 4] {
    rect(doc, d.get(key).unwrap()).unwrap()
}

fn size(r: [f64; 4]) -> (f64, f64) {
    (r[2] - r[0], r[3] - r[1])
}

/// The scale and translation the page's first content stream applies.
fn placement(doc: &Document, i: usize) -> (f64, f64, f64) {
    let contents = page(doc, i).get(b"Contents").cloned().unwrap();
    let first = doc.resolve(&contents).as_array().unwrap()[0].clone();
    let Object::Stream(s) = &*doc.resolve(&first) else { panic!("a stream") };
    let text = String::from_utf8(s.decoded().unwrap()).unwrap();
    let cm: Vec<f64> = text.lines().find(|l| l.ends_with(" cm")).unwrap().split(' ').filter_map(|t| t.parse().ok()).collect();
    (cm[0], cm[4], cm[5])
}

/// Where a point of the old page lands.
fn mapped(doc: &Document, i: usize, x: f64, y: f64) -> (f64, f64) {
    let (s, dx, dy) = placement(doc, i);
    (s * x + dx, s * y + dy)
}

/// Within a thousandth of a point.
fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn fit_shrinks_the_whole_page_inside_the_new_size_and_centres_it() {
    let mut doc = letter("/Annots [<< /Type /Annot /Subtype /Square /Rect [100 100 200 300] >>]");
    let report = scale_pages(&mut doc, &[0], request(A4.0, A4.1)).unwrap();
    let media = rect_at(&doc, &page(&doc, 0), b"MediaBox");
    assert!(close(size(media).0, A4.0) && close(size(media).1, A4.1), "the page is the size asked for");
    let (x0, y0) = mapped(&doc, 0, 0.0, 0.0);
    let (x1, y1) = mapped(&doc, 0, 612.0, 792.0);
    assert!(x0 >= -1e-3 && y0 >= -1e-3 && x1 <= A4.0 + 1e-3 && y1 <= A4.1 + 1e-3, "all of the old page shows");
    assert!(close(x0, A4.0 - x1) && close(y0, A4.1 - y1), "the spare room is split evenly");
    assert!(close(x1 - x0, A4.0) || close(y1 - y0, A4.1), "it touches two edges");
    let annot = doc.resolve(page(&doc, 0).get(b"Annots").unwrap()).as_array().unwrap()[0].clone();
    let r = rect_at(&doc, annot.as_dict().unwrap(), b"Rect");
    let (ax, ay) = mapped(&doc, 0, 100.0, 100.0);
    assert!(close(r[0], ax) && close(r[1], ay), "the comment moves with the content");
    assert_eq!((report.pages.len(), report.annotations), (1, 1));
}

#[test]
fn fill_covers_the_new_size_and_overflows_both_sides() {
    let mut doc = letter("");
    scale_pages(&mut doc, &[0], ScaleRequest { mode: ScaleMode::Fill, ..request(A4.0, A4.1) }).unwrap();
    let (x0, y0) = mapped(&doc, 0, 0.0, 0.0);
    let (x1, y1) = mapped(&doc, 0, 612.0, 792.0);
    assert!(x0 <= 1e-3 && y0 <= 1e-3 && x1 >= A4.0 - 1e-3 && y1 >= A4.1 - 1e-3, "the content covers the page");
    assert!(x0 < -1.0 && close(-x0, x1 - A4.0), "the overflow is split evenly");
}

#[test]
fn the_size_follows_the_page_orientation_unless_exact() {
    let landscape = (A4.1, A4.0);
    let mut doc = letter("");
    scale_pages(&mut doc, &[0], request(landscape.0, landscape.1)).unwrap();
    let (w, h) = size(rect_at(&doc, &page(&doc, 0), b"MediaBox"));
    assert!(w < h, "a portrait page stays portrait");
    let mut doc = letter("");
    scale_pages(&mut doc, &[0], ScaleRequest { orientation: ScaleOrientation::Exact, ..request(landscape.0, landscape.1) }).unwrap();
    let (w, h) = size(rect_at(&doc, &page(&doc, 0), b"MediaBox"));
    assert!(w > h, "exact keeps the size as given");
}

#[test]
fn a_turned_page_gets_the_size_as_shown() {
    let mut doc = letter("/Rotate 90");
    scale_pages(&mut doc, &[0], ScaleRequest { orientation: ScaleOrientation::Exact, ..request(A4.1, A4.0) }).unwrap();
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    let pdf = hayro_syntax::Pdf::new(bytes).unwrap();
    let (w, h) = pdf.pages()[0].render_dimensions();
    assert!((f64::from(w) - A4.1).abs() < 0.01 && (f64::from(h) - A4.0).abs() < 0.01, "shown {w}×{h}");
}

#[test]
fn a_user_unit_page_is_sized_in_points() {
    let mut doc = letter("/UserUnit 2");
    scale_pages(&mut doc, &[0], request(A4.0, A4.1)).unwrap();
    let (w, h) = size(rect_at(&doc, &page(&doc, 0), b"MediaBox"));
    assert!(close(w * 2.0, A4.0) && close(h * 2.0, A4.1));
}

#[test]
fn every_box_present_becomes_the_new_page_and_an_offset_crop_is_what_scales() {
    let pages_node = "<< /Type /Pages /Kids [3 0 R] /Count 1 /CropBox [100 100 400 500] >>";
    let page_obj = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /TrimBox [10 10 600 780] /Contents 4 0 R >>";
    let mut doc = build(&["<< /Type /Catalog /Pages 2 0 R >>", pages_node, page_obj, CONTENT]);
    scale_pages(&mut doc, &[0], request(600.0, 800.0)).unwrap();
    let d = page(&doc, 0);
    let media = rect_at(&doc, &d, b"MediaBox");
    assert_eq!(rect_at(&doc, &d, b"CropBox"), media, "an inherited crop box is overridden");
    assert_eq!(rect_at(&doc, &d, b"TrimBox"), media);
    assert!(!d.contains(b"BleedBox"), "absent boxes stay absent");
    let (x0, y0) = mapped(&doc, 0, 100.0, 100.0);
    let (x1, y1) = mapped(&doc, 0, 400.0, 500.0);
    assert!(close(x0, 600.0 - x1) && close(y0, 800.0 - y1), "the old visible area is centred");
    assert!(close(x1 - x0, 600.0) || close(y1 - y0, 800.0), "and fills the page one way");
}

#[test]
fn annotation_geometry_moves_and_no_zoom_keeps_its_size() {
    let annots = "/Annots [<< /Subtype /Ink /Rect [0 0 100 100] /InkList [[0 0 100 100]] /RD [10 10 10 10] >> \
                  << /Subtype /Text /F 8 /Rect [300 300 320 320] >> \
                  << /Subtype /Ink /F 8 /Rect [400 400 440 440] /InkList [[400 400 440 440]] /RD [5 5 5 5] >>                   << /Subtype /Text /F 4 /Rect [500 500 524 524] >>                   << /Subtype /Square /F 4 /Rect [500 100 524 124] >>]";
    let mut doc = letter(annots);
    scale_pages(&mut doc, &[0], request(306.0, 396.0)).unwrap();
    let list = doc.resolve(page(&doc, 0).get(b"Annots").unwrap()).as_array().unwrap().clone();
    let ink = list[0].as_dict().unwrap();
    let stroke = ink.get(b"InkList").unwrap().as_array().unwrap()[0].as_array().unwrap().clone();
    let end = mapped(&doc, 0, 100.0, 100.0);
    assert!(close(stroke[2].as_f64().unwrap(), end.0) && close(stroke[3].as_f64().unwrap(), end.1), "ink follows the content");
    assert!(ink.get(b"RD").unwrap().as_array().unwrap()[0].as_f64().unwrap() < 10.0, "margins shrink with it");
    let note = rect_at(&doc, list[1].as_dict().unwrap(), b"Rect");
    assert_eq!(size(note), (20.0, 20.0), "a no-zoom note keeps its size");
    let corner = mapped(&doc, 0, 300.0, 320.0);
    assert!(close(note[0], corner.0) && close(note[3], corner.1), "its upper-left corner moves with the page");
    // A no-zoom ink stroke stays where it was inside its rectangle, at its size.
    let ink = list[2].as_dict().unwrap();
    let r = rect_at(&doc, ink, b"Rect");
    let stroke: Vec<f64> = ink.get(b"InkList").unwrap().as_array().unwrap()[0].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert!(close(stroke[0] - r[0], 0.0) && close(stroke[1] - r[3], -40.0) && close(stroke[2] - stroke[0], 40.0), "{stroke:?} in {r:?}");
    assert_eq!(ink.get(b"RD").unwrap().as_array().unwrap()[0].as_f64(), Some(5.0), "its margins keep their size");
    // A text note is no-zoom without the flag (§12.5.6.4); another kind with the same flags is not.
    let sticky = rect_at(&doc, list[3].as_dict().unwrap(), b"Rect");
    assert_eq!(size(sticky), (24.0, 24.0), "a text note keeps its size without the no-zoom flag");
    let corner = mapped(&doc, 0, 500.0, 524.0);
    assert!(close(sticky[0], corner.0) && close(sticky[3], corner.1), "its upper-left corner moves with the page");
    let square = rect_at(&doc, list[4].as_dict().unwrap(), b"Rect");
    assert!(size(square).0 < 24.0, "a square with the same flags shrinks with the page");
}

#[test]
fn measurements_still_read_true_and_geospatial_ones_are_reported() {
    // Points to feet, then feet to inches for distances and square inches for areas.
    let measure = "<< /Type /Measure /Subtype /RL /R (1 in = 1 ft) /X [<< /U (ft) /C 0.1 >>] /D [<< /U (in) /C 12 >>] \
                   /A [<< /U (sq in) /C 144 >>] /O [100 200] >>";
    let annots = format!(
        "/Annots [<< /Subtype /Line /Rect [0 0 100 100] /L [0 0 100 0] /Measure {measure} >>] /VP [<< /BBox [0 0 612 792] /Measure 5 0 R >>]"
    );
    let geo = "<< /Type /Measure /Subtype /GEO >>";
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        &format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R {annots} >>"),
        CONTENT,
        geo,
    ]);
    let report = scale_pages(&mut doc, &[0], request(306.0, 396.0)).unwrap();
    let list = doc.resolve(page(&doc, 0).get(b"Annots").unwrap()).as_array().unwrap().clone();
    let line = list[0].as_dict().unwrap();
    let m = line.get(b"Measure").unwrap().as_dict().unwrap();
    let c = |k: &[u8]| m.get(k).unwrap().as_array().unwrap()[0].as_dict().unwrap().get(b"C").unwrap().as_f64().unwrap();
    let l = line.get(b"L").unwrap().as_array().unwrap().clone();
    let length = l[2].as_f64().unwrap() - l[0].as_f64().unwrap();
    // A reader converts with `/X` first, then with `/D` for a distance and `/A` for an area.
    let side = length * c(b"X");
    assert!(close(side * c(b"D"), 100.0 * 0.1 * 12.0), "the line measures as before");
    assert!(close(side * side * c(b"A"), (100.0 * 0.1) * (100.0 * 0.1) * 144.0), "a square on it too");
    let origin: Vec<f64> = m.get(b"O").unwrap().as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    let at = mapped(&doc, 0, 100.0, 200.0);
    assert!(close(origin[0], at.0) && close(origin[1], at.1), "the origin moves with the content");
    assert_eq!(report.geo_measures_unchanged, 1, "a geospatial measure is not rescaled, and said");
}

#[test]
fn destinations_to_a_scaled_page_move_wherever_they_are() {
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R /OpenAction [3 0 R /XYZ 8 0 R 700 0] /Dests << /a [3 0 R /FitH 500] /b [4 0 R /FitH 500] >> \
         /Names << /Dests 9 0 R >> >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R /PieceInfo << /App << /Private [3 0 R /XYZ 100 200 0] >> >> >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R /Annots [<< /Subtype /Link /Rect [0 0 9 9] /A << /S /GoTo /D [3 0 R /FitR 10 20 30 40] >> >>] >>",
        CONTENT,
        "<< /Type /Outlines /First 7 0 R /Last 7 0 R /Count 1 >>",
        "<< /Title (One) /Parent 6 0 R /Dest [3 0 R /XYZ null 600 null] >>",
        "100",
        "<< /Kids [10 0 R] >>",
        "<< /Names [(c) 11 0 R] /Limits [(c) (c)] >>",
        "[3 0 R /FitH 500]",
    ]);
    let report = scale_pages(&mut doc, &[0], request(306.0, 396.0)).unwrap();
    assert_eq!(report.destinations, 5);
    assert_eq!(doc.get(ObjRef::new(8, 0)).as_f64(), Some(100.0), "a referenced coordinate is replaced, not changed");
    let named = doc.get(ObjRef::new(11, 0)).as_array().unwrap().clone();
    assert!(close(named[2].as_f64().unwrap(), mapped(&doc, 0, 0.0, 500.0).1), "a named destination in the name tree moves");
    let private = page(&doc, 0).get(b"PieceInfo").unwrap().as_dict().unwrap().get(b"App").unwrap().as_dict().unwrap().get(b"Private").cloned();
    assert_eq!(private.unwrap().as_array().unwrap()[2], Object::Int(100), "an application's private array is left alone");
    let catalog = doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();
    let open = catalog.get(b"OpenAction").unwrap().as_array().unwrap().clone();
    let at = mapped(&doc, 0, 100.0, 700.0);
    assert!(close(open[2].as_f64().unwrap(), at.0) && close(open[3].as_f64().unwrap(), at.1));
    assert_eq!(open[4], Object::Int(0), "the zoom is kept");
    let item = doc.get(ObjRef::new(7, 0)).as_dict().unwrap().get(b"Dest").unwrap().as_array().unwrap().clone();
    assert_eq!(item[2], Object::Null, "a null coordinate stays unchanged");
    assert!(close(item[3].as_f64().unwrap(), mapped(&doc, 0, 0.0, 600.0).1));
    let dests = catalog.get(b"Dests").unwrap().as_dict().unwrap();
    assert_eq!(dests.get(b"b").unwrap().as_array().unwrap()[2], Object::Int(500), "a destination on another page is left alone");
}

#[test]
fn a_comment_shared_by_two_pages_moves_once() {
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R /Annots [6 0 R] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R /Annots [6 0 R] >>",
        CONTENT,
        "<< /Subtype /Square /Rect [100 100 200 200] >>",
    ]);
    scale_pages(&mut doc, &[0, 1], request(306.0, 396.0)).unwrap();
    let r = rect_at(&doc, doc.get(ObjRef::new(6, 0)).as_dict().unwrap(), b"Rect");
    let at = mapped(&doc, 0, 100.0, 100.0);
    assert!(close(r[0], at.0) && close(r[1], at.1), "moved by one page's placement, not twice");
}

#[test]
fn shared_viewports_and_measures_read_true_on_every_page() {
    let page_with = |size: u32| format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {size} {}] /Contents 5 0 R /VP 6 0 R >>", size * 2);
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R 9 0 R] /Count 3 >>",
        &page_with(300),
        &page_with(600),
        CONTENT,
        "[7 0 R << /BBox [0 0 100 100] /Measure 8 0 R >>]",
        "<< /BBox [200 100 100 200] /Measure 8 0 R >>",
        "<< /Type /Measure /Subtype /RL /X [<< /U (ft) /C 1 >>] >>",
        &page_with(300),
    ]);
    // Halving the first page and quartering the second.
    scale_pages(&mut doc, &[0, 1], request(150.0, 300.0)).unwrap();
    let viewports = |i: usize| {
        let vp = doc.resolve(page(&doc, i).get(b"VP").unwrap()).as_array().unwrap().clone();
        vp.iter().map(|v| doc.resolve(v).as_dict().cloned().unwrap()).collect::<Vec<_>>()
    };
    let factor = |v: &Dict| {
        let m = doc.resolve(v.get(b"Measure").unwrap()).as_dict().cloned().unwrap();
        let x = doc.resolve(m.get(b"X").unwrap()).as_array().unwrap()[0].clone();
        x.as_dict().unwrap().get(b"C").unwrap().as_f64().unwrap()
    };
    let bbox = |v: &Dict| v.get(b"BBox").unwrap().as_array().unwrap().iter().map(|n| n.as_f64().unwrap()).collect::<Vec<_>>();
    for i in [0, 1] {
        let (scale, _, _) = placement(&doc, i);
        for v in viewports(i) {
            assert!(close(factor(&v) * scale, 1.0), "page {i}: a length reads as before");
        }
        let turned = bbox(&viewports(i)[0]);
        assert!(turned[0] > turned[2] && turned[1] < turned[3], "page {i}: the corner order, so the axes, stay: {turned:?}");
    }
    // The page left alone still has the viewports and the measure as they were.
    for v in viewports(2) {
        assert_eq!(factor(&v), 1.0);
    }
    assert_eq!(bbox(&viewports(2)[0]), vec![200.0, 100.0, 100.0, 200.0]);
}

#[test]
fn a_tiny_scale_is_written_in_full() {
    let mut doc = letter("/UserUnit 1000");
    scale_pages(&mut doc, &[0], ScaleRequest { orientation: ScaleOrientation::Exact, ..request(600.0, 600.0) }).unwrap();
    let (w, h) = size(rect_at(&doc, &page(&doc, 0), b"MediaBox"));
    let (x0, y0) = mapped(&doc, 0, 0.0, 0.0);
    let (x1, y1) = mapped(&doc, 0, 612.0, 792.0);
    let fits = |lo: f64, hi: f64, room: f64| lo >= -1e-9 && hi <= room + 1e-9;
    assert!(fits(x0, x1, w) && fits(y0, y1, h), "the content stays inside the {w}×{h} page: {x0} {y0} {x1} {y1}");
    assert!((y1 - y0 - h).abs() < 1e-9, "and touches its top and bottom");
}

#[test]
fn bad_requests_change_nothing() {
    let mut doc = letter("");
    let before = page(&doc, 0);
    for (w, h) in [(0.0, 100.0), (f64::NAN, 100.0), (100.0, f64::INFINITY), (0.5, 100.0)] {
        assert!(matches!(scale_pages(&mut doc, &[0], request(w, h)), Err(OrganizeError::InvalidBox(_))), "{w}×{h}");
    }
    assert_eq!(scale_pages(&mut doc, &[0, 3], request(100.0, 100.0)), Err(OrganizeError::NoSuchPage(3)));
    assert_eq!(page(&doc, 0), before);
}

#[test]
fn patterns_move_with_the_content_and_a_page_sharing_them_keeps_its_own() {
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 /Resources << /Pattern << /Shade 6 0 R /Tile 7 0 R /Bad 8 0 R >> >> >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>",
        CONTENT,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>",
        "<< /PatternType 2 /Shading << /ShadingType 2 /ColorSpace /DeviceGray /Coords [0 0 1 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> >> /Matrix [2 0 0 2 10 20] >>",
        "<< /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 5 5] /XStep 5 /YStep 5 /Resources << >> /Length 8 >>\nstream\n0 0 m S\nendstream",
        "<< /PatternType 2 /Shading << /ShadingType 2 /ColorSpace /DeviceGray /Coords [0 0 1 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> >> /Matrix [1 0 0] >>",
    ]);
    let report = scale_pages(&mut doc, &[0], request(A4.0, A4.1)).unwrap();
    // A pattern whose matrix can't be read is left as it was, and said.
    assert_eq!(report.patterns_unplaced, 1);
    let pattern = |i: usize, name: &[u8]| {
        // The page's own resources, else those it inherits from the page tree.
        let tree = doc.get(ObjRef::new(2, 0)).as_dict().cloned().unwrap();
        let own = page(&doc, i);
        let res = doc.resolve(own.get(b"Resources").or(tree.get(b"Resources")).unwrap()).as_dict().cloned().unwrap();
        let pats = doc.resolve(res.get(b"Pattern").unwrap()).as_dict().cloned().unwrap();
        let m = match &*doc.resolve(pats.get(name).unwrap()) {
            Object::Stream(s) => s.dict.get(b"Matrix").cloned(),
            Object::Dict(d) => d.get(b"Matrix").cloned(),
            _ => None,
        };
        m.map(|m| doc.resolve(&m).as_array().unwrap().iter().map(|v| doc.resolve(v).as_f64().unwrap()).collect::<Vec<_>>())
    };
    let apply = |m: &[f64], x: f64, y: f64| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
    // A pattern point lands where the page content it was under went.
    for (name, old) in [(&b"Shade"[..], vec![2.0, 0.0, 0.0, 2.0, 10.0, 20.0]), (b"Tile", vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0])] {
        let new = pattern(0, name).unwrap();
        let (ox, oy) = apply(&old, 3.0, 4.0);
        let (nx, ny) = apply(&new, 3.0, 4.0);
        let (ex, ey) = mapped(&doc, 0, ox, oy);
        assert!(close(nx, ex) && close(ny, ey), "{}", String::from_utf8_lossy(name));
    }
    // The unscaled page still sees the patterns as they were.
    assert_eq!(pattern(1, b"Shade").unwrap(), vec![2.0, 0.0, 0.0, 2.0, 10.0, 20.0]);
    assert_eq!(pattern(1, b"Tile"), None);
}
