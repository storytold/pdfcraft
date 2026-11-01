//! Office export of pages that draw one image many times.

use std::sync::Arc;

use pdfcraft_engine::Session;

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

/// One image drawn many times is prepared once: every placement shares its encoded bytes.
#[test]
fn an_image_drawn_many_times_is_encoded_once() {
    let placements = 500;
    let content = "q 10 0 0 10 0 0 cm /Im Do Q\n".repeat(placements);
    let bytes = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /XObject << /Im 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
        "<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 4 >>\nstream\n\u{0}\u{40}\u{80}\u{ff}\nendstream".into(),
    ]);
    let mut s = Session::new();
    let id = s.open("many.pdf", None, Arc::new(bytes), None).unwrap();
    let pages = s.get(id).unwrap().export_pages();
    let images = &pages[0].images;
    assert_eq!(images.len(), placements);
    let first = images[0].bytes.as_ptr();
    assert!(!images[0].bytes.is_empty());
    assert!(images.iter().all(|im| im.bytes.as_ptr() == first), "each placement holds its own copy of the image");
}
