//! A redaction mark on the page is filled, outlined and hit exactly where applying removes, read
//! from the engine's checked view, never from `Annotation::quads` (which read a bad number as 0).

use std::sync::Arc;

use pdfcraft_engine::{DocId, Session};

use super::input::{REFUSED_REDACTION_TIP, hit};
use super::*;
use crate::canvas::DocView;
use crate::page_router::{CommentPart, Owner};

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

/// `AB1234CD` in 10 pt Courier at (10, 100) (AB is x 10–22, 1234 22–46), on a page turned by
/// `rotate`, with a crop box, and the marks `annots` (objects 6, 7, …).
fn page(rotate: u16, annots: &[&str]) -> Vec<u8> {
    let list: String = (0..annots.len()).map(|i| format!("{} 0 R ", i + 6)).collect();
    let body = "BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET";
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_string(),
        format!(
            "<< /Type /Page /Parent 2 0 R /CropBox [5 5 195 290] /Rotate {rotate} /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R /Annots [{list}] >>"
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>".to_string(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
    objs.extend(annots.iter().map(|s| s.to_string()));
    pdf_of(&objs)
}

const NULL_MARK: &str = "<< /Type /Annot /Subtype /Redact /Rect [22 95 46 110] /QuadPoints [null 110 46 110 22 95 46 95] >>";
const AREA: [f64; 4] = [22.0, 95.0, 46.0, 110.0];

fn open(bytes: Vec<u8>) -> (Session, DocId) {
    let mut s = Session::new();
    let id = s.open("marks.pdf", None, Arc::new(bytes), None).unwrap();
    (s, id)
}

/// A page context as the canvas builds one, its pointer at `at` (a user-space point).
fn with_cx<T>(doc: &pdfcraft_engine::Document, view_rot: u16, f: impl FnOnce(&PageCx<'_>) -> T) -> T {
    let info = &doc.info;
    let p = &info.pages[0];
    // Zoomed 1.5×, away from the origin; the view turned on top of the page's own /Rotate.
    let (w, h) = if view_rot.is_multiple_of(180) { (p.width, p.height) } else { (p.height, p.width) };
    let xf = PageXform { rect: Rect::from_min_size(pos2(30.0, 40.0), vec2(w * 1.5, h * 1.5)), rot: view_rot, pw: p.width, ph: p.height };
    let prefs = CommentPrefs::default();
    let cx = PageCx {
        page: 0,
        xf: &xf,
        ptr: &PagePointer::default(),
        info,
        tool: QuickTool::Select,
        prefs: &prefs,
        allowed: true,
        hidden: false,
        redact_mark: ([0.0; 3], 1.0),
        redactions: doc.redaction_previews(),
    };
    f(&cx)
}

/// Decisive regression 3: through every page and view rotation, zoom and crop, the fill is the
/// checked area (which the engine tests show is what applying removes), its screen rectangle maps
/// back onto it, a press over AB doesn't hit the mark, and one over 1234 does.
#[test]
fn a_redaction_mark_fills_and_is_hit_exactly_at_its_checked_area() {
    for rotate in [0, 90, 180, 270] {
        for view_rot in [0, 90] {
            let (s, id) = open(page(rotate, &[NULL_MARK]));
            let doc = s.get(id).unwrap();
            let a = doc.info.annotations.iter().find(|a| a.subtype == "Redact").unwrap();
            assert_eq!(a.quads[0][0], 0.0, "the comment list still reads the null as 0");
            with_cx(doc, view_rot, |cx| {
                assert_eq!(cx.redact_user_rects(a), &[AREA][..], "rotate {rotate}, view {view_rot}");
                let fill = cx.redact_rects(a);
                assert_eq!(fill.len(), 1);
                let back = cx.rect_to_user(fill[0]);
                for (b, e) in back.iter().zip(AREA) {
                    assert!((b - e).abs() < 1e-3, "rotate {rotate}, view {view_rot}: {back:?}");
                }
                assert_eq!(cx.screen_rects(a).collect::<Vec<_>>(), fill, "outlined and hit where it is filled");
                let over_ab = cx.to_screen([14.0, 102.0]);
                assert!(cx.comment_at(over_ab).is_none(), "AB is not under the mark");
                let over_1234 = cx.to_screen([34.0, 102.0]);
                assert_eq!(cx.comment_at(over_1234).map(|c| c.index), Some(0));
                let h = hit(cx, &DocView::new(id, &doc.info, Default::default()), over_1234, false).unwrap();
                assert_eq!((h.owner, h.tip), (Owner::Comment { index: 0, part: CommentPart::Body }, None));
            });
        }
    }
}

/// A mark whose area is a tiny fraction of a point is filled out to the nearest `f32` around it,
/// never inside it: the screen rectangle maps back over the whole area.
#[test]
fn the_fill_covers_an_area_that_is_not_an_f32() {
    let mark = "<< /Type /Annot /Subtype /Redact /QuadPoints [22.1 110.3 46.7 110.3 22.1 95.2 46.7 95.2] >>";
    let (s, id) = open(page(0, &[mark]));
    let doc = s.get(id).unwrap();
    let a = doc.info.annotations.iter().find(|a| a.subtype == "Redact").unwrap();
    with_cx(doc, 0, |cx| {
        let r = cx.redact_user_rects(a)[0];
        assert!(r[0] <= 22.1 && r[1] <= 95.2 && r[2] >= 46.7 && r[3] >= 110.3, "{r:?}");
        assert!(r.iter().all(|v| f64::from(*v as f32) == *v), "drawn without rounding: {r:?}");
        assert!(r[0] > 22.1 - 1e-5 && r[2] < 46.7 + 1e-5, "one f32 step at most: {r:?}");
    });
}

/// Decisive regression 4, on the page: a mark that can't be applied gets no fill, is outlined
/// (and hit) only at its warning locator, and its tip says it can't be applied. One with no safe
/// locator isn't on the page at all (the Comments panel lists it).
#[test]
fn a_refused_mark_gets_a_warning_and_no_fill() {
    // A partial quad with a usable /Rect is ready (the /Rect is brought in); the same without a
    // /Rect is refused, and has nothing safe to point at.
    let located = "<< /Type /Annot /Subtype /Redact /Rect [22 95 46 110] /QuadPoints [22 110 28 110 22 95 28 95 1 2 3] >>";
    let mixed = "<< /Type /Annot /Subtype /Redact /QuadPoints [22 110 28 110 22 95 28 95 null 1 2 3 4 5 6 7] >>";
    let (s, id) = open(page(0, &[located, mixed]));
    let doc = s.get(id).unwrap();
    let marks: Vec<&pdfcraft_render::Annotation> = doc.info.annotations.iter().filter(|a| a.subtype == "Redact").collect();
    with_cx(doc, 0, |cx| {
        assert!(!cx.redaction_refused(marks[0]));
        assert!(cx.redaction_refused(marks[1]));
        assert!(cx.redact_rects(marks[1]).is_empty(), "no fill for a refused mark");
        assert!(cx.redact_outline_rects(marks[1]).is_empty(), "no locator: no rectangle is made up");
        assert!(cx.screen_rects(marks[1]).next().is_none());
    });
    // A refused mark with a usable /Rect: located there, with the warning tip.
    let mut bytes = page(0, &[]);
    let at = bytes.windows(4).position(|w| w == b"[] >").unwrap();
    bytes.splice(at..at + 2, b"[<< /Type /Annot /Subtype /Redact /Rect [22 95 46 110] /QuadPoints [22 110 46 110 22 95 46 95] >>]".iter().copied());
    let (s, id) = open(bytes);
    let doc = s.get(id).unwrap();
    let a = doc.info.annotations.iter().find(|a| a.subtype == "Redact").unwrap();
    with_cx(doc, 0, |cx| {
        assert!(cx.redaction_refused(a), "written in place");
        assert!(cx.redact_rects(a).is_empty());
        let outline = cx.redact_outline_rects(a);
        assert_eq!(outline.len(), 1);
        let h = hit(cx, &DocView::new(id, &doc.info, Default::default()), cx.to_screen([34.0, 102.0]), false).unwrap();
        assert_eq!(h.tip.as_deref(), Some(REFUSED_REDACTION_TIP));
    });
}
