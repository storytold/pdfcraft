//! Property tests for annotation structural fidelity, stated over the **saved bytes**.
//!
//! What is proven here is a rule about files, not about structs: for every generated comment the
//! writer produces bytes that (a) hold the geometry as fixed-decimal number arrays, and (b) parse
//! back — with our reader *and* with vendored hayro-syntax — into dictionaries whose numbers,
//! names, flags and opacity equal values computed in this file from the input plus ISO 32000-2.
//! Two things make that more than "compare an object to itself": the expectations never read the
//! pre-save document, and the number tokens are re-parsed from raw `Vec<u8>` by `support::bytes`,
//! a strict scanner that is not `pdfcraft-cos`.
//!
//! Geometry comes from a half-point grid (`x = k / 2`, |k| ≤ 450). Every such value has an exact
//! decimal form and survives Flate, `f64` arithmetic and the writer's shortest-round-trip
//! formatting, so a mismatch means data changed shape on disk rather than drifting by an ulp.
//! Text is generated from an alphabet spanning ASCII punctuation, Latin-1 letters and CJK — the
//! cases where PDF's two string encodings disagree.
//!
//! Not covered, deliberately: appearance-stream content (the five open appearance PRs named in
//! `tests/round_trip.rs`), and the comment-list ordering rule of `summaries`, which is
//! deterministic and checked there instead.

mod support {
    pub mod bytes;
    pub mod fixture;
}

use pdfcraft_annot::{LineEnding, Markup, Meta, NewAnnotation, NoteIcon, Shape, Style, add_annotation, props, summaries};
use pdfcraft_cos::Document;
use proptest::prelude::*;

use support::bytes;
use support::fixture::{
    FLAG_NO_ROTATE, FLAG_NO_ZOOM, FLAG_PRINT, Save, annot_entries, blank, dict_of, name_of, num, nums, page_object_numbers, save_and_reopen, str_of,
};

/// proptest's assert macros cannot capture local names inside their format strings (the message is
/// built in proptest's own expansion scope), so every message here passes what it names explicitly.
fn tag(path: Save) -> String {
    format!("{path:?}")
}

/// A coordinate on the half-point grid.
fn coord() -> impl Strategy<Value = f64> {
    any::<i32>().prop_map(|k| f64::from(k.rem_euclid(901) - 450) / 2.0)
}

/// A rectangle as four grid coordinates, corners in any order (the writer normalises).
fn rect4() -> impl Strategy<Value = [f64; 4]> {
    (coord(), coord(), coord(), coord()).prop_map(|(x0, y0, x1, y1)| [x0, y0, x1.max(x0 + 1.0), y1.max(y0 + 1.0)])
}

/// A quadrilateral: eight grid numbers, in Acrobat's corner order (§12.5.6.10).
fn quad8() -> impl Strategy<Value = [f64; 8]> {
    prop::array::uniform8(coord())
}

/// Text from an alphabet where PDF's two string encodings disagree: printable ASCII (including the
/// punctuation a literal string has to escape around), Latin-1 letters beyond 0x7F, and CJK.
const TEXT_CHARS: &str = "abZq07 .,:;!?-_%$#*+=~()[]<>&@^|\n\t\u{e9}\u{fc}\u{f1}\u{df}\u{a7}\u{c0}\u{f6}\u{301}\u{65e5}\u{672c}";

fn text_of(max_len: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(TEXT_CHARS.chars().collect::<Vec<char>>()), 0..=max_len).prop_map(|c| c.into_iter().collect())
}

/// ASCII-only identity strings: `/NM` is a thread key other readers match literally, so keep it
/// free of characters that would make a byte-level comparison ambiguous.
fn id_of(max_len: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._".chars().collect::<Vec<char>>()),
        0..=max_len,
    )
    .prop_map(|c| c.into_iter().collect())
}

/// A grid coordinate's PDF decimal form, computed here rather than by the writer: integers without
/// a fraction, half-points with one digit (§7.3.3).
fn token(v: f64) -> String {
    if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{v:.1}") }
}

fn rect_token(r: &[f64]) -> String {
    format!("/Rect [{}]", r.iter().map(|v| token(*v)).collect::<Vec<_>>().join(" "))
}

/// Normalise as §12.5.4.2 requires a rectangle to be stored: lower-left then upper-right.
fn norm(r: [f64; 4]) -> Vec<f64> {
    vec![r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]
}

/// Bounds of a point list, computed here rather than by the crate.
fn bounds_of(points: &[[f64; 2]]) -> Vec<f64> {
    let mut out = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for p in points {
        out[0] = out[0].min(p[0]);
        out[1] = out[1].min(p[1]);
        out[2] = out[2].max(p[0]);
        out[3] = out[3].max(p[1]);
    }
    out.to_vec()
}

fn meta(id: String) -> Meta {
    Meta { date: Some("D:20261001120000Z".into()), id }
}

fn comment(page: usize, shape: Shape, style: Style, contents: String, author: String) -> NewAnnotation {
    NewAnnotation { page, shape, style, contents, author }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// A rectangle comment's `/Rect` reaches the file as four fixed-decimal numbers equal to the
    /// normalised input, and parses back to exactly those numbers on all three writer paths. The
    /// raw-token claim runs where the bytes are uncompressed, so it is a claim about the file.
    #[test]
    fn a_rectangles_stored_rectangle_is_the_input_normalised(rect in rect4(), id in id_of(12)) {
        for path in [Save::Incremental, Save::FullTable, Save::FullStreams] {
            let mut doc = blank(1);
            add_annotation(&mut doc, &comment(0, Shape::Rectangle { rect }, Style::default(), String::new(), "Ada".into()), &meta(id.clone()))
                .expect("a normalised box is at least a point wide");
            let (file, reopened) = save_and_reopen(&doc, path);
            let want = norm(rect);
            let entries = annot_entries(&reopened, 0);
            prop_assert_eq!(entries.len(), 1, "{}: exactly the one comment", tag(path));
            let d = dict_of(&reopened, &entries[0]).expect("one annotation");
            let stored = nums(&reopened, &d, b"Rect");
            prop_assert_eq!(stored.as_deref(), Some(want.as_slice()), "{}: /Rect must be the normalised input {:?}", tag(path), rect);
            if path != Save::FullStreams {
                let t = rect_token(&want);
                prop_assert!(file.windows(t.len()).any(|w| w == t.as_bytes()), "{}: {:?} must be written exactly as {}", tag(path), rect, t);
                prop_assert!(bytes::numbers_of(&file, "Rect").contains(&want), "{}: the file itself must hold {:?}", tag(path), want);
            }
        }
    }

    /// Markup keeps every quadrilateral, in order: `/QuadPoints` holds all 8·n numbers and `/Rect`
    /// is the bounds of all of them, not just the first quad's.
    #[test]
    fn markup_keeps_every_quadrilateral(quads in prop::collection::vec(quad8(), 1..=3)) {
        let flat: Vec<f64> = quads.concat();
        let points: Vec<[f64; 2]> = quads.iter().flat_map(|q| q.as_chunks::<2>().0.iter().copied()).collect();
        let want_rect = bounds_of(&points);
        for path in [Save::Incremental, Save::FullTable, Save::FullStreams] {
            let mut doc = blank(1);
            add_annotation(&mut doc, &comment(0, Shape::TextMarkup { kind: Markup::Highlight, quads: quads.clone() }, Style::default(), String::new(), "Ada".into()), &meta("q".into()))
                .expect("adds");
            let (file, reopened) = save_and_reopen(&doc, path);
            let entries = annot_entries(&reopened, 0);
            prop_assert_eq!(entries.len(), 1, "{}: exactly the one comment", tag(path));
            let d = dict_of(&reopened, &entries[0]).expect("one annotation");
            let stored = nums(&reopened, &d, b"QuadPoints");
            prop_assert_eq!(stored.as_deref(), Some(flat.as_slice()), "{}: all {} quad numbers, in order", tag(path), flat.len());
            let stored_rect = nums(&reopened, &d, b"Rect");
            prop_assert_eq!(stored_rect.as_deref(), Some(want_rect.as_slice()), "{}: /Rect bounds every quad", tag(path));
            prop_assert_eq!(summaries(&reopened).first().map(|s| s.quads.len()), Some(quads.len()), "{}: the comment list chunks them the same way", tag(path));
            if path != Save::FullStreams {
                prop_assert!(bytes::numbers_of(&file, "QuadPoints").contains(&flat), "{}: {} numbers must be in the file", tag(path), flat.len());
            }
        }
    }

    /// `/CA` obeys its rule whatever the caller asked for: written only when clamped opacity is
    /// below 1, and equal to the clamped value. A non-finite opacity is refused before any object
    /// is created, so the file never holds a number PDF cannot write.
    #[test]
    fn opacity_and_flags_follow_their_rules(opacity in any::<f64>()) {
        let mut doc = blank(1);
        let added = add_annotation(&mut doc, &comment(0, Shape::Rectangle { rect: [10.0, 10.0, 60.0, 40.0] }, Style { color: [0.5, 0.25, 0.75], opacity, width: 1.0, fill: None }, String::new(), "Ada".into()), &meta("c".into()));
        if !opacity.is_finite() {
            prop_assert!(added.is_err(), "a non-finite opacity ({:?}) must be refused, not written", opacity);
            return Ok(());
        }
        let _ = added.expect("adds");
        let want = opacity.clamp(0.0, 1.0);
        for path in [Save::Incremental, Save::FullTable, Save::FullStreams] {
            let (file, reopened) = save_and_reopen(&doc, path);
            let entries = annot_entries(&reopened, 0);
            prop_assert_eq!(entries.len(), 1, "{}: exactly the one comment", tag(path));
            let d = dict_of(&reopened, &entries[0]).expect("one annotation");
            if want < 1.0 {
                prop_assert_eq!(num(&reopened, &d, b"CA"), Some(want), "{}: opacity {:?} clamps to {:?}", tag(path), opacity, want);
                prop_assert_eq!(props(&reopened, 0, 0).map(|p| p.opacity), Some(want), "{}: reported opacity", tag(path));
                if path != Save::FullStreams {
                    prop_assert_eq!(bytes::scalars_of(&file, "CA").first().copied(), Some(want), "{}: /CA must be in the file as {}", tag(path), token(want));
                }
            } else {
                prop_assert!(d.get(b"CA").is_none(), "{}: full opacity writes no /CA (it is the default)", tag(path));
            }
            let flags = d.int(b"F").unwrap_or_default();
            prop_assert_ne!(flags & FLAG_PRINT, 0, "{}: a comment keeps its Print bit (§12.5.3)", tag(path));
            let colour = nums(&reopened, &d, b"C");
            prop_assert_eq!(colour.as_deref(), Some(&[0.5, 0.25, 0.75][..]), "{}: colours pass through unchanged", tag(path));
        }
    }

    /// Text, author and id survive exactly — including the empty string. `/Contents` is stored even
    /// when empty (an empty PDF string), while `/T` and `/NM` follow the crate's emptiness rule:
    /// no author or id means the key is not written at all.
    #[test]
    fn text_author_and_id_survive_exactly(contents in text_of(40), author in text_of(16), id in id_of(12)) {
        for path in [Save::Incremental, Save::FullTable] {
            let mut doc = blank(1);
            add_annotation(&mut doc, &comment(0, Shape::Rectangle { rect: [10.0, 20.0, 70.0, 45.0] }, Style::default(), contents.clone(), author.clone()), &meta(id.clone()))
                .expect("adds");
            let (file, reopened) = save_and_reopen(&doc, path);
            let entries = annot_entries(&reopened, 0);
            prop_assert_eq!(entries.len(), 1, "{}: exactly the one comment", tag(path));
            let d = dict_of(&reopened, &entries[0]).expect("one annotation");
            let storedcontents = str_of(&reopened, &d, b"Contents");
            prop_assert_eq!(storedcontents.as_deref(), Some(contents.as_str()), "{}: /Contents round trip for {:?}", tag(path), contents);
            let storedid = str_of(&reopened, &d, b"NM");
            if id.is_empty() {
                prop_assert!(storedid.is_none(), "{}: an empty id writes no /NM, got {storedid:?}", tag(path));
            } else {
                prop_assert_eq!(storedid.as_deref(), Some(id.as_str()), "{}: /NM round trip", tag(path));
            }
            prop_assert!(bytes::texts_of(&file, "Contents").contains(&contents), "{:?} must appear in the file as a PDF string", contents);
            if id.is_empty() {
                prop_assert!(bytes::texts_of(&file, "NM").is_empty(), "{}: no /NM in the file for an empty id", tag(path));
            } else {
                prop_assert!(bytes::texts_of(&file, "NM").contains(&id), "{:?} must appear in the file as a PDF string", id);
            }
            if author.is_empty() {
                prop_assert!(d.get(b"T").is_none(), "{}: an empty author writes no /T", tag(path));
            } else {
                let storedauthor = str_of(&reopened, &d, b"T");
            prop_assert_eq!(storedauthor.as_deref(), Some(author.as_str()), "{}: /T round trip for {:?}", tag(path), author);
            }
        }
    }

    /// Several comments on one page keep their count, their order and all of their rectangles —
    /// including the raw-file claim that the list of `/Rect` arrays is exactly what was added.
    #[test]
    fn several_rectangles_keep_count_order_and_geometry(rects in prop::collection::vec(rect4(), 1..=6), opacity_bits in any::<u8>()) {
        let mut doc = blank(1);
        let mut want_rects = Vec::new();
        for (i, r) in rects.iter().enumerate() {
            // Two bits of the byte give opacities 1.0, 0.5, -0.5 and 0.0: written, clamped or absent.
            let opacity = match opacity_bits & 3 {
                0 => 1.0,
                1 => 0.5,
                2 => -0.5,
                _ => 0.0,
            };
            add_annotation(&mut doc, &comment(0, Shape::Rectangle { rect: *r }, Style { color: [0.9, 0.1, 0.1], opacity, width: 2.0, fill: None }, format!("c{i}"), "Ada".into()), &meta(format!("k{i}")))
                .expect("adds");
            want_rects.push(norm(*r));
        }
        for path in [Save::Incremental, Save::FullTable, Save::FullStreams] {
            let (file, reopened) = save_and_reopen(&doc, path);
            let want = want_rects.clone();
            let entries = annot_entries(&reopened, 0);
            prop_assert_eq!(entries.len(), want.len(), "{}: one object per comment", tag(path));
            let got: Vec<Vec<f64>> = entries.iter().filter_map(|e| dict_of(&reopened, e)).flat_map(|d| nums(&reopened, &d, b"Rect")).collect();
            prop_assert_eq!(got, want.clone(), "{}: the same rectangles in the same order", tag(path));
            prop_assert_eq!(summaries(&reopened).len(), want.len(), "{}: every comment is listed", tag(path));
            if path != Save::FullStreams {
                prop_assert_eq!(bytes::numbers_of(&file, "Rect"), want, "{}: every rectangle is in the file", tag(path));
            }
        }
    }

    /// Sticky notes arrive with a pop-up partner, and the pairing is two references: so that means
    /// they still point at each other after a save, that the count is always two per note, and
    /// that the icon box stays 20 pt square at the position asked for.
    #[test]
    fn notes_always_bring_a_popup_that_points_back(at in prop::array::uniform2(coord()), n in 1usize..=3) {
        let at = [at[0], at[1].max(25.0)];
        let mut doc = blank(n);
        for p in 0..n {
            add_annotation(&mut doc, &comment(p, Shape::Note { at, icon: NoteIcon::Comment }, Style::default(), String::new(), "Ada".into()), &meta(format!("n{p}")))
                .expect("adds");
        }
        for path in [Save::Incremental, Save::FullTable, Save::FullStreams] {
            let (_, reopened) = save_and_reopen(&doc, path);
            let pages = page_object_numbers(&reopened);
            prop_assert_eq!(pages.len(), n, "{}", tag(path));
            for p in 0..n {
                let entries = annot_entries(&reopened, p);
                prop_assert_eq!(entries.len(), 2, "{}: note and pop-up on page {}", tag(path), p);
                let note = dict_of(&reopened, &entries[0]).expect("note dictionary");
                let pop = dict_of(&reopened, &entries[1]).expect("pop-up dictionary");
                let notekind = name_of(&reopened, &note, b"Subtype");
            prop_assert_eq!(notekind.as_deref(), Some("Text"), "{}", tag(path));
                let popupkind = name_of(&reopened, &pop, b"Subtype");
            prop_assert_eq!(popupkind.as_deref(), Some("Popup"), "{}", tag(path));
                prop_assert_eq!(pop.reference(b"Parent"), entries[0].as_ref(), "{}: /Parent back-reference", tag(path));
                prop_assert_eq!(note.reference(b"Popup"), entries[1].as_ref(), "{}: /Popup forward reference", tag(path));
                let iconbox = nums(&reopened, &note, b"Rect");
            prop_assert_eq!(iconbox.as_deref(), Some(&[at[0], at[1] - 20.0, at[0] + 20.0, at[1]][..]), "{}: the icon box for {:?}", tag(path), at);
                let flags = note.int(b"F").unwrap_or_default();
                prop_assert_ne!(flags & (FLAG_NO_ZOOM | FLAG_NO_ROTATE), 0, "{}: a note icon neither scales nor rotates", tag(path));
            }
        }
    }

    /// An incremental save and a garbage-collecting full rewrite must describe the same comments.
    /// Their bytes differ by design; what may not differ is what a reader learns from them.
    #[test]
    fn every_writer_path_reports_the_same_comments(rect in rect4(), contents in text_of(20), id in id_of(8)) {
        let mut doc = blank(2);
        let quad = [rect[0], rect[3], rect[2], rect[3], rect[0], rect[1], rect[2], rect[1]];
        add_annotation(&mut doc, &comment(1, Shape::TextMarkup { kind: Markup::Underline, quads: vec![quad] }, Style::default(), contents, "Ada".into()), &meta(id))
            .expect("adds");

        let (_, inc) = save_and_reopen(&doc, Save::Incremental);
        let (_, full) = save_and_reopen(&doc, Save::FullTable);
        let (_, packed) = save_and_reopen(&doc, Save::FullStreams);
        let describe = |d: &Document| -> (Vec<String>, Vec<Vec<f64>>, Option<String>) {
            let dicts: Vec<_> = annot_entries(d, 1).iter().filter_map(|x| dict_of(d, x)).collect();
            (
                dicts.iter().map(|x| name_of(d, x, b"Subtype").unwrap_or_default()).collect(),
                dicts.iter().flat_map(|x| nums(d, x, b"QuadPoints")).collect(),
                dicts.first().and_then(|x| str_of(d, x, b"Contents")),
            )
        };
        prop_assert_eq!(describe(&inc), describe(&full));
        prop_assert_eq!(describe(&full), describe(&packed));
    }

    /// Border width and interior colour reach the file as given, and a `None` fill leaves no `/IC`
    /// behind — "no fill" is stored as absence, not as black.
    ///
    /// The range includes NEGATIVE widths on purpose: `add_annotation` screens width for
    /// non-finiteness only (`crates/annot/src/lib.rs:1018` tests `style.width.is_finite()`; the
    /// `abs() < 1e7` cap inside `finite()` at `:827-829` guards `color`, never width), so
    /// negatives are accepted and then clamped by `max(0.0)` on the way to `/BS /W` (`:1030`).
    /// Sampling `0.0..` alone left both clamps untested while this test's name implied coverage.
    /// The upper end stays modest deliberately: magnitude is not screened here either, so a huge
    /// width would be written verbatim — that case is pinned in `malformed.rs`, not by this
    /// property, whose job is the sign clamp and the fill/absence rules.
    #[test]
    fn width_and_fill_are_written_as_they_were_given(width in -4.0f64..12.0, fill in prop::option::of(prop::array::uniform3(0.0f64..1.0))) {
        let mut doc = blank(1);
        add_annotation(&mut doc, &comment(0, Shape::Oval { rect: [5.0, 5.0, 65.0, 35.0] }, Style { color: [1.0, 0.0, 0.0], opacity: 1.0, width, fill }, String::new(), "Ada".into()), &meta("f".into()))
            .expect("adds");
        for path in [Save::Incremental, Save::FullTable] {
            let (file, reopened) = save_and_reopen(&doc, path);
            let entries = annot_entries(&reopened, 0);
            prop_assert_eq!(entries.len(), 1, "{}: exactly the one comment", tag(path));
            let d = dict_of(&reopened, &entries[0]).expect("one annotation");
            let bs = d.get(b"BS").map(|o| reopened.resolve(o)).and_then(|o| o.as_dict().cloned()).expect("/BS");
            prop_assert_eq!(num(&reopened, &bs, b"W"), Some(width.max(0.0)), "{}: /BS /W for width {:?}", tag(path), width);
            prop_assert_eq!(d.contains(b"IC"), fill.is_some(), "{}: /IC is present exactly when a fill was given", tag(path));
            prop_assert_eq!(props(&reopened, 0, 0).and_then(|p| p.width), Some(width.max(0.0)), "{}: reported width {:?}", tag(path), width);
            prop_assert_eq!(props(&reopened, 0, 0).and_then(|p| p.fill), fill, "{}: /IC for {:?}", tag(path), fill);
            let ic = bytes::numbers_of(&file, "IC");
            match fill {
                Some(c) => prop_assert_eq!(ic.first(), Some(&vec![c[0], c[1], c[2]]), "{}: /IC must be in the file", tag(path)),
                None => prop_assert!(ic.is_empty(), "{}: no /IC when unfilled, got {:?}", tag(path), ic),
            }
        }
    }

    /// A line keeps its endpoints verbatim in `/L`, and the annotation's rectangle always spans
    /// both of them — the one geometry rule a line must obey however wide its stroke or arrowheads
    /// are drawn (§12.5.4.2). The padding itself lives in `appearance`, so it is not pinned here.
    #[test]
    fn a_line_keeps_its_endpoints_and_its_rectangle_spans_them(from in prop::array::uniform2(coord()), dx in 1.0f64..300.0, dy in -300.0f64..300.0, width in 0.5f64..8.0) {
        let to = [(from[0] + dx).min(450.0), from[1] + dy];
        let mut doc = blank(1);
        add_annotation(&mut doc, &comment(0, Shape::Line { from, to, start: LineEnding::None, end: LineEnding::OpenArrow }, Style { color: [0.0, 0.0, 0.0], opacity: 1.0, width, fill: None }, String::new(), "Ada".into()), &meta("l".into()))
            .expect("adds");
        let span = bounds_of(&[from, to]);
        for path in [Save::Incremental, Save::FullTable, Save::FullStreams] {
            let (file, reopened) = save_and_reopen(&doc, path);
            let entries = annot_entries(&reopened, 0);
            prop_assert_eq!(entries.len(), 1, "{}: exactly the one comment", tag(path));
            let d = dict_of(&reopened, &entries[0]).expect("one annotation");
            let r = nums(&reopened, &d, b"Rect").expect("/Rect");
            prop_assert!(r[0] <= span[0] && r[1] <= span[1] && r[2] >= span[2] && r[3] >= span[3], "{}: line {:?} to {:?} must lie inside {:?}", tag(path), from, to, r);
            let storedline = nums(&reopened, &d, b"L");
            prop_assert_eq!(storedline.as_deref(), Some(&[from[0], from[1], to[0], to[1]][..]), "{}: /L verbatim", tag(path));
            if path != Save::FullStreams {
                prop_assert!(bytes::numbers_of(&file, "L").contains(&vec![from[0], from[1], to[0], to[1]]), "{}: /L must be in the file", tag(path));
            }
        }
    }
}

/// A comment is addressed as `(page, index)`, and `lib.rs` documents `index` as its position in
/// that page's `/Annots`. Fixed rather than generated, because the interesting part is not the
/// arithmetic but the interaction: a note adds its pop-up to the same array, so an index claimed
/// before a save has to still name the same comment afterwards.
#[test]
fn indices_claimed_before_a_save_still_name_the_same_comment_after_it() {
    let mut doc = blank(1);
    let first = add_annotation(
        &mut doc,
        &comment(0, Shape::Rectangle { rect: [10.0, 700.0, 90.0, 730.0] }, Style::default(), "one".into(), "A".into()),
        &meta("i1".into()),
    )
    .expect("adds");
    let second = add_annotation(
        &mut doc,
        &comment(0, Shape::Note { at: [10.0, 600.0], icon: NoteIcon::Note }, Style::default(), "two".into(), "A".into()),
        &meta("i2".into()),
    )
    .expect("adds");
    let third = add_annotation(
        &mut doc,
        &comment(0, Shape::Rectangle { rect: [10.0, 400.0, 90.0, 430.0] }, Style::default(), "three".into(), "A".into()),
        &meta("i3".into()),
    )
    .expect("adds");
    assert_eq!((first, second, third), (0, 1, 3), "the pop-up takes index 2");

    for path in [Save::Incremental, Save::FullTable, Save::FullStreams] {
        let (_, reopened) = save_and_reopen(&doc, path);
        for (idx, want_id) in [(first, "i1"), (second, "i2"), (third, "i3")] {
            let entries = annot_entries(&reopened, 0);
            let d = dict_of(&reopened, &entries[idx]).unwrap_or_else(|| panic!("{}: index {idx} exists", tag(path)));
            assert_eq!(str_of(&reopened, &d, b"NM").as_deref(), Some(want_id), "{}: index {idx} is still the same comment", tag(path));
        }
    }
}
