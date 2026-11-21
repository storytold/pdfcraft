//! A certification signature stays valid only while what a reader shows is what was signed:
//! revisions appended by hand that reach back to an older generation, or that replace a signed
//! form field in place, are refused.

use std::sync::Arc;

use pdfcraft_cos::{Document, Object};
use pdfcraft_sign::{Modification, SignOptions, Status, TrustStore, pkcs12, signatures};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn open(b: &[u8]) -> Document {
    Document::open(Arc::new(b.to_vec())).unwrap()
}

/// A classic-xref PDF of `objs`, numbered from 1, with object 1 as the catalog.
fn write_pdf(objs: &[&str]) -> Vec<u8> {
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

/// One row of a hand-written revision: `num header obj body` under row generation `row`, or a
/// free row with its next generation.
enum Row<'a> {
    Obj { num: u32, header: u16, row: u16, body: &'a str },
    Free { num: u32, next: u16 },
}

/// `base` with one more revision of `rows`, written as a classic table pointing back with /Prev.
fn append(base: &[u8], rows: &[Row<'_>]) -> Vec<u8> {
    let doc = open(base);
    let size = doc.trailer().get(b"Size").and_then(Object::as_int).unwrap();
    let root = doc.root().unwrap();
    let tail = String::from_utf8_lossy(&base[base.len().saturating_sub(64)..]).into_owned();
    let prev: usize = tail.rsplit("startxref").next().unwrap().split_whitespace().next().unwrap().parse().unwrap();
    let mut out = base.to_vec();
    let mut table = String::new();
    for row in rows {
        match row {
            Row::Obj { num, header, row, body } => {
                table.push_str(&format!("{num} 1\n{:010} {row:05} n \n", out.len()));
                out.extend_from_slice(format!("{num} {header} obj\n{body}\nendobj\n").as_bytes());
            }
            Row::Free { num, next } => table.push_str(&format!("{num} 1\n0000000000 {next:05} f \n")),
        }
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n{table}trailer\n<< /Size {size} /Root {} {} R /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n", root.num, root.generation)
            .as_bytes(),
    );
    out
}

fn opts(certify: u8) -> SignOptions {
    SignOptions {
        page: 0,
        rect: Some([20.0, 100.0, 220.0, 150.0]),
        date: "D:20261002120000+01'00'".into(),
        certify: Some(certify),
        ..SignOptions::default()
    }
}

/// The certification signature of `bytes`, which must be refused.
fn assert_refused(bytes: &[u8]) {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    let s = signatures(&open(bytes), bytes, &trust).into_iter().find(|s| s.signed).unwrap();
    assert!(matches!(s.modification, Modification::Disallowed(_)) && s.status != Status::Valid, "main returns {:?}, {:?}", s.status, s.modification);
}

fn certify(unsigned: &[u8], p: u8) -> Vec<u8> {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    pdfcraft_sign::sign(&open(unsigned), &id, &opts(p)).unwrap()
}

const PAGE2: &str = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 7 0 R /Resources << /Font << /F1 6 0 R >> >> >>";

/// Revision 2 rotates page 2 (`4 0 obj`); revision 3 writes the signed page back as `4 1 obj`.
/// The page tree still says `4 0 R`.
fn intermediate_generation_cover(p: u8) {
    let unsigned = write_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 5 0 R /Resources << /Font << /F1 6 0 R >> >> >>",
        PAGE2,
        "<< /Length 41 >>\nstream\nBT /F1 14 Tf 20 250 Td (Page one) Tj ET\nendstream",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        "<< /Length 41 >>\nstream\nBT /F1 14 Tf 20 250 Td (Page two) Tj ET\nendstream",
    ]);
    let signed = certify(&unsigned, p);
    let rotated = PAGE2.replace(" >> >> >>", " >> >> /Rotate 180 >>");
    let two = append(&signed, &[Row::Obj { num: 4, header: 0, row: 0, body: &rotated }]);
    let three = append(&two, &[Row::Obj { num: 4, header: 1, row: 1, body: PAGE2 }]);
    let doc = open(&three);
    assert!(doc.xref_oddities().is_empty());
    assert_eq!(doc.revisions().len(), open(&signed).revisions().len() + 2);
    assert_eq!(doc.get(pdfcraft_cos::ObjRef { num: 4, generation: 1 }), open(&signed).get(pdfcraft_cos::ObjRef { num: 4, generation: 0 }));
    assert_refused(&three);
}

#[test]
fn intermediate_generation_cover_p1() {
    intermediate_generation_cover(1);
}

#[test]
fn intermediate_generation_cover_p2() {
    intermediate_generation_cover(2);
}

#[test]
fn intermediate_generation_cover_p3() {
    intermediate_generation_cover(3);
}

const COMMENT: &str = "<< /Type /Annot /Subtype /Text /Rect [20 200 40 220] /Contents (note) >>";

/// A page with text field `4 0 R`, listed in /Fields and in the page's inline /Annots, certified
/// allowing comments.
fn certified_form() -> Vec<u8> {
    certify(
        &write_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Annots [4 0 R] >>",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /V (Ann) /Rect [20 200 120 220] /P 3 0 R >>",
        ]),
        3,
    )
}

#[test]
fn widget_replaced_in_place_by_comment() {
    let signed = certified_form();
    assert_refused(&append(&signed, &[Row::Obj { num: 4, header: 0, row: 0, body: COMMENT }]));
}

#[test]
fn widget_freed_then_reused_as_comment() {
    let signed = certified_form();
    let freed = append(&signed, &[Row::Free { num: 4, next: 1 }]);
    assert_refused(&append(&freed, &[Row::Obj { num: 4, header: 0, row: 0, body: COMMENT }]));
}
