//! Finding the vector paths a page paints, on synthetic pages.

use std::sync::Arc;

use super::*;

/// A page with these dictionary entries whose content is these streams, each `(extra
/// dictionary entries, data)`.
fn page_dict(entries: &str, streams: &[(&str, &str)]) -> Document {
    let refs: Vec<String> = (0..streams.len()).map(|i| format!("{} 0 R", i + 4)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R {entries} /Contents [{}] >>", refs.join(" ")),
    ];
    objs.extend(streams.iter().map(|(dict, data)| format!("<< {dict} /Length {} >>\nstream\n{data}\nendstream", data.len())));
    document(&objs)
}

/// A document of these objects, numbered from 1; the first is the catalog.
pub(crate) fn document(objs: &[String]) -> Document {
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

/// A 600 × 800 page whose content is these streams.
pub(crate) fn page_with(streams: &[(&str, &str)]) -> Document {
    page_dict("/MediaBox [0 0 600 800]", streams)
}

pub(crate) fn page(content: &str) -> Document {
    page_with(&[("", content)])
}

fn paths(content: &str) -> Vec<PagePath> {
    page_paths(&page(content), 0).unwrap()
}

pub(crate) fn close(a: [f64; 4], b: [f64; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
}

#[test]
fn a_filled_rectangle_is_one_path() {
    let found = paths("1 0 0 rg 10 20 100 50 re f");
    assert_eq!(found.len(), 1);
    let path = &found[0];
    assert!(close(path.rect, [10.0, 20.0, 110.0, 70.0]), "{:?}", path.rect);
    assert_eq!(path.paint, Paint { fill: Some(FillRule::NonZero), stroke: false });
    assert_eq!((path.fill_rgb, path.stroke_rgb), (Some([1.0, 0.0, 0.0]), None));
    assert_eq!(path.ops, 1..=2);
    assert!(path.contains([50.0, 40.0], 0.0));
    assert!(!path.contains([5.0, 40.0], 0.0));
    assert!(path.contains([5.0, 40.0], 10.0), "within the tolerance of its edge");
}

#[test]
fn a_rule_gets_an_area_and_a_stroke_its_width() {
    let stroked = &paths("4 w 0 100 m 200 100 l S")[0];
    assert!(close(stroked.rect, [0.0, 98.0, 202.0, 102.0]), "{:?}", stroked.rect);
    assert_eq!(stroked.paint, Paint { fill: None, stroke: true });
    assert_eq!((stroked.fill_rgb, stroked.stroke_rgb, stroked.line_width_user), (None, Some([0.0; 3]), 4.0));
    assert!(stroked.contains([50.0, 101.5], 0.0));
    assert!(!stroked.contains([50.0, 102.5], 0.0));
    assert!(stroked.contains([50.0, 102.5], 1.0));
    assert!(!stroked.contains([50.0, 105.0], 1.0));
    // A hairline counts as a point wide; a filled rectangle with no height still gets one.
    let hairline = &paths("0 w 50 100 m 200 100 l S")[0];
    assert_eq!(hairline.line_width_user, 0.0);
    assert!(close(hairline.rect, [49.5, 99.5, 200.5, 100.5]), "{:?}", hairline.rect);
    let flat = &paths("50 100 150 0 re f")[0];
    assert!(close(flat.rect, [50.0, 99.5, 200.0, 100.5]), "{:?}", flat.rect);
}

#[test]
fn curves_are_bounded_by_their_extrema_and_fills_close_open_subpaths() {
    let curve = &paths("100 100 m 100 110 110 110 110 100 c f")[0];
    // The control points reach 110; the curve itself only 107.5.
    assert!(close(curve.rect, [100.0, 100.0, 110.0, 107.5]), "{:?}", curve.rect);
    assert!(curve.contains([105.0, 103.0], 0.0));
    assert!(!curve.contains([105.0, 109.0], 0.0));
    // Filled, the open triangle is closed back to its start.
    let triangle = &paths("100 100 m 200 100 l 200 200 l f")[0];
    assert!(triangle.contains([190.0, 110.0], 0.0));
    assert!(!triangle.contains([110.0, 190.0], 0.0));
}

#[test]
fn transforms_and_saved_states_apply() {
    let found = paths("q 2 0 0 2 10 10 cm 1 w 0 0 5 5 re S Q 30 30 10 10 re f q 6 w Q 50 300 m 150 300 l S");
    assert_eq!(found.len(), 3);
    assert_eq!(found[0].line_width_user, 2.0);
    assert!(close(found[0].rect, [9.0, 9.0, 21.0, 21.0]), "{:?}", found[0].rect);
    assert!(close(found[1].rect, [30.0, 30.0, 40.0, 40.0]), "{:?}", found[1].rect);
    assert_eq!(found[2].line_width_user, 1.0);
    assert!(close(found[2].rect, [49.5, 299.5, 150.5, 300.5]), "{:?}", found[2].rect);
}

#[test]
fn colours_follow_the_graphics_state() {
    let found = paths(concat!(
        "0.5 g 0 0 1 RG q 1 0 0 0 k 0 0 0 1 K 10 10 10 10 re B Q 30 30 10 10 re B ",
        "/DeviceRGB cs 0 1 0 sc /DeviceCMYK CS 0 0 0 0 SCN 50 50 10 10 re b ",
        "/Pattern cs /P1 scn /CS0 CS 1 SC 70 70 10 10 re B* ",
        "0.25 G 90 90 10 10 re S /DeviceGray cs 0.2 0.4 rg 110 110 10 10 re f ",
        "/DeviceGray cs 130 130 10 10 re f",
    ));
    let colours: Vec<_> = found.iter().map(|p| (p.fill_rgb, p.stroke_rgb)).collect();
    assert_eq!(
        colours,
        [
            (Some([0.0, 1.0, 1.0]), Some([0.0, 0.0, 0.0])),
            (Some([0.5, 0.5, 0.5]), Some([0.0, 0.0, 1.0])),
            (Some([0.0, 1.0, 0.0]), Some([1.0, 1.0, 1.0])),
            (None, None),
            (None, Some([0.25, 0.25, 0.25])),
            (None, None),
            (Some([0.0, 0.0, 0.0]), None),
        ]
    );
    assert_eq!(found[2].paint, Paint { fill: Some(FillRule::NonZero), stroke: true });
    assert_eq!(found[3].paint, Paint { fill: Some(FillRule::EvenOdd), stroke: true });
    // Only a pattern the path paints with counts: the stroked square after it keeps the fill.
    assert_eq!(found.iter().map(|p| p.pattern).collect::<Vec<_>>(), [false, false, false, true, false, false, false]);
}

#[test]
fn a_pattern_colour_space_resource_marks_its_paths() {
    let resources = "/MediaBox [0 0 600 800] /Resources << /ColorSpace << /CS0 [/Pattern /DeviceRGB] /CS1 /Pattern /CS2 [/CalGray << /WhitePoint [1 1 1] >>] >> >>";
    let content = "/CS0 CS /P0 SCN 0 100 m 100 100 l S /CS1 cs /P1 scn 10 10 10 10 re f /CS2 cs 0.5 sc 30 30 10 10 re f /Missing cs 50 50 10 10 re f q 1 0 0 rg 70 70 10 10 re B Q";
    let found = page_paths(&page_dict(resources, &[("", content)]), 0).unwrap();
    assert_eq!(found.iter().map(|p| p.pattern).collect::<Vec<_>>(), [true, true, false, false, true]);
}

#[test]
fn clipping_and_unpainted_paths_are_not_listed() {
    let found = paths("q 0 0 100 100 re W n Q q 200 200 50 50 re W* f Q 300 300 10 10 re f 10 10 5 5 re n");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].ops, 10..=11);
    assert!(close(found[0].rect, [300.0, 300.0, 310.0, 310.0]));
}

#[test]
fn a_path_takes_the_clip_it_is_painted_in() {
    let found = paths("q 100 100 m 200 100 l 150 200 l h W n 0 0 300 300 re f Q");
    assert_eq!(found.len(), 1);
    let clipped = &found[0];
    assert!(close(clipped.rect, [100.0, 100.0, 200.0, 200.0]), "{:?}", clipped.rect);
    assert!(clipped.contains([150.0, 120.0], 0.0));
    // Inside the fill and the clip's box, but outside the triangle.
    assert!(!clipped.contains([105.0, 190.0], 0.0));
    // A path painted entirely outside its clip shows nothing.
    let found = paths("q 0 0 50 50 re W n 100 100 10 10 re f Q 20 20 10 10 re f");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].ops, 7..=8);
}

#[test]
fn an_open_clip_is_closed_like_a_fill() {
    let found = paths("q 100 100 m 200 100 l 150 200 l W n 0 0 300 300 re f Q");
    assert_eq!(found.len(), 1);
    assert!(close(found[0].rect, [100.0, 100.0, 200.0, 200.0]), "{:?}", found[0].rect);
    assert!(found[0].contains([150.0, 120.0], 0.0));
    assert!(!found[0].contains([105.0, 190.0], 0.0));
}

#[test]
fn paths_under_a_text_clip_are_not_listed() {
    let found = paths("q BT 7 Tr 100 400 Td (A) Tj ET 0 0 600 800 re f Q 10 10 10 10 re f q 7 Tr BT ET 20 20 10 10 re f Q");
    let rects: Vec<_> = found.iter().map(|p| p.rect).collect();
    assert_eq!(rects.len(), 2, "{rects:?}");
    assert!(close(rects[0], [10.0, 10.0, 20.0, 20.0]) && close(rects[1], [20.0, 20.0, 30.0, 30.0]), "{rects:?}");
}

#[test]
fn an_ext_gstate_sets_the_line_width() {
    let resources = "/MediaBox [0 0 600 800] /Resources << /ExtGState << /GS0 << /LW 12 >> >> >>";
    let content = "/GS0 gs 0 100 m 200 100 l S q 2 w /Missing gs 0 200 m 200 200 l S Q 0 300 m 200 300 l S";
    let found = page_paths(&page_dict(resources, &[("", content)]), 0).unwrap();
    assert_eq!(found.iter().map(|p| p.line_width_user).collect::<Vec<_>>(), [12.0, 2.0, 12.0]);
    assert!(close(found[0].rect, [0.0, 94.0, 206.0, 106.0]), "{:?}", found[0].rect);
    assert!(found[0].contains([50.0, 105.0], 0.0));
}

#[test]
fn paths_beyond_the_editing_coordinate_limit_are_left_out() {
    let doc = page_dict("/MediaBox [0 0 5000000000 5000000000]", &[("", "0 0 4000000000 4000000000 re f 10 10 10 10 re f")]);
    let found = page_paths(&doc, 0).unwrap();
    assert_eq!(found.len(), 1);
    assert!(close(found[0].rect, [10.0, 10.0, 20.0, 20.0]), "{:?}", found[0].rect);
}

#[test]
fn paths_in_text_objects_or_interrupted_are_not_listed() {
    let found =
        paths(concat!("BT 0 0 10 10 re f ET 30 30 m BT ET 40 40 l S ", "50 50 m BI /W 1 /H 1 /CS /G /BPC 8 ID x EI 60 60 l S 20 20 10 10 re f"));
    assert_eq!(found.len(), 1);
    assert!(close(found[0].rect, [20.0, 20.0, 30.0, 30.0]));
}

#[test]
fn inline_images_shadings_and_forms_leave_paths_alone() {
    let found = paths("10 10 10 10 re f BI /W 1 /H 1 /CS /G /BPC 8 ID x EI /Sh0 sh /Fm0 Do 30 30 10 10 re S");
    assert_eq!(found.len(), 2);
    assert_eq!(found[1].paint, Paint { fill: None, stroke: true });
}

#[test]
fn a_path_split_across_streams_has_joined_operator_indexes() {
    let doc = page_with(&[("", "1 0 0 rg 10 10 m 100"), ("", "10 l 100 100 l"), ("", "f 200 200 10 10 re S")]);
    let found = page_paths(&doc, 0).unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!((found[0].ops.clone(), found[1].ops.clone()), (1..=4, 5..=6));
    assert!(close(found[0].rect, [10.0, 10.0, 100.0, 100.0]), "{:?}", found[0].rect);
    assert_eq!(found[0].fill_rgb, Some([1.0, 0.0, 0.0]));
    assert!(close(found[1].rect, [199.5, 199.5, 210.5, 210.5]), "{:?}", found[1].rect);
}

#[test]
fn added_content_is_not_listed_again() {
    let added = "/PCMark /Added /PCAdded << /Rect [10 20 110 40] /Kind /Text /Text (x) /Size 12 >>";
    let doc = page_with(&[("", "10 10 10 10 re f"), (added, "q 1 0 0 1 0 0 cm 50 50 10 10 re f Q")]);
    assert_eq!(crate::list_added(&doc).len(), 1);
    let found = page_paths(&doc, 0).unwrap();
    assert_eq!(found.len(), 1);
    assert!(close(found[0].rect, [10.0, 10.0, 20.0, 20.0]));
}

#[test]
fn contains_follows_the_fill_rule() {
    let shapes = "100 100 200 200 re 150 150 100 100 re";
    let even_odd = &paths(&format!("{shapes} f*"))[0];
    let non_zero = &paths(&format!("{shapes} f"))[0];
    assert!(!even_odd.contains([200.0, 200.0], 0.0), "the hole");
    assert!(non_zero.contains([200.0, 200.0], 0.0));
    assert!(even_odd.contains([120.0, 120.0], 0.0) && non_zero.contains([120.0, 120.0], 0.0));
    assert!(!non_zero.contains([90.0, 90.0], 0.0));
}

#[test]
fn too_many_paths_are_refused() {
    let at_limit = page(&"10 10 1 1 re f\n".repeat(MAX_PAGE_PATHS));
    assert_eq!(page_paths(&at_limit, 0).unwrap().len(), MAX_PAGE_PATHS);
    let over = page(&"10 10 1 1 re f\n".repeat(MAX_PAGE_PATHS + 1));
    assert!(matches!(page_paths(&over, 0), Err(EditError::Invalid(_))));
}

/// Every path under a clip walks it again when hit testing sweeps the page.
#[test]
fn too_much_clipping_is_refused() {
    // One clip of 16 000 elements.
    let clip = format!("{}W n ", "0 0 600 800 re ".repeat(3_200));
    let under = page(&format!("{clip}{}", "10 10 10 10 re f ".repeat(200)));
    assert_eq!(page_paths(&under, 0).unwrap().len(), 200);
    let over = page(&format!("{clip}{}", "10 10 10 10 re f ".repeat(300)));
    assert!(matches!(page_paths(&over, 0), Err(EditError::Invalid(_))));
}

/// Paths with no current point used to panic in kurbo (in images too, through the shared state).
#[test]
fn hostile_content_never_panics() {
    let doc = page("h f 10 10 l S 20 20 30 30 40 40 c 5 5 6 6 y h S 10 10 10 10 re f h W n");
    assert_eq!(page_paths(&doc, 0).unwrap().len(), 1);
    assert!(crate::images::page_images(&doc, 0).unwrap().is_empty());
    // Segments before any `m` draw nothing, as in the renderer (#3581): no phantom path to select.
    assert!(paths("10 10 l 50 50 l S").is_empty());
    assert!(page_paths(&page("10000000000000 0 m 0 0 l S"), 0).is_err());
    assert_eq!(paths("Q Q Q 10 10 10 10 re f").len(), 1);
    assert!(page_paths(&page(&format!("{}10 10 10 10 re f", "q ".repeat(2000))), 0).is_err());
    // The graphics state's own budgets: path elements, then active clips.
    assert!(page_paths(&page(&format!("0 0 m {}S", "1 1 l ".repeat(16_400))), 0).is_err());
    assert!(page_paths(&page(&format!("{}10 10 10 10 re f", "0 0 600 800 re W n ".repeat(300))), 0).is_err());
    let runaway = format!("{}1 1 m 2 2 l S 0 0 m 0 0 l S 3 3 4 4 re f", "1000000000000 0 0 1000000000000 0 0 cm ".repeat(30));
    assert!(paths(&runaway).is_empty());
    assert_eq!(page_paths(&page(""), 3), Err(EditError::NoSuchPage(3)));
    let path = &paths("10 10 10 10 re f")[0];
    assert!(!path.contains([f64::NAN, 15.0], 0.0));
    assert!(!path.contains([f64::INFINITY, 15.0], f64::INFINITY));
    assert!(path.contains([15.0, 15.0], f64::NAN));
    assert!(path.contains([15.0, 15.0], -5.0));
}
