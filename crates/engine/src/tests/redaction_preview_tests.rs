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
    let list: String = (0..annots.len()).map(|i| format!("{} 0 R ", i + 6)).collect();
    let body = "BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET";
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
