//! Structural fidelity of annotations across save and reopen (ISO 32000-2 §12.5).
//!
//! `src/tests.rs` checks comments against the document *in memory*: it edits a comment and
//! asserts what the dictionary now holds, with `reopen` used mainly to prove the bytes parse.
//! These tests take the other half — a comment is added, the file is written and re-read from
//! scratch, and what survives is checked entry by entry against values worked out here from the
//! standard rather than read back from the crate that wrote them.
//!
//! Each test replays its assertions over all three writer paths in `support::fixture`
//! (incremental, full save with a cross-reference table, full save into compressed object
//! streams). Object numbers change on the third path, so `/P`, `/Popup`, `/IRT` and `/Dest`
//! targets have to be re-pointed rather than copied — that is where reference-carrying data
//! actually breaks.
//!
//! Scope, deliberately: **structure only**. Nothing here asserts what an appearance stream draws,
//! how wide a stamp label is, or where glyphs land. Appearance generation is being rewritten by
//! several open PRs at once (#2345 stamps, #2137 pop-up windows, #2066 Vietnamese text, #1775
//! FreeText fonts, #1513 Turkish), and a test that pinned today's drawing would be noise for
//! them. Where stored geometry is defined in terms of an appearance measurement — a line's
//! arrowhead padding, a cloud's bulge, a callout box's re-fit — these tests assert the
//! containment and internal consistency that must hold whatever that measurement is.

mod support {
    pub mod bytes;
    pub mod fixture;
}

use pdfcraft_annot::{
    AnnotError, AttachIcon, LineEnding, Markup, Meta, NewAnnotation, NoteIcon, OverlayFont, OverlayLook, ReviewState, Shape, StampKind, Style,
    add_annotation, add_reply, delete_annotation, links, page_refs, props, set_locked, set_review_state, set_style, summaries,
};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions};

use support::bytes;
use support::fixture::{
    ALL_SAVES, FLAG_NO_ROTATE, FLAG_NO_ZOOM, FLAG_PRINT, FLAG_READONLY, Save, annot_entries, blank, blank_with_indirect_annots, dict_of, name_of,
    num, nums, page_object_numbers, pdf, save_and_reopen, str_of,
};

/// A deterministic `/M` date and `/NM` id, so nothing here depends on the clock.
fn meta(id: &str) -> Meta {
    Meta { date: Some("D:20261001120000Z".into()), id: id.into() }
}

/// A comment with plain styling unless a test asks otherwise.
fn ann(page: usize, shape: Shape, contents: &str) -> NewAnnotation {
    NewAnnotation { page, shape, style: Style::default(), contents: contents.into(), author: "Ada".into() }
}

fn styled(page: usize, shape: Shape, contents: &str, color: [f64; 3], opacity: f64, width: f64, fill: Option<[f64; 3]>) -> NewAnnotation {
    NewAnnotation { page, shape, style: Style { color, opacity, width, fill }, contents: contents.into(), author: "Ada".into() }
}

/// The dictionary of the `k`-th `/Annots` entry of a page.
fn annot_at(doc: &Document, page: usize, k: usize, path: Save) -> Dict {
    let entries = annot_entries(doc, page);
    let entry = entries.get(k).unwrap_or_else(|| panic!("{path:?}: page {page} has {} /Annots entries, wanted index {k}", entries.len()));
    dict_of(doc, entry).unwrap_or_else(|| panic!("{path:?}: /Annots[{k}] is not a dictionary"))
}

fn ref_at(doc: &Document, page: usize, k: usize) -> ObjRef {
    annot_entries(doc, page)[k].as_ref().expect("an indirect /Annots entry")
}

/// Comment subtypes on a page, in `/Annots` order.
fn kinds(doc: &Document, page: usize) -> Vec<String> {
    annot_entries(doc, page).iter().filter_map(|e| dict_of(doc, e)).map(|d| name_of(doc, &d, b"Subtype").unwrap_or_default()).collect()
}

/// A `/LE`-style array of names, as strings.
fn name_array(doc: &Document, d: &Dict, key: &[u8]) -> Option<Vec<String>> {
    let a = doc.resolve(d.get(key)?).as_array()?.clone();
    // Every element must be a name; an array like [/OpenArrow 3] is not a valid /LE.
    let mut out = Vec::with_capacity(a.len());
    for x in &a {
        out.push(String::from_utf8_lossy(x.as_name()?).into_owned());
    }
    Some(out)
}

// ── the sticky note: geometry, text, flags and its pop-up ─────────────────────────────────────

/// A sticky note carries more distinct data than any other comment: a position, an icon name, the
/// flag combination that keeps the icon unscaled on zoom and rotation, a closed `Open`, opacity,
/// author, text, date, id — plus a separate pop-up annotation that points back at it.
#[test]
fn sticky_note_and_its_popup_survive_every_save_path() {
    let mut doc = blank(2);
    // §12.5.6.4: a sticky note is a `Text` annotation; the icon's top-left sits at `at`, and
    // NOTE_SIZE (20 pt) gives the box, so /Rect is [100 680 120 700].
    let i = add_annotation(&mut doc, &ann(0, Shape::Note { at: [100.0, 700.0], icon: NoteIcon::Key }, "Left margin note"), &meta("note-1"))
        .expect("adds");
    assert_eq!(i, 0);

    for path in ALL_SAVES {
        let (file, reopened) = save_and_reopen(&doc, path);
        assert_eq!(kinds(&reopened, 0), vec!["Text".to_string(), "Popup".to_string()], "{path:?}");
        let n = annot_at(&reopened, 0, 0, path);
        assert_eq!(name_of(&reopened, &n, b"Type").as_deref(), Some("Annot"), "{path:?}");
        assert_eq!(nums(&reopened, &n, b"Rect").as_deref(), Some(&[100.0, 680.0, 120.0, 700.0][..]), "{path:?} /Rect");
        assert_eq!(name_of(&reopened, &n, b"Name").as_deref(), Some("Key"), "{path:?}: the icon (§12.5.6.4 Table 175)");
        assert_eq!(str_of(&reopened, &n, b"Contents").as_deref(), Some("Left margin note"), "{path:?}");
        assert_eq!(str_of(&reopened, &n, b"T").as_deref(), Some("Ada"), "{path:?}");
        assert_eq!(str_of(&reopened, &n, b"NM").as_deref(), Some("note-1"), "{path:?}");
        assert_eq!(str_of(&reopened, &n, b"M").as_deref(), Some("D:20261001120000Z"), "{path:?}");
        assert_eq!(str_of(&reopened, &n, b"Subj").as_deref(), Some("Sticky Note"), "{path:?}");
        // §12.5.3: /CA defaults to 1, so full opacity writes no /CA at all.
        assert!(n.get(b"CA").is_none(), "{path:?}: /CA must be absent at full opacity");
        // Notes print, and stay unscaled under zoom and rotation: Print|NoZoom|NoRotate.
        assert_eq!(n.int(b"F"), Some(FLAG_PRINT | FLAG_NO_ZOOM | FLAG_NO_ROTATE), "{path:?} /F");
        assert_eq!(n.get(b"Open").map(|o| matches!(o, Object::Bool(false))), Some(true), "{path:?}: /Open false");
        assert_eq!(nums(&reopened, &n, b"C").as_deref(), Some(&[1.0, 0.82, 0.0][..]), "{path:?} /C");
        // /P names the page it belongs to — the same object, after any renumbering.
        assert_eq!(n.reference(b"P").map(|r| r.num), Some(page_object_numbers(&reopened)[0]), "{path:?}: /P resolves to page 1");

        // The pop-up names its parent and the parent names it back.
        let pop = annot_at(&reopened, 0, 1, path);
        assert_eq!(name_of(&reopened, &pop, b"Subtype").as_deref(), Some("Popup"), "{path:?}");
        assert_eq!(pop.reference(b"Parent"), Some(ref_at(&reopened, 0, 0)), "{path:?}: /Popup and /Parent agree both ways");
        assert_eq!(n.reference(b"Popup"), Some(ref_at(&reopened, 0, 1)), "{path:?}");
        // Acrobat's pop-up is a 200×120 pt window beside the icon, top aligned with it.
        assert_eq!(nums(&reopened, &pop, b"Rect").as_deref(), Some(&[130.0, 580.0, 330.0, 700.0][..]), "{path:?}: pop-up /Rect");

        if !matches!(path, Save::FullStreams) {
            // Claims about the file's own bytes (see `support::bytes`), not via our reader.
            assert_eq!(bytes::numbers_of(&file, "Rect").first(), Some(&vec![100.0, 680.0, 120.0, 700.0]), "{path:?}");
            assert!(bytes::texts_of(&file, "NM").contains(&"note-1".to_string()), "{path:?}");
            assert!(bytes::texts_of(&file, "Contents").contains(&"Left margin note".to_string()), "{path:?}");
            assert!(bytes::has_name(&file, "Key"), "{path:?}: the icon name is in the file");
        }
    }
}

/// Opacity below 1 must reach the file as `/CA`; an out-of-range value is clamped, and a clamp
/// to 1.0 means the key disappears rather than being written as `1`.
#[test]
fn opacity_is_written_only_when_it_differs_from_one() {
    let cases = [(0.65, Some(0.65)), (1.0, None), (1.75, None), (0.0, Some(0.0)), (-0.25, Some(0.0))];
    for (opacity, expected) in cases {
        let mut doc = blank(1);
        add_annotation(
            &mut doc,
            &styled(0, Shape::Rectangle { rect: [10.0, 10.0, 60.0, 40.0] }, "x", [0.9, 0.1, 0.1], opacity, 2.0, None),
            &meta("o"),
        )
        .expect("adds");
        for path in ALL_SAVES {
            let (file, reopened) = save_and_reopen(&doc, path);
            let d = annot_at(&reopened, 0, 0, path);
            assert_eq!(num(&reopened, &d, b"CA"), expected, "{path:?}: /CA at the given opacity {opacity}");
            assert_eq!(props(&reopened, 0, 0).map(|p| p.opacity), Some(expected.unwrap_or(1.0)), "{path:?}: reported opacity {opacity}");
            if !matches!(path, Save::FullStreams) {
                assert_eq!(bytes::scalars_of(&file, "CA").first().copied(), expected, "{path:?}: /CA written at opacity {opacity}");
            }
        }
    }
}

// ── text markup: quadrilaterals are the geometry (§12.5.6.10) ─────────────────────────────────

/// The four markup kinds differ in `/Subtype` alone; each keeps its quadrilateral list and derived
/// rectangle. The two-quad case shows the array is stored whole (16 numbers) rather than
/// truncated to the first quad, and that `/Rect` spans both.
#[test]
fn markup_kinds_keep_quads_and_rectangles() {
    let kinds =
        [("Highlight", Markup::Highlight), ("Underline", Markup::Underline), ("StrikeOut", Markup::StrikeOut), ("Squiggly", Markup::Squiggly)];
    let q1 = [72.0, 712.0, 200.0, 712.0, 72.0, 700.0, 200.0, 700.0];
    let q2 = [72.0, 692.0, 320.0, 692.0, 72.0, 680.0, 320.0, 680.0];
    let mut doc = blank(1);
    for (k, name) in kinds.iter() {
        add_annotation(&mut doc, &ann(0, Shape::TextMarkup { kind: *name, quads: vec![q1] }, "word"), &meta(&format!("m-{k}"))).expect("adds");
    }
    // A two-quad highlight last, so the multi-array case sits at a known index.
    let two = add_annotation(&mut doc, &ann(0, Shape::TextMarkup { kind: Markup::Highlight, quads: vec![q1, q2] }, "two lines"), &meta("m-two"))
        .expect("adds");
    assert_eq!(two, kinds.len());

    for path in ALL_SAVES {
        let (file, reopened) = save_and_reopen(&doc, path);
        let mut want = kinds.iter().map(|(label, _)| (*label).to_string()).collect::<Vec<String>>();
        want.push("Highlight".into()); // the two-quad comment added last
        assert_eq!(kinds_of(&reopened, 0), want, "{path:?}");
        for (n, label) in kinds.iter().map(|(label, _)| *label).enumerate() {
            let d = annot_at(&reopened, 0, n, path);
            assert_eq!(nums(&reopened, &d, b"QuadPoints").as_deref(), Some(&q1[..]), "{path:?}: /QuadPoints verbatim"); // attribution dropped: nothing here tests any specific reader's ordering
            assert_eq!(nums(&reopened, &d, b"Rect").as_deref(), Some(&[72.0, 700.0, 200.0, 712.0][..]), "{path:?}: /Rect bounds the quad");
            assert!(d.get(b"BS").is_none(), "{path:?}: markup writes no border style");
            assert_eq!(str_of(&reopened, &d, b"NM").as_deref(), Some(format!("m-{label}").as_str()), "{path:?}");
        }
        let d = annot_at(&reopened, 0, two, path);
        assert_eq!(
            nums(&reopened, &d, b"QuadPoints").as_deref(),
            Some(&[q1.to_vec(), q2.to_vec()].concat()[..]),
            "{path:?}: two quads are 16 numbers"
        );
        assert_eq!(nums(&reopened, &d, b"Rect").as_deref(), Some(&[72.0, 680.0, 320.0, 712.0][..]), "{path:?}: /Rect spans both quads");
        if !matches!(path, Save::FullStreams) {
            let sixteen = [q1.to_vec(), q2.to_vec()].concat();
            assert_eq!(bytes::numbers_of(&file, "QuadPoints").last(), Some(&sixteen), "{path:?}: 16 fixed-decimal numbers in the file");
        }
    }
}

fn kinds_of(doc: &Document, page: usize) -> Vec<String> {
    kinds(doc, page)
}

/// `Summary` reports quadrilaterals as 8-number chunks; the same list must chunk identically
/// after a save, and its rectangle must be the bounds those chunks imply.
#[test]
fn summary_quads_and_rect_agree_after_reopen() {
    let q1 = [72.0, 300.0, 160.0, 300.0, 72.0, 286.0, 160.0, 286.0];
    let q2 = [72.0, 280.0, 500.0, 280.0, 72.0, 266.0, 500.0, 266.0];
    let mut doc = blank(1);
    add_annotation(&mut doc, &ann(0, Shape::TextMarkup { kind: Markup::Squiggly, quads: vec![q1, q2] }, "s"), &meta("sq")).expect("adds");
    for path in ALL_SAVES {
        let (_, reopened) = save_and_reopen(&doc, path);
        let sum = summaries(&reopened).into_iter().find(|s| s.subtype == "Squiggly").expect("the squiggly is listed");
        assert_eq!(sum.quads, vec![q1.map(|v| v as f32), q2.map(|v| v as f32)], "{path:?}");
        assert_eq!(sum.rect, [72.0f32, 266.0, 500.0, 300.0], "{path:?}");
        assert_eq!(sum.contents.as_deref(), Some("s"), "{path:?}");
        assert_eq!(sum.author.as_deref(), Some("Ada"), "{path:?}");
    }
}

// ── shapes, lines and drawings ────────────────────────────────────────────────────────────────

/// Squares, ovals and stamps keep an exact `/Rect` (reversed corners normalised as §12.5.4.2
/// requires), interior colour, border width and stamp identity.
#[test]
fn shapes_keep_rectangles_fills_borders_and_stamp_identity() {
    let mut doc = blank(1);
    add_annotation(
        &mut doc,
        &styled(0, Shape::Rectangle { rect: [300.0, 300.0, 200.0, 200.0] }, "box", [0.9, 0.1, 0.1], 1.0, 2.0, Some([0.0, 0.5, 0.0])),
        &meta("sq"),
    )
    .expect("adds square");
    add_annotation(&mut doc, &styled(0, Shape::Oval { rect: [50.0, 60.0, 90.0, 120.0] }, "oval", [0.0, 0.4, 0.8], 1.0, 3.0, None), &meta("ov"))
        .expect("adds oval");
    add_annotation(
        &mut doc,
        &styled(
            0,
            Shape::Stamp { rect: [40.0, 600.0, 180.0, 650.0], stamp: StampKind::Approved, by: Some("Ada".into()) },
            "stamped",
            [0.13, 0.55, 0.13],
            1.0,
            2.0,
            None,
        ),
        &meta("st"),
    )
    .expect("adds stamp");

    for path in ALL_SAVES {
        let (file, reopened) = save_and_reopen(&doc, path);
        let sq = annot_at(&reopened, 0, 0, path);
        assert_eq!(name_of(&reopened, &sq, b"Subtype").as_deref(), Some("Square"), "{path:?}");
        // Corners were given as [300 300 200 200]; a rectangle is stored lower-left then upper-right.
        assert_eq!(nums(&reopened, &sq, b"Rect").as_deref(), Some(&[200.0, 200.0, 300.0, 300.0][..]), "{path:?}");
        assert_eq!(nums(&reopened, &sq, b"IC").as_deref(), Some(&[0.0, 0.5, 0.0][..]), "{path:?}: /IC (§12.5.6.8)");
        assert_eq!(nums(&reopened, &sq, b"C").as_deref(), Some(&[0.9, 0.1, 0.1][..]), "{path:?}");
        let p = props(&reopened, 0, 0).expect("properties");
        assert_eq!(p.width, Some(2.0), "{path:?}: /BS /W");
        assert_eq!(p.fill, Some([0.0, 0.5, 0.0]), "{path:?}");
        assert!(p.fillable, "{path:?}: rectangles can be filled");

        let ov = annot_at(&reopened, 0, 1, path);
        assert_eq!(name_of(&reopened, &ov, b"Subtype").as_deref(), Some("Circle"), "{path:?}: an oval is /Circle (§12.5.6.9)");
        assert_eq!(nums(&reopened, &ov, b"Rect").as_deref(), Some(&[50.0, 60.0, 90.0, 120.0][..]), "{path:?}");
        assert!(ov.get(b"IC").is_none(), "{path:?}: no interior colour when unfilled");
        assert_eq!(props(&reopened, 0, 1).and_then(|q| q.width), Some(3.0), "{path:?}");

        let st = annot_at(&reopened, 0, 2, path);
        assert_eq!(name_of(&reopened, &st, b"Subtype").as_deref(), Some("Stamp"), "{path:?}");
        assert_eq!(name_of(&reopened, &st, b"Name").as_deref(), Some("Approved"), "{path:?}: a standard stamp keeps its Table 184 name");
        assert_eq!(nums(&reopened, &st, b"Rect").as_deref(), Some(&[40.0, 600.0, 180.0, 650.0][..]), "{path:?}");
        assert!(st.contains(b"PCStamp"), "{path:?}: PdfCraft's own artwork is marked as such");
        assert_eq!(str_of(&reopened, &st, b"PCByLine").as_deref(), Some("Ada"), "{path:?}");
        if !matches!(path, Save::FullStreams) {
            assert_eq!(bytes::numbers_of(&file, "IC").first(), Some(&vec![0.0, 0.5, 0.0]), "{path:?}");
            assert!(bytes::has_name(&file, "Approved"), "{path:?}");
        }
    }
}

/// Lines, connected lines and polygons keep their endpoint arrays (`/L`, `/Vertices`), their
/// ending pair (`/LE`) and a cloud's intent. Their `/Rect` is grown by arrowhead and bulge
/// measurements that live in `appearance`, so this checks containment rather than exact padding.
#[test]
fn lines_and_polygons_keep_endpoints_endings_and_cloud_intent() {
    let mut doc = blank(1);
    add_annotation(
        &mut doc,
        &ann(0, Shape::Line { from: [50.0, 50.0], to: [150.0, 100.0], start: LineEnding::None, end: LineEnding::OpenArrow }, "arrow"),
        &meta("l1"),
    )
    .expect("adds arrow");
    add_annotation(
        &mut doc,
        &ann(
            0,
            Shape::PolyLine { vertices: vec![[10.0, 20.0], [40.0, 60.0], [70.0, 25.0]], start: LineEnding::ClosedArrow, end: LineEnding::Butt },
            "path",
        ),
        &meta("pl"),
    )
    .expect("adds polyline");
    add_annotation(
        &mut doc,
        &ann(0, Shape::Polygon { vertices: vec![[200.0, 100.0], [260.0, 110.0], [230.0, 160.0]], cloud: true }, "cloud"),
        &meta("pg"),
    )
    .expect("adds cloud");
    add_annotation(
        &mut doc,
        &ann(0, Shape::Ink { strokes: vec![vec![[0.0, 0.0], [10.0, 10.0], [20.0, 0.0]], vec![[40.0, 40.0], [50.0, 50.0]]] }, "ink"),
        &meta("ink"),
    )
    .expect("adds ink");

    for path in ALL_SAVES {
        let (file, reopened) = save_and_reopen(&doc, path);

        let l = annot_at(&reopened, 0, 0, path);
        assert_eq!(name_of(&reopened, &l, b"Subtype").as_deref(), Some("Line"), "{path:?}");
        assert_eq!(nums(&reopened, &l, b"L").as_deref(), Some(&[50.0, 50.0, 150.0, 100.0][..]), "{path:?}: /L (§12.5.6.7)");
        assert_eq!(
            name_array(&reopened, &l, b"LE").as_deref(),
            Some(&["None".to_string(), "OpenArrow".to_string()][..]),
            "{path:?}: /LE (§12.5.6.7 Table 217)"
        );
        let r = nums(&reopened, &l, b"Rect").expect("/Rect");
        assert!(r[0] <= 50.0 && r[1] <= 50.0 && r[2] >= 150.0 && r[3] >= 100.0, "{path:?}: /Rect must contain both endpoints, got {r:?}");
        assert_eq!(props(&reopened, 0, 0).and_then(|p| p.endings), Some(vec![LineEnding::None, LineEnding::OpenArrow]), "{path:?}");

        let pl = annot_at(&reopened, 0, 1, path);
        assert_eq!(name_of(&reopened, &pl, b"Subtype").as_deref(), Some("PolyLine"), "{path:?}");
        assert_eq!(nums(&reopened, &pl, b"Vertices").as_deref(), Some(&[10.0, 20.0, 40.0, 60.0, 70.0, 25.0][..]), "{path:?}: /Vertices (§12.5.6.9)");
        assert_eq!(props(&reopened, 0, 1).and_then(|p| p.endings), Some(vec![LineEnding::ClosedArrow, LineEnding::Butt]), "{path:?}");

        let pg = annot_at(&reopened, 0, 2, path);
        assert_eq!(name_of(&reopened, &pg, b"Subtype").as_deref(), Some("Polygon"), "{path:?}");
        assert_eq!(name_of(&reopened, &pg, b"IT").as_deref(), Some("PolygonCloud"), "{path:?}: a cloud is `/IT /PolygonCloud` (§12.5.6.9)");
        let be = pg.get(b"BE").map(|o| reopened.resolve(o)).and_then(|o| o.as_dict().cloned()).expect("/BE");
        assert_eq!(name_of(&reopened, &be, b"S").as_deref(), Some("C"), "{path:?}: cloudy border");
        assert_eq!(be.int(b"I"), Some(1), "{path:?}");
        assert_eq!(nums(&reopened, &pg, b"Vertices").as_deref(), Some(&[200.0, 100.0, 260.0, 110.0, 230.0, 160.0][..]), "{path:?}");

        let ink = annot_at(&reopened, 0, 3, path);
        assert_eq!(name_of(&reopened, &ink, b"Subtype").as_deref(), Some("Ink"), "{path:?}");
        let strokes: Vec<Vec<f64>> = ink
            .get(b"InkList")
            .map(|o| reopened.resolve(o))
            .and_then(|o| o.as_array().cloned())
            .expect("/InkList")
            .iter()
            .map(|s| reopened.resolve(s).as_array().expect("a stroke array").iter().filter_map(|x| x.as_f64()).collect())
            .collect();
        assert_eq!(strokes, vec![vec![0.0, 0.0, 10.0, 10.0, 20.0, 0.0], vec![40.0, 40.0, 50.0, 50.0]], "{path:?}: both strokes, all their points");
        if !matches!(path, Save::FullStreams) {
            assert_eq!(bytes::numbers_of(&file, "L").first(), Some(&vec![50.0, 50.0, 150.0, 100.0]), "{path:?}");
            assert_eq!(bytes::numbers_of(&file, "Vertices").first(), Some(&vec![10.0, 20.0, 40.0, 60.0, 70.0, 25.0]), "{path:?}");
        }
    }
}

// ── FreeText: the box, its font size and a callout's leader line ───────────────────────────────

/// A text box stores its rectangle, colour and size in `/DA`/`Q`, and typewriter text marks its
/// intent (§12.5.6.6). The stored box is the requested one: re-fitting happens on later edits,
/// not at creation.
#[test]
fn text_boxes_keep_rectangles_font_size_and_intent() {
    let mut doc = blank(1);
    add_annotation(
        &mut doc,
        &styled(0, Shape::TextBox { rect: [300.0, 600.0, 500.0, 650.0], font_size: 14.0 }, "Type here", [0.1, 0.2, 0.3], 1.0, 0.0, None),
        &meta("tb"),
    )
    .expect("adds text box");
    add_annotation(
        &mut doc,
        &styled(0, Shape::Typewriter { rect: [72.0, 100.0, 300.0, 124.0], font_size: 12.0 }, "Typed words", [0.0, 0.0, 0.0], 1.0, 0.0, None),
        &meta("tw"),
    )
    .expect("adds typewriter");

    for path in ALL_SAVES {
        let (file, reopened) = save_and_reopen(&doc, path);
        let tb = annot_at(&reopened, 0, 0, path);
        assert_eq!(name_of(&reopened, &tb, b"Subtype").as_deref(), Some("FreeText"), "{path:?}");
        assert_eq!(nums(&reopened, &tb, b"Rect").as_deref(), Some(&[300.0, 600.0, 500.0, 650.0][..]), "{path:?}");
        assert_eq!(str_of(&reopened, &tb, b"DA").as_deref(), Some("0.1 0.2 0.3 rg /Helv 14 Tf"), "{path:?}: /DA (§12.5.6.6, §12.7.2.3)");
        assert_eq!(tb.int(b"Q"), Some(0), "{path:?}: left aligned");
        assert!(tb.get(b"IT").is_none(), "{path:?}: a plain text box sets no intent");
        assert_eq!(props(&reopened, 0, 0).and_then(|p| p.font_size), Some(14.0), "{path:?}");
        assert_eq!(props(&reopened, 0, 0).and_then(|p| p.color), Some([0.1, 0.2, 0.3]), "{path:?}: FreeText colour comes from /DA");

        let tw = annot_at(&reopened, 0, 1, path);
        assert_eq!(name_of(&reopened, &tw, b"IT").as_deref(), Some("FreeTextTypeWriter"), "{path:?}");
        assert_eq!(nums(&reopened, &tw, b"Rect").as_deref(), Some(&[72.0, 100.0, 300.0, 124.0][..]), "{path:?}");
        assert_eq!(props(&reopened, 0, 1).and_then(|p| p.font_size), Some(12.0), "{path:?}");
        assert!(tw.contains(b"PCFillSign"), "{path:?}: typewriter text is a Fill & Sign item");
        if !matches!(path, Save::FullStreams) {
            assert_eq!(bytes::texts_of(&file, "DA").first().map(String::as_str), Some("0.1 0.2 0.3 rg /Helv 14 Tf"), "{path:?}");
        }
    }
}

/// A callout keeps six numbers in `/CL` (arrow point, knee, box attach) plus the inset `/RD`;
/// together those must rebuild exactly the text box that was asked for. That is a consistency
/// claim, so it does not depend on how wide the arrowhead is drawn.
#[test]
fn callout_keeps_its_leader_line_and_rebuilds_the_same_text_box() {
    let mut doc = blank(1);
    add_annotation(
        &mut doc,
        &ann(
            0,
            Shape::Callout {
                rect: [300.0, 400.0, 420.0, 460.0],
                knee: [250.0, 380.0],
                point: [200.0, 350.0],
                font_size: 16.0,
                ending: LineEnding::OpenArrow,
            },
            "why",
        ),
        &meta("co"),
    )
    .expect("adds callout");
    for path in ALL_SAVES {
        let (file, reopened) = save_and_reopen(&doc, path);
        let d = annot_at(&reopened, 0, 0, path);
        assert_eq!(name_of(&reopened, &d, b"IT").as_deref(), Some("FreeTextCallout"), "{path:?}");
        // The knee is left of the box, so the leader attaches at the box's left edge midpoint.
        assert_eq!(nums(&reopened, &d, b"CL").as_deref(), Some(&[200.0, 350.0, 250.0, 380.0, 300.0, 430.0][..]), "{path:?}: /CL point→knee→attach");
        assert_eq!(props(&reopened, 0, 0).and_then(|p| p.endings), Some(vec![LineEnding::OpenArrow]), "{path:?}");
        assert_eq!(props(&reopened, 0, 0).and_then(|p| p.font_size), Some(16.0), "{path:?}");
        let rect = nums(&reopened, &d, b"Rect").expect("/Rect");
        let rd = nums(&reopened, &d, b"RD").expect("/RD");
        assert!(rd.iter().all(|v| *v >= 0.0), "{path:?}: /RD insets are non-negative (§12.5.3)");
        assert_eq!(
            [rect[0] + rd[0], rect[1] + rd[1], rect[2] - rd[2], rect[3] - rd[3]],
            [300.0, 400.0, 420.0, 460.0],
            "{path:?}: /Rect inset by /RD is the text box"
        );
        if !matches!(path, Save::FullStreams) {
            assert_eq!(bytes::numbers_of(&file, "CL").first(), Some(&vec![200.0, 350.0, 250.0, 380.0, 300.0, 430.0]), "{path:?}");
        }
    }
}

// ── redaction and file attachment: payload data, not just dictionary keys ─────────────────────

/// A redaction stores the area it will cover, the colour it will paint and its overlay text with
/// style. Applying a redaction is other crates' business; here only the data has to survive.
#[test]
fn redaction_keeps_quads_fill_and_overlay_text() {
    let quads = vec![[72.0, 712.0, 300.0, 712.0, 72.0, 690.0, 300.0, 690.0], [72.0, 680.0, 240.0, 680.0, 72.0, 660.0, 240.0, 660.0]];
    let mut doc = blank(1);
    add_annotation(
        &mut doc,
        &styled(
            0,
            Shape::Redact {
                quads: quads.clone(),
                overlay: "SECRET".into(),
                look: OverlayLook { font: OverlayFont::Courier, size: 0.0, color: [1.0, 0.0, 0.0], align: 2, repeat: true },
            },
            "remove this",
            [0.89, 0.13, 0.13],
            1.0,
            1.0,
            Some([0.0, 0.0, 0.0]),
        ),
        &meta("red"),
    )
    .expect("adds redaction");
    for path in ALL_SAVES {
        let (file, reopened) = save_and_reopen(&doc, path);
        let d = annot_at(&reopened, 0, 0, path);
        assert_eq!(name_of(&reopened, &d, b"Subtype").as_deref(), Some("Redact"), "{path:?}");
        assert_eq!(nums(&reopened, &d, b"QuadPoints").as_deref(), Some(&quads.concat()[..]), "{path:?}");
        assert_eq!(nums(&reopened, &d, b"IC").as_deref(), Some(&[0.0, 0.0, 0.0][..]), "{path:?}: /IC is the colour applied (§12.5.6.23)");
        assert_eq!(str_of(&reopened, &d, b"OverlayText").as_deref(), Some("SECRET"), "{path:?}");
        assert_eq!(str_of(&reopened, &d, b"DA").as_deref(), Some("1 0 0 rg /Cour 0 Tf"), "{path:?}: the overlay font resource");
        assert_eq!(d.int(b"Q"), Some(2), "{path:?}: right aligned");
        assert_eq!(d.get(b"Repeat").map(|o| matches!(o, Object::Bool(true))), Some(true), "{path:?}");
        assert_eq!(nums(&reopened, &d, b"Rect").as_deref(), Some(&[72.0, 660.0, 300.0, 712.0][..]), "{path:?}");
        if !matches!(path, Save::FullStreams) {
            assert_eq!(bytes::texts_of(&file, "OverlayText").first().map(String::as_str), Some("SECRET"), "{path:?}");
        }
    }
}

/// An attached file is the heaviest annotation there is: a file specification, an embedded file
/// stream with its own parameters, and the bytes inside it. A full rewrite renumbers all three
/// objects and may move them into object streams, so this is the strongest reference test here.
#[test]
fn attached_file_specification_and_bytes_survive_every_save_path() {
    let body = b"first line\nsecond line, 27 bytes more\n".to_vec();
    let mut doc = blank(1);
    add_annotation(
        &mut doc,
        &ann(0, Shape::Attachment { at: [300.0, 600.0], icon: AttachIcon::Paperclip, file: "notes.txt".into(), data: body.clone() }, ""),
        &meta("att"),
    )
    .expect("attaches");
    for path in ALL_SAVES {
        let (_, reopened) = save_and_reopen(&doc, path);
        let d = annot_at(&reopened, 0, 0, path);
        assert_eq!(name_of(&reopened, &d, b"Subtype").as_deref(), Some("FileAttachment"), "{path:?}");
        assert_eq!(name_of(&reopened, &d, b"Name").as_deref(), Some("Paperclip"), "{path:?}");
        // With no comment text the file's name is shown as the contents (§12.5.6.15).
        assert_eq!(str_of(&reopened, &d, b"Contents").as_deref(), Some("notes.txt"), "{path:?}");
        assert_eq!(nums(&reopened, &d, b"Rect").as_deref(), Some(&[300.0, 580.0, 320.0, 600.0][..]), "{path:?}");

        let fs_ref = d.reference(b"FS").expect("/FS must reference a file specification");
        let fs = reopened.get(fs_ref).as_dict().cloned().expect("/FS resolves to a dictionary");
        assert_eq!(name_of(&reopened, &fs, b"Type").as_deref(), Some("Filespec"), "{path:?}");
        assert_eq!(str_of(&reopened, &fs, b"F").as_deref(), Some("notes.txt"), "{path:?}");
        let ef = fs.get(b"EF").map(|o| reopened.resolve(o)).and_then(|o| o.as_dict().cloned()).expect("/EF");
        let file_ref = ef.reference(b"F").expect("/EF /F references the embedded stream");
        let Object::Stream(s) = &*reopened.get(file_ref) else { panic!("the embedded file is a stream") };
        assert_eq!(s.decoded().expect("decodes").as_slice(), &body[..], "{path:?}: the file's bytes survive");
        let params = s.dict.get(b"Params").map(|o| reopened.resolve(o)).and_then(|o| o.as_dict().cloned()).expect("/Params");
        assert_eq!(params.int(b"Size").map(|v| v as usize), Some(body.len()), "{path:?}");
    }
}

// ── order, indices and page ownership across several comments ─────────────────────────────────

/// Five comments on two pages keep their per-page order, the `(page, index)` addressing documented
/// at the top of `lib.rs`, and each one's `/P`. `summaries` additionally promises page order then
/// top edge first; both are checked, because `index` refers to `/Annots` position while the list
/// order does not.
/// COVERAGE HONESTY (ten single-site mutations, run independently by two reviewers plus the
/// author). FIVE are also killed by pre-existing inline tests in `crates/annot/src/tests.rs`, and
/// they share a family — the clamp is INVERTED, or a key/order changed rather than removed:
/// `lib.rs:857` inverted (`ink_strokes_are_smooth_curves_through_their_points`), `:1030` inverted
/// (six inline tests), `:1930` inverted (`restyle_and_resize_redraw_the_appearance`), `:2161`
/// tie-break swapped (`stamps_draw_their_label_and_by_line`, the same tie-break as here) and
/// `:1241` `/Parent` renamed (`every_tool_creates_a_drawable_comment`). This file shares that
/// credit and claims none of it alone. The other FIVE are killed only by tests added here, and
/// they share a different family — the clamp is REMOVED outright, or introduced where the code
/// does not have one: `lib.rs:1030` deleted, `lib.rs:857` deleted, the opacity clamp at `:1024`
/// removed, the colour clamp in `rgb()` at `:824` removed (killed only by
/// `malformed::the_same_huge_fill_is_accepted_on_create_and_refused_on_edit`), and a clamp ADDED
/// to the read path at `:2238`. Note that `lib.rs:857` appears on both sides, and deliberately so:
/// inverting that clamp trips an inline test as well, while deleting it trips nothing but
/// `a_negative_and_a_zero_border_pad_a_line_identically`. Confusing those two mutations is easy
/// enough to produce a false refutation, so the distinction is recorded here rather than left to
/// whoever re-runs them.
/// Two further notes: the stored `/Annots` order checked below goes through all three save paths,
/// which no inline test does; and `properties.rs::several_rectangles_keep_count_order_and_geometry`
/// constrains stored array order only — it does NOT exercise `summaries()` ordering.
#[test]
fn several_comments_keep_order_indices_and_page_ownership() {
    let mut doc = blank(4);
    add_annotation(&mut doc, &ann(0, Shape::Rectangle { rect: [10.0, 100.0, 60.0, 140.0] }, "low"), &meta("a")).expect("adds");
    add_annotation(&mut doc, &ann(0, Shape::Note { at: [10.0, 700.0], icon: NoteIcon::Note }, "high note"), &meta("b")).expect("adds");
    add_annotation(
        &mut doc,
        &ann(0, Shape::TextMarkup { kind: Markup::Highlight, quads: vec![[10.0, 450.0, 90.0, 450.0, 10.0, 436.0, 90.0, 436.0]] }, "mid"),
        &meta("c"),
    )
    .expect("adds");
    add_annotation(&mut doc, &ann(2, Shape::Oval { rect: [20.0, 30.0, 80.0, 90.0] }, "p3 oval"), &meta("d")).expect("adds");
    add_annotation(&mut doc, &ann(2, Shape::Caret { rect: [300.0, 400.0, 312.0, 424.0] }, "p3 caret"), &meta("e")).expect("adds");

    for path in ALL_SAVES {
        let (_, reopened) = save_and_reopen(&doc, path);
        assert_eq!(kinds(&reopened, 0), ["Square", "Text", "Popup", "Highlight"].map(str::to_owned), "{path:?}: page 1 order");
        assert_eq!(kinds(&reopened, 2), ["Circle", "Caret"].map(str::to_owned), "{path:?}: page 3 order");
        assert_eq!(annot_entries(&reopened, 1).len(), 0, "{path:?}: page 2 stays comment-free");
        assert_eq!(annot_entries(&reopened, 3).len(), 0, "{path:?}");

        // Every comment's /P names its own page object (pop-ups carry no /P).
        let pages = page_object_numbers(&reopened);
        for (page, want) in [(0usize, pages[0]), (2, pages[2])] {
            for e in annot_entries(&reopened, page) {
                let d = dict_of(&reopened, &e).expect("a dictionary");
                if name_of(&reopened, &d, b"Subtype").as_deref() == Some("Popup") {
                    continue;
                }
                assert_eq!(d.reference(b"P").map(|r| r.num), Some(want), "{path:?}: /P of {:?}", name_of(&reopened, &d, b"Subtype"));
            }
        }

        let sums = summaries(&reopened);
        assert_eq!(sums.len(), 5, "{path:?}: pop-ups are not comments");
        let mut got: Vec<(usize, usize, String)> = sums.iter().map(|s| (s.page, s.index, s.subtype.clone())).collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                (0, 0, "Square".to_string()),
                (0, 1, "Text".to_string()),
                (0, 3, "Highlight".to_string()),
                (2, 0, "Circle".to_string()),
                (2, 1, "Caret".to_string())
            ],
            "{path:?}: (page, index in /Annots, subtype)"
        );
        for w in sums.windows(2) {
            if w[0].page == w[1].page {
                assert!(w[0].rect[3] >= w[1].rect[3], "{path:?}: top edge first, got {:?} then {:?}", w[0].rect, w[1].rect);
            } else {
                assert!(w[0].page < w[1].page, "{path:?}: page by page");
            }
        }
    }
}

/// `annot::page_refs` — the crate's own page walk, cycle guard and all — must agree with this
/// file's independent walk over the same reopened bytes.
#[test]
fn page_refs_agrees_with_an_independent_walk() {
    let mut doc = blank(3);
    add_annotation(&mut doc, &ann(1, Shape::Rectangle { rect: [0.0, 0.0, 20.0, 20.0] }, "x"), &meta("p")).expect("adds");
    for path in ALL_SAVES {
        let (_, reopened) = save_and_reopen(&doc, path);
        let mine = page_object_numbers(&reopened);
        let theirs: Vec<u32> = page_refs(&reopened).expect("a page tree").iter().map(|r| r.num).collect();
        assert_eq!(mine, theirs, "{path:?}");
    }
}

// ── threads: replies and review states keep their edges ────────────────────────────────────────

/// A reply is a text annotation whose `/IRT` points at its parent; setting a status adds another
/// one. Deleting a comment takes its pop-up and its replies with it. Every edge there is a
/// reference, so a full rewrite has to re-point them all.
#[test]
fn reply_threads_and_the_delete_cascade_survive_every_save_path() {
    let mut doc = blank(1);
    add_annotation(&mut doc, &ann(0, Shape::Note { at: [100.0, 700.0], icon: NoteIcon::Comment }, "parent"), &meta("p1")).expect("adds parent");
    add_reply(&mut doc, 0, 0, "child one", "Ben", &meta("r1")).expect("replies");
    let status = set_review_state(&mut doc, 0, 0, ReviewState::Accepted, "Ben", &meta("r2")).expect("sets status");
    assert_eq!(status, 3);

    for path in ALL_SAVES {
        let (_, reopened) = save_and_reopen(&doc, path);
        assert_eq!(kinds(&reopened, 0), vec!["Text", "Popup", "Text", "Text"], "{path:?}: parent, pop-up, reply, status reply");
        let parent = ref_at(&reopened, 0, 0);
        for k in [2usize, 3] {
            let d = annot_at(&reopened, 0, k, path);
            assert_eq!(d.reference(b"IRT"), Some(parent), "{path:?}: /IRT points at the parent after renumbering");
            assert_eq!(str_of(&reopened, &d, b"T").as_deref(), Some("Ben"), "{path:?}");
        }
        let st = annot_at(&reopened, 0, status, path);
        assert_eq!(str_of(&reopened, &st, b"State").as_deref(), Some("Accepted"), "{path:?}");
        assert_eq!(str_of(&reopened, &st, b"StateModel").as_deref(), Some("Review"), "{path:?}");
        assert_eq!(str_of(&reopened, &st, b"Subj").as_deref(), Some("Status"), "{path:?}");
        // Replies are printed but keep the icon flags of a comment; no Hidden bit here.
        assert_eq!(st.int(b"F"), Some(FLAG_PRINT | FLAG_NO_ZOOM | FLAG_NO_ROTATE), "{path:?}");
        let listed = summaries(&reopened);
        assert_eq!(listed.iter().filter(|s| s.in_reply_to.as_deref() == Some("p1")).count(), 2, "{path:?}: both replies name the parent's /NM");
        assert_eq!(listed.iter().filter(|s| s.state.as_deref() == Some("Accepted")).count(), 1, "{path:?}");

        // Deleting the comment takes its pop-up and both replies with it.
        let mut cut = reopened.clone();
        delete_annotation(&mut cut, 0, 0).expect("deletes the thread");
        assert_eq!(annot_entries(&cut, 0).len(), 0, "{path:?}: nothing of the thread is left in memory");
        let (_, cut) = save_and_reopen(&cut, path);
        assert_eq!(annot_entries(&cut, 0).len(), 0, "{path:?}: and the file agrees");
        assert!(summaries(&cut).is_empty(), "{path:?}");
    }
}

/// The Locked flag is not decoration: a comment that reads back locked must refuse the edits the
/// crate says it refuses. Checking after a save means the flag survived on disk, not merely in
/// memory that happened to still hold it.
#[test]
fn locked_flag_survives_and_still_refuses_edits_after_reopen() {
    let mut doc = blank(1);
    add_annotation(&mut doc, &ann(0, Shape::Rectangle { rect: [10.0, 20.0, 60.0, 50.0] }, "locked box"), &meta("lk")).expect("adds");
    set_locked(&mut doc, 0, 0, true).expect("locks");

    for path in ALL_SAVES {
        let (file, reopened) = save_and_reopen(&doc, path);
        let d = annot_at(&reopened, 0, 0, path);
        assert_eq!(d.int(b"F"), Some(FLAG_PRINT | FLAG_READONLY), "{path:?}: Print plus ReadOnly (bit 8)");
        assert_eq!(props(&reopened, 0, 0).map(|p| p.locked), Some(true), "{path:?}");
        let mut try_move = reopened.clone();
        let e = pdfcraft_annot::move_annotation(&mut try_move, 0, 0, 5.0, 5.0, &meta("nope")).err();
        assert_eq!(e, Some(AnnotError::Invalid("the comment is locked".into())), "{path:?}: moving a locked comment refuses");
        let mut try_delete = reopened.clone();
        assert!(delete_annotation(&mut try_delete, 0, 0).is_err(), "{path:?}: deleting a locked comment refuses");
        if !matches!(path, Save::FullStreams) {
            assert_eq!(bytes::scalars_of(&file, "F").first(), Some(&132.0), "{path:?}: the flag value is in the file");
        }
    }
}

// ── links (§12.5.6.5) ────────────────────────────────────────────────────────────────────────

/// A link's action is a destination array naming a page object, or a URI action. Page targets are
/// the interesting case across a full rewrite: the link must land on the same page after every
/// object number changes. NOTE (naming): this covers link structure — `/A`, `/Dest`, `/URI` and the
/// dash pattern. It deliberately says nothing about appearance streams, which are out of scope here
/// because #2345/#2137/#2066/#1775/#1513 are rewriting appearance generation.
#[test]
fn links_keep_page_targets_uri_and_dash_structure_across_renumbering() {
    let mut doc = blank(3);
    add_annotation(&mut doc, &ann(0, Shape::Rectangle { rect: [10.0, 700.0, 90.0, 720.0] }, "shape"), &meta("s")).expect("adds a comment too");
    let i = links::add(
        &mut doc,
        0,
        [120.0, 700.0, 240.0, 720.0],
        &links::LinkAction::Page(2),
        &links::LinkStyle { visible: true, width: 2.0, dashed: true, ..Default::default() },
    )
    .expect("links to page 3");
    assert_eq!(i, 1);
    links::add(
        &mut doc,
        1,
        [50.0, 600.0, 200.0, 620.0],
        &links::LinkAction::Uri("https://example.com/a?b=c&d=1".into()),
        &links::LinkStyle::default(),
    )
    .expect("links out");

    for path in ALL_SAVES {
        let (file, reopened) = save_and_reopen(&doc, path);
        let pages = page_object_numbers(&reopened);
        let got: Vec<(usize, usize, [f64; 4], String, bool)> = links::list(&reopened)
            .into_iter()
            .map(|l| {
                let a = match l.action {
                    links::LinkAction::Page(p) => format!("page {p}"),
                    links::LinkAction::Uri(u) => u,
                    links::LinkAction::Other(o) => format!("other:{o}"),
                };
                (l.page, l.index, l.rect, a, l.style.visible)
            })
            .collect();
        assert_eq!(got.len(), 2, "{path:?}");
        assert_eq!(got[0], (0, 1, [120.0, 700.0, 240.0, 720.0], "page 2".into(), true), "{path:?}: the target is still page 3");
        assert_eq!(got[1], (1, 0, [50.0, 600.0, 200.0, 620.0], "https://example.com/a?b=c&d=1".into(), false), "{path:?}");

        let d = annot_at(&reopened, 0, 1, path);
        let dest = d.get(b"Dest").map(|o| reopened.resolve(o)).and_then(|o| o.as_array().cloned()).expect("/Dest is an array");
        assert_eq!(dest.first().and_then(|x| x.as_ref()).map(|r| r.num), Some(pages[2]), "{path:?}: /Dest names the page object");
        assert_eq!(name_of(&reopened, &d, b"H").as_deref(), Some("I"), "{path:?}: highlight /H (invert, §12.5.3)");
        let bs = d.get(b"BS").map(|o| reopened.resolve(o)).and_then(|o| o.as_dict().cloned()).expect("/BS");
        assert_eq!(name_of(&reopened, &bs, b"S").as_deref(), Some("D"), "{path:?}: dashed");
        assert_eq!(bs.get(b"W").and_then(|w| w.as_f64()), Some(2.0), "{path:?}");
        assert_eq!(summaries(&reopened).iter().filter(|s| s.page == 0).count(), 1, "{path:?}: only the rectangle is a comment");
        if !matches!(path, Save::FullStreams) {
            assert_eq!(
                bytes::texts_of(&file, "URI").first().map(String::as_str),
                Some("https://example.com/a?b=c&d=1"),
                "{path:?}: the URI is in the file"
            );
        }
    }
}

/// Removing every link leaves comments alone. Both directions are index arithmetic over one shared
/// array, so a mistake shows up as a lost comment or an orphaned link.
#[test]
fn removing_all_links_keeps_comments() {
    let mut doc = blank(2);
    add_annotation(&mut doc, &ann(0, Shape::Note { at: [50.0, 700.0], icon: NoteIcon::Help }, "note"), &meta("n")).expect("adds");
    links::add(&mut doc, 0, [200.0, 700.0, 300.0, 715.0], &links::LinkAction::Page(0), &links::LinkStyle::default()).expect("links");
    let removed = links::remove_all(&mut doc, None).expect("removes all links");
    assert_eq!(removed, 1);

    for path in ALL_SAVES {
        let (_, reopened) = save_and_reopen(&doc, path);
        assert_eq!(kinds(&reopened, 0), vec!["Text", "Popup"], "{path:?}: the note and its pop-up stay");
        assert!(links::list(&reopened).is_empty(), "{path:?}");
    }
}

// ── data the crate does not understand, and nothing at all ─────────────────────────────────────

/// A stranger's file can hold inline annotation dictionaries and keys PdfCraft has never heard of.
/// Fidelity says they are preserved rather than dropped (`CLAUDE.md`: preserve unknown data).
#[test]
fn inline_annotations_and_unknown_keys_survive_every_save_path() {
    let objs: Vec<&[u8]> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [<< /Type /Annot /Subtype /Square /Rect [10 10 50 50] /C [1 0 0] /OddKey (kept) /AnotherLevel << /Deep 42 >> >>] >>",
    ];
    let mut doc = Document::open(std::sync::Arc::new(pdf(&objs))).expect("the stranger's file opens");
    add_annotation(&mut doc, &ann(0, Shape::Rectangle { rect: [100.0, 100.0, 150.0, 130.0] }, "ours"), &meta("mine")).expect("adds ours");

    for path in ALL_SAVES {
        let (_, reopened) = save_and_reopen(&doc, path);
        assert_eq!(kinds(&reopened, 0), vec!["Square", "Square"], "{path:?}: theirs first, ours second");
        let foreign = annot_at(&reopened, 0, 0, path);
        assert_eq!(str_of(&reopened, &foreign, b"OddKey").as_deref(), Some("kept"), "{path:?}: an unknown string key");
        let deep = foreign.get(b"AnotherLevel").map(|o| reopened.resolve(o)).and_then(|o| o.as_dict().cloned()).expect("an unknown dictionary");
        assert_eq!(deep.int(b"Deep"), Some(42), "{path:?}");
        assert_eq!(nums(&reopened, &foreign, b"Rect").as_deref(), Some(&[10.0, 10.0, 50.0, 50.0][..]), "{path:?}");
        assert_eq!(str_of(&reopened, &annot_at(&reopened, 0, 1, path), b"NM").as_deref(), Some("mine"), "{path:?}: our own comment is still there");
    }
}

/// A document with nothing on it round trips without acquiring stray keys: no `/Annots` at all,
/// and an empty comment list. Cheap to check, and it is the case where a writer bug would add
/// noise that no other assertion in this file would notice.
#[test]
fn an_empty_document_stays_empty() {
    let doc = blank(2);
    for path in ALL_SAVES {
        let (file, reopened) = save_and_reopen(&doc, path);
        let pages = page_object_numbers(&reopened);
        assert_eq!(pages.len(), 2, "{path:?}");
        for (page, &number) in pages.iter().enumerate() {
            assert!(annot_entries(&reopened, page).is_empty(), "{path:?}: no entries on page {}", page + 1);
            let p = reopened.get(ObjRef::new(number, 0));
            assert!(p.as_dict().is_some_and(|d| !d.contains(b"Annots")), "{path:?}: page {} has no /Annots key", page + 1);
        }
        assert!(summaries(&reopened).is_empty(), "{path:?}");
        assert!(file.starts_with(b"%PDF-"), "{path:?}");
    }
}

/// An empty `/Annots` array written by someone else and referenced indirectly: adding the first
/// comment must write through that shared object rather than replacing the page's reference with a
/// fresh array, or a viewer holding the old reference would still see no annotations.
#[test]
fn first_comment_into_an_indirect_annotss_array_writes_through_it() {
    let mut doc = blank_with_indirect_annots(2, 1);
    add_annotation(&mut doc, &ann(1, Shape::Oval { rect: [20.0, 30.0, 60.0, 70.0] }, "o"), &meta("w")).expect("adds");
    let shared = {
        let pages = page_object_numbers(&doc);
        doc.get(ObjRef::new(pages[1], 0)).as_dict().and_then(|d| d.reference(b"Annots")).expect("the skeleton's shared /Annots object")
    };
    for path in ALL_SAVES {
        let (_, reopened) = save_and_reopen(&doc, path);
        assert_eq!(kinds(&reopened, 1), vec!["Circle"], "{path:?}");
        if path == Save::Incremental {
            // The original object is still the page's /Annots on an incremental save.
            let pages = page_object_numbers(&reopened);
            let now = reopened.get(ObjRef::new(pages[1], 0)).as_dict().and_then(|d| d.reference(b"Annots"));
            assert_eq!(now, Some(shared), "{path:?}: the page still points at the same array object");
            assert_eq!(reopened.get(shared).as_array().map(|a| a.len()), Some(1), "{path:?}");
        }
    }
}

/// Text is the payload a comment exists to carry. Acrobat separates lines with CR, PDF strings may
/// hold parentheses and backslashes, and anything outside printable ASCII goes out as UTF-16BE. All
/// of it must come back identical — checked against the file's bytes, not only through our reader.
#[test]
fn awkward_comment_text_survives_to_the_bytes() {
    let cases = [
        "plain",
        "(parenthesised) \\backslash\\",
        "line one\r\nline two",
        "Annà",             // non-ASCII → UTF-16BE with a BOM
        "emoji: \u{1F642}", // a surrogate pair in UTF-16BE
        "tabs\tand\nbreaks",
    ];
    for text in cases {
        let mut doc = blank(1);
        add_annotation(&mut doc, &ann(0, Shape::Note { at: [10.0, 700.0], icon: NoteIcon::Note }, text), &meta("t")).expect("adds");
        for path in [Save::Incremental, Save::FullTable] {
            let (file, reopened) = save_and_reopen(&doc, path);
            let d = annot_at(&reopened, 0, 0, path);
            assert_eq!(str_of(&reopened, &d, b"Contents").as_deref(), Some(text), "{path:?}: {text:?} through our reader");
            let found = bytes::texts_of(&file, "Contents");
            assert!(found.contains(&text.to_string()), "{path:?}: {text:?} is in the file as a PDF string, got {found:?}");
            assert_eq!(props(&reopened, 0, 0).map(|p| p.subtype), Some("Text".into()), "{path:?}");
        }
    }
}

/// Enum names reach the file as the strings the standard defines, which is what other programs
/// read. `/H`'s one-letter codes are checked through the reader instead: a bare `/P` in raw bytes
/// is ambiguous with `/Pages`, `/Parent` and every page object's `/P`.
#[test]
fn enum_names_are_written_as_the_standard_name_strings() {
    let mut doc = blank(1);
    add_annotation(&mut doc, &ann(0, Shape::Note { at: [10.0, 700.0], icon: NoteIcon::NewParagraph }, "n"), &meta("e1")).expect("adds");
    links::add(
        &mut doc,
        0,
        [200.0, 700.0, 300.0, 715.0],
        &links::LinkAction::Uri("https://example.com".into()),
        &links::LinkStyle { highlight: links::Highlight::Inset, visible: true, ..Default::default() },
    )
    .expect("links");
    add_annotation(
        &mut doc,
        &ann(0, Shape::Line { from: [10.0, 200.0], to: [80.0, 240.0], start: LineEnding::RClosedArrow, end: LineEnding::Slash }, "l"),
        &meta("e2"),
    )
    .expect("adds line");

    for path in ALL_SAVES {
        let (file, reopened) = save_and_reopen(&doc, path);
        // The page holds the note, its pop-up, then the link and the line.
        assert_eq!(name_of(&reopened, &annot_at(&reopened, 0, 0, path), b"Name").as_deref(), Some("NewParagraph"), "{path:?}");
        assert_eq!(name_of(&reopened, &annot_at(&reopened, 0, 2, path), b"H").as_deref(), Some("P"), "{path:?}: Inset highlight is /H /P (§12.5.3)");
        assert_eq!(
            name_array(&reopened, &annot_at(&reopened, 0, 3, path), b"LE").as_deref(),
            Some(&["RClosedArrow".to_string(), "Slash".to_string()][..]),
            "{path:?}: both line endings"
        );
        assert_eq!(props(&reopened, 0, 3).and_then(|p| p.endings), Some(vec![LineEnding::RClosedArrow, LineEnding::Slash]), "{path:?}");
        if !matches!(path, Save::FullStreams) {
            assert!(bytes::has_name(&file, "NewParagraph"), "{path:?}");
            assert!(bytes::has_name(&file, "RClosedArrow") && bytes::has_name(&file, "Slash"), "{path:?}");
        }
    }
}

/// Which subtypes count as a comment is a reading rule, and it must not shift when the file is
/// rewritten: pop-ups and links stay out of `summaries`, everything else stays in.
#[test]
fn comment_and_non_comment_subtypes_stay_separated_after_saving() {
    let mut doc = blank(1);
    add_annotation(
        &mut doc,
        &ann(0, Shape::TextMarkup { kind: Markup::StrikeOut, quads: vec![[10.0, 700.0, 90.0, 700.0, 10.0, 686.0, 90.0, 686.0]] }, "strike"),
        &meta("c1"),
    )
    .expect("adds");
    add_annotation(&mut doc, &ann(0, Shape::Note { at: [10.0, 600.0], icon: NoteIcon::Comment }, "note"), &meta("c2")).expect("adds");
    links::add(&mut doc, 0, [10.0, 500.0, 90.0, 520.0], &links::LinkAction::Uri("https://example.com".into()), &links::LinkStyle::default())
        .expect("adds link");
    for path in ALL_SAVES {
        let (_, reopened) = save_and_reopen(&doc, path);
        assert_eq!(kinds(&reopened, 0), vec!["StrikeOut", "Text", "Popup", "Link"], "{path:?}");
        let mut listed: Vec<String> = summaries(&reopened).iter().map(|s| s.subtype.clone()).collect();
        listed.sort();
        assert_eq!(listed, vec!["StrikeOut".to_string(), "Text".to_string()], "{path:?}: pop-ups and links are not comments");
    }
}

/// A full save with object streams and one without must describe the same annotations: same
/// count, same subtypes, same rectangles. That makes the two writer paths interchangeable for
/// annotation data, which is what "the object graph is the model" promises.
#[test]
fn a_full_rewrite_into_object_streams_describes_the_same_comments_as_a_plain_table() {
    let mut doc = blank(2);
    add_annotation(&mut doc, &ann(0, Shape::Note { at: [10.0, 700.0], icon: NoteIcon::Paragraph }, "both paths"), &meta("cmp1")).expect("adds");
    add_annotation(&mut doc, &ann(0, Shape::Rectangle { rect: [10.0, 20.0, 90.0, 60.0] }, "box"), &meta("cmp2")).expect("adds");
    add_annotation(
        &mut doc,
        &ann(1, Shape::TextMarkup { kind: Markup::Underline, quads: vec![[50.0, 600.0, 150.0, 600.0, 50.0, 586.0, 150.0, 586.0]] }, "u"),
        &meta("cmp3"),
    )
    .expect("adds");

    let (_, plain) = save_and_reopen(&doc, Save::FullTable);
    let (_, packed) = save_and_reopen(&doc, Save::FullStreams);
    let shape_of = |d: &Document| -> Vec<(usize, Vec<String>, Vec<[f64; 4]>)> {
        (0..page_object_numbers(d).len())
            .map(|p| {
                let e = annot_entries(d, p);
                (
                    e.len(),
                    kinds(d, p),
                    e.iter()
                        .filter_map(|x| dict_of(d, x))
                        .filter_map(|x| nums(d, &x, b"Rect"))
                        .filter_map(|v| <[f64; 4]>::try_from(v).ok())
                        .collect(),
                )
            })
            .filter(|t: &(usize, Vec<String>, Vec<[f64; 4]>)| t.0 > 0)
            .collect()
    };
    assert_eq!(shape_of(&plain), shape_of(&packed), "the two full-save paths must agree");
    assert_eq!(shape_of(&plain).len(), 2, "and both must find the two pages that have comments");
    assert_eq!(summaries(&plain).len(), summaries(&packed).len());

    // The plain-table output is bigger here: object streams compress, which is the point of them.
    let opts = SaveOptions { mod_date: None, id_seed: 0x5052_494E_5443_5241, object_streams: true };
    let packed_bytes = pdfcraft_cos::write_full(&doc, &opts).expect("writes");
    assert!(
        packed_bytes.len() < pdfcraft_cos::write_full(&doc, &SaveOptions { object_streams: false, ..opts }).expect("writes").len(),
        "the packed file should be smaller"
    );
}

// ── measured clamps, pinned so a change is visible (§12.5.3 border width) ──────────────────

/// Negative border width — CURRENT behaviour, measured on this branch, not an endorsement.
///
/// `add_annotation` screens width for NON-FINITENESS only. The guard at
/// `crates/annot/src/lib.rs:1018` reads
/// `!finite(&style.color) || !style.opacity.is_finite() || !style.width.is_finite()`: the
/// `abs() < 1e7` cap belongs to `finite()` (`:827-829`) and is applied to `color` alone, so width
/// carries no magnitude limit at all. Measured on this branch: `width = 1e9` and `width = 1e299`
/// are both ACCEPTED and written to `/BS /W` verbatim (a 10-digit number, then a 300-digit one),
/// while `color = [1e9, 0, 0]` is refused with `Invalid("invalid style")`.
/// So `width = -4.0` is ACCEPTED too; the value reaching `/BS /W` comes from `style.width.max(0.0)`
/// (`:1030`) and the `/Rect` growth from `style.width.max(0.0) / 2.0` (`:857`). The file then holds
/// `0`, and neither document's repair log records it. (`finite()` magnitude-screens `color`;
/// `opacity` and `width` get finiteness checks only, and the interior `fill` is not screened by it.)
///
/// The edit path behaves identically for width: `set_style` applies the same kind of screen
/// (`:1896`) and `width = Some(-2.5)` is accepted, storing `0`. There is therefore NO
/// create-vs-edit asymmetry for negative widths — unlike a huge or non-finite fill colour, which
/// creation never screens (`:1018` magnitude-screens `color`, finiteness-checks `opacity` and
/// `width`, and takes no look at `fill`, which is merely clamped component-wise by `rgb()` at
/// `:823-825`) while the edit path refuses the same
/// value explicitly near `:2010`. Flagged for maintainers rather than asserted as
/// correct: a refusal, or at minimum a repair-log entry naming the substitution, would be the
/// stricter design. If either clamp changes or goes away, this test fails on purpose so that
/// decision is made in the open.
#[test]
fn negative_border_width_is_accepted_and_stored_as_zero() {
    for (label, width) in [("minus-four", -4.0f64), ("minus-half", -0.5)] {
        let mut doc = blank(1);
        add_annotation(&mut doc, &styled(0, Shape::Oval { rect: [5.0, 5.0, 65.0, 35.0] }, "w", [1.0, 0.0, 0.0], 1.0, width, None), &meta("neg"))
            .unwrap_or_else(|e| panic!("{label}: negative width {width} should be accepted today, got {e:?}"));
        for path in ALL_SAVES {
            let (file, reopened) = save_and_reopen(&doc, path);
            let d = annot_at(&reopened, 0, 0, path);
            let w_stored = d.get(b"BS").map(|o| reopened.resolve(o)).and_then(|o| o.as_dict().cloned()).and_then(|bs| num(&reopened, &bs, b"W"));
            assert_eq!(w_stored, Some(0.0), "{label}: {path:?} stores the clamped zero, not {width}");
            assert_eq!(props(&reopened, 0, 0).and_then(|p| p.width), Some(0.0), "{label}: {path:?} reports the clamped zero");
            assert!(!file.windows(4).any(|x| x == b"-4.0" || x == b"-0.5"), "{label}: {path:?} must not write the negative literal it was given");
            assert!(doc.repair_log().is_empty() && reopened.repair_log().is_empty(), "{label}: {path:?} records nothing about the substitution");
        }
    }

    let mut doc = blank(1);
    add_annotation(&mut doc, &styled(0, Shape::Oval { rect: [5.0, 5.0, 65.0, 35.0] }, "w", [1.0, 0.0, 0.0], 1.0, 2.0, None), &meta("neg2"))
        .expect("adds");
    set_style(&mut doc, 0, 0, None, None, Some(-2.5), None, &meta("neg2")).expect("the edit path screens non-finite and magnitude, not sign");
    let (file, reopened) = save_and_reopen(&doc, Save::FullTable);
    assert_eq!(props(&reopened, 0, 0).and_then(|p| p.width), Some(0.0), "an edited negative width is stored as zero too");
    assert!(!file.windows(4).any(|x| x == b"-2.5"), "the negative literal never reaches the file");
    assert!(doc.repair_log().is_empty() && reopened.repair_log().is_empty(), "silently, with nothing recorded");
}

/// The `/Rect` half of the same guard (`lib.rs:857`). Measured on this branch: `Rectangle` and
/// `Oval` store the given `/Rect` verbatim, so they cannot detect this guard at all — a first
/// version of this test compared rectangles and survived the mutation, which is exactly how an
/// empty test looks green. `Shape::Line` IS padded by it (`:897`, `pad = half + arrow`), so the
/// comparison is made there: with `-9pt` and `0pt` borders a line's `/Rect` must grow identically,
/// because `max(0.0)` collapses both before the padding is applied.
#[test]
fn a_negative_and_a_zero_border_pad_a_line_identically() {
    let line = |w: f64| {
        let mut doc = blank(1);
        add_annotation(
            &mut doc,
            &styled(
                0,
                Shape::Line { from: [20.0, 700.0], to: [120.0, 740.0], start: LineEnding::None, end: LineEnding::None },
                "l",
                [0.0, 0.0, 0.0],
                1.0,
                w,
                None,
            ),
            &meta("ln"),
        )
        .expect("adds a line");
        save_and_reopen(&doc, Save::FullTable).0
    };
    let minus = line(-9.0);
    let zero = line(0.0);
    let rect_minus = bytes::numbers_of(&minus, "Rect");
    let rect_zero = bytes::numbers_of(&zero, "Rect");
    assert!(!rect_minus.is_empty(), "the -9pt file must contain a /Rect array of numbers");
    assert!(!rect_zero.is_empty(), "the 0pt file must contain a /Rect array of numbers");
    assert_eq!(rect_minus, rect_zero, "-9pt and 0pt borders must pad a line's /Rect identically — max(0.0) at :857 is what guarantees it");
    // `/BS /W` is a SCALAR real, never an array, so `numbers_of` (which only accepts `[...]`)
    // compared two empty lists here and passed whatever the file contained. `scalars_of` is the
    // right helper, and both sides are guarded non-empty so this can never go green on missing data.
    let w_minus = bytes::scalars_of(&minus, "W");
    let w_zero = bytes::scalars_of(&zero, "W");
    assert!(!w_minus.is_empty() && !w_zero.is_empty(), "both files must actually write a /BS /W, got {w_minus:?} vs {w_zero:?}");
    assert_eq!(w_minus, w_zero, "and both store the same /BS /W");
    assert_eq!(w_minus.as_slice(), &[0.0], "the clamp at lib.rs:857 stores a negative border width as 0");
}
