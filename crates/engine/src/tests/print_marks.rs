//! Printer's marks on the printed copy only (Acrobat's Marks and Bleeds).

use super::*;

fn marks() -> PrinterMarks {
    PrinterMarks { label: "proof.pdf".into(), date: "2026-11-12".into(), ..PrinterMarks::default() }
}

fn actual(pages: Vec<usize>) -> print::Settings {
    print::Settings { pages, layout: print::Layout::Size(print::SizeMode::Actual), ..Default::default() }
}

/// The pixels on the first sheet that aren't white.
fn inked(bytes: Vec<u8>) -> usize {
    let (_, _, rgba) = printed_pixels(bytes);
    rgba.as_chunks::<4>().0.iter().filter(|p| p[0] < 200 || p[1] < 200 || p[2] < 200).count()
}

fn sheet_texts(bytes: Vec<u8>) -> Vec<String> {
    let mut s = Session::new();
    let id = s.open("sheets.pdf", None, Arc::new(bytes), None).unwrap();
    page_texts(&s, id)
}

#[test]
fn printer_marks_go_on_the_printed_sheets_and_not_the_document() {
    let (s, id) = session_with(3);
    let before = s.get(id).unwrap().bytes.clone();
    let plain = s.print_pdf(id, &actual(vec![1])).unwrap();
    let marked = s.print_pdf_marked(id, &actual(vec![1]), Some(&marks())).unwrap();
    assert!(inked(marked.clone()) > inked(plain.clone()) + 200, "trim and bleed marks, targets and colour bars are drawn");
    let texts = sheet_texts(marked);
    assert!(texts[0].contains("proof.pdf") && texts[0].contains("2 of 3") && texts[0].contains("2026-11-12"), "{texts:?}");
    assert!(!sheet_texts(plain)[0].contains("2 of 3"));
    let doc = s.get(id).unwrap();
    assert!(Arc::ptr_eq(&doc.bytes, &before) && !doc.dirty, "the document itself is untouched");
}

/// Each printed page carries its own place in the document, in the order printed, a page
/// printed twice included.
#[test]
fn each_printed_page_is_marked_with_its_own_number() {
    let (s, id) = session_with(4);
    let texts = sheet_texts(s.print_pdf_marked(id, &actual(vec![2, 0, 2]), Some(&marks())).unwrap());
    assert_eq!(texts.len(), 3, "{texts:?}");
    for (text, page) in texts.iter().zip(["3 of 4", "1 of 4", "3 of 4"]) {
        assert_eq!(text.matches(" of 4").count(), 1, "{texts:?}");
        assert!(text.contains(page), "{page}: {texts:?}");
    }
}

#[test]
fn marks_the_document_already_has_are_replaced_on_paper() {
    let (mut s, id) = session_with(2);
    let old = PrinterMarks { label: "old.pdf".into(), date: "2020-01-01".into(), ..PrinterMarks::default() };
    s.apply(id, Edit::AddPrinterMarks { pages: vec![0], settings: old, replace: true }).unwrap();
    let texts = sheet_texts(s.print_pdf_marked(id, &actual(vec![0]), Some(&marks())).unwrap());
    assert!(texts[0].contains("proof.pdf") && texts[0].contains("2026-11-12"), "the print's marks: {texts:?}");
    assert!(!texts[0].contains("old.pdf") && !texts[0].contains("2020-01-01"), "the document's marks are replaced: {texts:?}");
}

#[test]
fn printer_marks_are_refused_for_a_print_as_pictures() {
    let (s, id) = session_with(1);
    let raster = Some(print::Raster { dpi: 72.0, grayscale: false, colour: None });
    let settings = print::Settings { raster, ..actual(vec![0]) };
    assert!(matches!(s.print_pdf_marked(id, &settings, Some(&marks())), Err(EditError::Print(m)) if m.contains("as_image")));
    assert!(s.print_pdf_marked(id, &settings, None).is_ok());
}
