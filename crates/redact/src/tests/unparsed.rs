//! Bytes the content parser can't read are read past by the redaction walk, so text they hide (a
//! stray `)` or `}` in a `TJ` array) is neither found nor removed, while other readers still draw
//! it. A page whose walk meets such bytes is refused, by Apply Redactions and by Remove Hidden
//! Information.

use super::*;
use crate::sanitize::{Hidden, remove_hidden, sanitize, scan};

/// `TJ` arrays each holding a byte the parser can't read beside the text.
const BROKEN: [&[u8]; 4] = [b"[(TAIL) ) (SECRET)] TJ", b"[(TAIL) } (SECRET)] TJ", b"[(TAILSECRET) )] TJ", b"[) (TAILSECRET)] TJ"];

/// Over the top third of the 300 × 300 page.
const MARK: [f64; 4] = [0.0, 200.0, 300.0, 300.0];

/// `tj` shown in /F1 at (20, 260), under [`MARK`].
fn shown(tj: &[u8]) -> Vec<u8> {
    [b"BT /F1 10 Tf 20 260 Td ".as_slice(), tj, b" ET"].concat()
}

/// The only text under the mark sits in such a `TJ`: the walk finds nothing there and rewrites
/// nothing, so it went on, and the text stayed in the file.
#[test]
fn text_under_a_mark_beside_bytes_the_parser_cannot_read_is_refused() {
    for tj in BROKEN {
        let mut doc = one_page(&shown(tj), "", vec![]);
        mark(&mut doc, 0, &[MARK], "");
        assert_eq!(apply(&mut doc, None), Err(RedactError::Unreadable(1)), "{}", String::from_utf8_lossy(tj));
    }
    // With other text on the page, outside the mark.
    let mut doc = one_page(&[b"BT /F1 10 Tf 20 20 Td (PUBLIC) Tj ET ".as_slice(), &shown(BROKEN[0])].concat(), "", vec![]);
    mark(&mut doc, 0, &[MARK], "");
    assert_eq!(apply(&mut doc, None), Err(RedactError::Unreadable(1)), "beside other text");
}

/// In a stream the walk rewrites, it removed the text it could read and wrote the broken `TJ` out
/// as `(TAIL) (SECRET) TJ`: the stray byte and the brackets went, the strings stayed.
#[test]
fn bytes_the_parser_cannot_read_in_a_stream_the_walk_rewrites_are_refused() {
    for tj in BROKEN {
        let mut doc = one_page(&[b"BT /F1 10 Tf 20 230 Td (SECRET) Tj ET ".as_slice(), &shown(tj)].concat(), "", vec![]);
        mark(&mut doc, 0, &[MARK], "");
        assert_eq!(apply(&mut doc, None), Err(RedactError::Unreadable(1)), "{}", String::from_utf8_lossy(tj));
    }
}

/// A form under the mark with resources of its own, which the walk enters but doesn't rewrite:
/// no later read of the page follows it.
#[test]
fn bytes_the_parser_cannot_read_in_a_form_under_a_mark_are_refused() {
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >>", shown(BROKEN[0]));
    let mut doc = one_page(b"/Fm Do", "/XObject << /Fm 7 0 R >>", vec![form]);
    mark(&mut doc, 0, &[MARK], "");
    assert_eq!(apply(&mut doc, None), Err(RedactError::Unreadable(1)));
}

/// A form the mark doesn't reach isn't walked: bytes in it can't hide anything under the mark, and
/// the walk doesn't refuse the page over them. This one has resources of its own, so the check after
/// the walk doesn't read it either (it would refuse one without), and the page is redacted.
#[test]
fn bytes_the_parser_cannot_read_in_a_form_away_from_the_marks_are_left() {
    let form = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 100] /Resources << /Font << /F1 5 0 R >> >>",
        [b"BT /F1 10 Tf 20 20 Td ".as_slice(), BROKEN[0], b" ET"].concat(),
    );
    let mut doc = one_page(b"BT /F1 10 Tf 20 230 Td (SECRET) Tj ET /Fm Do", "/XObject << /Fm 7 0 R >>", vec![form]);
    mark(&mut doc, 0, &[MARK], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 6);
    assert!(!content(&doc, 0).contains("(SECRET)"));
}

/// The page sets its font only through an ExtGState `/Font` and lists no `/Font` resources, so the
/// cleanups after the walk, which look at the listed fonts, read nothing.
#[test]
fn bytes_the_parser_cannot_read_on_a_page_without_font_resources_are_refused() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents [4 0 R 5 0 R] /Resources << /ExtGState << /GS1 << /Font [6 0 R 10] >> >> >> >>"
            .to_vec(),
        stream("", b"BT /GS1 gs 20 230 Td (SECRET) Tj ET"),
        stream("", b"BT /GS1 gs 20 260 Td [(TAIL) ) (SECRET)] TJ ET"),
        FONT.replace("95 0 R", "7 0 R").into_bytes(),
        widths(),
    ]);
    mark(&mut doc, 0, &[MARK], "");
    assert_eq!(apply(&mut doc, None), Err(RedactError::Unreadable(1)));
}

/// Hidden text inside a broken `TJ` is never counted: alone, Remove Hidden Information found
/// nothing to remove and Sanitize Document went on; beside other hidden text, the removal went on
/// and left it. The scan now counts the page, and removing hidden text refuses it.
#[test]
fn hidden_text_beside_bytes_the_parser_cannot_read_is_refused() {
    let alone = b"BT /F1 10 Tf 3 Tr 20 260 Td [(TAIL) ) (HIDDEN)] TJ ET".as_slice();
    let beside = [b"BT /F1 10 Tf 3 Tr 20 230 Td (HIDDEN) Tj ET ".as_slice(), alone].concat();
    for content in [alone, &beside] {
        let label = String::from_utf8_lossy(content);
        let doc = one_page(content, "", vec![]);
        let counts: std::collections::HashMap<Hidden, usize> = scan(&doc).into_iter().collect();
        assert!(counts[&Hidden::HiddenText] > 0, "{label}");
        let mut doc = one_page(content, "", vec![]);
        assert_eq!(remove_hidden(&mut doc, &[Hidden::HiddenText]), Err(RedactError::Unreadable(1)), "{label}");
        let mut doc = one_page(content, "", vec![]);
        assert_eq!(sanitize(&mut doc), Err(RedactError::Unreadable(1)), "{label}");
    }
    // In a form with resources of its own, which the walk enters on every page.
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >>", alone);
    let mut doc = one_page(b"/Fm Do", "/XObject << /Fm 7 0 R >>", vec![form]);
    assert_eq!(remove_hidden(&mut doc, &[Hidden::HiddenText]), Err(RedactError::Unreadable(1)), "in a form");
}

/// Removing hidden layers alone refuses such bytes too, even in a document without
/// `/OCProperties`: a membership dictionary (`/AllOff` of a layer that is on) still hides what it
/// marks, for the walk as for the app's renderer. With nothing in front of `/OC`, the marked text
/// is removed. Here a string beside a stray `}` is left in front of it, which keeps the block from
/// the walk (it reads a `BDC`'s first operands; a string alone does so too, a matter of its own),
/// and the stray byte has the page refused rather than the text kept.
#[test]
fn bytes_the_parser_cannot_read_refuse_removing_hidden_layers_alone() {
    let marked = |before: &[u8]| {
        let content = [before, b" /OC /MC0 BDC BT /F1 10 Tf 20 260 Td (LAYERSECRET) Tj ET EMC BT /F1 10 Tf 20 20 Td (PUBLIC) Tj ET"].concat();
        one_page(
            &content,
            "/Properties << /MC0 8 0 R >>",
            vec![b"<< /Type /OCG /Name (Layer) >>".to_vec(), b"<< /Type /OCMD /OCGs [7 0 R] /P /AllOff >>".to_vec()],
        )
    };
    let mut doc = marked(b"");
    let removed = remove_hidden(&mut doc, &[Hidden::HiddenLayers]);
    assert!(removed.as_ref().is_ok_and(|r| r.iter().any(|(h, n)| *h == Hidden::HiddenLayers && *n > 0)), "{removed:?}");
    assert!(!content(&doc, 0).contains("LAYERSECRET"));
    let mut doc = marked(b"(x) }");
    assert_eq!(remove_hidden(&mut doc, &[Hidden::HiddenLayers]), Err(RedactError::Unreadable(1)));
}

/// With form fields removed too, a field's appearance is flattened into page content and read the
/// same way, though the scan, which runs before flattening, doesn't count it: hidden text in a
/// broken `TJ` there refuses removing hidden text, and Sanitize Document, which went on and left it.
/// Removing form fields alone doesn't read the content, and goes on.
#[test]
fn hidden_text_beside_bytes_the_parser_cannot_read_in_a_field_appearance_is_refused() {
    let fresh = || {
        pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] >> >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>"
                .to_vec(),
            stream("", b"BT /F1 10 Tf 10 230 Td (Name:) Tj ET"),
            FONT.replace("95 0 R", "8 0 R").into_bytes(),
            b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /P 3 0 R /Rect [50 225 200 240] /AP << /N 7 0 R >> >>".to_vec(),
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 150 15] /Resources << /Font << /F1 5 0 R >> >>",
                b"BT /F1 10 Tf 3 Tr 2 3 Td [(TAIL) ) (HIDDEN)] TJ ET",
            ),
            widths(),
        ])
    };
    assert!(remove_hidden(&mut fresh(), &[Hidden::FormFields]).is_ok());
    assert_eq!(remove_hidden(&mut fresh(), &[Hidden::FormFields, Hidden::HiddenText]), Err(RedactError::Unreadable(1)));
    assert_eq!(sanitize(&mut fresh()), Err(RedactError::Unreadable(1)));
}

/// An inline image whose end the parser guesses, as Apply Redactions already refuses it: what
/// follows it may not be what the page draws, so Remove Hidden Information refuses it too.
#[test]
fn hidden_text_beside_an_inline_image_the_renderer_cannot_end_is_refused() {
    let page = b"BT /F1 10 Tf 3 Tr 10 100 Td (HIDDEN) Tj ET BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 7F".as_slice();
    let mut doc = one_page(page, "", vec![]);
    assert_eq!(remove_hidden(&mut doc, &[Hidden::HiddenText]), Err(RedactError::Unreadable(1)));
}
