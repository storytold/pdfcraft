use std::sync::Arc;

use super::*;

/// A document from its objects (object 1 is the catalog).
fn document(objects: &[String]) -> pdfcraft_cos::Document {
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
    pdfcraft_cos::Document::open(Arc::new(out)).unwrap()
}

/// Two check boxes whose shown state is one round appearance stream, read once and shared; with
/// nothing left to read, a widget's shape is the form reader's (a check box is square), so a
/// file of thousands of widgets with appearances of their own can't make the export read them all.
#[test]
fn widget_shapes_are_read_once_per_stream_within_the_exports_budget() {
    let check = |t: &str, x: i32| {
        format!(
            "<< /Type /Annot /Subtype /Widget /FT /Btn /T ({t}) /Rect [{x} 10 {} 20] /P 3 0 R /MK << /BC [0] >> /AS /Off /AP << /N << /Off 7 0 R >> >> >>",
            x + 10
        )
    };
    let data = "0 G 5 0 m 7.8 0 10 2.2 10 5 c 10 7.8 7.8 10 5 10 c S";
    let doc = document(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R] >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [5 0 R 6 0 R] >>".into(),
        "<< /Length 0 >>\nstream\n\nendstream".into(),
        check("A", 10),
        check("B", 30),
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length {} >>\nstream\n{data}\nendstream", data.len()),
    ]);
    let fields = pdfcraft_forms::fields(&doc);
    let widgets: Vec<&pdfcraft_forms::Widget> = fields.iter().flat_map(|f| &f.widgets).collect();
    assert_eq!(widgets.len(), 2);

    let mut looks = Looks::new();
    for w in &widgets {
        assert_eq!(widget_look(&doc, w, &mut looks), (None, None), "drawn round");
    }
    assert_eq!(looks.shapes.len(), 1, "one stream, read once");
    assert_eq!(looks.bytes, MAX_LOOKS_BYTES - data.len());

    let mut spent = Looks::within(0);
    let square = Some(pdfcraft_export::Stroke { color: [0.0; 3], width: 1.0, dashed: false });
    assert_eq!(widget_look(&doc, widgets[0], &mut spent), (square, None), "the reader's shape: a square check box");
}

/// Three pages share one content stream, as a small hostile file can repeat expensive artwork
/// on arbitrarily many pages. Framing boxes must still select positioned layout after the shared
/// point budget is spent; simple underlines still flow and do not report unused artwork.
#[test]
fn word_shape_points_are_bounded_across_pages_without_changing_layout() {
    for framed in [true, false] {
        let mut data = String::from("BT /F1 10 Tf 20 100 Td (Label) Tj ET ");
        for x in 15..21 {
            if framed {
                data.push_str(&format!("{x} 95 60 20 re S "));
            } else {
                data.push_str(&format!("{x} 95 m 75 95 l S "));
            }
        }
        let doc = document(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 7 0 R >> >> /Contents 6 0 R >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 7 0 R >> >> /Contents 6 0 R >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 7 0 R >> >> /Contents 6 0 R >>".into(),
            format!("<< /Length {} >>\nstream\n{data}\nendstream", data.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ]);
        let mut session = Session::new();
        let id = session.open("repeated-art.pdf", None, doc.bytes().clone(), None).unwrap();
        let d = session.get(id).unwrap();
        let cos = &d.editor.as_ref().unwrap().cos;
        let points_per_shape = if framed { 4 } else { 2 };
        // One whole page and one shape on the next page fit; the last page retains no shapes.
        let limit = 7 * points_per_shape;
        let (pages, dropped) = d.export_pages_with(false, 0, &mut pdfcraft_cos::ImageBudget::new(cos), true, limit, MAX_EXPORT_FIELD_BYTES);
        assert!(dropped.is_empty(), "shape omissions are reported by the writer, after choosing layout");
        assert_eq!(pages.iter().map(|p| p.shapes.len()).collect::<Vec<_>>(), [6, 1, 0]);
        let points: usize = pages.iter().flat_map(|p| &p.shapes).flat_map(|s| &s.paths).map(|(p, _)| p.len()).sum();
        assert_eq!(points, limit);
        assert_eq!(pages.iter().map(|p| p.art_truncated).collect::<Vec<_>>(), [false, true, true]);
        assert!(pages.iter().all(|p| p.art_framed == framed));
        assert!(pages.iter().all(|p| p.blocks.iter().any(|b| b.text == "Label")), "a point limit never drops text");
        let (_, dropped) = pdfcraft_export::docx_checked(&pages, "repeated artwork");
        assert_eq!(dropped, if framed { vec![1, 2] } else { vec![] });
    }
}

/// One PDF field can repeat its value or choices in widgets on many pages. The export must
/// charge each prepared copy before cloning, while keeping the layout of pages whose fields
/// no longer fit. A small budget exercises the same boundary without allocating hostile data.
#[test]
fn word_field_payload_is_bounded_across_pages_before_cloning_widgets() {
    for choice in [false, true] {
        let value = "value".repeat(400);
        let props = if choice { format!("/FT /Ch /V (v) /Opt [[(v) ({value})] (other)]") } else { format!("/FT /Tx /V ({value})") };
        let data = "BT /F1 10 Tf 20 100 Td (Label) Tj ET";
        let page = |widget: usize| {
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 7 0 R >> >> /Contents 6 0 R /Annots [{widget} 0 R] >>"
            )
        };
        let widget = |page: usize| format!("<< /Type /Annot /Subtype /Widget /Parent 8 0 R /Rect [20 70 100 85] /P {page} 0 R >>");
        let doc = document(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [8 0 R] >> >>".into(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".into(),
            page(9),
            page(10),
            page(11),
            format!("<< /Length {} >>\nstream\n{data}\nendstream", data.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
            format!("<< /T (Repeated) {props} /Kids [9 0 R 10 0 R 11 0 R] >>"),
            widget(3),
            widget(4),
            widget(5),
        ]);
        let fields = pdfcraft_forms::fields(&doc);
        assert_eq!(fields.len(), 1);
        let limit = prepared_field_bytes(&fields[0]).unwrap();
        let mut session = Session::new();
        let id = session.open("repeated-field.pdf", None, doc.bytes().clone(), None).unwrap();
        let d = session.get(id).unwrap();
        let cos = &d.editor.as_ref().unwrap().cos;
        let (pages, dropped) = d.export_pages_with(false, 0, &mut pdfcraft_cos::ImageBudget::new(cos), true, MAX_EXPORT_SHAPE_POINTS, limit);
        assert_eq!(pages.iter().map(|p| p.fields.len()).collect::<Vec<_>>(), [1, 0, 0]);
        assert_eq!(pages.iter().map(|p| p.fields_truncated).collect::<Vec<_>>(), [false, true, true]);
        assert_eq!(dropped, [1, 2]);
        assert!(pages.iter().all(|p| p.blocks.iter().any(|b| b.text == "Label")), "a field limit never drops page text");
        let (_, dropped) = pdfcraft_export::docx_checked(&pages, "repeated field");
        assert_eq!(dropped, [1, 2], "pages with no retained fields still use positioned layout and report their loss");
    }
}

#[test]
fn word_fields_use_each_widgets_default_appearance() {
    let doc = document(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /DR << /Font << /Helv 8 0 R /Cour 9 0 R >> >> >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [6 0 R 7 0 R] >>".into(),
        "<< /Length 0 >>\nstream\n\nendstream".into(),
        "<< /FT /Tx /T (Repeated) /DA (/Helv 12 Tf 0 g) /Kids [6 0 R 7 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /Parent 5 0 R /Rect [10 10 100 30] /DA (/Cour 9 Tf 0 g) >>".into(),
        "<< /Type /Annot /Subtype /Widget /Parent 5 0 R /Rect [10 40 100 60] >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>".into(),
    ]);
    let mut session = Session::new();
    let id = session.open("widget-fonts.pdf", None, doc.bytes().clone(), None).unwrap();
    let pages = session.get(id).unwrap().export_pages();
    assert_eq!(pages[0].fields.iter().map(|f| (f.font.as_str(), f.size)).collect::<Vec<_>>(), [("Courier", 9.0), ("Helvetica", 12.0)]);
}

#[test]
fn word_widget_looks_decode_escaped_appearance_state_names() {
    for name in ["A#23B", "#FF"] {
        let data = "0 G 0.5 0.5 9 9 re S";
        let doc = document(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [5 0 R] >>".into(),
            "<< /Length 0 >>\nstream\n\nendstream".into(),
            format!(
                "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 32768 /T (Radio) /Rect [10 10 20 20] /MK << /BC [0] /BG [1] >> /AS /{name} /AP << /N << /{name} 6 0 R >> >> >>"
            ),
            format!("<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length {} >>\nstream\n{data}\nendstream", data.len()),
        ]);
        let fields = pdfcraft_forms::fields(&doc);
        let mut looks = Looks::new();
        let (border, background) = widget_look(&doc, &fields[0].widgets[0], &mut looks);
        assert!(border.is_some(), "the square appearance must override the radio's round fallback: {name}");
        assert_eq!(background, Some([1.0; 3]));
        assert_eq!(looks.shapes.len(), 1);
    }
}

/// Read a deflated XML member of the Office package generated by this test.
fn office_part(zip: &[u8], name: &str) -> String {
    let at = |i: usize, n: usize| zip[i..i + n].iter().rev().fold(0usize, |v, b| v << 8 | *b as usize);
    let mut i = 0;
    while zip[i..].starts_with(b"PK\x03\x04") {
        let (size, name_len, extra_len) = (at(i + 18, 4), at(i + 26, 2), at(i + 28, 2));
        let data = i + 30 + name_len + extra_len;
        if &zip[i + 30..i + 30 + name_len] == name.as_bytes() {
            let bytes = pdfcraft_filters::decode(&[(pdfcraft_filters::Filter::Flate, Default::default())], &zip[data..data + size], 1 << 20).unwrap();
            return String::from_utf8(bytes).unwrap();
        }
        i = data + size;
    }
    panic!("{name} missing")
}

#[test]
fn word_export_omits_password_controls_and_reports_the_page() {
    let doc = document(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [5 0 R] >>".into(),
        "<< /Length 0 >>\nstream\n\nendstream".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /Ff 8192 /T (Password) /V (never-export-this-secret) /Rect [10 10 100 30] >>".into(),
    ]);
    let mut session = Session::new();
    let id = session.open("password-field.pdf", None, doc.bytes().clone(), None).unwrap();
    let d = session.get(id).unwrap();
    let pages = d.export_pages();
    assert!(pages[0].fields.is_empty());
    assert!(pages[0].fields_truncated, "the page stays positioned even when its only control is omitted");
    let export = d.export_office_checked(OfficeFormat::Docx);
    assert_eq!(export.truncated_pages, [0]);
    let xml = office_part(&export.bytes, "word/document.xml");
    assert!(!xml.contains("never-export-this-secret"), "the saved XML must not disclose the password");
    assert!(!xml.contains("<w:sdt"), "no unmasked control or editable placeholder replaces the password field");
}
