//! Office documents converted by a real LibreOffice through the tools that take them
//! (`doc_create_multiple`, `doc_combine`), with the converted text checked. Every test here
//! skips when LibreOffice isn't installed, which is the case on CI; run them on a machine that
//! has it. The Word, Excel and PowerPoint files are written here, as the smallest packages
//! LibreOffice reads, so no binary fixtures are committed.

use std::path::Path;

use pdfcraft_automation::{Automation, Content, ToolError};
use serde_json::{Value, json};

fn auto(dir: &Path) -> Automation {
    Automation::new().with_root(dir).unwrap().with_clock(|| 1_700_000_000)
}

fn ok(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args) {
        Ok(mut c) => match c.remove(0) {
            Content::Json(v) => v,
            other => panic!("{tool}: expected JSON, got {other:?}"),
        },
        Err(e) => panic!("{tool} {args}: {e}"),
    }
}

/// Each page's text, with runs of whitespace made single spaces.
fn page_text(a: &mut Automation, doc: u64) -> Vec<String> {
    ok(a, "text_extract", json!({ "doc": doc }))["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["text"].as_str().unwrap().split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

/// A ZIP of `entries`, stored without compression (Office packages may be).
fn zip(entries: &[(&str, &str)]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
    let (mut out, mut central) = (Vec::new(), Vec::new());
    for (name, data) in entries {
        let (crc, size, offset) = (crc32(data.as_bytes()), data.len() as u32, out.len() as u32);
        // version 2.0, no flags, stored, 1980-01-01 00:00
        let common = [&20u16.to_le_bytes()[..], &0u16.to_le_bytes(), &0u16.to_le_bytes(), &0u16.to_le_bytes(), &0x21u16.to_le_bytes()].concat();
        let sizes = [crc.to_le_bytes(), size.to_le_bytes(), size.to_le_bytes()].concat();
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&common);
        out.extend_from_slice(&sizes);
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data.as_bytes());
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&common);
        central.extend_from_slice(&sizes);
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        // extra, comment, disk, internal and external attributes
        central.extend_from_slice(&[0u8; 12]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let (at, count) = (out.len() as u32, entries.len() as u16);
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

const XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;
const PACKAGE_RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const DOC_RELS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// `[Content_Types].xml` with one override per (part, type).
fn content_types(parts: &[(&str, &str)]) -> String {
    let overrides: String = parts.iter().map(|(part, kind)| format!(r#"<Override PartName="/{part}" ContentType="{kind}"/>"#)).collect();
    format!(
        r#"{XML}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>{overrides}</Types>"#
    )
}

/// A relationships part with one relationship per (id, type under officeDocument, target).
fn rels(list: &[(&str, &str, &str)]) -> String {
    let items: String =
        list.iter().map(|(id, kind, target)| format!(r#"<Relationship Id="{id}" Type="{DOC_RELS}/{kind}" Target="{target}"/>"#)).collect();
    format!(r#"{XML}<Relationships xmlns="{PACKAGE_RELS}">{items}</Relationships>"#)
}

/// A Word document of one paragraph.
fn docx(text: &str) -> Vec<u8> {
    let document = format!(
        r#"{XML}<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:body></w:document>"#
    );
    zip(&[
        (
            "[Content_Types].xml",
            &content_types(&[("word/document.xml", "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml")]),
        ),
        ("_rels/.rels", &rels(&[("rId1", "officeDocument", "word/document.xml")])),
        ("word/document.xml", &document),
    ])
}

/// An Excel workbook whose one sheet holds `text` in A1 and a formula, `=6*7`, in A2.
fn xlsx(text: &str) -> Vec<u8> {
    let workbook = format!(
        r#"{XML}<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="{DOC_RELS}"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#
    );
    let sheet = format!(
        r#"{XML}<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>{text}</t></is></c></row><row r="2"><c r="A2"><f>6*7</f></c></row></sheetData></worksheet>"#
    );
    zip(&[
        (
            "[Content_Types].xml",
            &content_types(&[
                ("xl/workbook.xml", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"),
                ("xl/worksheets/sheet1.xml", "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"),
            ]),
        ),
        ("_rels/.rels", &rels(&[("rId1", "officeDocument", "xl/workbook.xml")])),
        ("xl/workbook.xml", &workbook),
        ("xl/_rels/workbook.xml.rels", &rels(&[("rId1", "worksheet", "worksheets/sheet1.xml")])),
        ("xl/worksheets/sheet1.xml", &sheet),
    ])
}

/// A PowerPoint presentation of one slide with one text box.
fn pptx(text: &str) -> Vec<u8> {
    const P: &str =
        r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
    let presentation = format!(
        r#"{XML}<p:presentation {P} xmlns:r="{DOC_RELS}"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:sldIdLst><p:sldId id="256" r:id="rId2"/></p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#
    );
    let tree = r#"<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>"#;
    let master = format!(
        r#"{XML}<p:sldMaster {P} xmlns:r="{DOC_RELS}">{tree}</p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst></p:sldMaster>"#
    );
    let layout = format!(r#"{XML}<p:sldLayout {P}>{tree}</p:spTree></p:cSld></p:sldLayout>"#);
    let slide = format!(
        r#"{XML}<p:sld {P}>{tree}<p:sp><p:nvSpPr><p:cNvPr id="2" name="Text"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="914400" y="914400"/><a:ext cx="6000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:rPr lang="en-US" sz="2800"/><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
    );
    let theme = format!(
        r#"{XML}<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Plain"><a:themeElements><a:clrScheme name="Plain"><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="000000"/></a:dk2><a:lt2><a:srgbClr val="FFFFFF"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme><a:fontScheme name="Plain"><a:majorFont><a:latin typeface="Arial"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Arial"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="Plain"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#
    );
    const PML: &str = "application/vnd.openxmlformats-officedocument.presentationml";
    let types = content_types(&[
        ("ppt/presentation.xml", &format!("{PML}.presentation.main+xml")),
        ("ppt/slideMasters/slideMaster1.xml", &format!("{PML}.slideMaster+xml")),
        ("ppt/slideLayouts/slideLayout1.xml", &format!("{PML}.slideLayout+xml")),
        ("ppt/slides/slide1.xml", &format!("{PML}.slide+xml")),
        ("ppt/theme/theme1.xml", "application/vnd.openxmlformats-officedocument.theme+xml"),
    ]);
    zip(&[
        ("[Content_Types].xml", &types),
        ("_rels/.rels", &rels(&[("rId1", "officeDocument", "ppt/presentation.xml")])),
        ("ppt/presentation.xml", &presentation),
        (
            "ppt/_rels/presentation.xml.rels",
            &rels(&[
                ("rId1", "slideMaster", "slideMasters/slideMaster1.xml"),
                ("rId2", "slide", "slides/slide1.xml"),
                ("rId3", "theme", "theme/theme1.xml"),
            ]),
        ),
        ("ppt/slideMasters/slideMaster1.xml", &master),
        (
            "ppt/slideMasters/_rels/slideMaster1.xml.rels",
            &rels(&[("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml"), ("rId2", "theme", "../theme/theme1.xml")]),
        ),
        ("ppt/slideLayouts/slideLayout1.xml", &layout),
        ("ppt/slideLayouts/_rels/slideLayout1.xml.rels", &rels(&[("rId1", "slideMaster", "../slideMasters/slideMaster1.xml")])),
        ("ppt/slides/slide1.xml", &slide),
        ("ppt/slides/_rels/slide1.xml.rels", &rels(&[("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml")])),
        ("ppt/theme/theme1.xml", &theme),
    ])
}

/// An RTF document of one paragraph.
fn rtf(text: &str) -> Vec<u8> {
    format!("{{\\rtf1\\ansi{{\\fonttbl\\f0\\fswiss Helvetica;}}\\f0\\pard {text}\\par}}").into_bytes()
}

/// A folder with one Word, Excel, PowerPoint and RTF file, each carrying its own word, and the
/// tools rooted there; `None` when LibreOffice isn't installed.
fn office_files() -> Option<(tempfile::TempDir, Automation)> {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("letter.docx"), docx("Quokka letter")).unwrap();
    std::fs::write(dir.path().join("budget.xlsx"), xlsx("Narwhal budget")).unwrap();
    std::fs::write(dir.path().join("deck.pptx"), pptx("Axolotl deck")).unwrap();
    std::fs::write(dir.path().join("notes.rtf"), rtf("Pangolin notes")).unwrap();
    let mut a = auto(dir.path());
    match a.call("doc_create_multiple", &json!({ "paths": ["notes.rtf"] })) {
        Err(ToolError::Failed(e)) if e.contains("needs LibreOffice") => {
            eprintln!("skipped: LibreOffice isn't installed");
            None
        }
        _ => Some((dir, a)),
    }
}

/// Each Office format converts to a PDF of its own that holds the document's text (and, for
/// the spreadsheet, the formula's value).
#[test]
fn office_documents_convert_separately_with_their_text() {
    let Some((dir, mut a)) = office_files() else { return };
    let paths = ["letter.docx", "budget.xlsx", "deck.pptx", "notes.rtf"];
    let made = ok(&mut a, "doc_create_multiple", json!({ "paths": paths, "mode": "separate", "out_dir": "out" }));
    let files = made["files"].as_array().unwrap();
    assert_eq!(files.len(), 4, "{made}");
    let expected = [
        ("letter.pdf", &["Quokka letter"][..]),
        ("budget.pdf", &["Narwhal budget", "42"]),
        ("deck.pdf", &["Axolotl deck"]),
        ("notes.pdf", &["Pangolin notes"]),
    ];
    for (file, (name, words)) in files.iter().zip(expected) {
        let output = file["output"].as_str().unwrap_or_else(|| panic!("{file}"));
        // The tool reports the folder in its canonical form (`\\?\C:\…` on Windows).
        assert_eq!(Path::new(output).file_name(), Some(std::ffi::OsStr::new(name)), "{output}");
        assert!(dir.path().join("out").join(name).is_file(), "{name}");
        let doc = ok(&mut a, "doc_open", json!({ "path": output }))["doc"].as_u64().unwrap();
        let text = page_text(&mut a, doc).join("\n");
        for word in words {
            assert!(text.contains(word), "{output}: {word:?} not in {text:?}");
        }
    }
}

/// Office documents combine, in the order given, with a PDF made here, one bookmark each.
#[test]
fn office_documents_combine_in_order_with_a_bookmark_each() {
    let Some((dir, mut a)) = office_files() else { return };
    let made = ok(
        &mut a,
        "doc_create_multiple",
        json!({ "paths": ["deck.pptx", "letter.docx", "budget.xlsx", "notes.rtf"], "out": "all.pdf", "open": true }),
    );
    let doc = made["document"]["doc"].as_u64().unwrap();
    let pages = page_text(&mut a, doc);
    assert_eq!(pages.len(), 4, "{pages:?}");
    for (page, word) in pages.iter().zip(["Axolotl deck", "Quokka letter", "Narwhal budget", "Pangolin notes"]) {
        assert!(page.contains(word), "{word:?} not in {page:?}");
    }
    let titles: Vec<String> = ok(&mut a, "bookmark_list", json!({ "doc": doc }))["bookmarks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["title"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(titles, ["deck", "letter", "budget", "notes"]);
    assert!(dir.path().join("all.pdf").is_file());

    // doc_combine takes them beside a PDF, with a page range for one of them.
    std::fs::write(dir.path().join("cover.txt"), "Cover page").unwrap();
    ok(&mut a, "doc_create_multiple", json!({ "paths": ["cover.txt"], "out": "cover.pdf" }));
    assert!(dir.path().join("cover.pdf").is_file());
    let combined = ok(&mut a, "doc_combine", json!({ "paths": ["cover.pdf", "notes.rtf", "letter.docx"], "pages": [null, "1", null], "open": true }));
    let doc = combined["document"]["doc"].as_u64().unwrap();
    let pages = page_text(&mut a, doc);
    assert_eq!(pages.len(), 3, "{pages:?}");
    for (page, word) in pages.iter().zip(["Cover page", "Pangolin notes", "Quokka letter"]) {
        assert!(page.contains(word), "{word:?} not in {page:?}");
    }
}
