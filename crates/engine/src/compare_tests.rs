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
