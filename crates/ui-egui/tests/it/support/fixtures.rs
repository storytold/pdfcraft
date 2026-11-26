//! Synthetic documents shared by the shell tests.

/// `count` valid pages (proper xref, so no repair banner covers a dialog), each 300 x 200 pt with
/// black text on white.
pub fn pdf_pages(count: usize) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        {
            let body = b"BT /F1 14 Tf 20 150 Td (The quick brown fox) Tj ET";
            format!("<< /Length {} >>\nstream\n{}\nendstream", body.len(), String::from_utf8_lossy(body)).into_bytes()
        },
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    let mut kids = vec!["3 0 R".to_owned()];
    for _ in 1..count {
        kids.push(format!("{} 0 R", objs.len() + 1));
        objs.push(objs[2].clone());
    }
    objs[1] = format!("<< /Type /Pages /Kids [{}] /Count {count} >>", kids.join(" ")).into_bytes();
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}
