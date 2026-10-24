//! Damaged annotation dictionaries: what the reader does, and what it must never do.
//!
//! **These are never-crash tests.** They assert that a stranger's broken annotation data produces an
//! error or a defined degradation instead of a panic. Where the current behaviour is questionable,
//! that is said out loud and pinned as it is: an expected value below is *what PdfCraft does today*,
//! not a claim that it is right. Each such place carries a `NOTE` comment naming the alternative a
//! reader could reasonably choose, so a later fix shows up as a changed expectation here rather than a
//! silent edit. (`AGENTS.md` §4 and `CLAUDE.md` Fidelity: no crash, and never repair silently.)
//!
//! The shapes come from two directions: bytes written by hand (a file we did not make, including
//! tokens our typed API cannot even express — a three-number quadrilateral or an unknown keyword), and
//! values handed to the builder (`add_annotation` with NaN in a style or geometry).

mod support {
    pub mod bytes;
    pub mod fixture;
}

use pdfcraft_annot::{
    AnnotError, AttachIcon, Meta, NewAnnotation, NoteIcon, Shape, Style, add_annotation, delete_annotation, has_visible_fill_sign, links, page_refs,
    picture_rotation, props, signature_image, summaries,
};
use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use proptest::prelude::*;
use std::sync::Arc;

use support::bytes;
use support::fixture::{Save, annot_entries, blank, dict_of, name_of, nums, page_object_numbers, pdf, save_and_reopen};

/// Open a hand-written file that must parse well enough to be examined.
fn open(objs: &[&[u8]]) -> Document {
    Document::open(Arc::new(pdf(objs))).expect("a damaged file still opens")
}

/// A one-page file whose single annotation is `body`.
fn one_annot(body: &[u8]) -> Document {
    open(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [4 0 R] >>",
        body,
    ])
}

/// Run every annotation reader over a document. The point is that none of them panics; the counts
/// are checked by the callers, which know what the damage was.
fn read_everything(doc: &Document) {
    let listed = summaries(doc);
    let pages = page_refs(doc).map(|p| p.len()).unwrap_or(0);
    for page in 0..pages {
        let entries = annot_entries(doc, page);
        for index in 0..entries.len() + 2 {
            let _ = props(doc, page, index);
        }
    }
    let _ = links::list(doc);
    let _ = has_visible_fill_sign(doc);
    for page in 0..pages {
        let entries = annot_entries(doc, page);
        for index in 0..entries.len() {
            let _ = signature_image(doc, page, index);
            let _ = picture_rotation(doc, page, index);
        }
    }
    let _ = listed;
}

// ── wrong-shaped numbers ───────────────────────────────────────────────────────────────────────

/// A `/Rect` that is not four numbers cannot place a comment. Today it reads as the empty rectangle
/// at the origin and the comment is still listed, so a viewer would draw it in the page corner.
/// NOTE: refusing the entry, or listing it at no position, are both defensible; pinning whichever
/// is chosen keeps the change honest. The invariant held here is "listed, with a finite rect, no
/// panic" — not that the position is meaningful.
#[test]
fn a_malformed_rectangle_reads_as_the_origin_instead_of_panicking() {
    let bodies: Vec<&[u8]> = vec![
        b"<< /Type /Annot /Subtype /Square /Rect [10 20] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [1 2 3] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [1 2 3 4 5] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [/a /b /c /d] >>",
        b"<< /Type /Annot /Subtype /Square /Rect (a string) >>",
        b"<< /Type /Annot /Subtype /Square /Rect null >>",
        // Colour components outside [0,1]: clamped on read rather than refused.
        b"<< /Type /Annot /Subtype /Square /Rect [10 20 30 40] /IC [0.5 2 -0.5] >>",
    ];
    for (n, body) in bodies.into_iter().enumerate() {
        let doc = one_annot(body);
        read_everything(&doc);
        let listed = summaries(&doc);
        assert_eq!(listed.len(), 1, "case {n}: the comment is still listed ({body:?})");
        assert!(listed[0].rect.iter().all(|v| v.is_finite()), "case {n}: a finite rectangle, got {:?}", listed[0].rect);
        assert_eq!(
            name_of(&doc, &dict_of(&doc, &annot_entries(&doc, 0)[0]).expect("a dictionary"), b"Subtype").as_deref(),
            Some("Square"),
            "case {n}"
        );
        // Save it on and the damage must not turn into a crash or a file we cannot reopen.
        for path in [Save::Incremental, Save::FullTable] {
            let (_, reopened) = save_and_reopen(&doc, path);
            read_everything(&reopened);
            assert_eq!(summaries(&reopened).len(), 1, "case {n} after {path:?}");
        }
    }
}

/// `/QuadPoints` must come in eights (§12.5.6.10). A list that does not divide evenly is truncated
/// to whole quadrilaterals: seven numbers give none, twelve give one, and the leftover numbers are
/// dropped without a word. NOTE: "drop silently" is what happens today; reporting the odd tail would
/// be kinder. What is asserted here is that the chunking never over-reads and never panics.
#[test]
fn a_quad_list_that_is_not_a_multiple_of_eight_keeps_whole_quads_only() {
    let cases: Vec<(&[u8], usize)> = vec![
        (b"[1 2 3 4 5 6 7]", 0),
        (b"[1 2 3 4 5 6 7 8]", 1),
        (b"[1 2 3 4 5 6 7 8 9]", 1),
        (b"[1 2 3 4 5 6 7 8 9 10 11 12]", 1),
        (b"[16 numbers follow: this token is not a number at all]", 0),
        (b"[]", 0),
    ];
    for (n, (values, want_quads)) in cases.into_iter().enumerate() {
        let body = format!("<< /Type /Annot /Subtype /Highlight /Rect [10 20 60 30] /QuadPoints {} >>", String::from_utf8_lossy(values));
        let doc = one_annot(body.as_bytes());
        read_everything(&doc);
        if String::from_utf8_lossy(values).contains("numbers follow") {
            // An unparseable token inside the array loses the whole annotation object: see the note
            // on `an_unparseable_token_...` below. It is listed in /Annots but reads as nothing.
            assert!(summaries(&doc).is_empty(), "case {n}");
            continue;
        }
        let listed = summaries(&doc);
        assert_eq!(listed.len(), 1, "case {n}: the markup is still listed");
        assert_eq!(listed[0].quads.len(), want_quads, "case {n}: {}", String::from_utf8_lossy(values));
        // The stored array is preserved verbatim: reading truncates, saving must not rewrite.
        let d = dict_of(&doc, &annot_entries(&doc, 0)[0]).expect("a dictionary");
        let stored = nums(&doc, &d, b"QuadPoints").unwrap_or_default();
        let written = values.iter().filter(|c| c.is_ascii_digit() || **c == b' ').count();
        assert!(stored.len() <= written, "case {n}: we read at most what is there ({} vs {written})", stored.len());
    }
}

/// A token the object parser cannot interpret (a bare word that is not a number, name,
/// reference, string, `true`, `false` or `null`) loses the object it sits in. Inside a page's
/// `/Annots` that costs the *whole page*: `page_refs()` comes back `Ok([])` and every annotation
/// on that page disappears — with an empty repair log. Precision matters here: `/Pages` still
/// shows `Count 1, Kids [3 0 R]`, so it is not that the annotation list alone became
/// unreachable; the page object itself fails to parse, and nothing downstream sees it.
/// NOTE: this is `pdfcraft-cos` parser behaviour, reached through the annotation readers. The
/// bytes below are HAND-BUILT by this test's `open` helper — no writer path emits a bare
/// keyword there; they model damage an outside editor or a truncated edit leaves behind.
///
/// Cross-checked against qpdf core 12.2.0 via pikepdf 9.11 on these exact bytes (sha256
/// 582e0eb0e8a25e6975c0727d405372d7df6c8dfcb06e0289817f5b28e546b94e, 447 B — hand-built and
/// not committed; re-create it from the object bodies below, in order). Running qpdf's check,
/// `pikepdf.Job(["--check", <path>, "/dev/null"])`, exits 3 with warnings on process stderr,
/// verbatim:
///   `WARNING: <path> (object 3 0, offset 191): unknown token while reading object; treating as string`
/// — note `object 3 0` and the offset; there is no `/Annots` in qpdf's wording. qpdf KEEPS the
/// page, so this loss is ours alone — and qpdf's warning shows it was detectable. The word
/// 'silently' below refers only to our empty `repair_log()`, never to qpdf, which complains.
///
/// The honest claim here is limited to: no panic, an error or an empty page list, and
/// `add_annotation` refusing cleanly rather than writing onto a page it cannot see. Fixing the
/// parser should make this test fail, and that is exactly what it is for.
#[test]
fn an_unparseable_token_inside_annots_loses_the_page_and_says_nothing() {
    let doc = open(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [4 0 R notanumber] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [10 20 60 30] >>",
    ]);
    read_everything(&doc);
    assert_eq!(page_refs(&doc).map(|p| p.len()), Ok(0), "the page object could not be read");
    assert!(doc.repair_log().is_empty(), "and nothing was recorded about losing it");
    assert!(page_object_numbers(&doc).is_empty(), "this file's own page walk agrees there is nothing to read");
    assert!(summaries(&doc).is_empty());

    let mut try_add = doc.clone();
    let e = add_annotation(
        &mut try_add,
        &NewAnnotation {
            page: 0,
            shape: Shape::Rectangle { rect: [10.0, 10.0, 40.0, 30.0] },
            style: Style::default(),
            contents: "x".into(),
            author: "A".into(),
        },
        &Meta { date: None, id: "i".into() },
    )
    .err();
    assert_eq!(e, Some(AnnotError::NoSuchPage(0)), "adding to the vanished page refuses rather than inventing a page");
    assert_eq!(links::list(&doc).len(), 0);
}

// ── broken references ──────────────────────────────────────────────────────────────────────────

/// Dangling, duplicated and wrong-typed entries in one `/Annots` array: each is skipped by the
/// readers, none panics, and deleting one leaves the others alone. A duplicated reference really is
/// listed twice — the two comments share a `Rect` because they *are* the same object.
#[test]
fn dangling_duplicate_and_wrong_typed_entries_are_skipped() {
    let doc = open(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [5 0 R 4 0 R 4 0 R (a string) null] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [10 20 60 30] >>",
        b"[7 0 R]",
    ]);
    read_everything(&doc);
    let entries = annot_entries(&doc, 0);
    assert_eq!(entries.len(), 5, "the array keeps all five slots");
    let listed = summaries(&doc);
    assert_eq!(listed.iter().map(|s| s.index).collect::<Vec<_>>(), vec![1, 2], "only the two real dictionaries are comments");
    assert_eq!(listed[0].rect, listed[1].rect, "the duplicate is the same object");
    assert!(props(&doc, 0, 0).is_none(), "an entry naming an array is not a comment");
    assert!(props(&doc, 0, 3).is_none(), "a string entry is not a comment");
    assert_eq!(signature_image(&doc, 0, 1).ok().flatten(), None);

    // Deleting one of the duplicates removes **both** slots: `delete_annotation` drops every entry
    // that names a doomed object, not just the index asked for (that is how it takes pop-ups and
    // replies with their parent). So the shared dictionary leaves the page entirely.
    let mut cut = doc.clone();
    delete_annotation(&mut cut, 0, 1).expect("deletes the comment");
    assert_eq!(annot_entries(&cut, 0).len(), 3, "both slots naming that object are gone");
    assert!(summaries(&cut).is_empty());
    let (_, reopened) = save_and_reopen(&cut, Save::FullTable);
    assert!(summaries(&reopened).is_empty(), "and the file agrees");

    // An `/Annots` that is not an array at all reads as "no annotations".
    let weird = open(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots (hello) >>",
    ]);
    read_everything(&weird);
    assert!(annot_entries(&weird, 0).is_empty());
    let mut d = weird.clone();
    assert_eq!(delete_annotation(&mut d, 0, 0).err(), Some(AnnotError::NoSuchAnnotation { page: 0, index: 0 }));
}

/// Reference loops must terminate. These are the shapes a generated or half-repaired file produces:
/// an annotation replying to itself, two replies replying to each other, and a pop-up chain whose
/// back-references point at the next annotation instead of the previous one.
#[test]
fn reference_loops_terminate_and_a_delete_takes_the_cycle_with_it() {
    // Self-reference.
    let self_irt = one_annot(b"<< /Type /Annot /Subtype /Text /Rect [10 20 30 40] /IRT 4 0 R >>");
    read_everything(&self_irt);
    let mut cut = self_irt.clone();
    delete_annotation(&mut cut, 0, 0).expect("deletes the self-replying comment");
    assert!(annot_entries(&cut, 0).is_empty());

    // Mutual references: deleting either must take both and stop.
    let shared = open(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [4 0 R 5 0 R] >>",
        b"<< /Type /Annot /Subtype /Text /Rect [10 20 30 40] /IRT 5 0 R >>",
        b"<< /Type /Annot /Subtype /Text /Rect [10 20 30 40] /IRT 4 0 R >>",
    ]);
    read_everything(&shared);
    assert_eq!(summaries(&shared).len(), 2, "both halves of the pair are listed");
    let mut cut = shared.clone();
    delete_annotation(&mut cut, 0, 0).expect("deleting one of a mutual pair returns");
    assert!(annot_entries(&cut, 0).is_empty(), "the cycle is taken out together");

    // A pop-up whose /Parent names a third annotation, which in turn names the first.
    let chained = open(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [4 0 R 5 0 R 6 0 R] >>",
        b"<< /Type /Annot /Subtype /Text /Rect [10 20 30 40] /Popup 5 0 R >>",
        b"<< /Type /Annot /Subtype /Popup /Rect [40 20 90 60] /Parent 6 0 R >>",
        b"<< /Type /Annot /Subtype /Popup /Rect [40 20 90 60] /Parent 4 0 R >>",
    ]);
    read_everything(&chained);
    assert_eq!(summaries(&chained).len(), 1, "only the text comment is a comment");
    let mut cut = chained.clone();
    delete_annotation(&mut cut, 0, 0).expect("deleting the owner of a pop-up chain returns");
    assert!(annot_entries(&cut, 0).is_empty(), "and the whole chain goes with it");

    // A `/Popup` that names a missing object must not stop the comment being used.
    let dangling_popup = one_annot(b"<< /Type /Annot /Subtype /Text /Rect [10 20 30 40] /Popup 99 0 R >>");
    read_everything(&dangling_popup);
    assert_eq!(summaries(&dangling_popup).len(), 1);
    let mut reply = dangling_popup.clone();
    assert!(
        pdfcraft_annot::add_reply(&mut reply, 0, 0, "to you", "Ben", &Meta { date: None, id: "r".into() }).is_ok(),
        "a missing pop-up does not block replying"
    );
}

// ── wrong types where the reader wants a number or a name ─────────────────────────────────────

/// Values of the wrong type in flag, opacity and colour entries: the reader falls back to its
/// defaults instead of panicking. NOTE on opacity: reading does **not** clamp, so `/CA 7.5` comes
/// back as 7.5 and would hand a nonsense value to a 0–1 slider; writing clamps (`add_annotation`
/// refuses non-finite and clamps the range). The asymmetry is recorded here, not endorsed.
#[test]
fn wrong_typed_flag_opacity_and_colour_entries_fall_back_without_panicking() {
    let doc = one_annot(b"<< /Type /Annot /Subtype /Text /Rect [10 20 30 40] /CA 7.5 /F /NotAnInt /C [1 2] >>");
    read_everything(&doc);
    let p = props(&doc, 0, 0).expect("still a comment");
    assert_eq!(p.opacity, 7.5, "the out-of-range opacity is passed through as written");
    assert!(!p.locked, "a name where /F belongs reads as no flags at all");
    assert_eq!(p.color, None, "/C with two components is not a colour");
    let listed = summaries(&doc);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].color, None);

    // And the other direction: /F as a real number is read by its integer value.
    let flags_real = one_annot(b"<< /Type /Annot /Subtype /Text /Rect [10 20 30 40] /F 132.0 >>");
    let p = props(&flags_real, 0, 0).expect("still a comment");
    assert!(p.locked, "132 sets ReadOnly (bit 8) whatever the number's form");

    // An annotation with no /Subtype at all: nothing to name, so it is not a comment and has no
    // properties, but the entry survives untouched (preserve unknown data).
    let no_subtype = one_annot(b"<< /Type /Annot /Rect [10 20 30 40] >>");
    read_everything(&no_subtype);
    assert!(summaries(&no_subtype).is_empty());
    assert!(props(&no_subtype, 0, 0).is_none());
    assert_eq!(annot_entries(&no_subtype, 0).len(), 1);

    // A `/P` pointing at a page that does not exist is cosmetic: the comment still lists.
    let bad_parent_page = one_annot(b"<< /Type /Annot /Subtype /Square /Rect [10 20 30 40] /P 77 0 R >>");
    read_everything(&bad_parent_page);
    assert_eq!(summaries(&bad_parent_page).len(), 1);
}

// ── guards that only bite on damaged data, pinned so they stay visible ────────────────────────

/// `move_annotation` shifts the numbers of every geometry key, but only the first four of an array
/// it treats as a rectangle (`every || i < 4`, `crates/annot/src/lib.rs:1718`). A `/Rect` with
/// more than four numbers is not something PdfCraft writes, so the guard only shows up on damaged
/// data — which is precisely why it is pinned here: without this test the rule can be deleted
/// silently. What must hold is that the four rectangle numbers move together and whatever strays
/// the file carries are left exactly as they were, rather than being dragged along by half a point.
#[test]
fn moving_a_damaged_rectangle_shifts_its_four_numbers_and_leaves_the_strays_alone() {
    let mut doc = one_annot(b"<< /Type /Annot /Subtype /Square /Rect [10 20 60 30] >>");
    let r = annot_entries(&doc, 0)[0].as_ref().expect("an indirect entry");
    doc.update_dict(r, |d| {
        d.set(
            b"Rect".to_vec(),
            Object::Array(vec![
                Object::Real(10.0),
                Object::Real(20.0),
                Object::Real(60.0),
                Object::Real(30.0),
                Object::Real(9.0),
                Object::Real(-9.5),
            ]),
        );
    })
    .expect("the entry is a dictionary");

    pdfcraft_annot::move_annotation(&mut doc, 0, 0, 5.0, 7.0, &Meta { date: None, id: "m".into() }).expect("the comment is not locked");
    let d = dict_of(&doc, &annot_entries(&doc, 0)[0]).expect("a dictionary");
    let stored = nums(&doc, &d, b"Rect").expect("six numbers");
    assert_eq!(stored, vec![15.0, 27.0, 65.0, 37.0, 9.0, -9.5], "four shift, two strays do not");

    // It survives a save that way, and the comment list still cannot use such a rectangle.
    let (file, reopened) = save_and_reopen(&doc, Save::FullTable);
    let stored_again = bytes::numbers_of(&file, "Rect");
    assert_eq!(stored_again.first(), Some(&vec![15.0, 27.0, 65.0, 37.0, 9.0, -9.5]), "the file keeps all six");
    let listed = summaries(&reopened);
    assert_eq!(listed.len(), 1, "still a comment");
    assert_eq!(listed[0].rect, [0.0, 0.0, 0.0, 0.0], "and still not a usable position");
}

/// A link area is checked before anything is written: it needs finite corners whose span is at
/// least two points in both directions. Two different reasons, two different messages — and in
/// every case the page gains no entry.
#[test]
fn a_link_area_that_is_too_small_or_broken_is_refused_before_it_is_written() {
    let cases = [
        ("one point wide", [10.0, 20.0, 11.0, 40.0], "too small"),
        ("zero height", [10.0, 20.0, 50.0, 20.0], "too small"),
        ("a NaN corner", [f64::NAN, 1.0, 50.0, 60.0], "finite coordinates"),
        ("corners whose span overflows", [-1e308, 1.0, 1e308, 60.0], "too small or too large"),
    ];
    for (label, rect, want_in_message) in cases {
        let mut doc = blank(2);
        let e = links::add(&mut doc, 0, rect, &links::LinkAction::Page(1), &links::LinkStyle::default()).err();
        let msg = match &e {
            Some(AnnotError::Invalid(m)) => m.clone(),
            other => panic!("{label}: expected Invalid, got {other:?}"),
        };
        assert!(msg.contains(want_in_message), "{label}: message {msg:?} should mention {want_in_message:?}");
        assert!(annot_entries(&doc, 0).is_empty(), "{label}: nothing was written");
        assert!(links::list(&doc).is_empty(), "{label}: no link to list");
    }
}

// ── hostile numbers handed to the builder ─────────────────────────────────────────────────────

/// Non-finite style and geometry must be refused before anything is written: no partial object, no
/// `NaN` or exponent token in the file. (`AGENTS.md` §4: guard NaN/inf casts.)
#[test]
fn non_finite_builder_input_is_refused_and_leaves_the_document_untouched() {
    let shapes = [
        ("nan rect", Shape::Rectangle { rect: [f64::NAN, 10.0, 40.0, 30.0] }),
        ("infinite rect", Shape::Rectangle { rect: [f64::NEG_INFINITY, 10.0, 40.0, 30.0] }),
        ("nan quad", Shape::TextMarkup { kind: pdfcraft_annot::Markup::Highlight, quads: vec![[10.0, 10.0, f64::NAN, 20.0, 10.0, 5.0, 30.0, 5.0]] }),
        ("far away note", Shape::Note { at: [1e308, 2.0], icon: NoteIcon::Note }),
        ("nan ink", Shape::Ink { strokes: vec![vec![[f64::NAN, 1.0], [2.0, 3.0]]] }),
        ("empty ink", Shape::Ink { strokes: vec![vec![]] }),
        (
            "degenerate line",
            Shape::Line { from: [10.0, 10.0], to: [10.0, 10.5], start: pdfcraft_annot::LineEnding::None, end: pdfcraft_annot::LineEnding::None },
        ),
        ("nan polygon", Shape::Polygon { vertices: vec![[0.0, 0.0], [f64::INFINITY, 10.0], [5.0, 5.0]], cloud: false }),
        ("two point polygon", Shape::Polygon { vertices: vec![[0.0, 0.0], [10.0, 10.0]], cloud: false }),
        (
            "nan callout",
            Shape::Callout {
                rect: [10.0, 10.0, 40.0, 30.0],
                knee: [f64::NAN, 0.0],
                point: [0.0, 0.0],
                font_size: 12.0,
                ending: pdfcraft_annot::LineEnding::None,
            },
        ),
        ("no quads", Shape::TextMarkup { kind: pdfcraft_annot::Markup::StrikeOut, quads: vec![] }),
    ];
    for (label, shape) in shapes {
        let mut doc = blank(1);
        let e = add_annotation(
            &mut doc,
            &NewAnnotation { page: 0, shape, style: Style::default(), contents: "x".into(), author: "A".into() },
            &Meta { date: None, id: "i".into() },
        )
        .err();
        assert!(matches!(e, Some(AnnotError::Invalid(_))), "{label}: refused with Invalid, got {e:?}");
        assert!(annot_entries(&doc, 0).is_empty(), "{label}: nothing was written");
        // The refusal must not corrupt the document either: it still saves and reloads empty.
        let (_, reopened) = save_and_reopen(&doc, Save::Incremental);
        assert!(summaries(&reopened).is_empty(), "{label}: the file is still clean");
    }

    for (label, style) in [
        ("nan opacity", Style { color: [0.0, 0.0, 0.0], opacity: f64::NAN, width: 1.0, fill: None }),
        ("infinite opacity", Style { color: [0.0, 0.0, 0.0], opacity: f64::INFINITY, width: 1.0, fill: None }),
        ("nan colour", Style { color: [f64::NAN, 0.0, 0.0], opacity: 1.0, width: 1.0, fill: None }),
        ("nan width", Style { color: [0.0, 0.0, 0.0], opacity: 1.0, width: f64::NAN, fill: None }),
    ] {
        let mut doc = blank(1);
        let e = add_annotation(
            &mut doc,
            &NewAnnotation { page: 0, shape: Shape::Rectangle { rect: [10.0, 10.0, 40.0, 30.0] }, style, contents: "x".into(), author: "A".into() },
            &Meta { date: None, id: "i".into() },
        )
        .err();
        assert_eq!(e.as_ref(), Some(&AnnotError::Invalid("invalid style".into())), "{label}");
        assert!(annot_entries(&doc, 0).is_empty(), "{label}: nothing was written");
    }

    // Edits take the same care: moving by a non-finite offset is refused and leaves geometry alone.
    let mut doc = blank(1);
    add_annotation(
        &mut doc,
        &NewAnnotation {
            page: 0,
            shape: Shape::Rectangle { rect: [10.0, 20.0, 60.0, 45.0] },
            style: Style::default(),
            contents: "keep".into(),
            author: "A".into(),
        },
        &Meta { date: None, id: "ok".into() },
    )
    .expect("adds");
    let before = summaries(&doc)[0].rect;
    assert_eq!(
        pdfcraft_annot::move_annotation(&mut doc, 0, 0, f64::NAN, 1.0, &Meta { date: None, id: "m".into() }).err(),
        Some(AnnotError::Invalid("invalid offset".into()))
    );
    assert_eq!(summaries(&doc)[0].rect, before, "a refused move does not shift the comment");
    assert_eq!(
        pdfcraft_annot::set_rect(&mut doc, 0, 0, [f64::NAN, 0.0, 1e308, 1e308], &Meta { date: None, id: "m".into() }).err().as_ref(),
        Some(&AnnotError::Invalid("invalid rectangle (too small)".into()))
    );
    assert_eq!(summaries(&doc)[0].rect, before, "a refused resize does not shift the comment");
}

/// Attachment input is checked too: a blank file name is refused with an actionable message and
/// writes nothing, so no half-built file specification is left in the document.
#[test]
fn an_attachment_needs_a_name_and_leaves_nothing_behind_when_it_has_none() {
    for name in ["", "   ", "\t"] {
        let mut doc = blank(1);
        let e = add_annotation(
            &mut doc,
            &NewAnnotation {
                page: 0,
                shape: Shape::Attachment { at: [10.0, 700.0], icon: AttachIcon::Paperclip, file: name.into(), data: b"x".to_vec() },
                style: Style::default(),
                contents: String::new(),
                author: "A".into(),
            },
            &Meta { date: None, id: "a".into() },
        )
        .err();
        assert_eq!(e.as_ref(), Some(&AnnotError::Invalid("the attached file needs a name".into())), "{name:?}");
        assert!(annot_entries(&doc, 0).is_empty(), "no orphan file specification for {name:?}");
    }
}

// ── fields that carry no magnitude screen (recorded, not endorsed) ────────────────────────────

/// This is a MAGNITUDE story, not a one-field story. `rect_for` checks the rectangle of every
/// variant but passes `Shape::CustomStamp { size }` and `Shape::TypedSignature { contours }`
/// straight into `/PCPictureSize` and `/PCOutline` (`lib.rs:872-892`, `:1104`, `:1111`) with no
/// magnitude screen whatsoever — and width gets none either. The only style guard on the create
/// path (`lib.rs:1018`) reads `!finite(&style.color) || !style.opacity.is_finite()
/// || !style.width.is_finite()`: `finite()` (`:827-829`, i.e. `is_finite() && abs() < 1e7`)
/// magnitude-screens `color` ALONE, while `opacity` and `width` are checked for non-finiteness
/// only. So even the one cap that exists sits at ten million, far below what a reader can turn
/// into an integer. Measured with qpdf's check (`pikepdf.Job(["--check", <path>, "/dev/null"])`,
/// warnings on process stderr, exit code 3). Read that exit code carefully: **3 is qpdf's warning
/// status**, printed together with "operation succeeded with warnings", so the check still completes
/// and writes its output — it is not a hard failure (libqpdf: 0 = no errors or warnings,
/// 2 = errors detected, 3 = warnings detected; a clean file returns 0 with no stderr at all).
/// Every warning probe in this file returns 3 — the three overflow cases below and the
/// unknown-token probe documented at `:155-157` above — so the exit code alone does not separate
/// them from each other; the warning text does. The 9,999,999 control below is the only probe
/// that returns 0. Re-measurement trap: `Job.stderr` stays `None` even when qpdf warns, because
/// the warnings go to the inherited process stderr — redirect at shell level or you will report
/// "no warnings" from a file that produced them. Measured per field, each file written by THIS
/// crate's own writer path:
///
/// `/BS /W` = 1e299 on an Oval: qpdf reports `object 4/0: error reading object:
/// overflow/underflow converting <1 followed by 299 digits> to 64-bit integer`.
///
/// `/PCPictureSize` = 1e300 plus NaN: same message. File 1198 B, sha256
/// 357946a6580e7c13fbcf3c80c396cd80f961a53e55aea9e11dff5e284f759e07, 301 digits confirmed.
///
/// `/PCOutline` = 1e300: same message.
///
/// `/BS /W` = 9999999: clean, exit code 0 — because that value has few enough digits for a
/// reader's 64-bit integer conversion (which tops out near 9.2e18), NOT because this crate caps
/// it: width never passes through `finite()`, so the ten-million figure plays no part here. The
/// boundary that matters is the reader's integer range, and it sits far above anything a real
/// border width could be. Measured here too: `/BS /W` = 1e9 is accepted and written verbatim as
/// `1000000000`, which is well past any sane border yet still parseable — the failure only
/// begins when the digits outrun a 64-bit conversion.
///
/// So ANY annotation number of sufficient magnitude can make the whole object unreadable to a
/// third-party reader — width included, not just stamp payload. Two measurement notes that
/// matter when re-running: `pikepdf.open()` alone prints nothing for these files and still
/// reports `/Annots` length 1, while resolving `/Annots[0]` yields **null** and stays null after
/// qpdf re-saves — a listed-but-null entry is exactly what silent corruption looks like. qpdf
/// classifies it as a WARNING and its open still reports success; pikepdf puts `WARNING:` on a
/// line of its own, so grepping raw stderr for one combined line misses it. What we did NOT
/// measure is what any particular viewer then shows a user — the null entry was observed, not a
/// renderer's behaviour.
/// Meanwhile our own `repair_log()` stays empty, which contradicts this project's rule that
/// dropped or repaired data must be recorded (`CLAUDE.md`, Fidelity). None of these probe files
/// is committed: any sample PDF would need its own `ATTRIBUTION.toml` entry (`AGENTS.md` §1),
/// which this change avoids — regenerate them from the `file` bytes bound below and re-measure.
///
/// How far a caller can actually get there, checked in this workspace rather than assumed: only
/// `/BS /W` is reachable through a shipped path. The automation tools read `width` as any finite
/// number (`crates/automation/src/comments.rs:402-403` assigns `style.width = w.max(0.0)` after
/// `opt_num`, whose only filter is `is_finite()` at `crates/automation/src/lib.rs:2233-2235`),
/// and the published schema sets `"minimum": 0` with no upper bound
/// (`crates/automation/src/tools.rs:655`) — so a script or MCP agent can write a file whose
/// annotation object qpdf refuses to parse. The desktop UI caps the same value at 0.5..=12.0
/// (`crates/ui-egui/src/comment_props.rs:245`), and `/PCPictureSize` / `/PCOutline` are computed
/// inside the engine — `crates/engine/src/lib.rs:2343` takes the source image's pixel size,
/// scales it to a 200 pt natural box at `:2347-2351` and hands that size to the stamp shape at
/// `:2353`, while contour sizing lives at `crates/engine/src/lib.rs:104-114` (which does reject a
/// non-positive width) — so reaching those two needs a caller using the crates directly
/// rather than an app or tool action. The defect is therefore automation-reachable today, and not
/// reachable by clicking in the desktop app.
///
/// Non-finite values are rewritten on the way out: `write_real` (`crates/cos/src/writer.rs:48`)
/// emits a literal `0` for any non-finite REAL that reaches it, which is the mechanism for
/// `size` and contour points. Colours are a different route — `rgb()` (`lib.rs:823-825`) clamps
/// each component to `0.0..=1.0` BEFORE the writer sees it, so infinity becomes `1.0`, not `0`
/// (see the create-vs-edit case below). The assertions that follow pin only what we write.
///
/// NOTE: these tests record current behaviour so a fix becomes visible. They assert nothing about
/// the result being correct — the values are wrong on the way in and quietly rewritten on the
/// way out.
#[test]
fn unvalidated_stamp_size_and_signature_contours_are_rewritten_silently_in_the_file() {
    let mut doc = blank(1);
    let picture = doc.add(Object::Dict(Dict::new()));
    add_annotation(
        &mut doc,
        &NewAnnotation {
            page: 0,
            shape: Shape::CustomStamp { rect: [10.0, 20.0, 60.0, 50.0], name: "Sig".into(), picture, image: true, size: (1e300, f64::NAN) },
            style: Style::default(),
            contents: String::new(),
            author: "A".into(),
        },
        &Meta { date: None, id: "cs".into() },
    )
    .expect("the size is not validated, so the comment is accepted");

    let (file, reopened) = save_and_reopen(&doc, Save::FullTable);
    let stored = bytes::numbers_of(&file, "PCPictureSize");
    assert_eq!(stored.len(), 1, "one /PCPictureSize array in the file");
    assert_eq!(stored[0][1], 0.0, "NaN was written as 0 ({:?})", stored[0]);
    assert!(stored[0][0] > 1e299, "the huge size was written rather than refused ({:?})", stored[0][0]);
    assert!(doc.repair_log().is_empty() && reopened.repair_log().is_empty(), "nothing recorded about either substitution");
    // The digits are really there: 301 of them, no exponent form.
    let pos = file.windows(b"/PCPictureSize".len()).position(|w| w == b"/PCPictureSize").expect("the key is in the file");
    let run = &file[pos..];
    let digits = run.iter().take(400).filter(|b| b.is_ascii_digit()).count();
    assert!(digits > 300, "a 301-digit decimal was written to the file ({digits} digits seen)");

    let mut doc2 = blank(1);
    add_annotation(
        &mut doc2,
        &NewAnnotation {
            page: 0,
            shape: Shape::TypedSignature { rect: [10.0, 20.0, 60.0, 50.0], contours: vec![vec![[f64::NAN, 1.0], [2.0, 3.0], [4.0, f64::INFINITY]]] },
            style: Style::default(),
            contents: String::new(),
            author: "A".into(),
        },
        &Meta { date: None, id: "ts".into() },
    )
    .expect("the contours are not validated either");
    let (file2, _) = save_and_reopen(&doc2, Save::FullTable);
    let outline = bytes::numbers_of(&file2, "PCOutline");
    assert_eq!(outline.as_slice(), &[vec![0.0, 1.0, 2.0, 3.0, 4.0, 0.0]], "both non-finite contour points became 0");
}

/// The fill colour is screened differently on the two paths. `add_annotation` magnitude-screens
/// only `style.color`, through `finite()` (`lib.rs:1018`; the helper at `:827-829` is
/// `is_finite() && abs() < 1e7`), gives `opacity` and `width` plain finiteness checks, and never
/// screens `fill` at all; `set_fill` refuses the same value (`lib.rs:2010`, "invalid fill
/// colour") — and that refusal calls `finite()` too, which is why it trips on anything from ten
/// million up as well as on NaN or infinity. The accepted value is then clamped component-wise by
/// `rgb()` (`lib.rs:823-825`, `clamp(0.0, 1.0)`), so what lands in the file is the top of the
/// range: NaN survives clamping as NaN and `write_real` (`crates/cos/src/writer.rs:48`) then
/// emits a literal `0`, while infinity becomes `1.0`. Measured across magnitudes
/// in `the_same_huge_fill_is_accepted_on_create_and_refused_on_edit`, which pins
/// 1e7 / 1e8 / infinity as well. Border width and stroke colour behave symmetrically on both
/// paths — width is finiteness-checked only, so a huge width gets written rather than refused (see
/// the magnitude note above), and both paths clamp it at zero — while stamp `size` and signature
/// `contours` have no edit path at all, so only create can diverge for them. Pinned as current
/// behaviour, not as desired behaviour.
#[test]
fn a_nan_fill_is_accepted_on_create_and_refused_on_edit() {
    let nan = [f64::NAN, 0.0, 0.0];
    let mut doc = blank(1);
    add_annotation(
        &mut doc,
        &NewAnnotation {
            page: 0,
            shape: Shape::Rectangle { rect: [10.0, 20.0, 60.0, 40.0] },
            style: Style { color: [0.9, 0.1, 0.1], opacity: 1.0, width: 2.0, fill: Some(nan) },
            contents: String::new(),
            author: "A".into(),
        },
        &Meta { date: None, id: "nf".into() },
    )
    .expect("create does not screen the fill colour");
    let (file, reopened) = save_and_reopen(&doc, Save::FullTable);
    let ic = bytes::numbers_of(&file, "IC");
    assert_eq!(ic.first(), Some(&vec![0.0, 0.0, 0.0]), "the NaN components were written as zeros");
    assert_eq!(props(&reopened, 0, 0).and_then(|p| p.fill), Some([0.0, 0.0, 0.0]));

    let e = pdfcraft_annot::set_fill(&mut reopened.clone(), 0, 0, Some(nan), &Meta { date: None, id: "nf2".into() }).err();
    assert_eq!(e.as_ref(), Some(&AnnotError::Invalid("invalid fill colour".into())), "the same value is refused two lines later in the same crate");
}

// ── generated damage, same rule: refuse or degrade, never panic ───────────────────────────────

/// proptest's assert macros cannot capture local names in their format strings, so the writer path
/// is formatted here and passed in explicitly.
fn tag(path: Save) -> String {
    format!("{path:?}")
}

/// The kinds of damage a stranger's annotation dictionary can carry. Each is applied to a comment
/// PdfCraft wrote, so the file stays structurally valid around one wrong entry.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Damage {
    RectTooShort,
    RectWrongType,
    QuadPointsOddLength,
    QuadPointsWrongType,
    FlagsAsName,
    OpacityOutOfRange,
    SubtypeMissing,
    PopupDangling,
    ParentToMissingObject,
    IrtSelfReference,
    DuplicateEntryInAnnots,
    InkListWrongShape,
    VerticesOddLength,
}

const DAMAGES: [Damage; 13] = [
    Damage::RectTooShort,
    Damage::RectWrongType,
    Damage::QuadPointsOddLength,
    Damage::QuadPointsWrongType,
    Damage::FlagsAsName,
    Damage::OpacityOutOfRange,
    Damage::SubtypeMissing,
    Damage::PopupDangling,
    Damage::ParentToMissingObject,
    Damage::IrtSelfReference,
    Damage::DuplicateEntryInAnnots,
    Damage::InkListWrongShape,
    Damage::VerticesOddLength,
];

/// Apply one damage to the annotation dictionary at `/Annots[index]`, resolving it first if inline.
fn damage(doc: &mut Document, index: usize, kind: Damage) {
    let entries = annot_entries(doc, 0);
    let Some(r) = entries.get(index).and_then(|e| e.as_ref()) else { return };
    doc.update_dict(r, |d| match kind {
        Damage::RectTooShort => d.set(b"Rect".to_vec(), Object::Array(vec![Object::Real(10.0), Object::Real(20.0)])),
        Damage::RectWrongType => {
            d.set(b"Rect".to_vec(), Object::Array(vec![Object::name("a"), Object::name("b"), Object::name("c"), Object::name("d")]))
        }
        Damage::QuadPointsOddLength => d.set(b"QuadPoints".to_vec(), Object::Array((0..7).map(|i| Object::Real(i as f64)).collect())),
        Damage::QuadPointsWrongType => d.set(b"QuadPoints".to_vec(), Object::Array(vec![Object::Real(1.0), Object::name("notanumber")])),
        Damage::FlagsAsName => d.set(b"F".to_vec(), Object::name("Locked")),
        Damage::OpacityOutOfRange => d.set(b"CA".to_vec(), Object::Real(4.25)),
        Damage::SubtypeMissing => {
            d.remove(b"Subtype");
        }
        Damage::PopupDangling => d.set(b"Popup".to_vec(), Object::Ref(ObjRef::new(9997, 0))),
        Damage::ParentToMissingObject => d.set(b"Parent".to_vec(), Object::Ref(ObjRef::new(9998, 0))),
        Damage::IrtSelfReference => d.set(b"IRT".to_vec(), Object::Ref(r)),
        Damage::DuplicateEntryInAnnots => {
            d.set(b"Rect".to_vec(), Object::Array(vec![Object::Real(1.0), Object::Real(2.0), Object::Real(3.0)]));
        }
        Damage::InkListWrongShape => d.set(b"InkList".to_vec(), Object::Array(vec![Object::Real(5.0), Object::name("notastroke")])),
        Damage::VerticesOddLength => d.set(b"Vertices".to_vec(), Object::Array(vec![Object::Real(1.0), Object::Real(2.0), Object::Real(3.0)])),
    })
    .expect("the entry is an annotation dictionary");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// For any single damage, and any of the three writer paths: opening, reading every annotation
    /// entry point, re-saving and reopening must all complete without a panic. Alongside that the
    /// test insists on something positive, so it cannot pass by reading nothing at all — an undamaged
    /// second comment on the same page keeps its exact rectangle, and the damaged comment is either
    /// listed or skipped but never half-parsed into a crash.
    #[test]
    fn damaged_annotations_never_panic(kind in prop::sample::select(DAMAGES.to_vec()), extra_annot in 0usize..=2) {
        // Build a page holding one comment that will be damaged plus `extra_annot` clean ones.
        let mut doc = blank(1);
        for i in 0..=extra_annot {
            add_annotation(&mut doc, &NewAnnotation { page: 0, shape: Shape::TextMarkup { kind: pdfcraft_annot::Markup::Highlight, quads: vec![[20.0 + 10.0 * i as f64, 500.0, 80.0 + 10.0 * i as f64, 500.0, 20.0 + 10.0 * i as f64, 486.0, 80.0 + 10.0 * i as f64, 486.0]] }, style: Style::default(), contents: format!("ok{i}"), author: "Ada".into() }, &Meta { date: None, id: format!("ok{i}") }).expect("adds");
        }
        let clean_rect = summaries(&doc).iter().map(|s| s.rect).collect::<Vec<_>>();
        damage(&mut doc, 0, kind);

        for path in [Save::Incremental, Save::FullTable, Save::FullStreams] {
            let (_, reopened) = save_and_reopen(&doc, path);
            read_everything(&reopened);
            // The undamaged comments are untouched by the damage and its save.
            let after = summaries(&reopened);
            // A comment stops being listed only when the damage removes what makes it a comment —
            // its `/Subtype` (measured, not assumed: the other twelve damages all stay listed).
            let want_listed = clean_rect.len() - usize::from(kind == Damage::SubtypeMissing);
            prop_assert_eq!(after.len(), want_listed, "damage {k} on {p}: listed comment count", k = format!("{kind:?}"), p = tag(path));
            // Every undamaged comment keeps its exact rectangle through the damage and the rewrite.
            for r in &clean_rect[1..] {
                prop_assert!(after.iter().any(|s| s.rect == *r), "damage {k} on {p}: the clean comment at {r:?} is gone", k = format!("{kind:?}"), p = tag(path));
            }
            // And deleting by index never panics, whatever the damage was.
            let mut cut = reopened.clone();
            for i in 0..clean_rect.len() + 1 {
                let _ = delete_annotation(&mut cut, 0, i);
            }
        }
    }
}

/// Read side, measured: a negative border width arriving from another tool is reported unchanged
/// by the public API. Our own writer clamps negatives to 0 (`lib.rs:857`, `:1030`), and the
/// helper `border_width_of` (`lib.rs:1872-1874`) does apply `.max(0.0)` — but the property read
/// path used here (`props()` -> `lib.rs:2237-2238`) takes `/BS /W` raw, so a foreign file
/// declaring -3 comes back as -3.0. Pinned because the asymmetry is visible in the public API:
/// what we refuse to keep, we are willing to report. (Compare `/CA`, likewise unclamped on read.)
#[test]
fn foreign_negative_border_width_is_reported_as_written() {
    let doc = open(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [4 0 R] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [10 20 60 30] /BS << /S /S /W -3 >> >>",
    ]);
    let p = props(&doc, 0, 0).expect("the annotation is readable");
    assert_eq!(
        p.width,
        Some(-3.0),
        "props() reports /BS /W as written; note border_width_of (lib.rs:1872-1874) does clamp at zero, so this is path-specific"
    );
    assert!(doc.repair_log().is_empty(), "and that neither is recorded as a repair");
}

/// The create-vs-edit divergence is wider than the NaN case: the edit-side refusal begins at `1e7`
/// **in magnitude**, because `finite()` (`lib.rs:827-829`, called by the edit path at `:2010`)
/// tests `abs() < 1e7`, so a component of magnitude exactly ten million is refused (positive or
/// negative) while 9,999,999 passes — what THIS test pins is the positive side (1e7, 1e8, infinity);
/// a negative-magnitude row is not pinned here, only reasoned from `abs()` (independently measured
/// in review round 5, which found `-1e7` refused on the edit path)
/// while create never screens `fill` at all, so any value whatsoever is accepted
/// there. Measured, not assumed — the
/// expected `/IC` components below are what this crate's writer actually produced on re-run
/// (clamping happens in `rgb()`, `lib.rs:823-825`, so an accepted huge component lands at 1.0).
#[test]
fn the_same_huge_fill_is_accepted_on_create_and_refused_on_edit() {
    let cases: [([f64; 3], &[f64; 3]); 3] =
        [([1e7, 0.0, 0.0], &[1.0, 0.0, 0.0]), ([1e8, 0.0, 0.0], &[1.0, 0.0, 0.0]), ([f64::INFINITY; 3], &[1.0, 1.0, 1.0])];
    for (probe, want) in cases {
        let mut doc = blank(1);
        add_annotation(
            &mut doc,
            &NewAnnotation {
                page: 0,
                shape: Shape::Rectangle { rect: [10.0, 20.0, 60.0, 40.0] },
                style: Style { color: [0.9, 0.1, 0.1], opacity: 1.0, width: 2.0, fill: Some(probe) },
                contents: String::new(),
                author: "A".into(),
            },
            &Meta { date: None, id: "huge-fill".into() },
        )
        .expect("create does not screen the fill colour");
        let (file, reopened) = save_and_reopen(&doc, Save::FullTable);
        let ic = bytes::numbers_of(&file, "IC");
        assert_eq!(ic.first().map(Vec::as_slice), Some(want.as_slice()), "{probe:?}: create stores the clamped colour");
        assert_eq!(props(&reopened, 0, 0).and_then(|p| p.fill), Some([want[0], want[1], want[2]]), "{probe:?}: read-back matches what was written",);
        let e = pdfcraft_annot::set_fill(&mut reopened.clone(), 0, 0, Some(probe), &Meta { date: None, id: "huge-fill-2".into() }).err();
        assert_eq!(e.as_ref(), Some(&AnnotError::Invalid("invalid fill colour".into())), "{probe:?}: the same value is refused on the edit path",);
    }
}
