use std::sync::Arc;

use super::*;

/// A one-page document whose page has `annots` as its `/Annots`, plus `extra` objects from 4 on.
fn doc(annots: &str, extra: &[Vec<u8>]) -> Document {
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /Annots {annots} >>").into_bytes(),
    ];
    objs.extend(extra.iter().cloned());
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
    Document::open(Arc::new(out)).expect("opens")
}

fn stream(dict: &str, body: &[u8]) -> Vec<u8> {
    let mut s = format!("<< {dict} /Length {} >>\nstream\n", body.len()).into_bytes();
    s.extend_from_slice(body);
    s.extend_from_slice(b"\nendstream");
    s
}

fn annot(dd: &str) -> Vec<u8> {
    format!("<< /Type /Annot /Subtype /3D /Rect [0 0 100 100] /3DD {dd} >>").into_bytes()
}

#[test]
fn a_plain_3d_stream_is_read_through_its_filters() {
    let d = doc("[4 0 R]", &[annot("5 0 R"), stream("/Type /3D /Subtype /PRC /Filter /ASCIIHexDecode", b"505243>")]);
    assert_eq!(model_3d(&d, 0, 0), Ok(Model3d { kind: Model3dKind::Prc, data: b"PRC".to_vec() }));
}

#[test]
fn a_3dref_leads_to_the_stream_it_names() {
    let d = doc("[4 0 R]", &[annot("<< /Type /3DRef /3DD 5 0 R >>"), stream("/Type /3D /Subtype /PRC", b"PRC")]);
    assert_eq!(model_3d(&d, 0, 0).map(|m| m.data), Ok(b"PRC".to_vec()));
}

#[test]
fn a_u3d_stream_is_read_and_says_so() {
    let d = doc("[4 0 R]", &[annot("5 0 R"), stream("/Type /3D /Subtype /U3D", b"U3D")]);
    assert_eq!(model_3d(&d, 0, 0).map(|m| m.kind), Ok(Model3dKind::U3d));
}

#[test]
fn a_3d_annotation_without_a_stream_is_invalid() {
    let d = doc("[<< /Type /Annot /Subtype /3D /Rect [0 0 1 1] >>]", &[]);
    assert!(matches!(model_3d(&d, 0, 0), Err(AnnotError::Invalid(m)) if m.contains("no 3D stream")));
}

#[test]
fn an_unknown_format_is_invalid() {
    let d = doc("[4 0 R]", &[annot("5 0 R"), stream("/Type /3D /Subtype /STL", b"x")]);
    assert!(matches!(model_3d(&d, 0, 0), Err(AnnotError::Invalid(m)) if m.contains("unknown format (STL)")));
}

#[test]
fn a_cycle_of_3drefs_ends() {
    let d = doc("[4 0 R]", &[annot("5 0 R"), b"<< /Type /3DRef /3DD 6 0 R >>".to_vec(), b"<< /Type /3DRef /3DD 5 0 R >>".to_vec()]);
    assert!(matches!(model_3d(&d, 0, 0), Err(AnnotError::Invalid(m)) if m.contains("no 3D stream")));
}

#[test]
fn another_annotation_is_not_a_3d_one() {
    let d = doc("[<< /Type /Annot /Subtype /Square /Rect [0 0 1 1] >> 4 0 R]", &[annot("5 0 R"), stream("/Subtype /PRC", b"PRC")]);
    assert!(matches!(model_3d(&d, 0, 0), Err(AnnotError::Invalid(m)) if m.contains("not a 3D annotation")));
    assert_eq!(model_3d(&d, 0, 2), Err(AnnotError::NoSuchAnnotation { page: 0, index: 2 }));
    assert_eq!(model_3d_annotations(&d, 0), Ok(vec![1]));
    assert_eq!(model_3d_annotations(&d, 1), Err(AnnotError::NoSuchPage(1)));
}
