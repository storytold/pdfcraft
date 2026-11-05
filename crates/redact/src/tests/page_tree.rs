//! Apply, Remove Hidden Information and Sanitize act on every page or refuse: never on the pages
//! before a page-tree walk limit.

use pdfcraft_model::{Limits, PageTreeLimit, with_limits};

use crate::sanitize::{Hidden, remove_hidden, sanitize};

use super::*;

/// Two pages that inherit their resources (one small font dictionary) from the tree node, each
/// drawing `content`.
fn two_inheriting_pages(content: &[u8]) -> Document {
    pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 /Resources << /Font << /F1 7 0 R >> >> >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R >>".to_vec(),
        stream("", content),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 6 0 R >>".to_vec(),
        stream("", content),
        FONT.replace("95 0 R", "8 0 R").into_bytes(),
        widths(),
    ])
}

/// A removal entry point.
type Removal = fn(&mut Document) -> Result<Vec<(Hidden, usize)>, RedactError>;

/// An inherited-copy budget that covers one page's copy of the tree node's resources, not two.
const ONE_COPY: Limits = Limits { inherited_copies: 300, ..Limits::DEFAULT };

/// The same pages with `n` null entries ahead of them in the root `/Kids`.
fn padded(mut doc: Document, n: usize) -> Document {
    let mut kids = vec![Object::Null; n];
    kids.extend([Object::Ref(ObjRef::new(3, 0)), Object::Ref(ObjRef::new(5, 0))]);
    doc.update_dict(ObjRef::new(2, 0), |d| d.set(b"Kids".to_vec(), Object::Array(kids))).unwrap();
    doc
}

/// Marks on both pages, and a copy budget that lets a whole walk copy the first page's resources
/// only: applying refuses rather than redact the first page and leave the second's text.
#[test]
fn apply_refuses_marks_past_the_inherited_copy_limit() {
    let mut doc = two_inheriting_pages(b"BT /F1 10 Tf 20 200 Td (SECRET) Tj ET");
    mark(&mut doc, 0, &[[15.0, 195.0, 90.0, 215.0]], "");
    mark(&mut doc, 1, &[[15.0, 195.0, 90.0, 215.0]], "");
    assert_eq!(with_limits(ONE_COPY, || pdfcraft_model::pages(&doc).len()), 1, "the budget covers one page");
    let refused = with_limits(ONE_COPY, || apply(&mut doc, None));
    assert_eq!(refused, Err(RedactError::PageTree(PageTreeLimit::InheritedCopies)));
    assert!(content(&doc, 1).contains("SECRET"), "nothing changed");
    apply(&mut doc, None).unwrap();
    assert!(!content(&doc, 0).contains("SECRET") && !content(&doc, 1).contains("SECRET"), "both pages, within the default limits");
}

/// Hidden text behind a walk limit (entries or inherited copies) is refused by Sanitize and by
/// Remove Hidden Information, whatever is chosen, instead of read as "nothing hidden" and reported
/// done.
#[test]
fn sanitize_and_remove_hidden_refuse_a_tree_past_a_walk_limit() {
    let hidden = two_inheriting_pages(b"BT /F1 12 Tf 3 Tr 10 10 Td (SECRET) Tj ET");
    let entries = Limits { kids_entries: 4, ..Limits::DEFAULT };
    for (doc, limits, limit) in
        [(padded(hidden.clone(), 5), entries, PageTreeLimit::KidsEntries), (hidden.clone(), ONE_COPY, PageTreeLimit::InheritedCopies)]
    {
        let runs: [Removal; 3] = [sanitize, |d| remove_hidden(d, &[Hidden::Comments]), |d| remove_hidden(d, &[Hidden::HiddenText])];
        for run in runs {
            let mut work = doc.clone();
            assert_eq!(with_limits(limits, || run(&mut work)), Err(RedactError::PageTree(limit)), "{limit:?}");
        }
    }
    let mut doc = padded(hidden, 5);
    let done = sanitize(&mut doc).unwrap();
    assert!(done.iter().any(|(h, n)| *h == Hidden::HiddenText && *n > 0), "within the default limits: {done:?}");
    assert!(!content(&doc, 1).contains("SECRET"));
}
