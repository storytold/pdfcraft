//! Bringing page objects forward and sending them backward, on synthetic pages. R is a red
//! square, B a blue one over it, Y a yellow one over both and G a green one away from them all.

use pdfcraft_cos::{SaveOptions, write_full};

use super::*;
use crate::paths::tests::{close, document};

/// A 600 × 800 page whose content is these streams, with a font (`/F1`), an image (`/Im0`), an
/// ExtGState (`/GA`) and an optional content group (`/OC1`) to use.
fn arrange_page(streams: &[&str]) -> Document {
    let refs: Vec<String> = (0..streams.len()).map(|i| format!("{} 0 R", i + 6)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents [{}] /Resources << /Font << /F1 4 0 R >> /XObject << /Im0 5 0 R >> /ExtGState << /GA << /ca 0.5 >> >> /Properties << /OC1 << /Type /OCG /Name (L) >> >> >> >>",
            refs.join(" ")
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        "<< /Type /XObject /Subtype /Image /Width 4 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 8 >>\nstream\n\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\nendstream".to_string(),
    ];
    objs.extend(streams.iter().map(|data| format!("<< /Length {} >>\nstream\n{data}\nendstream", data.len())));
    document(&objs)
}

fn path(index: usize) -> ObjectTarget {
    ObjectTarget { kind: ObjectKind::Path, index }
}

fn image(index: usize) -> ObjectTarget {
    ObjectTarget { kind: ObjectKind::Image, index }
}

/// The page's content streams, each with its runs of whitespace made single spaces.
fn contents(doc: &Document) -> Vec<String> {
    checked_streams(doc, 0).unwrap().iter().map(|(_, data)| String::from_utf8_lossy(data).split_whitespace().collect::<Vec<_>>().join(" ")).collect()
}

fn saved(doc: &Document) -> Vec<u8> {
    write_full(doc, &SaveOptions::default()).unwrap()
}

/// Everything the page lists, with the colours each path is painted in, in no particular order.
fn listed(doc: &Document) -> Vec<String> {
    let q = |v: &[f64]| v.iter().map(|x| (x * 1e4).round() as i64).collect::<Vec<_>>();
    let mut out: Vec<String> = page_paths(doc, 0)
        .unwrap()
        .iter()
        .map(|p| format!("path {:?} {:?} {:?}", q(&p.rect), p.fill_rgb.map(|c| q(&c)), p.stroke_rgb.map(|c| q(&c))))
        .collect();
    out.extend(page_images(doc, 0).unwrap().iter().map(|i| format!("image {:?}", q(&i.rect))));
    out.extend(editable_objects(doc, 0).unwrap().iter().map(|o| format!("{} {:?}", o.target.kind.as_str(), q(&o.rect))));
    out.sort();
    out
}

/// Arrange `target`: where it is listed afterwards is what [`arranged_target`] said, the page
/// lists the same objects in the same colours and places, and the document still saves and reads.
fn arrange(doc: &mut Document, target: ObjectTarget, order: ZOrder) -> ObjectTarget {
    let before = listed(doc);
    let predicted = arranged_target(doc, 0, target, order).unwrap();
    let moved = arrange_object(doc, 0, target, order).unwrap();
    assert_eq!(moved, predicted);
    assert_eq!(listed(doc), before);
    crate::tests::reopen(doc);
    moved
}

/// Arrange on a page with this content: the content afterwards, and where the object is listed.
fn restack(content: &str, target: ObjectTarget, order: ZOrder) -> (String, ObjectTarget) {
    let mut doc = arrange_page(&[content]);
    let moved = arrange(&mut doc, target, order);
    (contents(&doc).join(" | "), moved)
}

/// Why arranging on `doc` is refused; `doc` stays as it was.
fn refusal(doc: &Document, target: ObjectTarget, order: ZOrder) -> String {
    let mut work = doc.clone();
    let message = arrange_object(&mut work, 0, target, order).unwrap_err().to_string();
    assert_eq!(saved(&work), saved(doc));
    assert_eq!(arranged_target(doc, 0, target, order).unwrap_err().to_string(), message);
    message
}

fn refused(content: &str, target: ObjectTarget, order: ZOrder) -> String {
    refusal(&arrange_page(&[content]), target, order)
}

fn added_text_over_it(content: &str) -> Document {
    let mut doc = arrange_page(&[content]);
    let text = crate::AddedText { text: "Over".into(), rect: [20.0, 20.0, 120.0, 60.0], ..Default::default() };
    crate::add_content(&mut doc, 0, &crate::Content::Text(text)).unwrap();
    doc
}

#[test]
fn forward_goes_just_above_the_next_overlapping_object() {
    assert_eq!(
        restack("1 0 0 rg 10 10 100 100 re f 0 0 1 rg 50 50 100 100 re f", path(0), ZOrder::Forward),
        ("1 0 0 rg 0 0 1 rg 50 50 100 100 re f q 1 0 0 rg 10 10 100 100 re f Q".into(), path(1))
    );
    // G overlaps neither, so R goes past it to B.
    assert_eq!(
        restack("1 0 0 rg 10 10 100 100 re f 0 1 0 rg 400 400 50 50 re f 0 0 1 rg 50 50 100 100 re f", path(0), ZOrder::Forward),
        ("1 0 0 rg 0 1 0 rg 400 400 50 50 re f 0 0 1 rg 50 50 100 100 re f q 1 0 0 rg 10 10 100 100 re f Q".into(), path(2))
    );
}

#[test]
fn backward_goes_just_below_the_previous_overlapping_object() {
    assert_eq!(
        restack("1 0 0 rg 10 10 100 100 re f 0 0 1 rg 50 50 100 100 re f", path(1), ZOrder::Backward),
        ("1 0 0 rg q 0 0 1 rg 50 50 100 100 re f Q 10 10 100 100 re f 0 0 1 rg".into(), path(0))
    );
}

#[test]
fn front_and_back_go_past_every_overlapping_object() {
    let content = "1 0 0 rg 10 10 100 100 re f 0 0 1 rg 50 50 100 100 re f 1 1 0 rg 80 80 100 100 re f";
    assert_eq!(
        restack(content, path(0), ZOrder::Front),
        ("1 0 0 rg 0 0 1 rg 50 50 100 100 re f 1 1 0 rg 80 80 100 100 re f q 1 0 0 rg 10 10 100 100 re f Q".into(), path(2))
    );
    assert_eq!(
        restack(content, path(2), ZOrder::Back),
        ("1 0 0 rg q 1 1 0 rg 80 80 100 100 re f Q 10 10 100 100 re f 0 0 1 rg 50 50 100 100 re f 1 1 0 rg".into(), path(0))
    );
}

/// Moving it would change nothing visible, so the page isn't rewritten.
#[test]
fn an_object_nothing_overlaps_stays_where_it_is() {
    let content = "1 0 0 rg 10 10 100 100 re f 0 1 0 rg 400 400 50 50 re f";
    for order in [ZOrder::Front, ZOrder::Forward] {
        assert_eq!(refused(content, path(0), order), "nothing above this object overlaps it");
    }
    for order in [ZOrder::Backward, ZOrder::Back] {
        assert_eq!(refused(content, path(0), order), "nothing below this object overlaps it");
    }
}

/// The clip in the `q … Q` holding only R moves with it, so R shows the same part.
#[test]
fn a_private_clip_moves_with_its_object() {
    assert_eq!(
        restack("q 0 0 60 60 re W n 1 0 0 rg 10 10 100 100 re f Q 0 0 1 rg 50 50 100 100 re f", path(0), ZOrder::Forward),
        ("0 0 1 rg 50 50 100 100 re f q 0 g q 0 0 60 60 re W n 1 0 0 rg 10 10 100 100 re f Q Q".into(), path(1))
    );
}

#[test]
fn the_object_keeps_its_transform_under_another() {
    let (content, moved) = restack("1 0 0 rg 10 10 100 100 re f 2 0 0 2 0 0 cm 0 0 1 rg 25 25 50 50 re f", path(0), ZOrder::Forward);
    assert_eq!(
        (content.as_str(), moved),
        ("1 0 0 rg 2 0 0 2 0 0 cm 0 0 1 rg 25 25 50 50 re f q 1 0 0 rg 0.5 0 0 0.5 0 0 cm 10 10 100 100 re f Q", path(1))
    );
}

#[test]
fn the_line_style_it_was_drawn_with_is_replayed() {
    assert_eq!(
        restack("0 1 0 RG 4 w 1 J [3 1] 0 d 10 10 m 150 150 l S 1 w 0 J [] 0 d 0 0 1 RG 0 150 m 150 0 l S", path(0), ZOrder::Forward),
        ("0 1 0 RG 4 w 1 J [3 1] 0 d 1 w 0 J [] 0 d 0 0 1 RG 0 150 m 150 0 l S q 0 1 0 RG 4 w 1 J [3 1] 0 d 10 10 m 150 150 l S Q".into(), path(1))
    );
    // R never set a line width: the destination's goes back to the default. Its fill is the same.
    assert_eq!(
        restack("1 0 0 rg 10 10 100 100 re f 5 w 50 50 100 100 re S", path(0), ZOrder::Forward),
        ("1 0 0 rg 5 w 50 50 100 100 re S q 1 w 10 10 100 100 re f Q".into(), path(1))
    );
}

/// `sc` means nothing without the colour space it was set in: `rg` chose it, or nothing did.
#[test]
fn a_colour_keeps_its_colour_space() {
    assert_eq!(
        restack("1 0 0 rg 0 1 0 sc 10 10 100 100 re f 0 g 50 50 100 100 re f", path(0), ZOrder::Forward),
        ("1 0 0 rg 0 1 0 sc 0 g 50 50 100 100 re f q 1 0 0 rg 0 1 0 sc 10 10 100 100 re f Q".into(), path(1))
    );
    assert_eq!(
        restack("0.5 sc 10 10 100 100 re f /DeviceRGB cs 0 0 1 sc 50 50 100 100 re f", path(0), ZOrder::Forward),
        ("0.5 sc /DeviceRGB cs 0 0 1 sc 50 50 100 100 re f q /DeviceGray cs 0.5 sc 10 10 100 100 re f Q".into(), path(1))
    );
}

/// Undoing the destination's 0.21 scale leaves R a rounding error right of C's left edge, which
/// must not pair R with C when the page's objects are compared.
#[test]
fn rounding_in_the_moved_object_does_not_mismatch_its_neighbours() {
    let mut doc = arrange_page(&["1 0 0 rg 10 10 100 100 re f 0 1 0 rg 10 300 50 50 re f 0.21 0 0 0.21 0 0 cm 0 0 1 rg 300 300 400 400 re f"]);
    assert_eq!(arrange(&mut doc, path(0), ZOrder::Forward), path(2));
}

#[test]
fn its_transparency_comes_with_it_but_is_never_added() {
    assert_eq!(
        restack("q /GA gs 1 0 0 rg 10 10 100 100 re f 0 1 0 rg 400 400 50 50 re f Q 0 0 1 rg 50 50 100 100 re f", path(0), ZOrder::Forward),
        ("q /GA gs 1 0 0 rg 0 1 0 rg 400 400 50 50 re f Q 0 0 1 rg 50 50 100 100 re f q 1 0 0 rg /GA gs 10 10 100 100 re f Q".into(), path(2))
    );
    assert_eq!(
        refused("1 0 0 rg 10 10 100 100 re f /GA gs 0 0 1 rg 50 50 100 100 re f", path(0), ZOrder::Forward),
        "this object cannot move into content with different transparency or blending"
    );
}

#[test]
fn a_different_clip_is_left_whole_or_refused() {
    assert_eq!(
        refused("1 0 0 rg 10 10 100 100 re f 0 0 200 200 re W n 0 0 1 rg 50 50 100 100 re f", path(0), ZOrder::Forward),
        "this object cannot move past content that is clipped differently"
    );
    // The clip ends with its group, so R goes above the whole group.
    assert_eq!(
        restack("1 0 0 rg 10 10 100 100 re f q 0 0 200 200 re W n 0 0 1 rg 50 50 100 100 re f 0 1 0 rg 60 60 10 10 re f Q", path(0), ZOrder::Forward),
        ("1 0 0 rg q 0 0 200 200 re W n 0 0 1 rg 50 50 100 100 re f 0 1 0 rg 60 60 10 10 re f Q q 10 10 100 100 re f Q".into(), path(2))
    );
}

#[test]
fn marked_content_without_an_identifier_is_entered_again() {
    assert_eq!(
        restack("/Artifact BMC 1 0 0 rg 10 10 100 100 re f 0 1 0 rg 400 400 50 50 re f EMC 0 0 1 rg 50 50 100 100 re f", path(0), ZOrder::Forward),
        (
            "/Artifact BMC 1 0 0 rg 0 1 0 rg 400 400 50 50 re f EMC 0 0 1 rg 50 50 100 100 re f /Artifact BMC q 1 0 0 rg 10 10 100 100 re f Q EMC"
                .into(),
            path(2)
        )
    );
    // `/OC1` names a property list in the page's resources, which has no `/MCID`.
    assert_eq!(
        restack("/OC /OC1 BDC 1 0 0 rg 10 10 100 100 re f 0 1 0 rg 400 400 50 50 re f EMC 0 0 1 rg 50 50 100 100 re f", path(0), ZOrder::Forward),
        (
            "/OC /OC1 BDC 1 0 0 rg 0 1 0 rg 400 400 50 50 re f EMC 0 0 1 rg 50 50 100 100 re f /OC /OC1 BDC q 1 0 0 rg 10 10 100 100 re f Q EMC"
                .into(),
            path(2)
        )
    );
    // The sequence sets the colour for what follows, so it stays and R leaves it.
    assert_eq!(
        restack("/Artifact BMC 1 0 0 rg 10 10 100 100 re f EMC 0 0 1 rg 50 50 100 100 re f", path(0), ZOrder::Forward),
        ("/Artifact BMC 1 0 0 rg EMC 0 0 1 rg 50 50 100 100 re f /Artifact BMC q 1 0 0 rg 10 10 100 100 re f Q EMC".into(), path(1))
    );
}

#[test]
fn a_tagged_sequence_moves_whole_or_not_at_all() {
    assert_eq!(
        refused(
            "/P <</MCID 0>> BDC 1 0 0 rg 10 10 100 100 re f BT /F1 12 Tf 20 300 Td (Hi) Tj ET EMC 0 0 1 rg 50 50 100 100 re f",
            path(0),
            ZOrder::Forward
        ),
        "this object is tagged together with other content; arranging it would change the document structure"
    );
    let (content, moved) =
        restack("/Figure <</MCID 0>> BDC q 1 0 0 rg 10 10 100 100 re f Q EMC 0 0 1 rg 50 50 100 100 re f", path(0), ZOrder::Forward);
    assert_eq!(
        (content.as_str(), moved),
        ("0 0 1 rg 50 50 100 100 re f q 0 g /Figure <</MCID 0>> BDC q 1 0 0 rg 10 10 100 100 re f Q EMC Q", path(1))
    );
    assert_eq!(content.matches("/MCID 0").count(), 1);
    // A copy of the sequence would repeat its replacement text.
    assert_eq!(
        refused(
            "/Span <</ActualText (AB)>> BDC 1 0 0 rg 10 10 100 100 re f 0 1 0 rg 400 400 50 50 re f EMC 0 0 1 rg 50 50 100 100 re f",
            path(0),
            ZOrder::Forward
        ),
        "this object is tagged together with other content; arranging it would change the document structure"
    );
}

#[test]
fn an_image_takes_its_private_clip_along() {
    let mut doc = arrange_page(&["q 10 10 60 60 re W n 100 0 0 100 10 10 cm /Im0 Do Q 0 0 1 rg 50 50 100 100 re f"]);
    assert_eq!(arrange(&mut doc, image(0), ZOrder::Forward), image(0));
    assert_eq!(contents(&doc), ["0 0 1 rg 50 50 100 100 re f q 0 g q 10 10 60 60 re W n 100 0 0 100 10 10 cm /Im0 Do Q Q"]);
    // The clip is still the image's own, so moving the image moves it too.
    crate::move_objects(&mut doc, 0, &[image(0)], [5.0, 5.0]).unwrap();
    let moved = page_images(&doc, 0).unwrap()[0].visible_rect.unwrap();
    assert!(close(moved, [15.0, 15.0, 75.0, 75.0]), "{moved:?}");
}

#[test]
fn an_image_sharing_its_scope_moves_alone_with_its_transform() {
    assert_eq!(
        restack("q 100 0 0 100 10 10 cm /Im0 Do 0 1 0 rg 1 1 0.1 0.1 re f Q 0 0 1 rg 50 50 100 100 re f", image(0), ZOrder::Forward),
        ("q 100 0 0 100 10 10 cm 0 1 0 rg 1 1 0.1 0.1 re f Q 0 0 1 rg 50 50 100 100 re f q 0 g 100 0 0 100 10 10 cm /Im0 Do Q".into(), image(0))
    );
}

#[test]
fn a_path_split_between_streams_moves_whole_and_only_changed_streams_are_new() {
    let data = ["1 0 0 rg 10 10 100 100", "re f 0 0 1 rg 50 50 100 100 re f"];
    let mut doc = arrange_page(&data);
    let old: Vec<Object> = checked_streams(&doc, 0).unwrap().into_iter().map(|(o, _)| o).collect();
    assert_eq!(arrange(&mut doc, path(0), ZOrder::Forward), path(1));
    assert_eq!(contents(&doc), ["1 0 0 rg", "0 0 1 rg 50 50 100 100 re f q 1 0 0 rg 10 10 100 100 re f Q"]);
    let new: Vec<Object> = checked_streams(&doc, 0).unwrap().into_iter().map(|(o, _)| o).collect();
    assert!(new[0] != old[0] && new[1] != old[1], "{old:?} {new:?}");
    for (o, data) in old.iter().zip(data) {
        let Object::Stream(s) = &*doc.resolve(o) else { panic!("{o:?} is not a stream") };
        assert_eq!(s.decoded().unwrap(), data.as_bytes(), "the old stream is unchanged");
    }
}

#[test]
fn text_is_overlapped_unless_it_is_invisible() {
    let text = "BT /F1 24 Tf 20 40 Td (Hello) Tj ET";
    assert_eq!(
        restack(&format!("1 0 0 rg 10 10 100 100 re f {text}"), path(0), ZOrder::Forward),
        (format!("1 0 0 rg {text} q 10 10 100 100 re f Q"), path(0))
    );
    assert_eq!(
        refused("1 0 0 rg 10 10 100 100 re f BT /F1 24 Tf 3 Tr 20 40 Td (Hello) Tj ET", path(0), ZOrder::Forward),
        "nothing above this object overlaps it"
    );
}

#[test]
fn added_items_and_marks_keep_their_own_layer() {
    // R is the page's only content, so it takes the `q`/`Q` around the page with it.
    let doc = added_text_over_it("1 0 0 rg 10 10 100 100 re f");
    assert_eq!(refusal(&doc, path(0), ZOrder::Front), "items added in PdfCraft and page marks keep their own layer");
    // The only thing above R that overlaps it is the added text.
    let doc = added_text_over_it("1 0 0 rg 10 10 100 100 re f 0 1 0 rg 400 400 50 50 re f");
    assert_eq!(refusal(&doc, path(0), ZOrder::Front), "items added in PdfCraft and page marks keep their own layer");
    let added = ObjectTarget { kind: ObjectKind::Added, index: 0 };
    assert_eq!(refusal(&doc, added, ZOrder::Back), "only vector paths, images and grouped artwork can be arranged");
    // The page's unclosed `q` ends at PdfCraft's `Q`, so R's place would be in PdfCraft's stream.
    let doc = added_text_over_it("1 0 0 rg 10 10 100 100 re f q 0 0 200 200 re W n 0 0 1 rg 50 50 100 100 re f");
    assert_eq!(refusal(&doc, path(0), ZOrder::Forward), "the page content around this object is too irregular to rearrange it");
}

#[test]
fn text_and_objects_that_no_longer_exist_are_refused() {
    let content = "BT /F1 12 Tf 20 300 Td (Hi) Tj ET 1 0 0 rg 10 10 100 100 re f";
    let text = ObjectTarget { kind: ObjectKind::Text, index: 0 };
    assert_eq!(refused(content, text, ZOrder::Front), "only vector paths, images and grouped artwork can be arranged");
    assert_eq!(refused(content, path(1), ZOrder::Front), "the selected object no longer exists; select it again");
    assert_eq!(refused(content, image(0), ZOrder::Front), "the selected object no longer exists; select it again");
}

#[test]
fn an_image_inside_a_text_object_is_refused() {
    assert_eq!(
        refused("BT q 100 0 0 100 10 10 cm /Im0 Do Q ET 0 0 1 rg 50 50 100 100 re f", image(0), ZOrder::Forward),
        "an object inside a text object cannot be arranged"
    );
}

#[test]
fn irregular_content_is_refused() {
    let irregular = "the page content around this object is too irregular to rearrange it";
    // B's marked-content sequence never ends, so there is no leaving it.
    assert_eq!(refused("1 0 0 rg 10 10 100 100 re f /Artifact BMC 0 0 1 rg 50 50 100 100 re f", path(0), ZOrder::Forward), irregular);
    // B is drawn flat: no `cm` brings R back from there.
    assert_eq!(refused("1 0 0 rg 0 0 100 100 re f 1 0 0 0 0 0 cm 0 0 1 rg 20 20 100 100 re f", path(0), ZOrder::Forward), irregular);
}

/// The page never lists a path that clips as it paints, but if it did it couldn't be arranged.
#[test]
fn a_shape_that_also_clips_is_refused() {
    let ops = pdfcraft_content::parse(b"0 0 10 10 re W f").ops;
    assert_eq!(check_core(&ops, (0, 2), ObjectKind::Path).unwrap_err().to_string(), "this shape also clips other content, so it cannot be arranged");
}

#[test]
fn too_many_graphics_states_are_refused() {
    let states: String = (0..257).map(|i| format!("/G{i} gs ")).collect();
    assert_eq!(
        refused(&format!("{states}1 0 0 rg 10 10 100 100 re f 0 0 1 rg 50 50 100 100 re f"), path(0), ZOrder::Forward),
        "this page has too many graphics-state settings to rearrange an object"
    );
}

/// The final check: a page that would list anything differently isn't changed.
#[test]
fn inventories_must_match_to_rounding() {
    let before = [("path", [10.0, 10.0, 110.0, 110.0]), ("image", [0.0, 0.0, 1.0, 1.0])];
    assert!(same(&before, &before));
    assert!(same(&before, &[("path", [10.0, 10.0, 110.0 + 1e-9, 110.0]), ("image", [0.0, 0.0, 1.0, 1.0])]));
    assert!(!same(&before, &[("path", [10.0, 10.0, 110.5, 110.0]), ("image", [0.0, 0.0, 1.0, 1.0])]));
    assert!(!same(&before, &[("image", [10.0, 10.0, 110.0, 110.0]), ("image", [0.0, 0.0, 1.0, 1.0])]));
    assert!(!same(&before, &before[..1]));
}

#[test]
fn hostile_nesting_never_panics() {
    // A `q` that is never closed, and a stray `Q`.
    assert_eq!(
        restack("q 1 0 0 rg 10 10 100 100 re f 0 0 1 rg 50 50 100 100 re f", path(0), ZOrder::Forward),
        ("q 1 0 0 rg 0 0 1 rg 50 50 100 100 re f q 1 0 0 rg 10 10 100 100 re f Q".into(), path(1))
    );
    assert_eq!(
        restack("1 0 0 rg 10 10 100 100 re f Q 0 0 1 rg 50 50 100 100 re f", path(0), ZOrder::Forward),
        ("1 0 0 rg Q 0 0 1 rg 50 50 100 100 re f q 1 0 0 rg 10 10 100 100 re f Q".into(), path(1))
    );
    // A `q` around B that is never closed: R goes in with B.
    assert_eq!(
        restack("1 0 0 rg 10 10 100 100 re f q 0 0 1 rg 50 50 100 100 re f", path(0), ZOrder::Forward),
        ("1 0 0 rg q 0 0 1 rg 50 50 100 100 re f q 1 0 0 rg 10 10 100 100 re f Q".into(), path(1))
    );
    // A thousand `q … Q` holding only R all move with it.
    let (open, shut) = ("q ".repeat(1000), " Q".repeat(1000));
    assert_eq!(
        restack(&format!("{open}1 0 0 rg 10 10 100 100 re f{shut} 0 0 1 rg 50 50 100 100 re f"), path(0), ZOrder::Forward),
        (format!("0 0 1 rg 50 50 100 100 re f q 0 g {open}1 0 0 rg 10 10 100 100 re f{shut} Q"), path(1))
    );
}
