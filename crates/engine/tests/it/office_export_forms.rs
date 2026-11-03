//! Word export of a form keeps its layout: the boxes and rules the page paints, its text line by
//! line where it stands, and its fields as content controls.

use std::sync::Arc;

use pdfcraft_engine::Session;
use pdfcraft_engine::compare::OfficeFormat;
use pdfcraft_export::FieldKind;

fn pdf(objects: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

fn stream(dict: &str, data: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{data}\nendstream", data.len())
}

/// A claim form's first page: six labelled boxes (four drawn as `m l l l l S`, back to their
/// corner), comb ticks drawn upwards, a numbered heading whose number stands a tab away from
/// its text, a bold word inside a sentence, a comb field holding a postcode and a ticked check
/// box. A second page, turned a quarter, paints one rule.
fn claim_form() -> Vec<u8> {
    // The heading's text opens with a space and a 1.4 em `TJ` gap, as this form's producer
    // writes it: the line starts at 74, its first word at 74 + 2.78 + 14.
    let mut content = String::from("BT /F1 10 Tf 68 446 Td (1) Tj ET BT /F2 10 Tf 74 446 Td [( ) -1400 (Vom )] TJ ET ");
    // A date header set as one text object: a `TJ` gap of 8 em between two words is a column.
    content.push_str("BT /F2 6 Tf 300 600 Td [(Jahr) -8000 (Tag)] TJ ET ");
    content.push_str("BT /F1 10 Tf 111 446 Td (Arbeitgeber) Tj ET BT /F2 10 Tf 172 446 Td (auszufuellen:) Tj ET ");
    for (i, label) in ["Name", "Vorname", "Geburtsname", "Strasse", "Postleitzahl", "Wohnort"].iter().enumerate() {
        let y = 700 - 30 * i;
        content.push_str(&format!("BT /F2 8 Tf 60 {} Td ({label}) Tj ET ", y + 14));
        if i % 3 == 0 {
            content.push_str(&format!("0 G 0.7 w 56 {y} 484 24 re S "));
        } else {
            content.push_str(&format!("q 1 0 0 1 56 {y} cm 0 G 0.7 w 0 0 m 484 0 l 484 24 l 0 24 l 0 0 l S Q "));
        }
    }
    for x in [80, 95, 110, 125] {
        content.push_str(&format!("q 1 0 0 1 {x} 580 cm 0 G 0.25 w 0 0 m 0 4.6 l S Q "));
    }
    let resources = "/Font << /F1 7 0 R /F2 8 0 R >>";
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [9 0 R 10 0 R] /DA (/Helv 0 Tf 0 g) >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << {resources} >> /Contents 5 0 R /Annots [9 0 R 10 0 R] >>"),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Rotate 90 /Contents 6 0 R >>".into(),
        stream("", &content),
        stream("", "0 G 1 w 100 700 m 300 700 l S"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (PLZ) /Ff 16777216 /MaxLen 5 /V (10115) /DA (/Helv 10 Tf 0 g) /Rect [60 580 160 592] /P 3 0 R >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (Ja) /V /Yes /AS /Yes /Rect [60 520 70 530] /P 3 0 R /AP << /N << /Yes 11 0 R /Off 12 0 R >> >> >>".into(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", "0 g 2 2 6 6 re f"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", ""),
    ])
}

#[test]
fn exporting_a_form_to_word_keeps_its_boxes_text_and_fields_in_place() {
    let mut s = Session::new();
    let id = s.open("claim.pdf", None, Arc::new(claim_form()), None).unwrap();
    let d = s.get(id).unwrap();
    let pages = d.export_pages();
    let page = &pages[0];

    // Six boxes and four ticks, with their stroke.
    assert_eq!(page.shapes.len(), 10, "{:#?}", page.shapes);
    assert!(page.shapes.iter().all(|s| s.stroke.is_some() && s.fill.is_none()));
    let tick = &page.shapes[6];
    assert_eq!(tick.paths[0].0.len(), 2);
    assert!((tick.stroke.unwrap().width - 0.25).abs() < 1e-6);

    // The number and the heading are separate lines, each at its own words; the bold word and the
    // words around it are lines of their own too.
    let line = |t: &str| page.lines.iter().find(|l| l.text == t).unwrap_or_else(|| panic!("{t}: {:#?}", page.lines));
    assert!(line("1").rect[2] < 75.0);
    assert!((line("Vom").rect[0] - 90.78).abs() < 0.1, "the line starts at its word, not at the gap before it: {:?}", line("Vom").rect);
    assert!(line("Arbeitgeber").bold && !line("Vom").bold);
    assert_eq!(line("Arbeitgeber").font, "Helvetica-Bold");
    // Words a column apart in one line are pieces of their own, each at its word.
    assert!(line("Jahr").rect[2] < 315.0 && line("Tag").rect[0] > 345.0, "{:?} {:?}", line("Jahr").rect, line("Tag").rect);

    // The fields, at their widgets' boxes on the visible page.
    assert_eq!(page.fields.len(), 2, "{:#?}", page.fields);
    let plz = page.fields.iter().find(|f| f.name == "PLZ").unwrap();
    assert_eq!(plz.kind, FieldKind::Text { value: "10115".into(), multiline: false, comb: Some(5), max_len: Some(5) });
    assert!(plz.rect.iter().zip([60.0, 580.0, 160.0, 592.0]).all(|(a, b)| (a - b).abs() < 0.01), "{:?}", plz.rect);
    assert_eq!(plz.size, 10.0);
    let ja = page.fields.iter().find(|f| f.name == "Ja").unwrap();
    assert_eq!(ja.kind, FieldKind::Check { checked: true });

    // The turned page's rule lands where the page shows it: user (100, 700)–(300, 700) is the
    // upright line x = 700 from y = 512 down to 312 on the 792 × 612 visible page.
    let rule = &pages[1].shapes[0].paths[0].0;
    let close = |p: [f64; 2], q: [f64; 2]| (p[0] - q[0]).abs() < 0.01 && (p[1] - q[1]).abs() < 0.01;
    assert!(close(rule[0], [700.0, 512.0]) && close(rule[1], [700.0, 312.0]), "{rule:?}");

    let docx = d.export_office(OfficeFormat::Docx);
    let xml = String::from_utf8_lossy(&docx);
    // The package is deflated, so only its part names can be read here; the XML itself is
    // checked in pdfcraft-export. The shapes and fields reach the writer:
    assert!(xml.contains("word/document.xml"));
    let exported = pdfcraft_export::docx(&pages, "claim");
    assert_eq!(exported, docx, "export_office writes what export_pages reads");

    // HTML, RTF, Markdown and PowerPoint don't use the paths or fields, so they aren't read.
    let html = d.export_office(OfficeFormat::Html);
    assert!(!html.is_empty());
}

/// A form page that paints more shapes than a Word page holds says so: Word export lists it among
/// its truncated pages. Every subpath counts, so one path of many subpaths is listed too: it
/// wrote a shape per subpath past the limit without a word. The other formats don't read paths,
/// so they leave the page out of the list.
#[test]
fn word_export_reports_a_page_whose_paths_it_leaves_out() {
    // A form page (one text field) that paints `content`.
    let page = |content: String| {
        pdf(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [5 0 R] >>".into(),
            stream("", &content),
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /Rect [10 10 100 22] /P 3 0 R >>".into(),
        ])
    };
    let truncated = |content: String, format: OfficeFormat| {
        let mut s = Session::new();
        let id = s.open("paths.pdf", None, Arc::new(page(content)), None).unwrap();
        s.get(id).unwrap().export_office_checked(format).truncated_pages
    };
    let paths = |n: usize| "0 G 0 0 m 1 1 l S\n".repeat(n);
    let subpaths = |n: usize| format!("0 G {}S", "0 0 m 1 1 l ".repeat(n));
    let limit = pdfcraft_export::MAX_DOCX_PAGE_SHAPES;
    assert_eq!(truncated(paths(limit), OfficeFormat::Docx), Vec::<usize>::new());
    assert_eq!(truncated(paths(limit + 1), OfficeFormat::Docx), [0]);
    assert_eq!(truncated(subpaths(limit), OfficeFormat::Docx), Vec::<usize>::new());
    assert_eq!(truncated(subpaths(limit + 1), OfficeFormat::Docx), [0], "one path of many subpaths");
    assert_eq!(truncated(paths(limit + 1), OfficeFormat::Html), Vec::<usize>::new());
}

/// A field's border and fill come from its widget's `/MK /BC` and `/BG`, at `/BS /W` points
/// (1 by default; dashed for `/S /D`); a widget drawn round (its appearance curves, a radio
/// button's circle) gets none, as a square Word box can't draw it, nor does one without `/MK`.
#[test]
fn a_fields_border_and_fill_come_from_its_widget() {
    let widget =
        |t: &str, x: i32, extra: &str| format!("<< /Type /Annot /Subtype /Widget /FT /Btn /T ({t}) /Rect [{x} 10 {} 20] /P 3 0 R {extra} >>", x + 10);
    let bytes = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R 7 0 R 8 0 R] >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [5 0 R 6 0 R 7 0 R 8 0 R] >>".into(),
        stream("", ""),
        widget("Square", 10, "/MK << /BC [0] /BG [1] >> /AP << /N << /Off 9 0 R >> >>"),
        widget("Round", 30, "/MK << /BC [0] /BG [1] >> /AP << /N << /Off 10 0 R >> >>"),
        widget("Dashed", 50, "/MK << /BC [1 0 0] >> /BS << /W 2 /S /D >>"),
        widget("Plain", 70, ""),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", "0 G 0.5 0.5 9 9 re S"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", "0 G 5 0 m 7.8 0 10 2.2 10 5 c 10 7.8 7.8 10 5 10 c S"),
    ]);
    let mut s = Session::new();
    let id = s.open("boxes.pdf", None, Arc::new(bytes), None).unwrap();
    let pages = s.get(id).unwrap().export_pages();
    let look = |name: &str| {
        let f = pages[0].fields.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("{name}: {:#?}", pages[0].fields));
        (f.border, f.background)
    };
    assert_eq!(look("Square"), (Some(pdfcraft_export::Stroke { color: [0.0; 3], width: 1.0, dashed: false }), Some([1.0; 3])));
    assert_eq!(look("Round"), (None, None), "a round widget keeps the page's own drawing");
    assert_eq!(look("Dashed"), (Some(pdfcraft_export::Stroke { color: [1.0, 0.0, 0.0], width: 2.0, dashed: true }), None));
    assert_eq!(look("Plain"), (None, None));
}

/// A page of text past the path reader's limit (a detailed chart, a hatched background) flows in
/// Word, which draws none of its paths, so Word export doesn't list it as cut short; a form page
/// past the limit, whose layout Word keeps, is listed.
#[test]
fn unread_paths_cut_only_a_positioned_page_short() {
    let page = |field: bool| {
        let content =
            format!("BT /F1 12 Tf 72 700 Td (Quarterly figures) Tj ET\n{}", "0 G 0 0 m 1 1 l S\n".repeat(pdfcraft_edit::MAX_PAGE_SHAPES + 1));
        let (catalog, annots) = if field { (" /AcroForm << /Fields [6 0 R] >>", " /Annots [6 0 R]") } else { ("", "") };
        pdf(&[
            format!("<< /Type /Catalog /Pages 2 0 R{catalog} >>"),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >>{annots} >>"),
            stream("", &content),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /Rect [72 600 300 612] /P 3 0 R >>".into(),
        ])
    };
    let truncated = |field: bool| {
        let mut s = Session::new();
        let id = s.open("chart.pdf", None, Arc::new(page(field)), None).unwrap();
        s.get(id).unwrap().export_office_checked(OfficeFormat::Docx).truncated_pages
    };
    assert_eq!(truncated(false), Vec::<usize>::new(), "a flowing page lost none of its paths");
    assert_eq!(truncated(true), [0]);
}

/// A form page whose paths can't be read at all, its content being past the page's decode
/// budget, is listed as cut short: the reader's error left Word's page without its art and
/// without a word.
#[test]
fn word_export_reports_a_page_whose_paths_it_cannot_read() {
    // One 1 MiB content stream drawn 300 times is more than the page's decode budget.
    let refs = "6 0 R ".repeat(300);
    let doc = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents [4 0 R {refs}] /Annots [5 0 R] >>"),
        stream("", "0 G 0 0 m 100 0 l S"),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /Rect [10 10 100 22] /P 3 0 R >>".into(),
        stream("", &" ".repeat(1 << 20)),
    ]);
    const { assert!(300 << 20 > pdfcraft_cos::MAX_PAGE_DECODED) };
    let mut s = Session::new();
    let id = s.open("unreadable.pdf", None, Arc::new(doc), None).unwrap();
    assert_eq!(s.get(id).unwrap().export_office_checked(OfficeFormat::Docx).truncated_pages, [0]);
}

/// A field's font size and border width are in its page's units, as its box is: on a page of
/// `/UserUnit` 2 they are twice as big in the export (they came out at half size, the text too
/// small for its box).
#[test]
fn a_fields_text_and_border_scale_with_the_page_unit() {
    let field = |unit: &str| {
        let bytes = pdf(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200]{unit} /Contents 4 0 R /Annots [5 0 R] >>"),
            stream("", ""),
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /DA (/Helv 10 Tf 0 g) /MK << /BC [0] >> /BS << /W 1 >> /Rect [10 10 100 30] /P 3 0 R >>"
                .into(),
        ]);
        let mut s = Session::new();
        let id = s.open("unit.pdf", None, Arc::new(bytes), None).unwrap();
        let pages = s.get(id).unwrap().export_pages();
        let f = pages[0].fields[0].clone();
        (f.size, f.border.map(|b| b.width), f.rect)
    };
    assert_eq!(field(""), (10.0, Some(1.0), [10.0, 10.0, 100.0, 30.0]));
    let (size, width, rect) = field(" /UserUnit 2");
    assert_eq!((size, width), (20.0, Some(2.0)), "{rect:?}");
    assert!((rect[2] - rect[0] - 180.0).abs() < 0.01, "the box is twice as wide too: {rect:?}");
}

/// A field's font is the base font its `/DA` font resource names in the form's `/DR` (empty
/// when the form has no such resource), and each button of a radio group says how many buttons
/// its group has, so Word can say the group takes one choice.
#[test]
fn a_fields_font_and_radio_group_reach_the_export() {
    let button = |state: &str, x: i32| {
        format!(
            "<< /Type /Annot /Subtype /Widget /Parent 7 0 R /Rect [{x} 50 {} 60] /P 3 0 R /AP << /N << /{state} 11 0 R /Off 11 0 R >> >> >>",
            x + 10
        )
    };
    let bytes = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R 7 0 R] /DR << /Font << /Cour 10 0 R >> >> >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [5 0 R 6 0 R 8 0 R 9 0 R] >>".into(),
        stream("", ""),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Typed) /DA (/Cour 9 Tf 0 g) /Rect [10 10 100 22] /P 3 0 R >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Plain) /DA (/Missing 9 Tf 0 g) /Rect [10 30 100 42] /P 3 0 R >>".into(),
        "<< /FT /Btn /Ff 49152 /T (Pay) /V /A /Kids [8 0 R 9 0 R] >>".into(),
        button("A", 10),
        button("B", 30),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>".into(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", ""),
    ]);
    let mut s = Session::new();
    let id = s.open("fonts.pdf", None, Arc::new(bytes), None).unwrap();
    let pages = s.get(id).unwrap().export_pages();
    let fields = &pages[0].fields;
    let field = |name: &str| fields.iter().filter(|f| f.name == name).collect::<Vec<_>>();
    assert_eq!(field("Typed")[0].font, "Courier");
    assert_eq!(field("Plain")[0].font, "", "a /DA font the form doesn't have");
    let pay: Vec<&FieldKind> = field("Pay").iter().map(|f| &f.kind).collect();
    assert_eq!(pay, [&FieldKind::Radio { checked: true, buttons: 2 }, &FieldKind::Radio { checked: false, buttons: 2 }], "{fields:#?}");
}
