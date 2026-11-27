//! Redaction refusals name the page whose content or forms are their cause
//! (`RedactError::PageRefused`), in Apply Redactions and Remove Hidden Information, and errors
//! that aren't refusals keep their own text.

use super::*;
use crate::sanitize::{Hidden, remove_hidden};

/// Two pages: the first plain, the second showing hidden text (`3 Tr`) and then `content`, with
/// `res` as its resources; objects from 9 on are `extra`.
fn second_page_hidden(content: &[u8], res: &str, extra: Vec<Vec<u8>>) -> Document {
    let mut objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 7 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", b"BT /F1 10 Tf 20 260 Td (PLAIN) Tj ET"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 8 0 R /Resources << {res} >> >>").into_bytes(),
        stream("", [b"BT /F1 10 Tf 3 Tr 20 60 Td (HIDDEN) Tj ET ".as_slice(), content].concat()),
    ];
    objs.extend(extra);
    pdf(objs)
}

/// Asserts that removing hidden text from `doc` is refused, naming page 2, for a reason that holds
/// `reason`.
fn refuses_page_two(mut doc: Document, reason: &str) {
    let removed = remove_hidden(&mut doc, &[Hidden::HiddenText]);
    assert!(matches!(&removed, Err(RedactError::PageRefused { page: 2, reason: r }) if r.contains(reason)), "{removed:?}");
}

/// Past 2,000,000 operators, what the second page's resources still draw can't be told: the
/// refusal of the properties' cleanup names that page.
#[test]
fn the_properties_cleanup_names_the_page_it_refuses() {
    let many = b"n ".repeat(2_000_001);
    refuses_page_two(second_page_hidden(&many, "/Font << /F1 5 0 R >> /Properties << /P0 << >> >>", vec![]), "more than 2,000,000 operators");
}

/// The same for the cleanup of the forms the page replaced, which reads the page first.
#[test]
fn the_replaced_forms_cleanup_names_the_page_it_refuses() {
    let many = [b"/Fm Do ".as_slice(), &b"n ".repeat(2_000_001)].concat();
    let form = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >>",
        b"BT /F1 10 Tf 3 Tr 20 100 Td (FORMHIDDEN) Tj ET",
    );
    refuses_page_two(second_page_hidden(&many, "/Font << /F1 5 0 R >> /XObject << /Fm 9 0 R >>", vec![form]), "more than 2,000,000 operators");
}

/// With a Type 3 font listed, the fonts' cleanup reads the soft masks without resources of their
/// own that the page sets, which the walk doesn't enter: one with bytes the parser can't read is
/// refused, naming the page.
#[test]
fn the_type3_cleanup_names_the_page_it_refuses() {
    let group = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency >>", b"BT ( ET");
    let font = b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 65 /Widths [500] /Encoding << /Differences [65 /A] >> /CharProcs << >> >>".to_vec();
    let res = "/Font << /F1 5 0 R /T3 10 0 R >> /ExtGState << /GS1 << /SMask << /S /Luminosity /G 9 0 R >> >> >>";
    refuses_page_two(second_page_hidden(b"/GS1 gs", res, vec![group, font]), "a soft mask or form without resources of its own has bytes");
}

/// A page whose forms go past what one page may read is refused, named, by Apply Redactions and by
/// Remove Hidden Information: here it draws a form more often than the page may enter forms.
#[test]
fn a_page_past_what_it_may_read_is_named() {
    let many = [b"BT /F1 10 Tf 20 260 Td (SECRET) Tj ET ".as_slice(), &b"/Fm Do ".repeat(pdfcraft_cos::MAX_PAGE_FORMS + 1)].concat();
    let page = || one_page(&many, "/XObject << /Fm 7 0 R >>", vec![stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"0 0 m")]);
    let past = |error: &RedactError| {
        matches!(error, RedactError::PageRefused { page: 1, reason }
            if reason == &format!("content and forms go past what one page may read (the page draws more than {} form XObjects)", pdfcraft_cos::MAX_PAGE_FORMS))
    };
    let mut doc = page();
    mark(&mut doc, 0, &[[0.0, 250.0, 300.0, 280.0]], "");
    let applied = apply(&mut doc, None).map(|_| ());
    assert!(applied.as_ref().is_err_and(past), "{applied:?}");
    let removed = remove_hidden(&mut page(), &[Hidden::HiddenText]).map(|_| ());
    assert!(removed.as_ref().is_err_and(past), "{removed:?}");
}

/// Pages sharing a resource dictionary with an image a mark removed: the second draws, through
/// the shared resources, a form that doesn't decode, so whether it still draws the image can't be
/// told. The refusal names the second page, and doesn't say anything was found under the mark.
#[test]
fn a_page_sharing_resources_whose_forms_can_t_be_read_is_named() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 9 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 11 0 R >>".to_vec(),
        stream("", b"q 50 0 0 50 20 230 cm /Im1 Do Q"),
        b"null".to_vec(),
        b"null".to_vec(),
        stream("/Type /XObject /Subtype /Image /Width 16 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"REDIMGONECOVERED"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Filter /UnsupportedForTheTest", b"opaque"),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 10 0 R /Resources 11 0 R >>".to_vec(),
        stream("", b"/FmBad Do"),
        b"<< /XObject << /Im1 7 0 R /FmBad 8 0 R >> >>".to_vec(),
    ]);
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    let applied = apply(&mut doc, None).map(|_| ());
    assert!(matches!(&applied, Err(RedactError::PageRefused { page: 2, reason }) if reason.contains("can't all be read")), "{applied:?}");
}

/// A page whose forms spend exactly what it may enter, nothing refused, and a form that doesn't
/// decode: that form says why, and the page isn't said to go past what it may read. In the walk, as
/// the page's last entry, where with room left the walk would remove it, it still refuses the
/// page, as before, now with its own text; in the cleanup of the page's properties, outside the
/// mark, which the walk doesn't enter, for Apply Redactions; and in the Type 3 cleanup's read of an
/// annotation appearance, for Apply Redactions and Remove Hidden Information.
#[test]
fn a_form_that_doesn_t_decode_at_the_page_s_last_entry_says_why() {
    const MAX: usize = pdfcraft_cos::MAX_PAGE_FORMS;
    const SECRET: &[u8] = b"BT /F1 10 Tf 20 260 Td (SECRET) Tj ET ";
    const BAD: &str = "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Filter /UnsupportedForTheTest";
    let says_why = |result: Result<(), RedactError>| {
        assert!(matches!(&result, Err(RedactError::Cos(pdfcraft_cos::CosError::Filter(why))) if why.contains("UnsupportedForTheTest")), "{result:?}");
    };
    let form = || stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"0 0 m");
    let marked = |mut doc: Document| {
        mark(&mut doc, 0, &[[0.0, 250.0, 300.0, 280.0]], "");
        apply(&mut doc, None).map(|_| ())
    };
    // The walk: the form that doesn't decode is the page's last entry.
    let content = [SECRET, &b"/Fm Do ".repeat(MAX - 1), b"/Fc Do"].concat();
    says_why(marked(one_page(&content, "/XObject << /Fm 7 0 R /Fc 8 0 R >>", vec![form(), stream(BAD, b"opaque")])));
    // The properties' cleanup, after the walk entered every form the page may.
    let content = [SECRET, &b"/Fm Do ".repeat(MAX), b"/Fbad Do"].concat();
    let resources = "/XObject << /Fm 7 0 R /Fbad 8 0 R >> /Properties << /P0 << >> >>";
    says_why(marked(one_page(&content, resources, vec![form(), stream(&BAD.replace("300 300", "10 10"), b"opaque")])));
    // The Type 3 cleanup's appearances.
    let with_type3 = |content: Vec<u8>| {
        pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /T3 9 0 R >> /XObject << /Fm 7 0 R >> >> /Annots [10 0 R] >>".to_vec(),
            stream("", &content),
            FONT.replace("95 0 R", "6 0 R").into_bytes(),
            widths(),
            form(),
            stream(BAD, b"opaque"),
            b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 65 /Widths [500] /Encoding << /Differences [65 /A] >> /CharProcs << >> >>".to_vec(),
            b"<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /AP << /N 8 0 R >> >>".to_vec(),
        ])
    };
    says_why(marked(with_type3([SECRET, &b"/Fm Do ".repeat(MAX)].concat())));
    let hidden = [b"BT /F1 10 Tf 3 Tr 20 60 Td (HIDDEN) Tj ET ".as_slice(), &b"/Fm Do ".repeat(MAX)].concat();
    says_why(remove_hidden(&mut with_type3(hidden), &[Hidden::HiddenText]).map(|_| ()));
}
