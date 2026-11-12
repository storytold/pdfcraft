//! Manual two-sided printing (#2860), for a printer that can only print one side: the fronts of
//! the sheets go first, the person turns the printed stack over and puts it back, and the backs
//! follow.
//!
//! The passes are planned over the sides of the print-ready PDF ([`crate::impose`]'s pages), so
//! every layout that prints both sides of a sheet in turn works the same way. Side `2k` is the
//! front of sheet `k`, side `2k + 1` its back.
//!
//! The stack always goes back with the top of the pages going into the printer first, as it did
//! for the fronts. That makes the backs come out as "flip on long edge" would for paper fed short
//! edge first; "flip on short edge" turns each back 180° on the sheet instead
//! ([`Side::turned`]), so the person never has to turn the stack end to end. A tray fed long edge
//! first swaps the two; the dialog's wording says which edge the pages bind on, as two-sided
//! printing does.
//!
//! Whether the backs need `reverse` depends on how the stack goes back, not on the driver: a
//! driver that prints every job last page first does so for both passes, and it cancels out. Put
//! back printed side up without turning it over (most front-tray printers; checked on an HP
//! OfficeJet Pro 9120e), the last sheet printed is on top and the backs go reversed; turned over,
//! they go in sheet order.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_full};

use crate::PrintError;

/// The edge the sheets turn on, as automatic two-sided printing names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Flip {
    #[default]
    LongEdge,
    ShortEdge,
}

/// One side printed in a pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Side {
    /// The page of the print-ready PDF (0-based); `None` for the blank back of the last sheet
    /// when there is an odd number of sides.
    pub page: Option<usize>,
    /// Turned 180° on the sheet (the backs of a short-edge flip).
    pub turned: bool,
}

/// The two passes of a manual two-sided print, in the order each is sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Passes {
    /// The fronts, first sheet first.
    pub fronts: Vec<Side>,
    /// The backs, in the order the turned stack feeds the sheets; empty when no sheet has
    /// anything on its back (one side only).
    pub backs: Vec<Side>,
}

/// Plan the passes for `sides` sides of the print-ready PDF.
///
/// The backs follow the fronts' sheet order: turning the stack over puts the first sheet printed
/// on top again. `reverse` sends them last sheet first, for a printer whose stack comes back the
/// other way round (one that stacks its pages face up); the blank back of an odd count is then
/// sent first, so the last sheet still gets it. Nothing to print is an error.
pub fn plan(sides: usize, flip: Flip, reverse: bool) -> Result<Passes, PrintError> {
    if sides == 0 {
        return Err(PrintError::NoPages);
    }
    let sheets = sides.div_ceil(2);
    let fronts = (0..sheets).map(|k| Side { page: Some(k * 2), turned: false }).collect();
    let mut backs: Vec<Side> = if sides == 1 {
        Vec::new()
    } else {
        // k < sides / 2 rounded up, so k * 2 + 1 cannot overflow.
        (0..sheets).map(|k| Side { page: Some(k * 2 + 1).filter(|p| *p < sides), turned: flip == Flip::ShortEdge }).collect()
    };
    if reverse {
        backs.reverse();
    }
    Ok(Passes { fronts, backs })
}

/// The print-ready PDF of one pass: the `sides` of `print_ready` (an [`crate::impose`] result)
/// in order, a blank side the size of the last sheet where a side has no page, and turned sides
/// drawn 180° about the middle of their page.
pub fn pass_pdf(print_ready: &[u8], sides: &[Side]) -> Result<Vec<u8>, PrintError> {
    if sides.is_empty() {
        return Err(PrintError::NoPages);
    }
    let mut doc = Document::open(Arc::new(print_ready.to_vec()))?;
    let pages = pdfcraft_model::pages(&doc);
    let last = pages.last().ok_or(PrintError::NoPages)?.crop(&doc);
    let root = doc.add(Object::Null);
    let mut kids = Vec::with_capacity(sides.len());
    for side in sides {
        let Some(index) = side.page else {
            let mut blank = Dict::new();
            blank.set(b"Type".to_vec(), Object::name("Page"));
            blank.set(b"Parent".to_vec(), Object::Ref(root));
            blank.set(b"MediaBox".to_vec(), Object::Array(last.iter().map(|v| Object::Real(*v)).collect()));
            blank.set(b"Resources".to_vec(), Object::Dict(Dict::new()));
            kids.push(Object::Ref(doc.add(Object::Dict(blank))));
            continue;
        };
        let page = pages.get(index).ok_or(PrintError::OutOfRange(index.saturating_add(1), pages.len()))?;
        // The page's own dictionary with what it inherited filled in, so it needs no old parent.
        let mut dict = page.dict.clone();
        dict.set(b"Parent".to_vec(), Object::Ref(root));
        if side.turned {
            let [x0, y0, x1, y1] = page.crop(&doc);
            let before = doc.add(Object::Stream(Stream::from_raw(Dict::new(), format!("q -1 0 0 -1 {} {} cm\n", x0 + x1, y0 + y1).into_bytes())));
            let after = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"\nQ\n".to_vec())));
            let mut contents = vec![Object::Ref(before)];
            match dict.get(b"Contents").cloned() {
                Some(Object::Array(parts)) => contents.extend(parts),
                Some(Object::Ref(r)) => contents.push(Object::Ref(r)),
                _ => {}
            }
            contents.push(Object::Ref(after));
            dict.set(b"Contents".to_vec(), Object::Array(contents));
        }
        kids.push(Object::Ref(doc.add(Object::Dict(dict))));
    }
    let mut tree = Dict::new();
    tree.set(b"Type".to_vec(), Object::name("Pages"));
    tree.set(b"Count".to_vec(), Object::Int(i64::try_from(kids.len()).unwrap_or(i64::MAX)));
    tree.set(b"Kids".to_vec(), Object::Array(kids));
    doc.set(root, Object::Dict(tree));
    // A new catalog that keeps the layers' configuration, as `impose` writes it.
    let layers = doc.trailer().get(b"Root").and_then(|r| doc.resolve(r).as_dict().and_then(|c| c.get(b"OCProperties").cloned()));
    let mut cat = Dict::new();
    cat.set(b"Type".to_vec(), Object::name("Catalog"));
    cat.set(b"Pages".to_vec(), Object::Ref(root));
    if let Some(layers) = layers {
        cat.set(b"OCProperties".to_vec(), layers);
    }
    let cat = doc.add(Object::Dict(cat));
    doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(cat));
    Ok(write_full(&doc, &SaveOptions::default())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(page: usize, turned: bool) -> Side {
        Side { page: Some(page), turned }
    }

    const BLANK: Side = Side { page: None, turned: false };
    const BLANK_TURNED: Side = Side { page: None, turned: true };

    #[test]
    fn nothing_to_print_is_an_error() {
        for flip in [Flip::LongEdge, Flip::ShortEdge] {
            for reverse in [false, true] {
                assert_eq!(plan(0, flip, reverse), Err(PrintError::NoPages));
            }
        }
        assert!(pass_pdf(b"", &[]).is_err());
    }

    /// One side has no back: the second pass is empty, whatever the flip and order.
    #[test]
    fn one_side_has_no_second_pass() {
        for flip in [Flip::LongEdge, Flip::ShortEdge] {
            for reverse in [false, true] {
                assert_eq!(plan(1, flip, reverse), Ok(Passes { fronts: vec![side(0, false)], backs: vec![] }));
            }
        }
    }

    #[test]
    fn two_sides_make_one_sheet() {
        assert_eq!(plan(2, Flip::LongEdge, false), Ok(Passes { fronts: vec![side(0, false)], backs: vec![side(1, false)] }));
        assert_eq!(plan(2, Flip::LongEdge, true), Ok(Passes { fronts: vec![side(0, false)], backs: vec![side(1, false)] }));
        assert_eq!(plan(2, Flip::ShortEdge, false), Ok(Passes { fronts: vec![side(0, false)], backs: vec![side(1, true)] }));
        assert_eq!(plan(2, Flip::ShortEdge, true), Ok(Passes { fronts: vec![side(0, false)], backs: vec![side(1, true)] }));
    }

    /// An odd count gives the last sheet a blank back: last in sheet order, first reversed.
    #[test]
    fn five_sides_give_the_last_sheet_a_blank_back() {
        let fronts = vec![side(0, false), side(2, false), side(4, false)];
        let long = plan(5, Flip::LongEdge, false).unwrap();
        assert_eq!((&long.fronts, &long.backs), (&fronts, &vec![side(1, false), side(3, false), BLANK]));
        let long_rev = plan(5, Flip::LongEdge, true).unwrap();
        assert_eq!((&long_rev.fronts, &long_rev.backs), (&fronts, &vec![BLANK, side(3, false), side(1, false)]));
        let short = plan(5, Flip::ShortEdge, false).unwrap();
        assert_eq!((&short.fronts, &short.backs), (&fronts, &vec![side(1, true), side(3, true), BLANK_TURNED]));
        let short_rev = plan(5, Flip::ShortEdge, true).unwrap();
        assert_eq!((&short_rev.fronts, &short_rev.backs), (&fronts, &vec![BLANK_TURNED, side(3, true), side(1, true)]));
    }

    #[test]
    fn six_sides_fill_three_sheets() {
        let fronts = vec![side(0, false), side(2, false), side(4, false)];
        assert_eq!(
            plan(6, Flip::LongEdge, false).unwrap(),
            Passes { fronts: fronts.clone(), backs: vec![side(1, false), side(3, false), side(5, false)] }
        );
        assert_eq!(
            plan(6, Flip::LongEdge, true).unwrap(),
            Passes { fronts: fronts.clone(), backs: vec![side(5, false), side(3, false), side(1, false)] }
        );
        assert_eq!(
            plan(6, Flip::ShortEdge, false).unwrap(),
            Passes { fronts: fronts.clone(), backs: vec![side(1, true), side(3, true), side(5, true)] }
        );
        assert_eq!(plan(6, Flip::ShortEdge, true).unwrap(), Passes { fronts, backs: vec![side(5, true), side(3, true), side(1, true)] });
    }

    /// A pass keeps its sides in order, adds a blank the size of the last sheet, and turns a
    /// turned side about its middle.
    #[test]
    fn a_pass_lists_its_sides_blanks_and_turns() {
        // Three sides: two Letter, the last 300 × 400; side i's contents read "% side i".
        let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into(), "<< /Type /Pages /Kids [3 0 R 5 0 R 7 0 R] /Count 3 >>".into()];
        for (i, (w, h)) in [(612, 792), (612, 792), (300, 400)].into_iter().enumerate() {
            objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents {} 0 R >>", 4 + 2 * i));
            let body = format!("% side {i}\n");
            objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
        }
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offs = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offs.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let x = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offs {
            bytes.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());

        let out = pass_pdf(&bytes, &[BLANK_TURNED, side(1, true), side(0, false)]).unwrap();
        let doc = Document::open(Arc::new(out)).unwrap();
        let pages = pdfcraft_model::pages(&doc);
        assert_eq!(pages.len(), 3);
        assert_eq!(pages[0].crop(&doc), [0.0, 0.0, 300.0, 400.0], "a blank the size of the last sheet");
        assert!(pages[0].dict.get(b"Contents").is_none());
        let text = |p: &pdfcraft_model::Page| -> String {
            let parts = match doc.resolve(p.dict.get(b"Contents").unwrap()).as_ref() {
                Object::Array(a) => a.clone(),
                _ => vec![p.dict.get(b"Contents").unwrap().clone()],
            };
            parts
                .iter()
                .map(|o| match doc.resolve(o).as_ref() {
                    Object::Stream(s) => String::from_utf8(s.decoded().unwrap()).unwrap(),
                    other => panic!("not a stream: {other:?}"),
                })
                .collect()
        };
        assert_eq!(text(&pages[1]), "q -1 0 0 -1 612 792 cm\n% side 1\n\nQ\n");
        assert_eq!(text(&pages[2]), "% side 0\n");
        assert!(pass_pdf(&bytes, &[side(3, false)]).is_err(), "a side past the end");
    }
}
