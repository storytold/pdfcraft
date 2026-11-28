//! Page templates kept off the page tree (`/Names /Templates`) are pages no page walk reaches,
//! yet a full save writes them (#7919): Apply Redactions refuses them, and Form fields removes
//! them with what they hold.

use super::surviving_references::full_save_holds;
use super::*;
use crate::sanitize::{Hidden, remove_hidden, scan_only};

/// One visible page drawing `visible`, and the catalog's `/Names` set to `names`; object 7 on is
/// `extra`.
fn with_names(visible: &[u8], names: &str, extra: Vec<Vec<u8>>) -> Document {
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /Names {names} >>").into_bytes(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", visible),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
    ];
    objs.extend(extra);
    pdf(objs)
}

/// A hidden template page (object 7) drawing `text`.
fn template_drawing(text: &[u8]) -> Vec<Vec<u8>> {
    vec![b"<< /Type /Template /MediaBox [0 0 300 200] /Resources << /Font << /F1 5 0 R >> >> /Contents 8 0 R >>".to_vec(), stream("", text)]
}

const MARKED: [f64; 4] = [10.0, 140.0, 200.0, 170.0];

#[test]
fn apply_refuses_a_term_a_hidden_template_repeats() {
    let mut doc = with_names(
        b"BT /F1 12 Tf 20 150 Td (ZQ-ACCT-5521) Tj ET",
        "<< /Templates << /Names [(sheet2) 7 0 R] >> >>",
        template_drawing(b"BT /F1 12 Tf 20 150 Td (Spare sheet ZQ-ACCT-5521) Tj ET"),
    );
    mark(&mut doc, 0, &[MARKED], "");
    let before = doc.clone();
    assert!(matches!(apply(&mut doc, None), Err(RedactError::HiddenTemplates(n)) if n > 0), "the template would keep the term");
    assert_eq!(marks(&doc), marks(&before), "a refused apply changes nothing");
    assert!(content(&doc, 0).contains("ZQ-ACCT-5521"), "a refused apply changes nothing");
}

/// The template shares the marked page's content stream: rewriting the page alone would leave it.
#[test]
fn apply_refuses_a_hidden_template_sharing_the_marked_pages_content() {
    let mut doc = with_names(
        b"BT /F1 12 Tf 20 150 Td (ZQ-ACCT-5521) Tj ET",
        "<< /Templates << /Names [(sheet2) 7 0 R] >> >>",
        vec![b"<< /Type /Page /MediaBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec()],
    );
    mark(&mut doc, 0, &[MARKED], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::HiddenTemplates(n)) if n > 0));
}

/// A page `/Names /Pages` names that the page tree doesn't hold is as hidden as a template; the
/// pages it names in the tree are redacted with the rest.
#[test]
fn apply_refuses_a_named_page_outside_the_page_tree_but_not_one_inside() {
    let outside = || {
        with_names(
            b"BT /F1 12 Tf 20 150 Td (ZQ-ACCT-5521) Tj ET",
            "<< /Pages << /Names [(shown) 3 0 R (spare) 7 0 R] >> >>",
            template_drawing(b"BT /F1 12 Tf 20 150 Td (Spare sheet ZQ-ACCT-5521) Tj ET"),
        )
    };
    let mut doc = outside();
    mark(&mut doc, 0, &[MARKED], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::HiddenTemplates(n)) if n > 0));

    let mut doc = with_names(b"BT /F1 12 Tf 20 150 Td (ZQ-ACCT-5521) Tj ET", "<< /Pages << /Names [(shown) 3 0 R] >> >>", vec![]);
    mark(&mut doc, 0, &[MARKED], "");
    apply(&mut doc, None).expect("a named page in the page tree is redacted like any other");
    assert!(!full_save_holds(&doc, b"ZQ-ACCT-5521"));
}

/// Remove hidden information's Form fields (and so Sanitize) counts the template and takes it out
/// of the file, with the text it draws.
#[test]
fn form_fields_removes_a_hidden_template_and_its_text() {
    let mut doc = with_names(
        b"BT /F1 12 Tf 20 150 Td (Public) Tj ET",
        "<< /Templates << /Names [(sheet2) 7 0 R] >> >>",
        template_drawing(b"BT /F1 12 Tf 20 150 Td (Spare sheet ZQ-ACCT-5521) Tj ET"),
    );
    assert!(full_save_holds(&doc, b"ZQ-ACCT-5521"), "the template is written before the removal");
    assert!(scan_only(&doc, &[Hidden::FormFields])[0].1 > 0, "the template is counted");
    let removed = remove_hidden(&mut doc, &[Hidden::FormFields]).unwrap();
    assert!(removed.iter().any(|&(h, n)| h == Hidden::FormFields && n > 0), "{removed:?}");
    assert!(!full_save_holds(&doc, b"ZQ-ACCT-5521"), "the template's text is still in the file");
    assert!(content(&doc, 0).contains("Public"), "the visible page stays");
}

/// A Password field whose widget sits on a hidden template: Form fields leaves no copy of its value.
#[test]
fn a_password_on_a_hidden_template_leaves_with_form_fields() {
    let mut doc = with_names(
        b"BT /F1 12 Tf 20 150 Td (Public) Tj ET",
        "<< /Templates << /Names [(t1) 7 0 R] >> >> /AcroForm << /Fields [8 0 R] >>",
        vec![
            b"<< /Type /Template /MediaBox [0 0 300 200] /Annots [8 0 R] >>".to_vec(),
            b"<< /Type /Annot /Subtype /Widget /FT /Tx /Ff 8192 /T (vpw) /V (TYPEDPW8842) /P 7 0 R /Rect [10 10 100 30] >>".to_vec(),
        ],
    );
    assert!(full_save_holds(&doc, b"TYPEDPW8842"), "the value is written before the removal");
    remove_hidden(&mut doc, &[Hidden::FormFields]).unwrap();
    assert!(!full_save_holds(&doc, b"TYPEDPW8842"), "the password value is still in the file");
}

/// A hostile `/Templates` tree: many kids, each naming one shared `/Names` array of many pairs.
/// Its values cost the walk as its nodes do, so it is read within the budget rather than once per
/// kid, and the template it names still counts, so Apply refuses it.
#[test]
fn kids_sharing_one_long_names_array_are_read_within_the_budget() {
    const KIDS: usize = 400;
    const PAIRS: usize = 400;
    let pairs: Vec<String> = (0..PAIRS).map(|i| format!("(t{i}) 7 0 R")).collect();
    let kids: Vec<String> = (0..KIDS).map(|i| format!("{} 0 R", 11 + i)).collect();
    let mut extra = template_drawing(b"BT /F1 12 Tf 20 150 Td (Spare sheet) Tj ET");
    extra.push(format!("[{}]", pairs.join(" ")).into_bytes());
    extra.push(format!("<< /Kids [{}] >>", kids.join(" ")).into_bytes());
    extra.extend((0..KIDS).map(|_| b"<< /Names 9 0 R >>".to_vec()));
    let doc = with_names(b"BT /F1 12 Tf 20 150 Td (Public) Tj ET", "<< /Templates 10 0 R >>", extra);
    let (n, _) = crate::sanitize::hidden_templates(&doc);
    assert!(n > 0, "the template still counts");
    assert!(n < KIDS * PAIRS, "every kid's copy of the shared array was read: {n}");
}
