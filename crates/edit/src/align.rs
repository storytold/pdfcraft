//! Align and distribute page objects. Edges and centres are those of the page as displayed
//! (after `/Rotate`); each object then moves by its own offset in one atomic group move.

use std::collections::{BTreeMap, BTreeSet};

use pdfcraft_content::Matrix;
use pdfcraft_cos::Document;

use crate::{EditError, MAX_MOVE_OBJECTS, ObjectKind, ObjectTarget, editable_objects, move_objects_each};

/// How [`align_objects`] lines objects up: to an edge or the centre of the selection's union,
/// or with even gaps between neighbours (the outermost two stay where they are).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ObjectAlign {
    Left,
    CentreHorizontal,
    Right,
    Top,
    CentreVertical,
    Bottom,
    DistributeHorizontal,
    DistributeVertical,
}

impl ObjectAlign {
    pub fn is_distribute(self) -> bool {
        matches!(self, Self::DistributeHorizontal | Self::DistributeVertical)
    }
}

/// Align or distribute `targets` (references into [`editable_objects`]) as one atomic edit.
/// A refusal leaves the document untouched.
pub fn align_objects(doc: &mut Document, page: usize, targets: &[ObjectTarget], how: ObjectAlign) -> Result<(), EditError> {
    if targets.len() > MAX_MOVE_OBJECTS {
        return Err(EditError::Invalid("select at most 1000 objects to align".into()));
    }
    // Before the offsets: a repeat adds nothing to the union and would read as "already aligned".
    if targets.iter().collect::<BTreeSet<_>>().len() != targets.len() {
        return Err(EditError::Invalid("an object was selected more than once".into()));
    }
    let p = pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
    // Display space (y up) to user space. A quarter turn only swaps axes, so boxes stay exact.
    let view = Matrix(pdfcraft_model::view_matrix_for(p.rotation(doc), p.crop(doc)));
    let to_view = view.invert().ok_or_else(|| EditError::Invalid("the page has no area".into()))?;
    let inventory: BTreeMap<_, _> = editable_objects(doc, page)?.into_iter().map(|o| (o.target, o.rect)).collect();
    let rects = targets
        .iter()
        .map(|t| inventory.get(t).map(|r| to_view.bbox(*r)))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| EditError::Invalid("a selected object no longer exists; select the objects again".into()))?;
    let offsets = align_offsets(&rects, how)?;
    if offsets.iter().all(|o| *o == [0.0, 0.0]) {
        let done = if how.is_distribute() { "the objects are already evenly spaced" } else { "the objects are already aligned" };
        return Err(EditError::Invalid(done.into()));
    }
    let [a, b, c, d, _, _] = view.0;
    let moves: Vec<_> = targets.iter().zip(offsets).map(|(t, [x, y])| (*t, [a * x + c * y, b * x + d * y])).collect();
    let mut work = doc.clone();
    move_objects_each(&mut work, page, &moves)?;
    // A clip shared with other content (or the page edge) stays behind, so an image or path it
    // partly hides shows a different part and misses the line. Paragraphs may renumber, so not them.
    let moved: BTreeMap<_, _> = editable_objects(&work, page)?.into_iter().map(|o| (o.target, o.rect)).collect();
    for (t, [x, y]) in moves.iter().filter(|(t, _)| matches!(t.kind, ObjectKind::Image | ObjectKind::Path)) {
        let expected = inventory.get(t).map(|r| [r[0] + x, r[1] + y, r[2] + x, r[3] + y]);
        if !expected.zip(moved.get(t)).is_some_and(|(e, r)| e.iter().zip(r).all(|(e, r)| (e - r).abs() < 1e-3)) {
            let what = if t.kind == ObjectKind::Path { "a vector path" } else { "an image" };
            return Err(EditError::Invalid(format!(
                "{what} partly hidden by a shared clipping path or the page edge cannot be aligned; move it instead"
            )));
        }
    }
    *doc = work;
    Ok(())
}

/// Each rectangle's offset (display space, y up) for `how`, in the order given.
fn align_offsets(rects: &[[f64; 4]], how: ObjectAlign) -> Result<Vec<[f64; 2]>, EditError> {
    if how.is_distribute() && rects.len() < 3 {
        return Err(EditError::Invalid("select at least three objects to distribute".into()));
    }
    if rects.len() < 2 {
        return Err(EditError::Invalid("select at least two objects to align".into()));
    }
    let inf = f64::INFINITY;
    let u = rects.iter().fold([inf, inf, -inf, -inf], |u, r| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]);
    let mid = |lo: f64, hi: f64| (lo + hi) / 2.0;
    let mut offsets: Vec<[f64; 2]> = rects
        .iter()
        .map(|r| match how {
            ObjectAlign::Left => [u[0] - r[0], 0.0],
            ObjectAlign::CentreHorizontal => [mid(u[0], u[2]) - mid(r[0], r[2]), 0.0],
            ObjectAlign::Right => [u[2] - r[2], 0.0],
            ObjectAlign::Top => [0.0, u[3] - r[3]],
            ObjectAlign::CentreVertical => [0.0, mid(u[1], u[3]) - mid(r[1], r[3])],
            ObjectAlign::Bottom => [0.0, u[1] - r[1]],
            ObjectAlign::DistributeHorizontal | ObjectAlign::DistributeVertical => [0.0, 0.0],
        })
        .collect();
    if how.is_distribute() {
        let axis = usize::from(how == ObjectAlign::DistributeVertical);
        let lo = |i: usize| rects.get(i).and_then(|r| r.get(axis)).copied().unwrap_or(0.0);
        let hi = |i: usize| rects.get(i).and_then(|r| r.get(axis + 2)).copied().unwrap_or(0.0);
        let mut order: Vec<usize> = (0..rects.len()).collect();
        order.sort_by(|&i, &j| lo(i).total_cmp(&lo(j)));
        let (Some(&first), Some(&last)) = (order.first(), order.last()) else { return Ok(offsets) };
        let extents: f64 = order.iter().map(|&i| hi(i) - lo(i)).sum();
        // Overlapping objects make the gap negative; they then overlap evenly instead.
        let gap = (hi(last) - lo(first) - extents) / order.len().saturating_sub(1) as f64;
        let mut cursor = hi(first) + gap;
        for &i in order.iter().skip(1).take(order.len().saturating_sub(2)) {
            if let Some(o) = offsets.get_mut(i).and_then(|o| o.get_mut(axis)) {
                *o = cursor - lo(i);
            }
            cursor += hi(i) - lo(i) + gap;
        }
    }
    // Earlier moves leave rounding residues far below anything visible; don't rewrite for them.
    for v in offsets.iter_mut().flatten() {
        if v.abs() < 1e-6 {
            *v = 0.0;
        }
    }
    Ok(offsets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{image_page_with, reopen};
    use pdfcraft_cos::{Object, SaveOptions};

    const ALIGNS: [ObjectAlign; 6] =
        [ObjectAlign::Left, ObjectAlign::CentreHorizontal, ObjectAlign::Right, ObjectAlign::Top, ObjectAlign::CentreVertical, ObjectAlign::Bottom];

    /// Three images, user space [100 100 150 130], [300 200 380 240] and [200 400 260 420].
    const IMAGES: &str = "q 50 0 0 30 100 100 cm /Im0 Do Q q 80 0 0 40 300 200 cm /Im0 Do Q q 60 0 0 20 200 400 cm /Im0 Do Q";

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn rects(doc: &Document) -> Vec<[f64; 4]> {
        editable_objects(doc, 0).unwrap().iter().map(|o| o.rect).collect()
    }

    fn rotated(content: &str, rotation: i64) -> Document {
        let mut doc = image_page_with(content);
        let page = pdfcraft_model::pages(&doc)[0].obj;
        doc.update_dict(page, |d| d.set(b"Rotate".to_vec(), Object::Int(rotation))).unwrap();
        doc
    }

    fn align_all(doc: &mut Document, how: ObjectAlign) -> Result<(), EditError> {
        let targets: Vec<_> = editable_objects(doc, 0).unwrap().iter().map(|o| o.target).collect();
        align_objects(doc, 0, &targets, how)
    }

    #[test]
    fn offsets_line_up_edges_and_centres_of_the_union() {
        let rects = [[0.0, 0.0, 10.0, 10.0], [20.0, 5.0, 40.0, 25.0]];
        let cases = [
            (ObjectAlign::Left, [[0.0, 0.0], [-20.0, 0.0]]),
            (ObjectAlign::CentreHorizontal, [[15.0, 0.0], [-10.0, 0.0]]),
            (ObjectAlign::Right, [[30.0, 0.0], [0.0, 0.0]]),
            (ObjectAlign::Top, [[0.0, 15.0], [0.0, 0.0]]),
            (ObjectAlign::CentreVertical, [[0.0, 7.5], [0.0, -2.5]]),
            (ObjectAlign::Bottom, [[0.0, 0.0], [0.0, -5.0]]),
        ];
        for (how, expected) in cases {
            assert_eq!(align_offsets(&rects, how).unwrap(), expected, "{how:?}");
        }
        assert_eq!(align_offsets(&[[0.0, 0.0, 1.0, 1.0], [1e-9, 0.0, 1.0, 1.0]], ObjectAlign::Left).unwrap(), [[0.0, 0.0]; 2], "residue");
    }

    #[test]
    fn distributing_evens_the_gaps_and_keeps_the_outermost_objects() {
        // Given out of order: sorted by left edge they are 0, 2, 1.
        let rects = [[0.0, 0.0, 10.0, 10.0], [90.0, 50.0, 100.0, 60.0], [20.0, 20.0, 50.0, 30.0]];
        let offsets = align_offsets(&rects, ObjectAlign::DistributeHorizontal).unwrap();
        // Gap (100 - 0 - 50) / 2 = 25: the middle one starts at 35.
        assert_eq!(offsets, [[0.0, 0.0], [0.0, 0.0], [15.0, 0.0]]);
        let offsets = align_offsets(&rects, ObjectAlign::DistributeVertical).unwrap();
        // Gap (60 - 0 - 30) / 2 = 15: the middle one starts at 25.
        assert_eq!(offsets, [[0.0, 0.0], [0.0, 0.0], [0.0, 5.0]]);
        // Overlapping objects overlap evenly: gap (60 - 0 - 210) / 2 = -75.
        let overlapping = [[0.0, 0.0, 100.0, 10.0], [10.0, 0.0, 110.0, 10.0], [50.0, 0.0, 60.0, 10.0]];
        assert_eq!(align_offsets(&overlapping, ObjectAlign::DistributeHorizontal).unwrap(), [[0.0, 0.0], [15.0, 0.0], [0.0, 0.0]]);
    }

    #[test]
    fn too_few_objects_are_refused() {
        let one = [[0.0, 0.0, 1.0, 1.0]];
        let two = [[0.0, 0.0, 1.0, 1.0], [5.0, 5.0, 6.0, 6.0]];
        for how in ALIGNS {
            assert!(align_offsets(&[], how).unwrap_err().to_string().contains("two"));
            assert!(align_offsets(&one, how).unwrap_err().to_string().contains("two"));
            assert!(align_offsets(&two, how).is_ok());
        }
        for how in [ObjectAlign::DistributeHorizontal, ObjectAlign::DistributeVertical] {
            assert!(align_offsets(&two, how).unwrap_err().to_string().contains("three"));
        }
    }

    /// Mixed text and images on an upright page: the text is leftmost, so Align left leaves it
    /// where it is (a zero offset) and moves only the images.
    #[test]
    fn mixed_objects_align_to_the_selection_and_keep_their_size() {
        for how in ALIGNS {
            let mut doc = image_page_with(&format!("{IMAGES} BT /F1 12 Tf 72 700 Td (Caption) Tj ET"));
            let before = rects(&doc);
            assert_eq!(before.len(), 4);
            align_all(&mut doc, how).unwrap();
            let after = rects(&reopen(&doc));
            let union = |i: usize, max: bool| {
                before.iter().map(|r| r[i]).fold(if max { f64::MIN } else { f64::MAX }, |a, b| if max { a.max(b) } else { a.min(b) })
            };
            for (old, new) in before.iter().zip(&after) {
                assert!(near(new[2] - new[0], old[2] - old[0]) && near(new[3] - new[1], old[3] - old[1]), "{how:?}: size kept");
                let (moved, still) = match how {
                    ObjectAlign::Left => (near(new[0], union(0, false)), near(new[1], old[1])),
                    ObjectAlign::Right => (near(new[2], union(2, true)), near(new[1], old[1])),
                    ObjectAlign::CentreHorizontal => (near(new[0] + new[2], union(0, false) + union(2, true)), near(new[1], old[1])),
                    ObjectAlign::Top => (near(new[3], union(3, true)), near(new[0], old[0])),
                    ObjectAlign::Bottom => (near(new[1], union(1, false)), near(new[0], old[0])),
                    _ => (near(new[1] + new[3], union(1, false) + union(3, true)), near(new[0], old[0])),
                };
                assert!(moved && still, "{how:?}: {old:?} -> {new:?}");
            }
        }
    }

    /// On a page turned a quarter clockwise, the displayed left edge is the lowest user y and
    /// the displayed top is the lowest user x.
    #[test]
    fn alignment_follows_the_displayed_orientation() {
        for (rotation, how, expected) in [
            (0, ObjectAlign::Left, [[100.0, 100.0], [100.0, 200.0], [100.0, 400.0]]),
            (0, ObjectAlign::Top, [[100.0, 390.0], [300.0, 380.0], [200.0, 400.0]]),
            (90, ObjectAlign::Left, [[100.0, 100.0], [300.0, 100.0], [200.0, 100.0]]),
            (90, ObjectAlign::Top, [[100.0, 100.0], [100.0, 200.0], [100.0, 400.0]]),
            (90, ObjectAlign::Right, [[100.0, 390.0], [300.0, 380.0], [200.0, 400.0]]),
            (0, ObjectAlign::DistributeVertical, [[100.0, 100.0], [300.0, 245.0], [200.0, 400.0]]),
            (90, ObjectAlign::DistributeHorizontal, [[100.0, 100.0], [300.0, 245.0], [200.0, 400.0]]),
            (0, ObjectAlign::DistributeHorizontal, [[100.0, 100.0], [300.0, 200.0], [195.0, 400.0]]),
        ] {
            let mut doc = rotated(IMAGES, rotation);
            align_all(&mut doc, how).unwrap();
            let after = rects(&reopen(&doc));
            assert_eq!(after.len(), 3);
            for (r, [x, y]) in after.iter().zip(expected) {
                assert!(near(r[0], x) && near(r[1], y), "rotation {rotation} {how:?}: {after:?}");
            }
        }
    }

    /// Each paragraph moves by its own offset: Left and Right move two of the three by
    /// different amounts, centring moves all three.
    #[test]
    fn paragraphs_move_by_their_own_offsets() {
        let content = "BT /F1 12 Tf 50 700 Td (One) Tj ET BT /F1 12 Tf 150 500 Td (Two) Tj ET BT /F1 12 Tf 300 300 Td (Three) Tj ET";
        let by_text = |doc: &Document| editable_objects(doc, 0).unwrap().into_iter().map(|o| (o.text.unwrap(), o.rect)).collect::<BTreeMap<_, _>>();
        for how in [ObjectAlign::Left, ObjectAlign::CentreHorizontal, ObjectAlign::Right] {
            let mut doc = image_page_with(content);
            let before = by_text(&doc);
            assert_eq!(before.len(), 3);
            align_all(&mut doc, how).unwrap();
            let after = by_text(&reopen(&doc));
            let (lo, hi) = before.values().fold((f64::MAX, f64::MIN), |(lo, hi), r| (lo.min(r[0]), hi.max(r[2])));
            for (text, old) in &before {
                let new = after[text];
                let (edge, line) = match how {
                    ObjectAlign::Left => (new[0], lo),
                    ObjectAlign::Right => (new[2], hi),
                    _ => (new[0] + new[2], lo + hi),
                };
                assert!(near(edge, line) && near(new[1], old[1]) && near(new[2] - new[0], old[2] - old[0]), "{how:?} {text}: {old:?} -> {new:?}");
            }
        }
    }

    /// A clip shared with other content stays put, so an image it partly hides would show more
    /// of itself after the move and miss the line. Moving along the clip edge, or with a clip of
    /// its own (which moves with it), is fine.
    #[test]
    fn images_partly_hidden_by_a_shared_clip_are_refused() {
        // Image 0 is drawn at x 250..350 but shows only 250..300.
        let mut doc = image_page_with("q 0 0 300 800 re W n q 100 0 0 50 250 100 cm /Im0 Do Q q 50 0 0 50 100 300 cm /Im0 Do Q Q");
        let original = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
        let error = align_all(&mut doc, ObjectAlign::CentreHorizontal).unwrap_err().to_string();
        assert!(error.contains("clipping path"), "{error}");
        assert_eq!(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap(), original);
        align_all(&mut doc, ObjectAlign::Top).unwrap();
        assert!(near(rects(&reopen(&doc))[0][3], 350.0));
        let mut doc = image_page_with("q 0 0 300 800 re W n 100 0 0 50 250 100 cm /Im0 Do Q q 50 0 0 50 100 300 cm /Im0 Do Q");
        align_all(&mut doc, ObjectAlign::CentreHorizontal).unwrap();
        let after = rects(&reopen(&doc));
        assert!(near(after[0][0], 175.0) && near(after[0][2], 225.0) && near(after[1][0], 175.0), "{after:?}");
    }

    fn all_near(after: &[[f64; 4]], expected: &[[f64; 4]]) -> bool {
        after.len() == expected.len() && after.iter().zip(expected).all(|(r, e)| r.iter().zip(e).all(|(a, b)| near(*a, *b)))
    }

    /// A group move takes a selected path along with text and an image; the other path stays.
    #[test]
    fn a_group_move_takes_vector_paths_with_the_other_kinds() {
        let mut doc = image_page_with(concat!(
            "1 0 0 rg 10 20 100 50 re f q 200 0 0 100 100 100 cm /Im0 Do Q BT /F1 12 Tf 72 700 Td (Caption) Tj ET ",
            "4 w 50 300 m 250 300 l S",
        ));
        let before = editable_objects(&doc, 0).unwrap();
        let t = |kind, index| ObjectTarget { kind, index };
        let path = |doc: &Document| crate::page_paths(doc, 0).unwrap().iter().map(|p| p.rect).collect::<Vec<_>>();
        assert!(all_near(&path(&doc), &[[10.0, 20.0, 110.0, 70.0], [48.0, 298.0, 252.0, 302.0]]), "{:?}", path(&doc));
        let targets = [t(ObjectKind::Text, 0), t(ObjectKind::Image, 0), t(ObjectKind::Path, 0)];
        crate::move_objects(&mut doc, 0, &targets, [5.0, -10.0]).unwrap();
        let doc = reopen(&doc);
        assert!(all_near(&path(&doc), &[[15.0, 10.0, 115.0, 60.0], [48.0, 298.0, 252.0, 302.0]]), "{:?}", path(&doc));
        let after = editable_objects(&doc, 0).unwrap();
        assert_eq!(after.iter().map(|o| o.target).collect::<Vec<_>>(), before.iter().map(|o| o.target).collect::<Vec<_>>());
        for (old, new) in before.iter().zip(&after) {
            let [x, y] = if targets.contains(&old.target) { [5.0, -10.0] } else { [0.0, 0.0] };
            let r = old.rect;
            assert!(all_near(&[new.rect], &[[r[0] + x, r[1] + y, r[2] + x, r[3] + y]]), "{old:?} -> {new:?}");
        }
    }

    /// Paths line up with images. A shared clip partly hiding a path stays behind as it does for
    /// an image, so the path would show more of itself and miss the line: refused.
    #[test]
    fn vector_paths_align_with_images() {
        let mut doc = image_page_with("q 50 0 0 30 100 100 cm /Im0 Do Q 1 0 0 rg 300 200 80 40 re f 0 0 1 RG 2 w 400 500 m 450 500 l S");
        let targets: Vec<_> = editable_objects(&doc, 0).unwrap().iter().map(|o| o.target).collect();
        let listed: Vec<_> = targets.iter().map(|t| (t.kind, t.index)).collect();
        assert_eq!(listed, [(ObjectKind::Image, 0), (ObjectKind::Path, 0), (ObjectKind::Path, 1)]);
        // The image and the filled path; the line isn't selected.
        align_objects(&mut doc, 0, &targets[..2], ObjectAlign::Left).unwrap();
        let after = rects(&reopen(&doc));
        assert!(all_near(&after, &[[100.0, 100.0, 150.0, 130.0], [100.0, 200.0, 180.0, 240.0], [399.0, 499.0, 451.0, 501.0]]), "{after:?}");

        // Shown 300..330 of 300..380: moved 200 left it would show 100..180.
        let mut doc = image_page_with("q 50 0 0 30 100 100 cm /Im0 Do Q q 0 0 330 800 re W n 300 200 80 40 re f 10 10 5 5 re f Q");
        assert!(all_near(&rects(&doc)[1..], &[[300.0, 200.0, 330.0, 240.0], [10.0, 10.0, 15.0, 15.0]]), "{:?}", rects(&doc));
        let original = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
        let targets: Vec<_> = editable_objects(&doc, 0).unwrap().iter().map(|o| o.target).take(2).collect();
        let error = align_objects(&mut doc, 0, &targets, ObjectAlign::Left).unwrap_err().to_string();
        assert!(error.contains("vector path") && error.contains("clipping path"), "{error}");
        assert_eq!(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap(), original);
    }

    #[test]
    fn per_object_moves_leave_zero_offsets_in_place_but_need_one_move() {
        let mut doc = image_page_with(IMAGES);
        let original = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
        let t = |index| ObjectTarget { kind: ObjectKind::Image, index };
        for moves in [vec![(t(0), [0.0, 0.0]), (t(1), [-0.0, 0.0])], vec![(t(0), [5.0, 0.0]), (t(1), [f64::NAN, 0.0])]] {
            assert!(move_objects_each(&mut doc, 0, &moves).unwrap_err().to_string().contains("non-zero"));
            assert_eq!(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap(), original);
        }
        let before = rects(&doc);
        move_objects_each(&mut doc, 0, &[(t(0), [0.0, 0.0]), (t(1), [5.0, -5.0])]).unwrap();
        let after = rects(&reopen(&doc));
        assert_eq!(after[0], before[0]);
        assert!(after[1].iter().zip(before[1]).enumerate().all(|(i, (a, b))| near(*a, b + if i % 2 == 0 { 5.0 } else { -5.0 })));
        assert_eq!(after[2], before[2]);
    }

    #[test]
    fn refusals_leave_the_document_untouched() {
        let mut doc = image_page_with(IMAGES);
        let original = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
        let t = |index| ObjectTarget { kind: ObjectKind::Image, index };
        for (targets, how, why) in [
            (vec![], ObjectAlign::Left, "two"),
            (vec![t(0)], ObjectAlign::Left, "two"),
            (vec![t(0), t(1)], ObjectAlign::DistributeHorizontal, "three"),
            (vec![t(0), t(99)], ObjectAlign::Left, "no longer exists"),
            (vec![t(0), t(1), t(1)], ObjectAlign::Left, "more than once"),
            (vec![t(0), t(0)], ObjectAlign::Left, "more than once"),
            (vec![t(0); MAX_MOVE_OBJECTS + 1], ObjectAlign::Left, "1000"),
        ] {
            let error = align_objects(&mut doc, 0, &targets, how).unwrap_err().to_string();
            assert!(error.contains(why), "{error}");
            assert_eq!(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap(), original);
        }
        assert!(matches!(align_objects(&mut doc, 7, &[t(0), t(1)], ObjectAlign::Left), Err(EditError::NoSuchPage(7))));
        align_objects(&mut doc, 0, &[t(0), t(1)], ObjectAlign::Bottom).unwrap();
        let aligned = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
        let error = align_objects(&mut doc, 0, &[t(0), t(1)], ObjectAlign::Bottom).unwrap_err().to_string();
        assert!(error.contains("already aligned"), "{error}");
        assert_eq!(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap(), aligned);
    }
}
