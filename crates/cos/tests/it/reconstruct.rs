//! Rebuilding the cross-reference index of a damaged file finds the objects inside object
//! streams however the stream's type is written.

use std::sync::Arc;

use pdfcraft_cos::{Document, Object};

/// A file whose `startxref` points nowhere, with three object streams (type written plainly, as
/// `/Obj#53tm`, and after a string that contains the word `stream`), and one ordinary stream
/// whose data mentions `ObjStm` and `#`.
fn damaged() -> Vec<u8> {
    let mut out = b"%PDF-1.5\n".to_vec();
    let mut add = |n: u32, body: &[u8]| {
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    let objstm = |dict_prefix: &str, members: &[(u32, &str)]| {
        let mut header = String::new();
        let mut body = String::new();
        for (n, text) in members {
            header.push_str(&format!("{n} {} ", body.len()));
            body.push_str(text);
            body.push('\n');
        }
        let mut b =
            format!("<< {dict_prefix} /N {} /First {} /Length {} >>\nstream\n", members.len(), header.len(), header.len() + body.len()).into_bytes();
        b.extend_from_slice(header.as_bytes());
        b.extend_from_slice(body.as_bytes());
        b.extend_from_slice(b"\nendstream");
        b
    };
    add(1, b"<< /Type /Catalog /Pages 2 0 R >>");
    add(2, b"<< /Type /Pages /Kids [] /Count 0 >>");
    add(3, &objstm("/Type /ObjStm", &[(10, "<< /From 3 >>"), (11, "[3]")]));
    add(4, &objstm("/Type /Obj#53tm", &[(12, "<< /From 4 >>")]));
    add(5, &objstm("/Note (a stream) /Type /ObjStm", &[(13, "<< /From 5 >>")]));
    let data = b"/Type /ObjStm # 14 0 << /From 6 >>";
    let mut plain = format!("<< /Length {} >>\nstream\n", data.len()).into_bytes();
    plain.extend_from_slice(data);
    plain.extend_from_slice(b"\nendstream");
    add(6, &plain);
    // Strings before the type that look like an object header, or like the end of the
    // dictionary and the `stream` keyword, and a hex string and a comment.
    add(7, &objstm("/Note (\n99 0 obj\n) /Type /ObjStm", &[(15, "<< /From 7 >>")]));
    add(8, &objstm("/Note (>>stream\n) /Type /ObjStm", &[(16, "<< /From 8 >>")]));
    add(9, &objstm("/Note <3e3e> /Other << /A (a (b) \\) >>\nstream\n) >> % >>\nstream\n\n/Type /ObjStm", &[(17, "<< /From 9 >>")]));
    out.extend_from_slice(b"trailer\n<< /Size 20 /Root 1 0 R >>\nstartxref\n99999999\n%%EOF\n");
    out
}

#[test]
fn objects_in_object_streams_are_found_however_the_type_is_written() {
    let doc = Document::open(Arc::new(damaged())).expect("opens");
    assert!(doc.reconstructed());
    for n in [10, 11, 12, 13, 15, 16, 17] {
        let o = doc.try_get(n).unwrap_or_else(|e| panic!("object {n}: {e}"));
        assert!(matches!(&*o, Object::Dict(_) | Object::Array(_)), "object {n} is {o:?}");
    }
    // The data of an ordinary stream is not an object stream, whatever it says.
    assert!(!doc.object_numbers().contains(&14));
}
