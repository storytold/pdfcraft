//! Contributor-original synthetic pages: a form XObject whose content has more `Q` than `q`.
//! A content stream balances its own `q` and `Q` (ISO 32000-2 §8.4.2, §8.10.1), so a `Q` with
//! nothing of its own stream to restore is ignored and cannot pop the states of the page that
//! drew the form (vendored hayro-interpret patch 68). Each case is paired with the same page
//! drawing a balanced form, which must come out the same.
use pdfcraft_render::{PageRenderer, PageText, RenderConfig, RenderRequest, RequestKind};
use std::sync::Arc;

/// Forms /B (balanced), /S2 (two stray `Q`s) and /S3 (three), each moving its own pen.
const FORMS: [(&str, &[u8]); 3] =
    [("B", b"q BT /F1 12 Tf 200 0 Td ET Q"), ("S2", b"BT /F1 12 Tf 200 0 Td ET Q Q"), ("S3", b"BT /F1 12 Tf 200 0 Td ET Q Q Q")];

/// A page that shifts and clips inside `q … Q`, draws form `form` there, then text and a fill
/// that depend on the shift and the clip, and after its `Q` text that depends on neither.
fn page(form: &str, in_text: bool) -> String {
    let draw = if in_text {
        format!("BT /F1 12 Tf 72 300 Td (AA) Tj /{form} Do (SECRET) Tj ET")
    } else {
        format!("BT /F1 12 Tf 72 300 Td (AA) Tj ET /{form} Do BT /F1 12 Tf 72 280 Td (SECRET) Tj ET")
    };
    format!("q 1 0 0 1 20 -100 cm 0 0 200 400 re W n {draw} 0 0 1 rg 0 0 400 50 re f Q BT /F1 12 Tf 72 360 Td (TAIL) Tj ET")
}

/// One 400 × 400 page per entry of `contents`, with Helvetica as /F1 and the forms of `FORMS`.
fn pdf(contents: &[String]) -> Arc<Vec<u8>> {
    let first_page = 4 + FORMS.len();
    let n = contents.len();
    let kids: String = (0..n).map(|i| format!("{} 0 R ", first_page + 2 * i)).collect();
    let xobjects: String = FORMS.iter().enumerate().map(|(i, (name, _))| format!("/{name} {} 0 R ", 4 + i)).collect();
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {n} >>").into_bytes(),
        format!("<< /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> /XObject << {xobjects}>> >>").into_bytes(),
    ];
    for (_, body) in FORMS {
        objects.push(stream(b"/Type /XObject /Subtype /Form /BBox [0 0 400 400] /Resources 3 0 R", body));
    }
    for (i, content) in contents.iter().enumerate() {
        objects.push(
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources 3 0 R /Contents {} 0 R >>", first_page + 1 + 2 * i).into_bytes(),
        );
        objects.push(stream(b"", content.as_bytes()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(object);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    Arc::new(out)
}

fn stream(dict: &[u8], body: &[u8]) -> Vec<u8> {
    let mut s = b"<< ".to_vec();
    s.extend_from_slice(dict);
    s.extend_from_slice(format!(" /Length {} >>\nstream\n", body.len()).as_bytes());
    s.extend_from_slice(body);
    s.extend_from_slice(b"\nendstream");
    s
}

fn request(page: usize, kind: RequestKind) -> RenderRequest {
    RenderRequest { page, kind, tile: None, scale: 1.0, tag: 0, preview: false }
}

fn texts(contents: &[String]) -> Vec<Arc<PageText>> {
    let mut renderer = PageRenderer::new(pdf(contents), RenderConfig::default());
    (0..contents.len())
        .map(|page| {
            let out = renderer.render(request(page, RequestKind::Text));
            assert!(out.error.is_none(), "page {} text error: {:?}", page + 1, out.error);
            out.text.expect("page text")
        })
        .collect()
}

/// The boxes of the glyphs of `word`'s one match on the page.
fn boxes(text: &PageText, word: &str) -> Vec<[f32; 4]> {
    let hits = text.find(word);
    assert_eq!(hits.len(), 1, "one {word} in {:?}", text.plain_text());
    hits[0].clone().map(|i| text.glyphs[i].rect).collect()
}

fn assert_same_boxes(a: &[[f32; 4]], b: &[[f32; 4]]) {
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b) {
        assert!(x.iter().zip(y).all(|(p, q)| (p - q).abs() < 0.01), "{x:?} != {y:?}");
    }
}

fn assert_text_like_the_balanced_form(in_text: bool) {
    let pages = texts(&[page("B", in_text), page("S2", in_text), page("S3", in_text)]);
    for (stray, name) in pages[1..].iter().zip(["S2", "S3"]) {
        for word in ["SECRET", "TAIL"] {
            let (got, want) = (boxes(stray, word), boxes(&pages[0], word));
            assert_eq!(got.len(), want.len(), "{name}: {word}");
            assert_same_boxes(&got, &want);
        }
    }
}

#[test]
fn a_forms_stray_q_does_not_undo_the_pages_cm() {
    assert_text_like_the_balanced_form(false);
}

#[test]
fn a_forms_stray_q_inside_a_text_object_does_not_undo_the_pages_cm() {
    assert_text_like_the_balanced_form(true);
}

#[test]
fn a_forms_stray_q_does_not_undo_the_pages_clip() {
    let contents = [page("B", false), page("S2", false), page("S3", false)];
    let mut renderer = PageRenderer::new(pdf(&contents), RenderConfig::default());
    let balanced = renderer.render(request(0, RequestKind::Pixels));
    assert!(balanced.error.is_none());
    for i in 1..contents.len() {
        let stray = renderer.render(request(i, RequestKind::Pixels));
        assert!(stray.error.is_none());
        assert_eq!((stray.width, stray.height), (balanced.width, balanced.height));
        assert!(stray.rgba == balanced.rgba, "page {} draws differently from the balanced form", i + 1);
    }
}

#[test]
fn a_balanced_q_inside_a_form_still_restores() {
    // The balanced form's pen move stays inside its own `q … Q`, so SECRET sits where the page
    // put it: 72 + 20 from the page's `cm`, and to the right of AA.
    let pages = texts(&[page("B", false)]);
    let secret = boxes(&pages[0], "SECRET");
    assert!((secret[0][0] - 92.0).abs() < 0.5, "SECRET starts at {}", secret[0][0]);
}
