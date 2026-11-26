//! Desktop document conversion through the bundled Craft readers and PDF writers.
use crate::CreateError;

/// Document, spreadsheet and presentation types shown in the Office file picker.
pub const OFFICE_EXTENSIONS: &[&str] = &[
    "docx",
    "docm",
    "dotx",
    "dotm",
    "doc",
    "dot",
    "md",
    "markdown",
    "html",
    "htm",
    "xhtml",
    "mht",
    "mhtml",
    "rtf",
    "odt",
    "ott",
    "fodt",
    "tex",
    "latex",
    "ltx",
    "xml",
    "json",
    "xlsx",
    "xlsm",
    "xltx",
    "xltm",
    "xls",
    "xlt",
    "xlsb",
    "ods",
    "csv",
    "tsv",
    "tab",
    "scjson",
    "slk",
    "deckcraft",
    "pptx",
    "pptm",
    "potx",
    "potm",
    "ppsx",
    "ppsm",
    "ppt",
    "pps",
    "pot",
];

pub fn is_office(name: &str, bytes: &[u8]) -> bool {
    let ext = std::path::Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if OFFICE_EXTENSIONS.contains(&ext.as_str()) {
        return true;
    }
    if cfg!(not(target_arch = "wasm32")) && ext == "txt" && tabular_text(bytes) {
        return true;
    }
    if crate::TEXT_EXTENSIONS.contains(&ext.as_str())
        || crate::IMAGE_EXTENSIONS.contains(&ext.as_str())
        || matches!(ext.as_str(), "pdf" | "zip" | "ots" | "odp" | "otp")
    {
        return false;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        return !bytes.is_empty()
            && (bytes.starts_with(b"PK\x03\x04")
                || bytes.starts_with(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1")
                || wordcraft_formats::txt::is_text(bytes));
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = bytes;
        false
    }
}

fn tabular_text(bytes: &[u8]) -> bool {
    bytes.split(|b| *b == b'\n').next().is_some_and(|line| line.contains(&b'\t'))
}

#[cfg(not(target_arch = "wasm32"))]
fn grid_error(error: gridcraft_engine::EngineError) -> CreateError {
    let message = error.to_string();
    if message.contains(gridcraft_engine::io::PASSWORD_REQUIRED) {
        CreateError::NeedsPassword
    } else if message.contains(gridcraft_engine::io::WRONG_PASSWORD) {
        CreateError::WrongPassword
    } else {
        CreateError::Office(message)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn spreadsheet_pdf(name: &str, bytes: &[u8], password: Option<&str>, records: bool) -> Result<Vec<u8>, CreateError> {
    use gridcraft_engine::{DocState, Session, Trust, cmd::print};
    let doc = if records {
        let mut session = Session::new();
        session.execute("file.new", serde_json::json!({})).map_err(grid_error)?;
        session.execute("data.getData", serde_json::json!({"text": String::from_utf8_lossy(bytes)})).map_err(grid_error)?;
        session.doc().map_err(grid_error)?.wb.clone()
    } else {
        let (wb, _) = gridcraft_engine::io::open_bytes_with_password(name, bytes, password).map_err(grid_error)?;
        DocState::new(wb, None, name.into(), Trust::FromFile).wb
    };
    let pages = print::layouts(&doc, &print::PrintOptions { all_sheets: true, ..Default::default() });
    print::render_pdf(&doc, &pages, name, "").map(|pdf| pdf.bytes).map_err(grid_error)
}

/// OPC is shared by Word, spreadsheets and slide decks. Identify the main part before
/// handing the package to an importer (DeckCraft's own sniff accepts any OPC package).
#[cfg(not(target_arch = "wasm32"))]
fn package_family(bytes: &[u8]) -> Option<&'static str> {
    if !bytes.starts_with(b"PK") {
        return None;
    }
    let limits = craft_opc::Limits::new(100_000, 4 << 20, 8 << 20);
    let mut pkg = craft_opc::LazyPackage::open(bytes, &limits).ok()?;
    let rels = pkg.rels("").ok()?;
    let root = rels.first("officeDocument")?;
    let types = craft_opc::ContentTypes::parse(&pkg.read("[Content_Types].xml").ok()??);
    let ty = types.of(&root.target)?.to_ascii_lowercase();
    if ty.contains("presentationml") || ty.contains("ms-powerpoint") {
        Some("slides")
    } else if ty.contains("spreadsheetml") || ty.contains("ms-excel") {
        Some("sheet")
    } else {
        Some("document")
    }
}

pub fn office_to_pdf(name: &str, bytes: &[u8]) -> Result<Vec<u8>, CreateError> {
    office_to_pdf_with_password(name, bytes, None)
}

pub fn office_to_pdf_with_password(name: &str, bytes: &[u8], password: Option<&str>) -> Result<Vec<u8>, CreateError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use wordcraft_engine::io;
        let ext = std::path::Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let slide_type = matches!(ext.as_str(), "deckcraft" | "ppt" | "pptx" | "pptm" | "pps" | "ppsx" | "ppsm" | "pot" | "potx" | "potm");
        let package = package_family(bytes);
        let native = deckcraft_format::sniff(bytes);
        let binary = deckcraft_ppt::sniff(bytes);
        if native || binary || package == Some("slides") || (slide_type && package.is_none()) {
            let mut slides = if native {
                deckcraft_format::load(bytes).map_err(|e| CreateError::Office(e.to_string()))?
            } else if binary {
                deckcraft_ppt::import(bytes).map_err(|e| CreateError::Office(e.to_string()))?
            } else if deckcraft_pptx::is_encrypted(bytes) {
                return Err(CreateError::Office("Password-protected presentations are unsupported".into()));
            } else {
                deckcraft_pptx::import(bytes).map_err(|e| CreateError::Office(e.to_string()))?
            };
            deckcraft_format::repair(&mut slides);
            let options = deckcraft_pdf::PdfOptions { include_hidden: true, ..Default::default() };
            return deckcraft_pdf::export(&slides, &options).map_err(|e| CreateError::Office(e.to_string()));
        }
        let json = (ext == "json").then(|| serde_json::from_slice::<serde_json::Value>(bytes).ok()).flatten();
        let records = json.as_ref().is_some_and(|v| v.is_array());
        if package == Some("sheet")
            || matches!(gridcraft_xlsx::sniff(bytes), gridcraft_xlsx::Format::Xls | gridcraft_xlsx::Format::Xlsb | gridcraft_xlsx::Format::Ods)
            || records
            || json.as_ref().is_some_and(|v| v.get("sheets").is_some())
            || (package.is_none()
                && matches!(
                    ext.as_str(),
                    "xlsx" | "xlsm" | "xltx" | "xltm" | "xls" | "xlt" | "xlsb" | "ods" | "csv" | "tsv" | "tab" | "scjson" | "slk"
                ))
            || (ext == "txt" && tabular_text(bytes))
        {
            return spreadsheet_pdf(name, bytes, password, records);
        }
        let (doc, _) = io::open_bytes_with(name, bytes, password).map_err(|error| {
            if io::needs_password(&error) {
                CreateError::NeedsPassword
            } else if io::wrong_password(&error) {
                CreateError::WrongPassword
            } else {
                CreateError::Office(error)
            }
        })?;
        io::save_bytes("converted.pdf", &doc).map_err(CreateError::Office)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (bytes, password);
        Err(CreateError::Office(format!("{name}: Office conversion requires desktop PDFCraft")))
    }
}
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use wordcraft_engine::io;

    #[test]
    fn spreadsheet_text_and_native_json_reach_pdf_export() {
        let (workbook, _) = gridcraft_engine::io::open_bytes("input.csv", b"Product,Count\nPaper,42").unwrap();
        let native = gridcraft_engine::io::save_bytes(&workbook, "input.json").unwrap();
        for (name, bytes) in [
            ("workbook.json", native.as_slice()),
            ("workbook.scjson", &native),
            ("records.json", br#"[{"Product":"Paper","Count":42}]"#),
            ("input.csv", b"Product,Count\nPaper,42"),
            ("input.tsv", b"Product\tCount\nPaper\t42"),
            ("input.tab", b"Product\tCount\nPaper\t42"),
            ("input.txt", b"Product\tCount\nPaper\t42"),
            ("input.slk", b"ID;P\nC;X1;Y1;K42\nE\n"),
        ] {
            assert_eq!(crate::source_kind(name, bytes), Some(crate::SourceKind::Office));
            assert!(office_to_pdf(name, bytes).unwrap_or_else(|e| panic!("{name}: {e}")).starts_with(b"%PDF-"));
        }
    }

    #[test]
    fn native_and_legacy_slide_decks_reach_pdf_export() {
        let native = include_bytes!("../../../vendor/deckcraft/crates/format/tests/golden/deck.deckcraft");
        let legacy = include_bytes!("../../../vendor/deckcraft/crates/ppt/tests/fixtures/old-talk.pps");
        for (name, bytes) in [("native.deckcraft", native.as_slice()), ("show.pps", legacy.as_slice()), ("old.pot", legacy), ("talk.ppt", legacy)] {
            assert!(office_to_pdf(name, bytes).unwrap_or_else(|e| panic!("{name}: {e}")).starts_with(b"%PDF-"));
        }
    }

    #[test]
    fn every_word_reader_reaches_pdf_export() {
        let doc = io::open_bytes("source.txt", b"Quarterly report").unwrap();
        let docx = io::save_bytes("source.docx", &doc).unwrap();
        let model = craft_docx::read_with(&docx, &Default::default()).unwrap();
        let binary = craft_doc_binary::write(&model).unwrap();
        for &ext in io::OPEN_EXTS.iter().chain(&["xhtml", "ltx", "text"]) {
            let name = format!("report.{ext}");
            let input = match ext {
                "doc" | "dot" => binary.clone(),
                "md" | "markdown" => b"# Quarterly report".to_vec(),
                "htm" | "html" | "xhtml" => b"<html><body>Quarterly report</body></html>".to_vec(),
                "mht" | "mhtml" => b"MIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=q\r\n\r\n--q\r\nContent-Type: text/html\r\n\r\n<html>Quarterly report</html>\r\n--q--\r\n".to_vec(),
                "tex" | "latex" | "ltx" => br"\documentclass{article}\begin{document}Quarterly report\end{document}".to_vec(),
                "text" => b"Quarterly report".to_vec(),
                _ => io::save_bytes(&name, &doc).unwrap(),
            };
            assert!(crate::source_kind(&name, &input).is_some(), "{ext}");
            assert!(office_to_pdf(&name, &input).unwrap_or_else(|e| panic!("{ext}: {e}")).starts_with(b"%PDF-"), "{ext}");
        }
        let xml = io::save_bytes_opts("word.xml", &doc, &io::ExportOptions { word2003: true, ..Default::default() }).unwrap();
        assert!(office_to_pdf("word.xml", &xml).unwrap().starts_with(b"%PDF-"));
        assert!(office_to_pdf("renamed.doc", br"{\rtf1 Quarterly report}").unwrap().starts_with(b"%PDF-"));
        assert!(office_to_pdf("broken.docx", b"broken package").is_err());
    }
}
