use std::sync::Arc;

use pdfcraft_engine::{DocId, Session};

pub fn pdf() -> Vec<u8> {
    let content = "BT /F1 12 Tf 14.4 TL 1 0 0 1 10 250 Tm (Alpha paragraph, first row.) Tj T* (Alpha paragraph, second row.) Tj ET \
                   BT /F1 12 Tf 14.4 TL 1 0 0 1 10 200 Tm (Bravo paragraph, a single row.) Tj ET \
                   BT /F1 12 Tf 14.4 TL 1 0 0 1 10 150 Tm (Charlie paragraph, first row.) Tj T* (Charlie paragraph, second row.) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 4 0 R >> >> >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

pub fn session() -> (Session, DocId) {
    let mut session = Session::new();
    let id = session.open("fixture.pdf", None, Arc::new(pdf()), None).unwrap();
    (session, id)
}
