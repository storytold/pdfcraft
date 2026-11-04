//! The checked redaction view: what the interface fills is what applying removes, at open and
//! after every kind of change, and applying refuses whenever the view can't be trusted.

use super::*;
use crate::{MarkAreaError, MarkRefusal, PreviewUnavailable, RedactionPreviews};

fn pdf_of(objs: &[String]) -> Vec<u8> {
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
    out
}

/// One 200 × 200 page drawing `AB1234CD` in 10 pt Courier at (10, 100): each glyph 6 pt wide, so
/// AB is x 10–22, 1234 is 22–46 and CD 46–58. `annots` are listed as objects 6, 7, …; `extra`
/// objects follow them.
fn ab1234cd(annots: &[&str], extra: &[&str]) -> Vec<u8> {
    page_with("BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET", annots, extra)
}

/// [`ab1234cd`] with another content stream (10 pt Courier as /F1).
fn page_with(body: &str, annots: &[&str], extra: &[&str]) -> Vec<u8> {
    let list: String = (0..annots.len()).map(|i| format!("{} 0 R ", i + 6)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R /Annots [{list}] >>"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>".to_string(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
    objs.extend(annots.iter().chain(extra).map(|s| s.to_string()));
    pdf_of(&objs)
}

/// The reported mark: a null first corner, and a /Rect over 1234.
const NULL_MARK: &str = "<< /Type /Annot /Subtype /Redact /Rect [22 95 46 110] /QuadPoints [null 110 46 110 22 95 46 95] >>";
const AREA: [f64; 4] = [22.0, 95.0, 46.0, 110.0];

fn open(bytes: Vec<u8>) -> (Session, DocId) {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("marks.pdf", None, Arc::new(bytes), None).unwrap();
    (s, id)
}

/// The ready areas the view gives the mark at `(page, index)`.
fn ready(s: &Session, id: DocId, page: usize, index: usize) -> Vec<[f64; 4]> {
    let p = s.get(id).unwrap().redaction_previews().get(page, index).unwrap_or_else(|| panic!("no checked mark at {page}/{index}"));
    p.areas.clone().unwrap_or_else(|e| panic!("refused: {e}")).rects
}

/// The view is exactly what the collector reads from the working graph now: never stale.
fn assert_current(s: &Session, id: DocId) {
    let doc = s.get(id).unwrap();
    let fresh = pdfcraft_redact::mark_previews(&doc.editor.as_ref().unwrap().cos, None).unwrap();
    assert_eq!(doc.redaction_previews(), &RedactionPreviews::Available(fresh));
}

/// The text of the saved file, reopened.
fn saved_text(s: &Session, id: DocId) -> String {
    let (s2, id2) = open(s.save_bytes(id).unwrap().to_vec());
    page_texts(&s2, id2).concat()
}

/// Decisive regression 1. The renderer's inspection reads the null as 0, so the old hover fill
/// (a fold of `Annotation::quads`) covered `[0 95 46 110]`, AB included, while applying left AB.
/// The view now gives the area applying removes, the /Rect, and AB stays as shown.
#[test]
fn the_null_corner_mark_previews_what_apply_removes() {
    let (mut s, id) = open(ab1234cd(&[NULL_MARK], &[]));
    let doc = s.get(id).unwrap();
    let old = doc.info.annotations.iter().find(|a| a.subtype == "Redact").unwrap();
    let q = old.quads[0];
    let old_fold = [
        q[0].min(q[2]).min(q[4]).min(q[6]),
        q[1].min(q[3]).min(q[5]).min(q[7]),
        q[0].max(q[2]).max(q[4]).max(q[6]),
        q[1].max(q[3]).max(q[5]).max(q[7]),
    ];
    assert_eq!(old_fold, [0.0, 95.0, 46.0, 110.0], "the fixture still tells the old reading apart");
    assert_eq!(ready(&s, id, 0, 0), vec![AREA], "not [0 95 46 110]");
    assert_eq!(doc.redaction_marks(), 1);
    let applied: Vec<Vec<[f64; 4]>> = pdfcraft_redact::marks(&doc.editor.as_ref().unwrap().cos).into_iter().map(|m| m.rects).collect();
    assert_eq!(applied, vec![vec![AREA]]);
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    let text = saved_text(&s, id);
    assert!(!text.contains("1234") && text.contains("AB") && text.contains("CD"), "{text}");
}

/// Decisive regression 2. The view stays the collector's reading of the current graph through
/// comment, full, metadata and batch refreshes, undo and redo. The indirect corner is the case
/// the renderer's inspection read as 0 at open while the comment list resolved it after an edit.
#[test]
fn the_view_is_the_same_reading_at_open_and_after_every_refresh() {
    let indirect = "<< /Type /Annot /Subtype /Redact /QuadPoints [8 0 R 110 46 110 22 95 46 95] >>";
    let (mut s, id) = open(ab1234cd(&[NULL_MARK, indirect], &["22"]));
    assert_eq!(s.get(id).unwrap().info.annotations.iter().filter(|a| a.subtype == "Redact").nth(1).unwrap().quads[0][0], 0.0);
    let both = |s: &Session| (ready(s, id, 0, 0), ready(s, id, 0, 1));
    let at_open = both(&s);
    assert_eq!(at_open, (vec![AREA], vec![AREA]));
    assert_current(&s, id);
    let note = Shape::Note { at: [150.0, 150.0], icon: Default::default() };
    s.apply(
        id,
        Edit::AddAnnotation(NewAnnotation { page: 0, style: Style::default_for(&note), shape: note, contents: "n".into(), author: "A".into() }),
    )
    .unwrap();
    assert_eq!(both(&s), at_open, "comment refresh");
    assert_current(&s, id);
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    assert_eq!(both(&s), at_open, "full refresh: user space is unchanged by /Rotate");
    assert_current(&s, id);
    s.apply(id, Edit::SetInfo { key: "Title".into(), value: "t".into() }).unwrap();
    assert_eq!(both(&s), at_open, "metadata refresh");
    assert_current(&s, id);
    for _ in 0..3 {
        s.undo(id).unwrap();
        assert_eq!(both(&s), at_open, "undo");
        assert_current(&s, id);
    }
    s.redo(id).unwrap();
    assert_current(&s, id);
    // A batch that moves the indirect-cornered mark: the view follows the graph.
    s.apply(id, Edit::Batch { label: "Move".into(), edits: vec![Edit::MoveAnnotation { page: 0, index: 1, dx: 10.0, dy: 0.0 }] }).unwrap();
    assert_current(&s, id);
    assert_ne!(ready(&s, id, 0, 1), vec![AREA], "the moved mark is shown where it now is");
}

/// Decisive regression 4. A mark that can't be applied (mixed quads without /Rect; one written in
/// place) is reported with its typed reason and no removal area, and applying refuses with
/// nothing changed in the saved file.
#[test]
fn a_refused_mark_shows_no_removal_area_and_refuses_the_apply() {
    let mixed = "<< /Type /Annot /Subtype /Redact /QuadPoints [22 110 28 110 22 95 28 95 null 1 2 3 4 5 6 7] >>";
    let mut bytes = ab1234cd(&[mixed], &[]);
    // A second, inline mark: the page's list holds the dictionary itself.
    let inline = b"6 0 R ] >>";
    let at = bytes.windows(inline.len()).position(|w| w == inline).unwrap();
    bytes.splice(at..at + inline.len(), b"6 0 R << /Type /Annot /Subtype /Redact /Rect [22 95 46 110] >> ] >>".iter().copied());
    // The splice moved later objects: let the reader repair the cross-reference table.
    let (mut s, id) = open(bytes);
    let doc = s.get(id).unwrap();
    let views = doc.redaction_previews().on_page(0).to_vec();
    assert_eq!(views.len(), 2);
    assert_eq!(views[0].areas, Err(MarkRefusal::Geometry(MarkAreaError::QuadNeedsRect)));
    assert_eq!((views[0].removal_rects(), views[0].locator), (&[][..], None), "no made-up rectangle to point at");
    assert_eq!((views[1].index, views[1].object, views[1].areas.clone()), (1, None, Err(MarkRefusal::InlineAnnotation)));
    assert_eq!((views[1].removal_rects(), views[1].locator), (&[][..], Some(AREA)), "located by its /Rect, nothing to remove");
    assert_eq!(doc.redaction_marks(), 2);
    let before = s.save_bytes(id).unwrap();
    let r = s.apply(id, Edit::ApplyRedactions { pages: None });
    assert!(matches!(r, Err(EditError::Redact(pdfcraft_redact::RedactError::UnreadableMark(2)))), "{r:?}");
    assert_eq!(s.save_bytes(id).unwrap(), before, "nothing changed");
    assert!(saved_text(&s, id).contains("AB1234CD"));
}

/// Decisive regression 7. Slots that move, a failed refresh, an unavailable or stale view, a
/// batch, a comment list that leaves the mark out, and a mark past the cap: never a stale or
/// partial "ready", and applying refuses whenever the view can't be trusted.
#[test]
fn stale_unavailable_capped_or_unlisted_views_never_apply() {
    let note = "<< /Type /Annot /Subtype /Text /Rect [150 150 170 170] /Contents (n) >>";
    let (mut s, id) = open(ab1234cd(&[note, NULL_MARK], &[]));
    assert_eq!(ready(&s, id, 0, 1), vec![AREA]);
    // Deleting the note moves the mark to slot 0: nothing is left ready at slot 1.
    s.apply(id, Edit::DeleteAnnotation { page: 0, index: 0 }).unwrap();
    assert_eq!(ready(&s, id, 0, 0), vec![AREA]);
    assert!(s.get(id).unwrap().redaction_previews().get(0, 1).is_none());
    assert_current(&s, id);
    // A refresh that fails rolls back and leaves the view of the graph it rolled back to.
    crate::test_hooks::panic_on_refresh(1);
    assert!(s.apply(id, Edit::SetAnnotationContents { page: 0, index: 0, text: "x".into() }).is_err());
    assert_current(&s, id);
    // The comment list leaving the mark out (its cap) hides nothing from the count or the view.
    s.doc_mut(id).unwrap().info.annotations.clear();
    assert_eq!(s.get(id).unwrap().redaction_marks(), 1);
    assert_eq!(ready(&s, id, 0, 0), vec![AREA]);
    // A batch may not apply redactions.
    let r = s.apply(id, Edit::Batch { label: "b".into(), edits: vec![Edit::ApplyRedactions { pages: None }] });
    assert!(matches!(&r, Err(EditError::Invalid(m)) if m.contains("inside a batch")), "{r:?}");
    // An unavailable view refuses.
    s.doc_mut(id).unwrap().redaction_previews = RedactionPreviews::Unavailable(PreviewUnavailable::Display);
    let r = s.apply(id, Edit::ApplyRedactions { pages: None });
    assert!(matches!(&r, Err(EditError::Invalid(m)) if m.contains("can't be shown")), "{r:?}");
    // A view that isn't the graph's reading refuses.
    s.doc_mut(id).unwrap().redaction_previews = RedactionPreviews::Available(Vec::new());
    let r = s.apply(id, Edit::ApplyRedactions { pages: None });
    assert!(matches!(&r, Err(EditError::Invalid(m)) if m.contains("changed since")), "{r:?}");
    assert!(saved_text(&s, id).contains("AB1234CD"), "nothing was redacted");
    // Past the per-mark cap: refused even with a usable /Rect, and applying refuses.
    let mut q = vec!["22 110 46 110 22 95 46 95".to_string()];
    q.extend(std::iter::repeat_n("1 1 1 1 1 1 1 1".to_string(), pdfcraft_annot::redaction::MAX_QUADPOINT_VALUES / 8));
    let capped = format!("<< /Type /Annot /Subtype /Redact /Rect [22 95 46 110] /QuadPoints [{}] >>", q.join(" "));
    let (mut s, id) = open(ab1234cd(&[&capped], &[]));
    let view = s.get(id).unwrap().redaction_previews().get(0, 0).unwrap().clone();
    assert_eq!(view.areas, Err(MarkRefusal::Geometry(MarkAreaError::TooManyValues)));
    assert!(view.removal_rects().is_empty());
    assert!(s.apply(id, Edit::ApplyRedactions { pages: None }).is_err());
    assert!(saved_text(&s, id).contains("AB1234CD"));
}

/// The Redact mark at `index` on page 1 of `bytes`: its dictionary and normal appearance.
fn mark_in(bytes: &Arc<Vec<u8>>, index: usize) -> (pdfcraft_cos::Dict, pdfcraft_cos::Stream) {
    let shown = pdfcraft_cos::Document::open(bytes.clone()).unwrap();
    let page = pdfcraft_model::pages(&shown).swap_remove(0);
    let list = shown.resolve(page.dict.get(b"Annots").unwrap()).as_array().unwrap().clone();
    let d = shown.resolve(&list[index]).as_dict().unwrap().clone();
    let ap = shown.resolve(d.get(b"AP").unwrap()).as_dict().unwrap().clone();
    let pdfcraft_cos::Object::Stream(n) = (*shown.resolve(ap.get(b"N").unwrap())).clone() else { panic!("no /N stream") };
    (d, n)
}

fn nums(o: &pdfcraft_cos::Object) -> Vec<f64> {
    o.as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect()
}

/// Whether `bytes` draws page 1 dark at user point (x, y) (a 200 × 200 page at scale 2).
fn dark_at(bytes: &Arc<Vec<u8>>, x: f64, y: f64) -> bool {
    let mut r = pdfcraft_render::PageRenderer::new(bytes.clone(), pdfcraft_render::RenderConfig::default());
    let out = r.render(pdfcraft_render::RenderRequest { page: 0, scale: 2.0, ..Default::default() });
    let (px, py) = ((x * 2.0) as usize, ((200.0 - y) * 2.0) as usize);
    let i = (py * out.width as usize + px) * 4;
    let p = &out.rgba[i..i + 4];
    u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]) < 200
}

/// Decisive regression 5. A mark whose own appearance fills a box over AB (its /Rect is wider than
/// its quads, and it is NoZoom and NoRotate), and one with no appearance, partial quads and a
/// /Rect smaller than its usable quad: the page shows each as the outline of exactly its checked
/// areas, placed in the page's own space with no scaling, while the file keeps every original
/// dictionary, appearance and key.
#[test]
fn the_page_draws_the_checked_outline_whatever_the_mark_carries() {
    let lying = "<< /Type /Annot /Subtype /Redact /F 24 /Rect [10 95 46 110] /QuadPoints [22 110 46 110 22 95 46 95] /AP << /N 8 0 R /D 8 0 R >> /PCKeep 1 >>";
    let partial = "<< /Type /Annot /Subtype /Redact /Rect [100 150 110 160] /QuadPoints [120 110 140 110 120 95 140 95 1 2 3] >>";
    let filled = "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 16 >>\nstream\n0 g 0 0 1 1 re f\nendstream";
    let original = Arc::new(ab1234cd(&[lying, partial], &[filled]));
    let (s, id) = open(original.to_vec());
    // The file's own appearance fills under AB (below the baseline, where no glyph inks); the
    // checked outline leaves it white, and applying leaves AB.
    assert!(dark_at(&original, 16.0, 96.5), "the fixture's appearance fills AB");
    let doc = s.get(id).unwrap();
    assert!(!dark_at(&doc.display, 16.0, 96.5), "the page doesn't show AB as removed");
    let (d, n) = mark_in(&doc.display, 0);
    assert_eq!(nums(d.get(b"Rect").unwrap()), vec![21.0, 94.0, 47.0, 111.0], "the checked area, not the wider /Rect");
    assert_eq!(nums(n.dict.get(b"BBox").unwrap()), nums(d.get(b"Rect").unwrap()), "placed with no scaling");
    assert!(n.dict.get(b"Matrix").is_none());
    assert_eq!(d.int(b"F"), Some(0), "NoZoom and NoRotate are cleared in the view");
    assert!(d.get(b"AP").unwrap().as_dict().unwrap().get(b"D").is_none(), "no other appearance to show on press");
    assert_eq!(d.int(b"PCKeep"), Some(1), "other keys stay");
    let content = String::from_utf8_lossy(&n.decoded().unwrap()).into_owned();
    assert!(content.contains("22 95 24 15 re S") && !content.contains(" f"), "{content}");
    // No appearance, partial quads, /Rect smaller than the usable quad: both areas outlined.
    let (d, n) = mark_in(&doc.display, 1);
    let content = String::from_utf8_lossy(&n.decoded().unwrap()).into_owned();
    assert!(content.contains("120 95 20 15 re S") && content.contains("100 150 10 10 re S"), "{content}");
    assert_eq!(nums(d.get(b"Rect").unwrap()), vec![99.0, 94.0, 141.0, 161.0]);
    // None of this is the document: its graph and its saved bytes are the original.
    let cos = &doc.editor.as_ref().unwrap().cos;
    let first = cos.get(pdfcraft_cos::ObjRef::new(6, 0));
    let first = first.as_dict().unwrap();
    assert_eq!((first.int(b"F"), nums(first.get(b"Rect").unwrap())), (Some(24), vec![10.0, 95.0, 46.0, 110.0]));
    assert_eq!(s.save_bytes(id).unwrap().as_slice(), original.as_slice());
    // The isolated image a drag draws is the same outline.
    let layer = doc.comment_layer(0, &[0]).unwrap().unwrap();
    let (d, _) = mark_in(&layer.bytes, 0);
    assert_eq!(nums(d.get(b"Rect").unwrap()), vec![21.0, 94.0, 47.0, 111.0]);
}

/// A mark that can't be applied is drawn as a dashed warning at its /Rect, never its own
/// appearance's fill; with no safe /Rect it isn't drawn on the page at all.
#[test]
fn a_refused_mark_is_drawn_as_a_warning_or_not_at_all() {
    let located = "<< /Type /Annot /Subtype /Redact /Rect [10 95 46 110] /QuadPoints /oops /PCInline 1 >>";
    let lost = "<< /Type /Annot /Subtype /Redact /QuadPoints [10 110 22 110 10 95 22 95 null 1 2 3 4 5 6 7] /AP << /N 8 0 R >> >>";
    let filled = "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 16 >>\nstream\n0 g 0 0 1 1 re f\nendstream";
    // The first is refused by being written in place: splice it into the list.
    let mut bytes = ab1234cd(&[lost], &[filled]);
    let at = bytes.windows(10).position(|w| w == b"[6 0 R ] >").unwrap();
    bytes.splice(at + 1..at + 1, format!("{located} ").bytes());
    let (s, id) = open(bytes);
    let doc = s.get(id).unwrap();
    assert_eq!(doc.redaction_previews().refused(), 2);
    let (d, n) = mark_in(&doc.display, 0);
    let content = String::from_utf8_lossy(&n.decoded().unwrap()).into_owned();
    assert!(content.contains("[3 2] 0 d") && content.contains("10 95 36 15 re S"), "{content}");
    assert_eq!(d.int(b"PCInline"), Some(1));
    let shown = pdfcraft_cos::Document::open(doc.display.clone()).unwrap();
    let page = pdfcraft_model::pages(&shown).swap_remove(0);
    let list = shown.resolve(page.dict.get(b"Annots").unwrap()).as_array().unwrap().clone();
    let lost = shown.resolve(&list[1]).as_dict().unwrap().clone();
    assert!(lost.get(b"AP").is_none() && lost.int(b"F").is_some_and(|f| f & 2 != 0), "hidden: nothing safe to draw");
    assert!(!dark_at(&doc.display, 16.0, 96.5), "its own appearance's fill is not shown");
}

/// When the marks can't be drawn as they would be applied, the view is unavailable and
/// applying is refused; the document stays open and nothing changes.
#[test]
fn a_display_that_cant_be_drawn_blocks_applying() {
    crate::redaction_preview::FAIL_DISPLAY.with(|f| f.set(true));
    let (mut s, id) = open(ab1234cd(&[NULL_MARK], &[]));
    crate::redaction_preview::FAIL_DISPLAY.with(|f| f.set(false));
    assert_eq!(s.get(id).unwrap().redaction_previews(), &RedactionPreviews::Unavailable(PreviewUnavailable::Display));
    let r = s.apply(id, Edit::ApplyRedactions { pages: None });
    assert!(matches!(&r, Err(EditError::Invalid(m)) if m.contains("can't be shown")), "{r:?}");
    assert!(saved_text(&s, id).contains("AB1234CD"));
}

/// Decisive regression 6. Separate probes wholly inside each shown area, in the gap between
/// them and outside them: after applying, through the library and through the session (whose
/// renderer check runs too), saved and reopened, no probe under an area can be extracted or is
/// drawn, and the others all stay.
#[test]
fn what_the_preview_covers_is_gone_and_nothing_else() {
    let body = "BT /F1 10 Tf 10 100 Td (QXONE) Tj ET BT /F1 10 Tf 10 130 Td (QXGAP) Tj ET \
                BT /F1 10 Tf 10 160 Td (QXTWO) Tj ET BT /F1 10 Tf 120 100 Td (QXOUT) Tj ET";
    let mark = "<< /Type /Annot /Subtype /Redact /QuadPoints [8 110 42 110 8 95 42 95 8 170 42 170 8 155 42 155] >>";
    let bytes = page_with(body, &[mark], &[]);
    let (mut s, id) = open(bytes.clone());
    // Each covered probe (5 Courier glyphs, x 10–40, inked within y −1.6…+6.3 of its baseline) is
    // wholly inside one shown area; the gap and outside probes are wholly outside them all.
    let areas = ready(&s, id, 0, 0);
    let inside = |b: [f64; 4]| areas.iter().any(|a| a[0] <= b[0] && a[1] <= b[1] && b[2] <= a[2] && b[3] <= a[3]);
    let apart = |b: [f64; 4]| areas.iter().all(|a| b[2] <= a[0] || a[2] <= b[0] || b[3] <= a[1] || a[3] <= b[1]);
    assert!(inside([10.0, 98.0, 40.0, 107.0]) && inside([10.0, 158.0, 40.0, 167.0]), "{areas:?}");
    assert!(apart([10.0, 128.0, 40.0, 137.0]) && apart([120.0, 98.0, 150.0, 107.0]), "{areas:?}");
    let check = |text: String, how: &str| {
        assert!(!text.contains("QXONE") && !text.contains("QXTWO"), "{how}: {text}");
        assert!(text.contains("QXGAP") && text.contains("QXOUT"), "{how}: {text}");
    };
    // The library, on its own.
    let mut cos = pdfcraft_cos::Document::open(Arc::new(bytes)).unwrap();
    pdfcraft_redact::apply(&mut cos, None).unwrap();
    let written = pdfcraft_cos::write_full(&cos, &pdfcraft_cos::SaveOptions::default()).unwrap();
    let (s2, id2) = open(written.clone());
    check(page_texts(&s2, id2).concat(), "library");
    assert!(!written.windows(5).any(|w| w == b"QXONE" || w == b"QXTWO"), "no covered probe left in the file");
    // The session, with its renderer check.
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    check(saved_text(&s, id), "session");
}

// ---- Review 2 regressions (Sol): page identity, indirect /Subtype, every page picture, metadata,
// and the fallbacks said.

/// Equal page geometry doesn't prove the same pages: a tree that lists page 3 twice and an
/// untyped node of two pages is walked [3, 7, 8] by the model and [3, 3, 6] by the renderer.
#[test]
fn equal_page_geometry_does_not_prove_page_identity() {
    let body = "BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET";
    let bytes = pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 3 0 R 6 0 R] /Count 3 /MediaBox [0 0 200 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>".into(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
        "<< /Parent 2 0 R /Kids [7 0 R 8 0 R] /MediaBox [0 0 200 200] >>".into(),
        "<< /Type /Page /Parent 6 0 R /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R /Annots [9 0 R] >>".into(),
        "<< /Type /Page /Parent 6 0 R >>".into(),
        NULL_MARK.into(),
    ]);
    let (s, id) = open(bytes);
    let doc = s.get(id).unwrap();
    let model: Vec<_> = pdfcraft_model::pages(&doc.editor.as_ref().unwrap().cos).iter().map(|p| (p.obj.num, p.obj.generation)).collect();
    assert_eq!(model, vec![(3, 0), (7, 0), (8, 0)]);
    assert_eq!(doc.info.page_objects, vec![Some((3, 0)), Some((3, 0)), Some((6, 0))], "the renderer's walk");
    assert_eq!(doc.redaction_previews(), &RedactionPreviews::Unavailable(PreviewUnavailable::PageMapping));
}

/// With those trees apply used to succeed and leave the content painted under the area shown
/// on page 2. Now it is refused, and the page keeps its content honestly.
#[test]
fn a_page_identity_mismatch_refuses_the_apply() {
    let body = "0 g 22 95 24 15 re f";
    let bytes = pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 3 0 R 6 0 R] /Count 3 /MediaBox [0 0 200 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>".into(),
        "null".into(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
        "<< /Parent 2 0 R /Kids [7 0 R 8 0 R] /MediaBox [0 0 200 200] >>".into(),
        "<< /Type /Page /Parent 6 0 R /Annots [9 0 R] >>".into(),
        "<< /Type /Page /Parent 6 0 R >>".into(),
        "<< /Type /Annot /Subtype /Redact /Rect [22 95 46 110] >>".into(),
    ]);
    let (mut s, id) = open(bytes);
    assert!(!s.get(id).unwrap().redaction_previews().is_available());
    let r = s.apply(id, Edit::ApplyRedactions { pages: Some(vec![1]) });
    assert!(matches!(&r, Err(EditError::Invalid(m)) if m.contains("don't match the pages shown")), "{r:?}");
}

/// A Redact whose /Subtype is a reference is drawn by the renderer as a mark. It used to be
/// skipped by apply (which read /Subtype directly) while its fill stayed on the page. It is now
/// listed, refused with its reason, drawn only as a warning, and applying is refused.
#[test]
fn a_mark_with_an_indirect_subtype_is_refused_not_skipped() {
    let indirect = "<< /Type /Annot /Subtype 8 0 R /Rect [8 95 22 110] /AP << /N 9 0 R >> >>";
    let direct = "<< /Type /Annot /Subtype /Redact /Rect [22 95 46 110] >>";
    let filled = "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 16 >>\nstream\n0 g 0 0 1 1 re f\nendstream";
    let original = Arc::new(ab1234cd(&[indirect, direct], &["/Redact", filled]));
    assert!(dark_at(&original, 16.0, 96.5), "the file's own appearance fills AB");
    let (mut s, id) = open(original.to_vec());
    let doc = s.get(id).unwrap();
    assert_eq!(doc.info.annotations.iter().filter(|a| a.subtype == "Redact").count(), 2, "the renderer lists both");
    let views = doc.redaction_previews().on_page(0);
    assert_eq!(views.len(), 2);
    assert_eq!(views[0].areas, Err(MarkRefusal::IndirectSubtype));
    assert!(views[0].removal_rects().is_empty());
    assert_eq!(doc.redaction_marks(), 2);
    assert!(!dark_at(&doc.display, 16.0, 96.5), "drawn as a warning, not its own fill");
    let r = s.apply(id, Edit::ApplyRedactions { pages: None });
    assert!(matches!(r, Err(EditError::Redact(pdfcraft_redact::RedactError::UnreadableMark(1)))), "{r:?}");
    assert!(saved_text(&s, id).contains("AB1234CD"));
    // Clear all removes it with the others, refused or not.
    s.apply(id, Edit::ClearRedactions).unwrap();
    assert_eq!(s.get(id).unwrap().redaction_marks(), 0);
}

/// A Redact the page lists that the collector doesn't read leaves the view unavailable, never a
/// silent "ready".
#[test]
fn a_shown_mark_the_collector_lacks_makes_the_view_unavailable() {
    let (s, id) = open(ab1234cd(&[NULL_MARK], &[]));
    let doc = s.get(id).unwrap();
    let mut info = doc.info.clone();
    let mut extra = info.annotations.iter().find(|a| a.subtype == "Redact").unwrap().clone();
    extra.index = 5;
    info.annotations.push(extra);
    let view = crate::redaction_preview::previews_for(&doc.editor.as_ref().unwrap().cos, &info);
    assert_eq!(view, RedactionPreviews::Unavailable(PreviewUnavailable::UnlistedMark));
}

/// A metadata edit keeps the display copy: when that copy couldn't draw the marks, the view stays
/// unavailable (and applying refused) instead of being made ready over the old raster.
#[test]
fn a_metadata_edit_does_not_make_a_failed_display_ready() {
    let lying = "<< /Type /Annot /Subtype /Redact /Rect [10 95 46 110] /QuadPoints [22 110 46 110 22 95 46 95] /AP << /N 7 0 R >> >>";
    let filled = "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 16 >>\nstream\n0 g 0 0 1 1 re f\nendstream";
    crate::redaction_preview::FAIL_DISPLAY.with(|f| f.set(true));
    let (mut s, id) = open(ab1234cd(&[lying], &[filled]));
    crate::redaction_preview::FAIL_DISPLAY.with(|f| f.set(false));
    assert!(dark_at(&s.get(id).unwrap().display, 16.0, 96.5));
    s.apply(id, Edit::SetInfo { key: "Title".into(), value: "new title".into() }).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!(doc.redaction_previews(), &RedactionPreviews::Unavailable(PreviewUnavailable::Display));
    assert!(s.apply(id, Edit::ApplyRedactions { pages: None }).is_err());
    assert!(saved_text(&s, id).contains("AB1234CD"));
    // A refresh that draws the display makes it ready again.
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    assert!(s.get(id).unwrap().redaction_previews().is_available());
}

/// Every picture of the page made from a copy of the graph draws the marks as the page view does:
/// a stamp drag's background, a text edit's backdrop, a signature drag's, the isolated drag image,
/// and vector print (Print as Image reads the display copy).
#[test]
fn every_picture_of_the_page_draws_the_checked_outline() {
    let lying = "<< /Type /Annot /Subtype /Redact /Rect [10 95 46 110] /QuadPoints [22 110 46 110 22 95 46 95] /F 4 /AP << /N 8 0 R >> >>";
    let stamp = "<< /Type /Annot /Subtype /Stamp /Rect [100 150 120 170] /AP << /N 9 0 R >> >>";
    let filled = "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 16 >>\nstream\n0 g 0 0 1 1 re f\nendstream";
    let (s, id) = open(ab1234cd(&[lying, stamp], &[filled, filled]));
    let doc = s.get(id).unwrap();
    assert!(!dark_at(&doc.display, 16.0, 96.5));
    let pixel_dark = |out: &pdfcraft_render::RenderedPage| {
        let i = (207 * out.width as usize + 32) * 4;
        u32::from(out.rgba[i]) + u32::from(out.rgba[i + 1]) + u32::from(out.rgba[i + 2]) < 200
    };
    let background = doc.stamp_preview(0, 1).unwrap().unwrap().render_background(2.0).unwrap();
    assert!(!pixel_dark(&background), "a stamp drag's background");
    let backdrop = doc.text_edit_preview(0, 0).unwrap();
    assert!(!pixel_dark(&backdrop.render_background(2.0).unwrap()), "a text edit's backdrop");
    let settings = pdfcraft_print::Settings {
        pages: vec![0],
        paper: (200.0, 200.0),
        layout: pdfcraft_print::Layout::Size(pdfcraft_print::SizeMode::Actual),
        ..Default::default()
    };
    let vector = Arc::new(s.print_pdf(id, &settings).unwrap());
    assert!(!dark_at(&vector, 16.0, 96.5), "vector print");
    let raster = pdfcraft_print::Settings { raster: Some(pdfcraft_print::Raster { dpi: 144.0, grayscale: false }), ..settings };
    assert!(!dark_at(&Arc::new(s.print_pdf(id, &raster).unwrap()), 16.0, 96.5), "print as image");
}

/// A picture of the page that can't draw the marks refuses applying until the next refresh.
#[test]
fn a_page_picture_that_cant_draw_the_marks_blocks_applying() {
    let stamp = "<< /Type /Annot /Subtype /Stamp /Rect [100 150 120 170] /AP << /N 8 0 R >> >>";
    let filled = "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 16 >>\nstream\n0 g 0 0 1 1 re f\nendstream";
    let (mut s, id) = open(ab1234cd(&[NULL_MARK, stamp], &[filled]));
    crate::redaction_preview::FAIL_DISPLAY.with(|f| f.set(true));
    let preview = s.get(id).unwrap().stamp_preview(0, 1);
    crate::redaction_preview::FAIL_DISPLAY.with(|f| f.set(false));
    assert!(preview.is_err());
    let r = s.apply(id, Edit::ApplyRedactions { pages: None });
    assert!(matches!(&r, Err(EditError::Invalid(m)) if m.contains("couldn't draw the redaction marks")), "{r:?}");
    // The next refresh draws the display again, and applying goes ahead.
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
}

/// A mark applied through its /Rect (an unreadable quad) is said in the edit's notices and listed
/// in the report: what went is what was shown, and how it was read.
#[test]
fn a_fallback_reading_is_said_when_applied() {
    let (mut s, id) = open(ab1234cd(&[NULL_MARK], &[]));
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    let notices = &s.get(id).unwrap().edit_notices;
    let Some(crate::EditNotice::RedactionFallbacks { marks }) = notices.first() else { panic!("{notices:?}") };
    assert_eq!(marks.len(), 1);
    assert!(marks[0].reading.used_rect && marks[0].reading.quad_issues.invalid_groups == 1);
    assert!(notices[0].to_string().contains("through their Rect"), "{}", notices[0]);
}

/// `/UserUnit` doesn't change the checked areas, which are in user space, nor what applying
/// removes.
#[test]
fn user_unit_keeps_the_checked_mapping() {
    let mut bytes = ab1234cd(&[NULL_MARK], &[]);
    let at = bytes.windows(10).position(|w| w == b"/Resources").unwrap();
    bytes.splice(at..at, b"/UserUnit 2 ".iter().copied());
    let (mut s, id) = open(bytes);
    assert_eq!(ready(&s, id, 0, 0), vec![AREA]);
    assert_eq!(s.get(id).unwrap().user_unit(0), Some(2.0));
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    let text = saved_text(&s, id);
    assert!(text.contains("AB") && text.contains("CD") && !text.contains("1234"), "{text}");
}

/// Encrypted documents keep their saved bytes untouched by the display copy and apply the checked
/// area.
#[test]
fn encrypted_documents_keep_the_original_and_apply_the_checked_area() {
    for algorithm in [pdfcraft_cos::Algorithm::Rc4_128, pdfcraft_cos::Algorithm::Aes128, pdfcraft_cos::Algorithm::Aes256] {
        let mut cos = pdfcraft_cos::Document::open(Arc::new(ab1234cd(&[NULL_MARK], &[]))).unwrap();
        cos.set_encryption(&pdfcraft_cos::NewEncryption {
            algorithm,
            user_password: "u",
            owner_password: "o",
            permissions: -4,
            encrypt_metadata: true,
            seed: [1; 32],
        })
        .unwrap();
        let original = Arc::new(pdfcraft_cos::write_full(&cos, &Default::default()).unwrap());
        let mut s = Session::new();
        let id = s.open("encrypted.pdf", None, original.clone(), Some("o")).unwrap();
        assert!(s.get(id).unwrap().redaction_previews().is_available(), "{algorithm:?}");
        assert_eq!(s.save_bytes(id).unwrap().as_slice(), original.as_slice(), "{algorithm:?}");
        s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
        let mut reopened = Session::new();
        let id2 = reopened.open("saved.pdf", None, s.save_bytes(id).unwrap(), Some("o")).unwrap();
        let text = page_texts(&reopened, id2).concat();
        assert!(text.contains("AB") && text.contains("CD") && !text.contains("1234"), "{algorithm:?}: {text}");
    }
}
