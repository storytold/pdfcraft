//! Preflight must preserve references accepted by the COS generation-repair policy.
use pdfcraft_cos::{Document, Object, SaveOptions, write_full, write_incremental};
use pdfcraft_preflight::{Level, convert};
use std::sync::Arc;

fn image_file(damaged: bool) -> Arc<Vec<u8>> {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let bodies = [
        "<< /Type /Catalog /Pages 2 0 R /HeaderImage 5 1 R /RowImage 5 0 R /StaleImage 5 2 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Im 5 1 R >> >> >>",
        "null",
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /Interpolate true /Length 1 >>\nstream\nX\nendstream",
    ];
    let mut offsets = Vec::new();
    for (i, body) in bodies.iter().enumerate() {
        offsets.push(bytes.len());
        let generation = u16::from(i == 4);
        bytes.extend_from_slice(format!("{} {generation} obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for (i, offset) in offsets.into_iter().enumerate() {
        let generation = u16::from(i == 4 && !damaged);
        bytes.extend_from_slice(format!("{offset:010} {generation:05} n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    Arc::new(bytes)
}

fn assert_images(doc: &Document, converted: bool, dual: bool) {
    let root = doc.get(doc.root().unwrap());
    let catalog = root.as_dict().unwrap();
    for key in [b"HeaderImage".as_slice(), b"RowImage".as_slice()] {
        let value = doc.resolve(catalog.get(key).unwrap());
        if key == b"RowImage" && !dual {
            assert_eq!(*value, Object::Null);
        } else {
            let Object::Stream(image) = &*value else { panic!("{key:?} became {value:?}") };
            assert_eq!(image.dict.get(b"Interpolate"), Some(&Object::Bool(!converted)));
            assert_eq!(&image.raw[..], b"X");
        }
    }
    assert_eq!(*doc.resolve(catalog.get(b"StaleImage").unwrap()), Object::Null);
}

#[test]
fn preflight_preserves_repaired_image_references_in_snapshots_and_saves() {
    for damaged in [false, true] {
        let original = image_file(damaged);
        let mut doc = Document::open(original.clone()).unwrap();
        assert_images(&doc, false, damaged);
        let snapshot = doc.clone();
        let report = convert(&mut doc, Level::A2b).unwrap();
        assert!(report.fixed.iter().any(|line| line.contains("cleared image interpolation")));
        assert_images(&doc, true, damaged);
        assert_eq!(doc.full_save_required(), damaged);
        assert_images(&snapshot, false, damaged);
        let mut restored = snapshot.clone();
        restored.overlay_changes_from(&doc).unwrap();
        assert_images(&restored, true, damaged);
        for changed in [&doc, &restored] {
            for object_streams in [false, true] {
                let opts = SaveOptions { object_streams, ..SaveOptions::default() };
                let incremental = write_incremental(changed, &opts).unwrap();
                if !damaged {
                    assert!(incremental.starts_with(&original));
                }
                assert_images(&Document::open(Arc::new(incremental)).unwrap(), true, damaged);
                let full = write_full(changed, &opts).unwrap();
                let reopened = Document::open(Arc::new(full)).unwrap();
                assert_images(&reopened, true, damaged);
                if damaged {
                    let catalog = reopened.get(reopened.root().unwrap());
                    let catalog = catalog.as_dict().unwrap();
                    assert_eq!(catalog.reference(b"HeaderImage"), catalog.reference(b"RowImage"), "saved aliases must share one object");
                }
            }
        }
    }
}
