//! Moving, resizing, recolouring and deleting vector paths, on synthetic pages.

use pdfcraft_cos::{SaveOptions, write_full};

use super::*;
use crate::paths::tests::{close, document, page, page_with};

fn rects(doc: &Document) -> Vec<[f64; 4]> {
    page_paths(doc, 0).unwrap().iter().map(|p| p.rect).collect()
}

/// The page's decoded content streams.
fn contents(doc: &Document, page: usize) -> Vec<String> {
    checked_streams(doc, page).unwrap().into_iter().map(|(_, data)| String::from_utf8(data).unwrap()).collect()
}

/// The page's content stream objects.
fn refs(doc: &Document, page: usize) -> Vec<Object> {
    checked_streams(doc, page).unwrap().into_iter().map(|(o, _)| o).collect()
}

fn saved(doc: &Document) -> Vec<u8> {
    write_full(doc, &SaveOptions::default()).unwrap()
}

fn shift(x: f64, y: f64) -> PathChange {
    PathChange::Transform([1.0, 0.0, 0.0, 1.0, x, y])
}

#[test]
fn moving_a_path_wraps_only_its_operators() {
    let mut doc = page("1 0 0 rg 10 20 100 50 re f 30 30 10 10 re S");
    let before = rects(&doc);
    change_path(&mut doc, 0, 0, &shift(5.0, -3.0)).unwrap();
    assert_eq!(contents(&doc, 0), ["1 0 0 rg q\n1 0 0 1 5 -3 cm\n10 20 100 50 re f\nQ\n 30 30 10 10 re S"]);
    let after = rects(&doc);
    assert!(close(after[0], [15.0, 17.0, 115.0, 67.0]), "{after:?}");
    assert_eq!(after[1], before[1]);
}

#[test]
fn resizing_maps_the_path_onto_the_new_box() {
    let mut doc = page("10 20 100 50 re f 0 0 1 RG 300 300 m 400 300 l S");
    let before = page_paths(&doc, 0).unwrap();
    let t = crate::rect_to_rect(before[0].rect, [20.0, 40.0, 220.0, 140.0]);
    assert_eq!(t, [2.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
    change_path(&mut doc, 0, 0, &PathChange::Transform(t)).unwrap();
    let after = page_paths(&doc, 0).unwrap();
    assert!(close(after[0].rect, [20.0, 40.0, 220.0, 140.0]), "{:?}", after[0].rect);
    assert_eq!((after[1].rect, after[1].stroke_rgb), (before[1].rect, Some([0.0, 0.0, 1.0])));
}

#[test]
fn a_private_clip_moves_with_its_path() {
    let mut doc = page("q 100 100 50 50 re W n 0 0 300 300 re f Q 400 400 10 10 re f");
    let before = page_paths(&doc, 0).unwrap();
    assert!(close(before[0].rect, [100.0, 100.0, 150.0, 150.0]), "{:?}", before[0].rect);
    assert!(before[0].contains([105.0, 105.0], 0.0) && !before[0].contains([155.0, 155.0], 0.0));
    change_path(&mut doc, 0, 0, &shift(10.0, 10.0)).unwrap();
    assert_eq!(contents(&doc, 0), ["q\n1 0 0 1 10 10 cm\nq 100 100 50 50 re W n 0 0 300 300 re f Q\nQ\n 400 400 10 10 re f"]);
    let after = page_paths(&doc, 0).unwrap();
    assert!(close(after[0].rect, [110.0, 110.0, 160.0, 160.0]), "{:?}", after[0].rect);
    assert!(after[0].contains([155.0, 155.0], 0.0) && !after[0].contains([105.0, 105.0], 0.0));
    assert_eq!(after[1].rect, before[1].rect);
}

/// The clip is shared with another path, so it stays and the moved path shows more of itself.
#[test]
fn a_shared_clip_stays_where_it_is() {
    let mut doc = page("q 100 100 50 50 re W n 90 90 30 30 re f 130 130 10 10 re f Q");
    let before = page_paths(&doc, 0).unwrap();
    assert!(close(before[0].rect, [100.0, 100.0, 120.0, 120.0]), "{:?}", before[0].rect);
    change_path(&mut doc, 0, 0, &shift(10.0, 10.0)).unwrap();
    assert_eq!(contents(&doc, 0), ["q 100 100 50 50 re W n q\n1 0 0 1 10 10 cm\n90 90 30 30 re f\nQ\n 130 130 10 10 re f Q"]);
    let after = page_paths(&doc, 0).unwrap();
    assert!(close(after[0].rect, [100.0, 100.0, 130.0, 130.0]), "{:?}", after[0].rect);
    assert!(after[0].contains([102.0, 102.0], 0.0));
    assert_eq!(after[1].rect, before[1].rect);
}

/// A clip set before the path's `q … Q` (the page's, or a group's) comes before the inserted
/// `cm`, so it stays; every clip in the outermost `q … Q` painting only the path moves with it.
#[test]
fn private_clips_move_with_their_path_under_an_outer_clip() {
    let mut doc = page("0 0 600 800 re W n q 100 100 50 50 re W n 0 0 300 300 re f Q");
    change_path(&mut doc, 0, 0, &shift(200.0, 0.0)).unwrap();
    assert_eq!(contents(&doc, 0), ["0 0 600 800 re W n q\n1 0 0 1 200 0 cm\nq 100 100 50 50 re W n 0 0 300 300 re f Q\nQ\n"]);
    let after = page_paths(&doc, 0).unwrap();
    assert!(close(after[0].rect, [300.0, 100.0, 350.0, 150.0]), "{:?}", after[0].rect);
    assert!(after[0].contains([320.0, 120.0], 0.0) && !after[0].contains([120.0, 120.0], 0.0));

    // Both nested clips are the path's alone.
    let mut doc = page("q 0 0 120 120 re W n q 100 100 50 50 re W n 0 0 300 300 re f Q Q 400 400 10 10 re f");
    change_path(&mut doc, 0, 0, &shift(200.0, 0.0)).unwrap();
    assert_eq!(contents(&doc, 0), ["q\n1 0 0 1 200 0 cm\nq 0 0 120 120 re W n q 100 100 50 50 re W n 0 0 300 300 re f Q Q\nQ\n 400 400 10 10 re f"]);
    let after = rects(&doc);
    assert!(close(after[0], [300.0, 100.0, 320.0, 120.0]) && close(after[1], [400.0, 400.0, 410.0, 410.0]), "{after:?}");
}

/// A path moved wholly out of its clip or off the page would leave the list, and every later
/// index would then name another path.
#[test]
fn a_move_that_would_hide_the_path_is_refused() {
    let mut doc = page("q 100 100 50 50 re W n 90 90 30 30 re f 130 130 10 10 re f Q 10 10 10 10 re f");
    let original = saved(&doc);
    let refused = translate_paths(&mut doc, 0, &[(0, [100.0, 100.0])]).unwrap_err().to_string();
    assert!(refused.contains("out of its clip"), "{refused}");
    assert!(change_path(&mut doc, 0, 2, &shift(700.0, 0.0)).is_err());
    assert_eq!(saved(&doc), original);
}

/// Pattern space is fixed to the page (ISO 32000-2 §8.7.3.1): a `cm` would move the outline
/// and leave its pattern behind.
#[test]
fn a_path_painted_with_a_pattern_is_not_moved_but_recoloured_or_deleted() {
    let mut doc = page("/Pattern cs /P0 scn 10 10 100 100 re f 200 200 m 300 200 l S");
    assert_eq!(page_paths(&doc, 0).unwrap().iter().map(|p| p.pattern).collect::<Vec<_>>(), [true, false]);
    let original = saved(&doc);
    let refused = change_path(&mut doc, 0, 0, &shift(5.0, 5.0)).unwrap_err().to_string();
    assert!(refused.contains("pattern"), "{refused}");
    assert!(change_path(&mut doc, 0, 0, &PathChange::Transform([2.0, 0.0, 0.0, 2.0, 0.0, 0.0])).is_err());
    assert!(translate_paths(&mut doc, 0, &[(1, [5.0, 5.0]), (0, [5.0, 5.0])]).is_err());
    assert_eq!(saved(&doc), original);
    // Left where it is, it doesn't hold back a group move.
    translate_paths(&mut doc, 0, &[(1, [5.0, 5.0]), (0, [0.0, 0.0])]).unwrap();
    assert!(close(rects(&doc)[1], [204.5, 204.5, 305.5, 205.5]), "{:?}", rects(&doc));
    let mut deleted = doc.clone();
    delete_paths(&mut deleted, 0, &[0]).unwrap();
    assert_eq!(page_paths(&deleted, 0).unwrap().len(), 1);
    recolour_paths(&mut doc, 0, &[0], Some([1.0, 0.0, 0.0]), None).unwrap();
    let recoloured = &page_paths(&doc, 0).unwrap()[0];
    assert_eq!((recoloured.pattern, recoloured.fill_rgb), (false, Some([1.0, 0.0, 0.0])));
}

#[test]
fn moves_are_in_user_space_under_any_transform() {
    let mut doc = page("q 2 0 0 2 10 10 cm 1 w 0 0 5 5 re S Q q 0 1 -1 0 300 100 cm 0 0 50 20 re f Q");
    let before = rects(&doc);
    assert!(close(before[0], [9.0, 9.0, 21.0, 21.0]) && close(before[1], [280.0, 100.0, 300.0, 150.0]), "{before:?}");
    translate_paths(&mut doc, 0, &[(0, [4.0, 6.0]), (1, [10.0, -20.0])]).unwrap();
    // Twice the size inside the `cm`, so half the move.
    assert!(contents(&doc, 0)[0].starts_with("q 2 0 0 2 10 10 cm 1 w q\n1 0 0 1 2 3 cm\n0 0 5 5 re S\nQ\n Q"), "{:?}", contents(&doc, 0));
    let after = rects(&doc);
    assert!(close(after[0], [13.0, 15.0, 25.0, 27.0]) && close(after[1], [290.0, 80.0, 310.0, 130.0]), "{after:?}");
}

#[test]
fn recolouring_paints_only_what_the_path_paints() {
    let mut doc = page("0 0 1 rg 1 0 0 RG 10 10 50 50 re B 100 100 50 50 re f 0 300 m 200 300 l S");
    let colours = |doc: &Document| page_paths(doc, 0).unwrap().iter().map(|p| (p.fill_rgb, p.stroke_rgb)).collect::<Vec<_>>();
    recolour_paths(&mut doc, 0, &[0], Some([0.0, 0.5, 0.0]), None).unwrap();
    assert_eq!(contents(&doc, 0), ["0 0 1 rg 1 0 0 RG q\n0 0.5 0 rg\n10 10 50 50 re B\nQ\n 100 100 50 50 re f 0 300 m 200 300 l S"]);
    assert_eq!(colours(&doc), [(Some([0.0, 0.5, 0.0]), Some([1.0, 0.0, 0.0])), (Some([0.0, 0.0, 1.0]), None), (None, Some([1.0, 0.0, 0.0]))]);
    // A stroked line takes only the stroke colour; a filled shape only the fill.
    change_path(&mut doc, 0, 2, &PathChange::Recolour { fill: Some([1.0; 3]), stroke: Some([0.0, 1.0, 1.0]) }).unwrap();
    recolour_paths(&mut doc, 0, &[0, 1], Some([1.0, 1.0, 0.0]), Some([0.25; 3])).unwrap();
    assert_eq!(colours(&doc), [(Some([1.0, 1.0, 0.0]), Some([0.25; 3])), (Some([1.0, 1.0, 0.0]), None), (None, Some([0.0, 1.0, 1.0]))]);
    recolour_paths(&mut doc, 0, &[1], Some([2.0, -1.0, 0.5]), None).unwrap();
    assert_eq!(colours(&doc)[1], (Some([1.0, 0.0, 0.5]), None));
    let original = saved(&doc);
    assert!(recolour_paths(&mut doc, 0, &[2], Some([1.0, 0.0, 0.0]), None).unwrap_err().to_string().contains("no fill"));
    assert!(recolour_paths(&mut doc, 0, &[1], None, Some([1.0, 0.0, 0.0])).unwrap_err().to_string().contains("no stroke"));
    assert!(recolour_paths(&mut doc, 0, &[0, 2], Some([1.0, 0.0, 0.0]), None).is_err());
    assert_eq!(saved(&doc), original);
}

#[test]
fn deleting_a_path_keeps_the_state_and_marked_content_around_it() {
    let mut doc = page("1 0 0 rg 10 10 10 10 re f q 0 1 0 rg 30 30 10 10 re f Q /Span <</MCID 0>> BDC 50 50 10 10 re f EMC");
    delete_paths(&mut doc, 0, &[1]).unwrap();
    assert_eq!(contents(&doc, 0), ["1 0 0 rg 10 10 10 10 re f q 0 1 0 rg   Q /Span <</MCID 0>> BDC 50 50 10 10 re f EMC"]);
    let after = rects(&doc);
    assert!(after.len() == 2 && close(after[0], [10.0, 10.0, 20.0, 20.0]) && close(after[1], [50.0, 50.0, 60.0, 60.0]), "{after:?}");
    delete_paths(&mut doc, 0, &[1]).unwrap();
    assert_eq!(contents(&doc, 0), ["1 0 0 rg 10 10 10 10 re f q 0 1 0 rg   Q /Span <</MCID 0>> BDC   EMC"]);
}

#[test]
fn a_path_split_across_streams_moves_and_deletes_whole() {
    let split = [("", "1 0 0 rg 10 10 m 100"), ("", "10 l 100 100 l"), ("", "f 200 200 10 10 re S")];
    let mut doc = page_with(&split);
    let before = refs(&doc, 0);
    change_path(&mut doc, 0, 0, &shift(5.0, 5.0)).unwrap();
    assert_eq!(contents(&doc, 0), ["1 0 0 rg q\n1 0 0 1 5 5 cm\n10 10 m 100", "10 l 100 100 l", "f\nQ\n 200 200 10 10 re S"]);
    let after = refs(&doc, 0);
    assert!(after[0] != before[0] && after[1] == before[1] && after[2] != before[2], "only the changed streams are new");
    let found = page_paths(&doc, 0).unwrap();
    assert!(close(found[0].rect, [15.0, 15.0, 105.0, 105.0]), "{:?}", found[0].rect);
    assert_eq!(found[0].fill_rgb, Some([1.0, 0.0, 0.0]));

    let mut doc = page_with(&split);
    delete_paths(&mut doc, 0, &[0]).unwrap();
    assert_eq!(contents(&doc, 0), ["1 0 0 rg  ", " ", " 200 200 10 10 re S"]);
    let found = page_paths(&doc, 0).unwrap();
    assert_eq!((found.len(), found[0].ops.clone()), (1, 1..=2));
    assert!(close(found[0].rect, [199.5, 199.5, 210.5, 210.5]), "{:?}", found[0].rect);
}

#[test]
fn several_paths_change_in_one_pass() {
    let mut doc = page("10 10 10 10 re f 100 100 10 10 re f 200 200 10 10 re f 300 300 10 10 re f");
    // A path that doesn't move is left alone.
    translate_paths(&mut doc, 0, &[(2, [0.0, -5.0]), (0, [5.0, 0.0]), (1, [0.0, 0.0])]).unwrap();
    assert_eq!(
        contents(&doc, 0),
        ["q\n1 0 0 1 5 0 cm\n10 10 10 10 re f\nQ\n 100 100 10 10 re f q\n1 0 0 1 0 -5 cm\n200 200 10 10 re f\nQ\n 300 300 10 10 re f"]
    );
    let r = rects(&doc);
    assert!(
        close(r[0], [15.0, 10.0, 25.0, 20.0])
            && close(r[1], [100.0, 100.0, 110.0, 110.0])
            && close(r[2], [200.0, 195.0, 210.0, 205.0])
            && close(r[3], [300.0, 300.0, 310.0, 310.0]),
        "{r:?}"
    );
    delete_paths(&mut doc, 0, &[3, 1]).unwrap();
    let r = rects(&doc);
    assert!(r.len() == 2 && close(r[0], [15.0, 10.0, 25.0, 20.0]) && close(r[1], [200.0, 195.0, 210.0, 205.0]), "{r:?}");
    recolour_paths(&mut doc, 0, &[1, 0], Some([1.0, 0.0, 0.0]), None).unwrap();
    assert!(page_paths(&doc, 0).unwrap().iter().all(|p| p.fill_rgb == Some([1.0, 0.0, 0.0])));
}

#[test]
fn refusals_leave_the_document_untouched() {
    // The second path is drawn flat: its CTM has no inverse.
    let mut doc = page("10 10 10 10 re f q 1 0 0 0 0 0 cm 20 20 10 10 re f Q 0 0 m 100 0 l S");
    assert_eq!(page_paths(&doc, 0).unwrap().len(), 3);
    let original = saved(&doc);
    let red = Some([1.0, 0.0, 0.0]);
    let results = [
        change_path(&mut doc, 0, 3, &shift(5.0, 5.0)),
        change_path(&mut doc, 1, 0, &shift(5.0, 5.0)),
        change_path(&mut doc, 0, 1, &shift(5.0, 5.0)),
        change_path(&mut doc, 0, 0, &PathChange::Transform([f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0])),
        change_path(&mut doc, 0, 0, &shift(1e300, 0.0)),
        change_path(&mut doc, 0, 0, &shift(2e9, 0.0)),
        change_path(&mut doc, 0, 0, &PathChange::Transform([0.0; 6])),
        change_path(&mut doc, 0, 0, &PathChange::Recolour { fill: None, stroke: None }),
        change_path(&mut doc, 0, 0, &PathChange::Recolour { fill: Some([f64::NAN, 0.0, 0.0]), stroke: None }),
        change_path(&mut doc, 0, 2, &PathChange::Recolour { fill: red, stroke: None }),
        recolour_paths(&mut doc, 0, &[], red, None),
        delete_paths(&mut doc, 0, &[]),
        delete_paths(&mut doc, 0, &[0, 0]),
        delete_paths(&mut doc, 0, &[0, 3]),
        delete_paths(&mut doc, 0, &vec![0; MAX_MOVE_OBJECTS + 1]),
        translate_paths(&mut doc, 0, &[(0, [f64::INFINITY, 0.0])]),
        translate_paths(&mut doc, 0, &[(0, [1.0, 1.0]), (1, [1.0, 1.0])]),
        translate_paths(&mut doc, 0, &[(9, [1.0, 1.0])]),
    ];
    for (i, result) in results.iter().enumerate() {
        assert!(result.is_err(), "case {i}: {result:?}");
    }
    assert_eq!(saved(&doc), original);
}

#[test]
fn a_stream_shared_with_another_page_is_copied_not_changed() {
    let data = "10 10 10 10 re f 30 30 10 10 re f";
    let mut doc = document(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 5 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 5 0 R >>".into(),
        format!("<< /Length {} >>\nstream\n{data}\nendstream", data.len()),
    ]);
    let shared = refs(&doc, 1);
    let other = page_paths(&doc, 1).unwrap();
    change_path(&mut doc, 0, 1, &shift(0.0, 50.0)).unwrap();
    assert_eq!((refs(&doc, 1), contents(&doc, 1)), (shared.clone(), vec![data.to_string()]));
    assert_eq!(page_paths(&doc, 1).unwrap(), other);
    assert_ne!(refs(&doc, 0), shared);
    assert!(close(page_paths(&doc, 0).unwrap()[1].rect, [30.0, 80.0, 40.0, 90.0]));
}

/// Backgrounds, watermarks, headers and added content are edited with their own tools, and the
/// q/Q wrapper streams PdfCraft puts around the page's content when it draws on top stay as they
/// are (removing the marks removes them).
#[test]
fn pdfcraft_marks_are_not_listed_and_their_wrapper_stays() {
    let mut doc = page_with(&[
        ("/PCMark /Background", "q 0 0 600 800 re f Q"),
        ("/PCMark /Wrap", "q"),
        ("", "100 100 50 50 re W n 0 0 300 300 re f"),
        ("/PCMark /Wrap", "Q"),
        ("/PCMark /Watermark", "q 0.5 g 500 500 10 10 re f Q"),
        ("/PCMark /HeaderFooter", "0 g 400 400 10 10 re f"),
    ]);
    let found = page_paths(&doc, 0).unwrap();
    assert_eq!(found.len(), 1);
    assert!(close(found[0].rect, [100.0, 100.0, 150.0, 150.0]), "{:?}", found[0].rect);
    let before = refs(&doc, 0);
    change_path(&mut doc, 0, 0, &shift(10.0, 10.0)).unwrap();
    assert_eq!(contents(&doc, 0)[2], "100 100 50 50 re W n q\n1 0 0 1 10 10 cm\n0 0 300 300 re f\nQ\n");
    let after = refs(&doc, 0);
    for i in [0, 1, 3, 4, 5] {
        assert_eq!(after[i], before[i], "stream {i}");
    }
}
