//! Optional Windows Print Support App; PDF conversion remains independent of Windows drivers.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(any(target_os = "linux", test))]
pub mod cups;

#[cfg(all(windows, feature = "winrt-server"))]
mod exports;
#[cfg(windows)]
mod workflow;

/// The same bounded XPS converter exposed by the engine and doc_create, with no raster fallback.
pub fn convert(title: &str, source: &[u8]) -> Result<Vec<u8>, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let doc = pdfcraft_create::from_xps(title, source).map_err(|e| e.to_string())?;
        pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).map_err(|e| e.to_string())
    }))
    .map_err(|_| "The print job could not be converted; no PDF was written".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "local synthetic native print capture; never publish source fonts or output"]
    fn native_xps_capture_converts_without_page_images() {
        let source = std::fs::read(std::env::var_os("PDFCRAFT_TEST_XPS").expect("synthetic XPS path")).unwrap();
        let pdf = convert("Native printer test", &source).unwrap();
        let doc = pdfcraft_cos::Document::open(pdf.clone().into()).unwrap();
        assert!(doc.scan_objects().any(|(_, o)| o.as_dict().is_some_and(|d| d.get(b"ToUnicode").is_some())));
        assert!(!doc.scan_objects().any(|(_, o)| o.as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Image".as_slice()))));
        std::fs::write(std::env::var_os("PDFCRAFT_TEST_PDF").expect("private PDF output path"), pdf).unwrap();
    }

    #[test]
    fn printer_entry_preserves_unicode_text_and_rejects_bad_jobs() {
        let mut zip = pdfcraft_export::Zip::default();
        for (name, bytes) in [
            ("_rels/.rels",br#"<Relationships><Relationship Type="http://schemas.microsoft.com/xps/2005/06/fixedrepresentation" Target="Fixed.fdseq"/></Relationships>"#.as_slice()),
            ("Fixed.fdseq",br#"<FixedDocumentSequence><DocumentReference Source="Doc.fdoc"/></FixedDocumentSequence>"#),
            ("Doc.fdoc",br#"<FixedDocument><PageContent Source="Page.fpage"/></FixedDocument>"#),
            ("Page.fpage",br##"<FixedPage Width="816" Height="1056"><Glyphs Fill="#000000" FontUri="font.ttf" FontRenderingEmSize="20" OriginX="96" OriginY="96" UnicodeString="Select these words"/></FixedPage>"##),
            ("font.ttf",include_bytes!("../../../assets/fonts/Inter-Regular.ttf")),
        ] { zip.add(name,bytes,true); }
        let pdf = convert("Print test", &zip.finish()).unwrap();
        let doc = pdfcraft_cos::Document::open(pdf.into()).unwrap();
        assert!(doc.scan_objects().any(|(_, o)| o.as_dict().is_some_and(|d| d.get(b"ToUnicode").is_some())));
        assert!(!doc.scan_objects().any(|(_, o)| o.as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Image".as_slice()))));
        assert!(convert("Bad print", b"invalid").is_err());
    }
}
