//! Remove Hidden Information's refusals name the page whose content or forms are their cause
//! (`RedactError::PageRefused`), from each cleanup that reads a rewritten page.

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

/// Whether removing hidden text from `doc` is refused, naming page 2, for a reason that holds
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
