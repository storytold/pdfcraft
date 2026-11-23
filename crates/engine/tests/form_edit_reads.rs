//! A form edit reads the document's fields once, however many the form holds: filling,
//! recalculating, the XFA data and the view each read them, mostly with nothing changed since
//! the step before, and on a form with a thousand fields each reading takes tens of milliseconds.
//! Counted, not timed: `form_reads_on_this_thread` counts the whole-form walks this thread made.

use std::sync::Arc;

use pdfcraft_engine::{DocId, Edit, FieldValue, Session};
use pdfcraft_forms::form_reads_on_this_thread as form_reads;

/// An AcroForm of `n` text fields `f0`…, fifty to a page.
fn acroform(n: usize) -> Vec<u8> {
    let pages = n.div_ceil(50).max(1);
    let first_page = 5;
    let first_field = first_page + pages;
    let refs = |from: usize, count: usize| (from..from + count).map(|i| format!("{i} 0 R")).collect::<Vec<_>>().join(" ");
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 3 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{}] /Count {pages} /MediaBox [0 0 612 792] >>", refs(first_page, pages)),
        format!("<< /Fields [{}] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 4 0 R >> >> >>", refs(first_field, n)),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
    ];
    for p in 0..pages {
        let count = 50.min(n - p * 50);
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Annots [{}] >>", refs(first_field + p * 50, count)));
    }
    for i in 0..n {
        let (page, row) = (i / 50, i % 50);
        let y = 760 - 15 * row;
        objs.push(format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (f{i}) /TU (Field {i}) /Rect [40 {} 300 {y}] /P {} 0 R /F 4 >>",
            y - 12,
            first_page + page
        ));
    }
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

/// Fill `name` with `value`, returning how many times the fields were read for it.
fn fill(s: &mut Session, id: DocId, name: &str, value: &str) -> usize {
    let before = form_reads();
    s.apply(id, Edit::SetFieldValue { name: name.into(), value: FieldValue::Text(value.into()) }).unwrap();
    form_reads() - before
}

fn value(s: &Session, id: DocId, name: &str) -> Vec<String> {
    s.get(id).unwrap().form.iter().find(|f| f.name == name).unwrap().value.clone()
}

#[test]
fn a_form_edit_reads_the_fields_once_however_many_the_form_holds() {
    for n in [20, 600] {
        let mut s = Session::new();
        let id = s.open("form.pdf", None, Arc::new(acroform(n)), None).unwrap();
        for k in 0..3 {
            let name = format!("f{k}");
            assert_eq!(fill(&mut s, id, &name, &format!("value {k}")), 1, "{n} fields, edit {k}");
            assert_eq!(value(&s, id, &name), [format!("value {k}")]);
        }
        let shown = |s: &Session| s.get(id).unwrap().info.fields.iter().find(|f| f.name == "f2").and_then(|f| f.value.clone());
        assert_eq!(shown(&s).as_deref(), Some("value 2"));
        // Undo and redo read the restored state once each, and show it.
        let before = form_reads();
        s.undo(id).unwrap();
        assert!(value(&s, id, "f2").is_empty());
        assert_eq!(shown(&s), None);
        s.redo(id).unwrap();
        assert_eq!(value(&s, id, "f2"), ["value 2"]);
        assert!(form_reads() - before <= 2, "{n} fields: undo and redo read {} times", form_reads() - before);
        // The same value again changes nothing and still reads the form at most once.
        assert!(fill(&mut s, id, "f2", "value 2") <= 1);
    }
}

/// A laid-out XFA form keeps its data in step with the fields as well: the edit reads them
/// once to fill and once after its data is written (its scripts read the fields), however many
/// fields the form holds.
#[test]
fn an_xfa_form_edit_reads_the_fields_a_fixed_number_of_times() {
    let mut counts = Vec::new();
    for rows in [2, 60] {
        let mut s = Session::new();
        let xdp = pdfcraft_xfa::fixtures::template(rows);
        let id = s.open("form.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&xdp)), None).unwrap();
        let name = s.get(id).unwrap().form.iter().find(|f| f.name.contains("familyName")).unwrap().name.clone();
        let fields = s.get(id).unwrap().form.len();
        let reads: Vec<usize> = (0..3).map(|k| fill(&mut s, id, &name, &format!("Name {k}"))).collect();
        assert_eq!(value(&s, id, &name), ["Name 2"]);
        counts.push((fields, reads));
    }
    let [(small, a), (large, b)] = counts.as_slice() else { panic!("{counts:?}") };
    assert!(large > small, "{counts:?}");
    assert_eq!(a, b, "{counts:?}");
    assert!(a.iter().all(|&r| r <= 2), "{counts:?}");
}
